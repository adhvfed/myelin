use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard};

use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
use zeroize::Zeroizing;

use myelin_tenancy::{Region, TenantId};

use crate::kms::{
    CellRoot, DekId, ExportedKek, KekId, KeyClass, KmsCore, KmsDurableSnapshot, KmsEngine,
    KmsError, PiiKeyRef, SealKey, SealKeyError, SealedRoot, WrappedDek, KEY_LEN, NONCE_LEN,
};
use crate::pg::PgError;

pub const KMS_SEALED_ROOT_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS kms_sealed_root (
    cell_id    text PRIMARY KEY,
    nonce      bytea       NOT NULL,
    ciphertext bytea       NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);";

pub const KMS_WRAPPED_KEK_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS kms_wrapped_kek (
    cell_id   text   NOT NULL,
    tenant_id text   NOT NULL,
    region    text   NOT NULL,
    nonce     bytea  NOT NULL,
    wrapped   bytea  NOT NULL,
    epoch     bigint NOT NULL,
    PRIMARY KEY (cell_id, tenant_id, region)
);";

pub const KMS_WRAPPED_DEK_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS kms_wrapped_dek (
    cell_id   text   NOT NULL,
    tenant_id text   NOT NULL,
    class     text   NOT NULL,
    nonce     bytea  NOT NULL,
    wrapped   bytea  NOT NULL,
    kek_epoch bigint NOT NULL,
    dek_epoch bigint NOT NULL,
    PRIMARY KEY (cell_id, tenant_id, class)
);";

pub const KMS_EPOCH_INVARIANTS_EXPAND_MIGRATION: &str = "\
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conrelid = 'kms_wrapped_kek'::regclass
           AND conname = 'kms_wrapped_kek_epoch_nonnegative'
    ) THEN
        ALTER TABLE kms_wrapped_kek
            ADD CONSTRAINT kms_wrapped_kek_epoch_nonnegative
            CHECK (epoch >= 0) NOT VALID;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conrelid = 'kms_wrapped_dek'::regclass
           AND conname = 'kms_wrapped_dek_epochs_nonnegative'
    ) THEN
        ALTER TABLE kms_wrapped_dek
            ADD CONSTRAINT kms_wrapped_dek_epochs_nonnegative
            CHECK (kek_epoch >= 0 AND dek_epoch >= 0) NOT VALID;
    END IF;
END
$$;";

pub const KMS_EPOCH_INVARIANTS_VALIDATE_MIGRATION: &str = "\
ALTER TABLE kms_wrapped_kek VALIDATE CONSTRAINT kms_wrapped_kek_epoch_nonnegative;
ALTER TABLE kms_wrapped_dek VALIDATE CONSTRAINT kms_wrapped_dek_epochs_nonnegative;";

const READ_BACKUP_DEKS_SQL: &str = "\
SELECT tenant_id, class, nonce, wrapped, kek_epoch, dek_epoch
  FROM kms_wrapped_dek dek
 WHERE cell_id = $1
   AND EXISTS (SELECT 1 FROM kms_wrapped_kek kek
                WHERE kek.cell_id = dek.cell_id
                  AND kek.tenant_id = dek.tenant_id)
 ORDER BY tenant_id, class";

pub fn kms_durable_migrations() -> crate::migration::Migrations {
    use crate::migration::{Migration, Migrations};
    Migrations::of([
        Migration::plain("0040_kms_sealed_root", KMS_SEALED_ROOT_MIGRATION),
        Migration::plain("0041_kms_wrapped_kek", KMS_WRAPPED_KEK_MIGRATION),
        Migration::plain("0042_kms_wrapped_dek", KMS_WRAPPED_DEK_MIGRATION),
    ])
}

pub fn kms_epoch_invariant_migrations() -> crate::migration::Migrations {
    use crate::migration::{Migration, Migrations};
    Migrations::of([
        Migration::plain(
            "0120_kms_epoch_invariants_expand",
            KMS_EPOCH_INVARIANTS_EXPAND_MIGRATION,
        ),
        Migration::plain(
            "0121_kms_epoch_invariants_validate",
            KMS_EPOCH_INVARIANTS_VALIDATE_MIGRATION,
        ),
    ])
}

pub const SEAL_KEY_ENV: &str = "MYELIN_KMS_SEAL_KEY";

#[derive(Debug)]
pub enum KmsDurableError {
    WrongSealKey { cell_id: String },
    SealKeyMissing,
    SealKeyDecode(SealKeyError),
    InvalidSnapshot(&'static str),
    Kms(KmsError),
    Db(PgError),
}

impl core::fmt::Display for KmsDurableError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            KmsDurableError::WrongSealKey { cell_id } => write!(
                f,
                "KMS REFUSED TO START for cell {cell_id}: a sealed cell root exists but did NOT \
                 unseal under the supplied seal key (wrong/absent MYELIN_KMS_SEAL_KEY) - fail-closed, \
                 NEVER generating a new root (that would orphan every existing ciphertext)"
            ),
            KmsDurableError::SealKeyMissing => write!(
                f,
                "KMS seal key is not set: {SEAL_KEY_ENV} must supply the 256-bit unseal key (64 hex \
                 chars) at boot - fail-closed, never a default key"
            ),
            KmsDurableError::SealKeyDecode(e) => write!(f, "KMS seal key from {SEAL_KEY_ENV}: {e}"),
            KmsDurableError::InvalidSnapshot(why) => {
                write!(f, "KMS snapshot restore REFUSED before writing: {why}")
            }
            KmsDurableError::Kms(e) => write!(f, "KMS engine error: {e}"),
            KmsDurableError::Db(e) => write!(f, "durable KMS store error: {e}"),
        }
    }
}

impl std::error::Error for KmsDurableError {}

impl From<PgError> for KmsDurableError {
    fn from(e: PgError) -> Self {
        KmsDurableError::Db(e)
    }
}

impl From<KmsError> for KmsDurableError {
    fn from(error: KmsError) -> Self {
        KmsDurableError::Kms(error)
    }
}

pub fn seal_key_from_env() -> Result<SealKey, KmsDurableError> {
    let raw =
        Zeroizing::new(std::env::var(SEAL_KEY_ENV).map_err(|_| KmsDurableError::SealKeyMissing)?);
    SealKey::from_encoded(raw.as_str()).map_err(KmsDurableError::SealKeyDecode)
}

#[derive(Clone)]
pub struct DurableKmsBacking {
    pool: PgPool,
    cell_id: String,
}

impl DurableKmsBacking {
    pub fn new(pool: PgPool, cell_id: impl Into<String>) -> DurableKmsBacking {
        let connect_options = (*pool.connect_options()).clone();
        DurableKmsBacking {
            pool: PgPoolOptions::new()
                .max_connections(2)
                .connect_lazy_with(connect_options),
            cell_id: cell_id.into(),
        }
    }

    pub fn cell_id(&self) -> &str {
        &self.cell_id
    }

    #[cfg(any(test, feature = "test-support"))]
    pub async fn close_pool_for_test(&self) {
        self.pool.close().await;
    }

    pub async fn load_or_generate(&self, seal_key: &SealKey) -> Result<KmsEngine, KmsDurableError> {
        let root = match self.read_sealed_root().await? {
            Some(sealed) => CellRoot::unseal(seal_key, &sealed).ok_or_else(|| {
                KmsDurableError::WrongSealKey {
                    cell_id: self.cell_id.clone(),
                }
            })?,
            None => {
                let fresh = CellRoot::generate();
                self.insert_sealed_root_if_absent(&fresh.seal(seal_key))
                    .await?;
                let stored = self.read_sealed_root().await?.ok_or_else(|| {
                    KmsDurableError::Db(PgError::Query(
                        "sealed cell root vanished immediately after insert".into(),
                    ))
                })?;
                CellRoot::unseal(seal_key, &stored).ok_or_else(|| {
                    KmsDurableError::WrongSealKey {
                        cell_id: self.cell_id.clone(),
                    }
                })?
            }
        };
        let core = KmsCore::from_root(root);
        self.load_keks(&core).await?;
        self.load_deks(&core).await?;
        Ok(KmsEngine::durable(DurableKms {
            core,
            backing: self.clone(),
            rt: tokio::runtime::Handle::current(),
            cache_sync: Mutex::new(()),
        }))
    }

    async fn read_sealed_root(&self) -> Result<Option<SealedRoot>, PgError> {
        let row = sqlx::query("SELECT nonce, ciphertext FROM kms_sealed_root WHERE cell_id = $1")
            .bind(&self.cell_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        row.map(sealed_root_from_row).transpose()
    }

    async fn insert_sealed_root_if_absent(&self, sealed: &SealedRoot) -> Result<(), PgError> {
        sqlx::query(
            "INSERT INTO kms_sealed_root (cell_id, nonce, ciphertext) VALUES ($1, $2, $3) \
             ON CONFLICT (cell_id) DO NOTHING",
        )
        .bind(&self.cell_id)
        .bind(sealed.nonce.as_slice())
        .bind(&sealed.ciphertext)
        .execute(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    async fn upsert_sealed_root_on<'e, E: sqlx::PgExecutor<'e>>(
        &self,
        ex: E,
        sealed: &SealedRoot,
    ) -> Result<(), PgError> {
        sqlx::query(
            "INSERT INTO kms_sealed_root (cell_id, nonce, ciphertext) VALUES ($1, $2, $3) \
             ON CONFLICT (cell_id) DO UPDATE SET nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext",
        )
        .bind(&self.cell_id)
        .bind(sealed.nonce.as_slice())
        .bind(&sealed.ciphertext)
        .execute(ex)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    async fn load_keks(&self, core: &KmsCore) -> Result<(), KmsDurableError> {
        let rows = sqlx::query(
            "SELECT tenant_id, region, nonce, wrapped, epoch FROM kms_wrapped_kek WHERE cell_id = $1",
        )
        .bind(&self.cell_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        for row in rows {
            let (id, kek) = kek_with_identity_from_row(row)?;
            core.install_wrapped_kek(id, kek.nonce, kek.wrapped, kek.epoch)?;
        }
        Ok(())
    }

    async fn insert_kek_if_absent(&self, id: &KekId, k: &ExportedKek) -> Result<(), PgError> {
        let epoch = encode_epoch("epoch", k.epoch)?;
        sqlx::query(
            "INSERT INTO kms_wrapped_kek (cell_id, tenant_id, region, nonce, wrapped, epoch) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .bind(id.region.as_str())
        .bind(k.nonce.as_slice())
        .bind(&k.wrapped)
        .bind(epoch)
        .execute(&self.pool)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
        Ok(())
    }

    async fn read_kek(&self, id: &KekId) -> Result<Option<ExportedKek>, PgError> {
        let row = sqlx::query(
            "SELECT nonce, wrapped, epoch FROM kms_wrapped_kek \
              WHERE cell_id = $1 AND tenant_id = $2 AND region = $3",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .bind(id.region.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
        row.map(kek_material_from_row).transpose()
    }

    async fn upsert_kek_row_on<'e, E: sqlx::PgExecutor<'e>>(
        &self,
        ex: E,
        id: &KekId,
        k: &ExportedKek,
    ) -> Result<(), PgError> {
        let epoch = encode_epoch("epoch", k.epoch)?;
        sqlx::query(
            "INSERT INTO kms_wrapped_kek (cell_id, tenant_id, region, nonce, wrapped, epoch) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (cell_id, tenant_id, region) DO UPDATE SET \
               nonce = EXCLUDED.nonce, wrapped = EXCLUDED.wrapped, epoch = EXCLUDED.epoch",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .bind(id.region.as_str())
        .bind(k.nonce.as_slice())
        .bind(&k.wrapped)
        .bind(epoch)
        .execute(ex)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    async fn delete_kek_row(&self, id: &KekId) -> Result<bool, PgError> {
        let deleted = sqlx::query(
            "DELETE FROM kms_wrapped_kek WHERE cell_id = $1 AND tenant_id = $2 AND region = $3",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .bind(id.region.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(deleted.rows_affected() > 0)
    }

    async fn load_deks(&self, core: &KmsCore) -> Result<(), KmsDurableError> {
        let rows = sqlx::query(
            "SELECT tenant_id, class, nonce, wrapped, kek_epoch, dek_epoch FROM kms_wrapped_dek \
             WHERE cell_id = $1",
        )
        .bind(&self.cell_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        for row in rows {
            let (id, wrapped, epoch) = dek_with_identity_from_row(row)?;
            core.install_wrapped_dek(id, wrapped, epoch)?;
        }
        Ok(())
    }

    async fn read_dek(&self, id: &DekId) -> Result<Option<(WrappedDek, u64)>, PgError> {
        let row = sqlx::query(
            "SELECT nonce, wrapped, kek_epoch, dek_epoch FROM kms_wrapped_dek \
              WHERE cell_id = $1 AND tenant_id = $2 AND class = $3",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .bind(id.class.as_token())
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
        row.map(dek_material_from_row).transpose()
    }

    async fn read_deks_for_tenant(
        &self,
        tenant: &TenantId,
    ) -> Result<Vec<(DekId, WrappedDek, u64)>, PgError> {
        let rows = sqlx::query(
            "SELECT class, nonce, wrapped, kek_epoch, dek_epoch FROM kms_wrapped_dek \
              WHERE cell_id = $1 AND tenant_id = $2 ORDER BY class",
        )
        .bind(&self.cell_id)
        .bind(tenant.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
        rows.into_iter()
            .map(|row| {
                let class_token: String = row.try_get("class").map_err(kms_row_decode)?;
                let class = KeyClass::parse_token(&class_token).ok_or_else(|| {
                    PgError::Query("kms_wrapped_dek row has an invalid key class".into())
                })?;
                let (wrapped, epoch) = dek_material_from_row(row)?;
                Ok((DekId::new(tenant.clone(), class), wrapped, epoch))
            })
            .collect()
    }

    async fn read_all_deks(&self) -> Result<Vec<(DekId, WrappedDek, u64)>, PgError> {
        let rows = sqlx::query(READ_BACKUP_DEKS_SQL)
            .bind(&self.cell_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
        rows.into_iter().map(dek_with_identity_from_row).collect()
    }

    async fn read_snapshot(&self) -> Result<KmsDurableSnapshot, PgError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;

        let sealed_root =
            sqlx::query("SELECT nonce, ciphertext FROM kms_sealed_root WHERE cell_id = $1")
                .bind(&self.cell_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| PgError::Query(error.to_string()))?
                .map(sealed_root_from_row)
                .transpose()?
                .ok_or_else(|| {
                    PgError::Query("durable KMS sealed root is absent during snapshot".into())
                })?;

        let kek_rows = sqlx::query(
            "SELECT tenant_id, region, nonce, wrapped, epoch FROM kms_wrapped_kek \
              WHERE cell_id = $1 ORDER BY tenant_id, region",
        )
        .bind(&self.cell_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
        let keks = kek_rows
            .into_iter()
            .map(kek_with_identity_from_row)
            .collect::<Result<Vec<_>, PgError>>()?;

        let dek_rows = sqlx::query(READ_BACKUP_DEKS_SQL)
            .bind(&self.cell_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
        let deks = dek_rows
            .into_iter()
            .map(dek_with_identity_from_row)
            .collect::<Result<Vec<_>, PgError>>()?;

        tx.commit()
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
        Ok(KmsDurableSnapshot {
            sealed_root,
            keks,
            deks,
        })
    }

    async fn read_dek_material(
        &self,
        id: &DekId,
        region: &Region,
    ) -> Result<Option<(ExportedKek, WrappedDek, u64)>, PgError> {
        let row = sqlx::query(
            "SELECT kek.nonce AS kek_nonce, kek.wrapped AS kek_wrapped, kek.epoch AS kek_epoch, \
                    dek.nonce AS dek_nonce, dek.wrapped AS dek_wrapped, \
                    dek.kek_epoch AS dek_kek_epoch, dek.dek_epoch \
               FROM kms_wrapped_kek kek \
               JOIN kms_wrapped_dek dek \
                 ON dek.cell_id = kek.cell_id AND dek.tenant_id = kek.tenant_id \
              WHERE kek.cell_id = $1 AND kek.tenant_id = $2 AND kek.region = $3 \
                AND dek.class = $4",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .bind(region.as_str())
        .bind(id.class.as_token())
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
        row.map(|row| {
            let kek_epoch = decode_epoch(
                "kek_epoch",
                row.try_get::<i64, _>("kek_epoch").map_err(kms_row_decode)?,
            )?;
            let dek_kek_epoch = decode_epoch(
                "dek_kek_epoch",
                row.try_get::<i64, _>("dek_kek_epoch")
                    .map_err(kms_row_decode)?,
            )?;
            if kek_epoch != dek_kek_epoch {
                return Err(PgError::Query(format!(
                    "durable KMS rows disagree on KEK epoch for tenant={} class={}",
                    id.tenant.as_str(),
                    id.class.as_token(),
                )));
            }
            let dek_epoch = decode_epoch(
                "dek_epoch",
                row.try_get::<i64, _>("dek_epoch").map_err(kms_row_decode)?,
            )?;
            let kek_nonce = row
                .try_get::<Vec<u8>, _>("kek_nonce")
                .map_err(kms_row_decode)?;
            let dek_nonce = row
                .try_get::<Vec<u8>, _>("dek_nonce")
                .map_err(kms_row_decode)?;
            Ok((
                ExportedKek {
                    nonce: nonce_from(&kek_nonce)?,
                    wrapped: row.try_get("kek_wrapped").map_err(kms_row_decode)?,
                    epoch: kek_epoch,
                },
                WrappedDek {
                    nonce: nonce_from(&dek_nonce)?,
                    wrapped: row.try_get("dek_wrapped").map_err(kms_row_decode)?,
                    kek_epoch: dek_kek_epoch,
                },
                dek_epoch,
            ))
        })
        .transpose()
    }

    async fn insert_dek_if_absent(
        &self,
        id: &DekId,
        wrapped: &WrappedDek,
        dek_epoch: u64,
    ) -> Result<(), PgError> {
        let kek_epoch = encode_epoch("kek_epoch", wrapped.kek_epoch)?;
        let dek_epoch = encode_epoch("dek_epoch", dek_epoch)?;
        sqlx::query(
            "INSERT INTO kms_wrapped_dek \
               (cell_id, tenant_id, class, nonce, wrapped, kek_epoch, dek_epoch) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .bind(id.class.as_token())
        .bind(wrapped.nonce.as_slice())
        .bind(&wrapped.wrapped)
        .bind(kek_epoch)
        .bind(dek_epoch)
        .execute(&self.pool)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
        Ok(())
    }

    async fn upsert_dek_row_on<'e, E: sqlx::PgExecutor<'e>>(
        &self,
        ex: E,
        id: &DekId,
        w: &WrappedDek,
        dek_epoch: u64,
    ) -> Result<(), PgError> {
        let kek_epoch = encode_epoch("kek_epoch", w.kek_epoch)?;
        let dek_epoch = encode_epoch("dek_epoch", dek_epoch)?;
        sqlx::query(
            "INSERT INTO kms_wrapped_dek \
               (cell_id, tenant_id, class, nonce, wrapped, kek_epoch, dek_epoch) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (cell_id, tenant_id, class) DO UPDATE SET \
               nonce = EXCLUDED.nonce, wrapped = EXCLUDED.wrapped, \
               kek_epoch = EXCLUDED.kek_epoch, dek_epoch = EXCLUDED.dek_epoch",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .bind(id.class.as_token())
        .bind(w.nonce.as_slice())
        .bind(&w.wrapped)
        .bind(kek_epoch)
        .bind(dek_epoch)
        .execute(ex)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    async fn persist_rotation(
        &self,
        id: &KekId,
        kek: &ExportedKek,
        deks: &[(DekId, WrappedDek, u64)],
    ) -> Result<(), PgError> {
        let next_kek_epoch = encode_epoch("epoch", kek.epoch)?;
        let expected_kek_epoch = kek.epoch.checked_sub(1).ok_or_else(|| {
            PgError::Query("KMS rotation candidate did not advance its KEK epoch".into())
        })?;
        let expected_kek_epoch_db = encode_epoch("epoch", expected_kek_epoch)?;
        let encoded_deks = deks
            .iter()
            .map(|(dek_id, wrapped, dek_epoch)| {
                let previous_epoch = dek_epoch.checked_sub(1).ok_or_else(|| {
                    PgError::Query("KMS rotation candidate did not advance a DEK".into())
                })?;
                Ok((
                    dek_id,
                    wrapped,
                    encode_epoch("kek_epoch", wrapped.kek_epoch)?,
                    encode_epoch("dek_epoch", *dek_epoch)?,
                    encode_epoch("dek_epoch", previous_epoch)?,
                ))
            })
            .collect::<Result<Vec<_>, PgError>>()?;

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        let current_kek_epoch = sqlx::query_scalar::<_, i64>(
            "SELECT epoch FROM kms_wrapped_kek \
              WHERE cell_id = $1 AND tenant_id = $2 AND region = $3 FOR UPDATE",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .bind(id.region.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
        let current_kek_epoch = current_kek_epoch
            .map(|epoch| decode_epoch("epoch", epoch))
            .transpose()?;
        if current_kek_epoch != Some(expected_kek_epoch) {
            return Err(PgError::Query(
                "KMS rotation refused stale durable KEK state".into(),
            ));
        }
        let durable_deks = sqlx::query_as::<_, (String, i64)>(
            "SELECT class, dek_epoch FROM kms_wrapped_dek \
              WHERE cell_id = $1 AND tenant_id = $2 ORDER BY class FOR UPDATE",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
        for (_, epoch) in &durable_deks {
            decode_epoch("dek_epoch", *epoch)?;
        }
        let mut candidate_deks = encoded_deks
            .iter()
            .map(|(dek_id, _, _, _, previous_epoch)| {
                (dek_id.class.as_token().to_string(), *previous_epoch)
            })
            .collect::<Vec<_>>();
        candidate_deks.sort_unstable();
        if durable_deks != candidate_deks {
            return Err(PgError::Query(
                "KMS rotation refused stale durable DEK membership".into(),
            ));
        }
        let updated_kek = sqlx::query(
            "UPDATE kms_wrapped_kek SET nonce = $4, wrapped = $5, epoch = $6 \
              WHERE cell_id = $1 AND tenant_id = $2 AND region = $3 AND epoch = $7",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .bind(id.region.as_str())
        .bind(kek.nonce.as_slice())
        .bind(&kek.wrapped)
        .bind(next_kek_epoch)
        .bind(expected_kek_epoch_db)
        .execute(&mut *tx)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
        if updated_kek.rows_affected() != 1 {
            return Err(PgError::Query(
                "KMS rotation lost its durable KEK compare-and-swap".into(),
            ));
        }
        for (dek_id, wrapped, kek_epoch, dek_epoch, previous_epoch) in encoded_deks {
            let updated = sqlx::query(
                "UPDATE kms_wrapped_dek \
                    SET nonce = $4, wrapped = $5, kek_epoch = $6, dek_epoch = $7 \
                  WHERE cell_id = $1 AND tenant_id = $2 AND class = $3 AND dek_epoch = $8",
            )
            .bind(&self.cell_id)
            .bind(dek_id.tenant.as_str())
            .bind(dek_id.class.as_token())
            .bind(wrapped.nonce.as_slice())
            .bind(&wrapped.wrapped)
            .bind(kek_epoch)
            .bind(dek_epoch)
            .bind(previous_epoch)
            .execute(&mut *tx)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
            if updated.rows_affected() != 1 {
                return Err(PgError::Query(
                    "KMS rotation lost a durable DEK compare-and-swap".into(),
                ));
            }
        }
        tx.commit()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    async fn delete_dek_row(&self, id: &DekId) -> Result<bool, PgError> {
        let deleted = sqlx::query(
            "DELETE FROM kms_wrapped_dek WHERE cell_id = $1 AND tenant_id = $2 AND class = $3",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .bind(id.class.as_token())
        .execute(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(deleted.rows_affected() > 0)
    }

    pub async fn restore(&self, snap: &KmsDurableSnapshot) -> Result<(), KmsDurableError> {
        validate_snapshot(snap)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;

        sqlx::query("DELETE FROM kms_wrapped_dek WHERE cell_id = $1")
            .bind(&self.cell_id)
            .execute(&mut *tx)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
        sqlx::query("DELETE FROM kms_wrapped_kek WHERE cell_id = $1")
            .bind(&self.cell_id)
            .execute(&mut *tx)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;

        self.upsert_sealed_root_on(&mut *tx, &snap.sealed_root)
            .await?;
        for (id, k) in &snap.keks {
            self.upsert_kek_row_on(&mut *tx, id, k).await?;
        }
        for (id, w, dek_epoch) in &snap.deks {
            self.upsert_dek_row_on(&mut *tx, id, w, *dek_epoch).await?;
        }
        tx.commit()
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
        Ok(())
    }
}

pub(crate) struct DurableKms {
    pub(crate) core: KmsCore,
    pub(crate) backing: DurableKmsBacking,
    pub(crate) rt: tokio::runtime::Handle,
    cache_sync: Mutex<()>,
}

impl DurableKms {
    pub(crate) fn core(&self) -> &KmsCore {
        &self.core
    }

    fn synchronize_cache(&self) -> Result<MutexGuard<'_, ()>, KmsError> {
        self.cache_sync
            .lock()
            .map_err(|_| KmsError::StateUnavailable("durable KMS cache synchronization"))
    }

    pub(crate) fn export_kek(&self, id: &KekId) -> Result<Option<ExportedKek>, KmsError> {
        let _cache = self.synchronize_cache()?;
        let durable = self
            .block(self.backing.read_kek(id))
            .map_err(|error| KmsError::Durability(error.to_string()))?;
        match durable {
            Some(kek) => {
                self.core.install_wrapped_kek(
                    id.clone(),
                    kek.nonce,
                    kek.wrapped.clone(),
                    kek.epoch,
                )?;
                Ok(Some(kek))
            }
            None => {
                self.core.destroy_kek(id)?;
                Ok(None)
            }
        }
    }

    pub(crate) fn export_dek(&self, id: &DekId) -> Result<Option<(WrappedDek, u64)>, KmsError> {
        let _cache = self.synchronize_cache()?;
        let durable = self
            .block(self.backing.read_dek(id))
            .map_err(|error| KmsError::Durability(error.to_string()))?;
        match durable {
            Some((wrapped, epoch)) => {
                self.core
                    .install_wrapped_dek(id.clone(), wrapped.clone(), epoch)?;
                Ok(Some((wrapped, epoch)))
            }
            None => {
                self.core.destroy_dek(id)?;
                Ok(None)
            }
        }
    }

    pub(crate) fn export_deks(&self) -> Result<Vec<(DekId, WrappedDek, u64)>, KmsError> {
        self.block(self.backing.read_all_deks())
            .map_err(|error| KmsError::Durability(error.to_string()))
    }

    pub(crate) fn backup_snapshot(&self) -> Result<Vec<(DekId, WrappedDek)>, KmsError> {
        Ok(self
            .backup_snapshot_durable()?
            .deks
            .into_iter()
            .map(|(id, wrapped, _)| (id, wrapped))
            .collect())
    }

    pub(crate) fn backup_snapshot_durable(&self) -> Result<KmsDurableSnapshot, KmsError> {
        self.block(self.backing.read_snapshot())
            .map_err(|error| KmsError::Durability(error.to_string()))
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| self.rt.block_on(fut)),
            Err(_) => self.rt.block_on(fut),
        }
    }

    pub(crate) fn ensure_kek(&self, id: &KekId) -> Result<u64, KmsError> {
        let _cache = self.synchronize_cache()?;
        self.ensure_kek_while_synchronized(id)
    }

    fn ensure_kek_while_synchronized(&self, id: &KekId) -> Result<u64, KmsError> {
        if let Some(stored) = self
            .block(self.backing.read_kek(id))
            .map_err(|error| KmsError::Durability(error.to_string()))?
        {
            self.core.install_wrapped_kek(
                id.clone(),
                stored.nonce,
                stored.wrapped,
                stored.epoch,
            )?;
            return Ok(stored.epoch);
        }
        self.core.destroy_kek(id)?;
        let (epoch, _) = self.core.ensure_kek_tracked(id)?;
        let Some(candidate) = self.core.export_kek(id)? else {
            self.core.destroy_kek(id)?;
            return Err(KmsError::Durability(format!(
                "freshly minted KEK was unavailable before persistence for tenant={} region={}",
                id.tenant.as_str(),
                id.region.as_str()
            )));
        };
        if let Err(error) = self.block(self.backing.insert_kek_if_absent(id, &candidate)) {
            self.core.destroy_kek(id)?;
            return Err(KmsError::Durability(format!(
                "freshly minted KEK for tenant={} region={} could not be persisted; the \
                 in-memory mint was rolled back: {error}",
                id.tenant.as_str(),
                id.region.as_str()
            )));
        }
        let winner = match self.block(self.backing.read_kek(id)) {
            Ok(Some(winner)) => winner,
            Ok(None) => {
                self.core.destroy_kek(id)?;
                return Err(KmsError::Durability(
                    "inserted or concurrently won KEK vanished".into(),
                ));
            }
            Err(error) => {
                self.core.destroy_kek(id)?;
                return Err(KmsError::Durability(error.to_string()));
            }
        };
        self.core
            .install_wrapped_kek(id.clone(), winner.nonce, winner.wrapped, winner.epoch)?;
        debug_assert_eq!(epoch, winner.epoch);
        Ok(winner.epoch)
    }

    pub(crate) fn ensure_dek(
        &self,
        tenant: &TenantId,
        region: &Region,
        class: KeyClass,
    ) -> Result<PiiKeyRef, KmsError> {
        let _cache = self.synchronize_cache()?;
        let kek_id = KekId::new(tenant.clone(), region.clone());
        self.ensure_kek_while_synchronized(&kek_id)?;
        let dek_id = DekId::new(tenant.clone(), class.clone());
        if let Some((kek, dek, dek_epoch)) = self
            .block(self.backing.read_dek_material(&dek_id, region))
            .map_err(|error| KmsError::Durability(error.to_string()))?
        {
            self.core
                .install_wrapped_kek(kek_id, kek.nonce, kek.wrapped, kek.epoch)?;
            self.core.install_wrapped_dek(dek_id, dek, dek_epoch)?;
            return Ok(PiiKeyRef::new(tenant.clone(), dek_epoch, class));
        }
        self.core.destroy_dek(&dek_id)?;
        let (candidate_ref, _) = self
            .core
            .ensure_dek_tracked(tenant, region, class.clone())?;
        let Some((candidate, candidate_epoch)) = self.core.export_dek(&dek_id)? else {
            self.core.destroy_dek(&dek_id)?;
            return Err(KmsError::Durability(format!(
                "freshly minted DEK was unavailable before persistence for tenant={} class={}",
                tenant.as_str(),
                class.as_token()
            )));
        };
        if let Err(error) = self.block(self.backing.insert_dek_if_absent(
            &dek_id,
            &candidate,
            candidate_epoch,
        )) {
            self.core.destroy_dek(&dek_id)?;
            return Err(KmsError::Durability(error.to_string()));
        }
        let (kek, winner, winner_epoch) = self
            .block(self.backing.read_dek_material(&dek_id, region))
            .map_err(|error| KmsError::Durability(error.to_string()))?
            .ok_or_else(|| {
                KmsError::Durability("inserted or concurrently won DEK vanished".into())
            })?;
        self.core
            .install_wrapped_kek(kek_id, kek.nonce, kek.wrapped, kek.epoch)?;
        self.core
            .install_wrapped_dek(dek_id, winner, winner_epoch)?;
        debug_assert_eq!(candidate_ref.dek_epoch, candidate_epoch);
        Ok(PiiKeyRef::new(tenant.clone(), winner_epoch, class))
    }

    pub(crate) fn resolve_dek(
        &self,
        key_ref: &PiiKeyRef,
        region: &Region,
    ) -> Result<crate::kms::DekHandle, KmsError> {
        let _cache = self.synchronize_cache()?;
        let dek_id = DekId::new(key_ref.tenant.clone(), key_ref.class.clone());
        let material = self
            .block(self.backing.read_dek_material(&dek_id, region))
            .map_err(|error| KmsError::Durability(error.to_string()))?;
        let Some((kek, dek, dek_epoch)) = material else {
            self.core.destroy_dek(&dek_id)?;
            return Err(KmsError::DekUnavailable(dek_id));
        };
        self.core.install_wrapped_kek(
            KekId::new(key_ref.tenant.clone(), region.clone()),
            kek.nonce,
            kek.wrapped,
            kek.epoch,
        )?;
        self.core.install_wrapped_dek(dek_id, dek, dek_epoch)?;
        self.core.resolve_dek(key_ref, region)
    }

    pub(crate) fn rotate_kek(&self, id: &KekId) -> Result<u64, KmsError> {
        let _cache = self.synchronize_cache()?;
        let durable_kek = self
            .block(self.backing.read_kek(id))
            .map_err(|error| KmsError::Durability(error.to_string()))?;
        let durable_deks = self
            .block(self.backing.read_deks_for_tenant(&id.tenant))
            .map_err(|error| KmsError::Durability(error.to_string()))?;
        let current = durable_kek.ok_or_else(|| KmsError::KekUnavailable(id.clone()))?;
        let rotation = self.core.prepare_kek_rotation(id, current, durable_deks)?;
        self.block(
            self.backing
                .persist_rotation(id, &rotation.kek, &rotation.deks),
        )
        .map_err(|error| KmsError::Durability(error.to_string()))?;
        self.core.publish_kek_rotation(id, &rotation)?;
        Ok(rotation.epoch())
    }

    pub(crate) fn try_destroy_kek(&self, id: &KekId) -> Result<bool, KmsError> {
        let _cache = self.synchronize_cache()?;
        let durable_removed = self
            .block(self.backing.delete_kek_row(id))
            .map_err(|error| KmsError::Durability(error.to_string()))?;
        Ok(self.core.destroy_kek(id)? || durable_removed)
    }

    pub(crate) fn try_destroy_dek(&self, id: &DekId) -> Result<bool, KmsError> {
        let _cache = self.synchronize_cache()?;
        let durable_removed = self
            .block(self.backing.delete_dek_row(id))
            .map_err(|error| KmsError::Durability(error.to_string()))?;
        Ok(self.core.destroy_dek(id)? || durable_removed)
    }

    pub(crate) fn wrap_dek_material(
        &self,
        tenant: &TenantId,
        region: &Region,
        material: &[u8; KEY_LEN],
    ) -> Result<WrappedDek, KmsError> {
        let _cache = self.synchronize_cache()?;
        let kek_id = KekId::new(tenant.clone(), region.clone());
        self.ensure_kek_while_synchronized(&kek_id)?;
        self.core.wrap_dek_material(tenant, region, material)
    }
}

const WRAPPED_KEY_CIPHERTEXT_LEN: usize = KEY_LEN + 16;

fn validate_snapshot(snapshot: &KmsDurableSnapshot) -> Result<(), KmsDurableError> {
    if snapshot.sealed_root.ciphertext.len() != WRAPPED_KEY_CIPHERTEXT_LEN {
        return Err(KmsDurableError::InvalidSnapshot(
            "sealed root ciphertext has the wrong length",
        ));
    }

    let mut kek_ids = BTreeSet::new();
    let mut kek_epochs_by_tenant: BTreeMap<TenantId, BTreeSet<u64>> = BTreeMap::new();
    for (id, kek) in &snapshot.keks {
        if id.tenant.as_str().is_empty() || id.region.as_str().is_empty() {
            return Err(KmsDurableError::InvalidSnapshot(
                "a KEK identity has an empty tenant or region",
            ));
        }
        if !kek_ids.insert(id.clone()) {
            return Err(KmsDurableError::InvalidSnapshot(
                "the same KEK identity appears more than once",
            ));
        }
        if kek.wrapped.len() != WRAPPED_KEY_CIPHERTEXT_LEN {
            return Err(KmsDurableError::InvalidSnapshot(
                "a wrapped KEK ciphertext has the wrong length",
            ));
        }
        if encode_epoch("epoch", kek.epoch).is_err() {
            return Err(KmsDurableError::InvalidSnapshot(
                "a KEK epoch exceeds the durable range",
            ));
        }
        kek_epochs_by_tenant
            .entry(id.tenant.clone())
            .or_default()
            .insert(kek.epoch);
    }

    let mut dek_ids = BTreeSet::new();
    for (id, wrapped, dek_epoch) in &snapshot.deks {
        if id.tenant.as_str().is_empty()
            || KeyClass::parse_token(&id.class.as_token()).as_ref() != Some(&id.class)
        {
            return Err(KmsDurableError::InvalidSnapshot(
                "a DEK identity is not canonical",
            ));
        }
        if !dek_ids.insert(id.clone()) {
            return Err(KmsDurableError::InvalidSnapshot(
                "the same DEK identity appears more than once",
            ));
        }
        if wrapped.wrapped.len() != WRAPPED_KEY_CIPHERTEXT_LEN {
            return Err(KmsDurableError::InvalidSnapshot(
                "a wrapped DEK ciphertext has the wrong length",
            ));
        }
        if encode_epoch("kek_epoch", wrapped.kek_epoch).is_err()
            || encode_epoch("dek_epoch", *dek_epoch).is_err()
        {
            return Err(KmsDurableError::InvalidSnapshot(
                "a DEK epoch exceeds the durable range",
            ));
        }
        if !kek_epochs_by_tenant
            .get(&id.tenant)
            .is_some_and(|epochs| epochs.contains(&wrapped.kek_epoch))
        {
            return Err(KmsDurableError::InvalidSnapshot(
                "a DEK has no matching tenant KEK epoch",
            ));
        }
    }
    Ok(())
}

fn dek_material_from_row(row: sqlx::postgres::PgRow) -> Result<(WrappedDek, u64), PgError> {
    let kek_epoch = decode_epoch(
        "kek_epoch",
        row.try_get::<i64, _>("kek_epoch").map_err(kms_row_decode)?,
    )?;
    let dek_epoch = decode_epoch(
        "dek_epoch",
        row.try_get::<i64, _>("dek_epoch").map_err(kms_row_decode)?,
    )?;
    let nonce = row.try_get::<Vec<u8>, _>("nonce").map_err(kms_row_decode)?;
    Ok((
        WrappedDek {
            nonce: nonce_from(&nonce)?,
            wrapped: row.try_get("wrapped").map_err(kms_row_decode)?,
            kek_epoch,
        },
        dek_epoch,
    ))
}

fn kek_material_from_row(row: sqlx::postgres::PgRow) -> Result<ExportedKek, PgError> {
    let epoch = decode_epoch(
        "epoch",
        row.try_get::<i64, _>("epoch").map_err(kms_row_decode)?,
    )?;
    let nonce = row.try_get::<Vec<u8>, _>("nonce").map_err(kms_row_decode)?;
    Ok(ExportedKek {
        nonce: nonce_from(&nonce)?,
        wrapped: row.try_get("wrapped").map_err(kms_row_decode)?,
        epoch,
    })
}

fn sealed_root_from_row(row: sqlx::postgres::PgRow) -> Result<SealedRoot, PgError> {
    let nonce = row.try_get::<Vec<u8>, _>("nonce").map_err(kms_row_decode)?;
    Ok(SealedRoot {
        nonce: nonce_from(&nonce)?,
        ciphertext: row.try_get("ciphertext").map_err(kms_row_decode)?,
    })
}

fn kek_with_identity_from_row(row: sqlx::postgres::PgRow) -> Result<(KekId, ExportedKek), PgError> {
    let tenant = TenantId(row.try_get("tenant_id").map_err(kms_row_decode)?);
    let region = Region(row.try_get("region").map_err(kms_row_decode)?);
    let kek = kek_material_from_row(row)?;
    Ok((KekId::new(tenant, region), kek))
}

fn dek_with_identity_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<(DekId, WrappedDek, u64), PgError> {
    let tenant = TenantId(row.try_get("tenant_id").map_err(kms_row_decode)?);
    let class_token: String = row.try_get("class").map_err(kms_row_decode)?;
    let class = KeyClass::parse_token(&class_token)
        .ok_or_else(|| PgError::Query("kms_wrapped_dek row has an invalid key class".into()))?;
    let (wrapped, epoch) = dek_material_from_row(row)?;
    Ok((DekId::new(tenant, class), wrapped, epoch))
}

fn nonce_from(bytes: &[u8]) -> Result<[u8; NONCE_LEN], PgError> {
    bytes.try_into().map_err(|_| {
        PgError::Query(format!(
            "durable KMS row has an invalid nonce length (expected {NONCE_LEN} bytes)"
        ))
    })
}

fn kms_row_decode(error: sqlx::Error) -> PgError {
    PgError::Query(format!("durable KMS row decode failed: {error}"))
}

fn decode_epoch(field: &'static str, epoch: i64) -> Result<u64, PgError> {
    u64::try_from(epoch).map_err(|_| {
        PgError::Query(format!(
            "durable KMS row has a negative `{field}` key epoch"
        ))
    })
}

fn encode_epoch(field: &'static str, epoch: u64) -> Result<i64, PgError> {
    i64::try_from(epoch).map_err(|_| {
        PgError::Query(format!(
            "KMS `{field}` key epoch exceeds the PostgreSQL bigint range"
        ))
    })
}

#[cfg(test)]
mod durable_decode_tests {
    use super::{decode_epoch, encode_epoch, nonce_from, NONCE_LEN};

    #[test]
    fn durable_nonce_requires_the_exact_aead_length() {
        let exact = vec![7; NONCE_LEN];
        assert_eq!(nonce_from(&exact).unwrap(), [7; NONCE_LEN]);
        assert!(nonce_from(&exact[..NONCE_LEN - 1]).is_err());

        let mut overlong = exact;
        overlong.push(7);
        assert!(nonce_from(&overlong).is_err());
    }

    #[test]
    fn key_epochs_cross_the_signed_database_boundary_without_wrapping() {
        assert_eq!(decode_epoch("epoch", 0).unwrap(), 0);
        assert_eq!(decode_epoch("epoch", i64::MAX).unwrap(), i64::MAX as u64);
        assert!(decode_epoch("epoch", -1).is_err());

        assert_eq!(encode_epoch("epoch", i64::MAX as u64).unwrap(), i64::MAX);
        assert!(encode_epoch("epoch", i64::MAX as u64 + 1).is_err());
    }
}
