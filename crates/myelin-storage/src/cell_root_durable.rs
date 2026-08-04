use sqlx::postgres::PgPool;
use sqlx::Row;

use aes_gcm::aead::OsRng;
use aes_gcm::{Aes256Gcm, KeyInit};

use crate::kms::{SealKey, KEY_LEN, NONCE_LEN};
use crate::pg::PgError;

pub const CELL_TOKEN_ROOT_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS cell_token_root (
    cell_id    text PRIMARY KEY,
    nonce      bytea       NOT NULL,
    ciphertext bytea       NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);";

pub fn cell_root_durable_migrations() -> crate::migration::Migrations {
    use crate::migration::{Migration, Migrations};
    Migrations::of([Migration::plain(
        "0060_cell_token_root",
        CELL_TOKEN_ROOT_MIGRATION,
    )])
}

const MATERIAL_LEN: usize = KEY_LEN * 2;

pub struct CellRootMaterial {
    pub ed25519_seed: [u8; KEY_LEN],
    pub mac_key: [u8; KEY_LEN],
}

impl core::fmt::Debug for CellRootMaterial {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("CellRootMaterial(<redacted seed + mac key>)")
    }
}

#[derive(Debug)]
pub enum CellRootError {
    WrongSealKey {
        cell_id: String,
    },
    Db(PgError),
}

impl core::fmt::Display for CellRootError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CellRootError::WrongSealKey { cell_id } => write!(
                f,
                "capability-token cell authority REFUSED TO START for cell {cell_id}: a sealed cell \
                 root exists but did NOT unseal under the supplied seal key (wrong/absent \
                 MYELIN_KMS_SEAL_KEY) - fail-closed, NEVER generating a new root (that would orphan \
                 every token ever minted under the old root)"
            ),
            CellRootError::Db(e) => write!(f, "durable cell-root store error: {e}"),
        }
    }
}

impl std::error::Error for CellRootError {}

impl From<PgError> for CellRootError {
    fn from(e: PgError) -> Self {
        CellRootError::Db(e)
    }
}

#[derive(Clone)]
pub struct DurableCellRootBacking {
    pool: PgPool,
    cell_id: String,
}

impl DurableCellRootBacking {
    pub fn new(pool: PgPool, cell_id: impl Into<String>) -> DurableCellRootBacking {
        DurableCellRootBacking {
            pool,
            cell_id: cell_id.into(),
        }
    }

    pub fn cell_id(&self) -> &str {
        &self.cell_id
    }

    pub async fn load_or_generate(
        &self,
        seal_key: &SealKey,
    ) -> Result<CellRootMaterial, CellRootError> {
        if let Some((nonce, ciphertext)) = self.read_sealed_root().await? {
            let plain = seal_key.open_bytes(&nonce, &ciphertext).ok_or_else(|| {
                CellRootError::WrongSealKey {
                    cell_id: self.cell_id.clone(),
                }
            })?;
            return Self::material_from_plain(&plain, &self.cell_id);
        }
        let mut plain = [0u8; MATERIAL_LEN];
        plain[..KEY_LEN].copy_from_slice(random_key().as_slice());
        plain[KEY_LEN..].copy_from_slice(random_key().as_slice());
        let (nonce, ciphertext) = seal_key.seal_bytes(&plain);
        self.insert_sealed_root_if_absent(&nonce, &ciphertext).await?;
        let (nonce, ciphertext) = self.read_sealed_root().await?.ok_or_else(|| {
            CellRootError::Db(PgError::Query(
                "sealed cell-authority root vanished immediately after insert".into(),
            ))
        })?;
        let plain = seal_key.open_bytes(&nonce, &ciphertext).ok_or_else(|| {
            CellRootError::WrongSealKey {
                cell_id: self.cell_id.clone(),
            }
        })?;
        Self::material_from_plain(&plain, &self.cell_id)
    }

    fn material_from_plain(plain: &[u8], cell_id: &str) -> Result<CellRootMaterial, CellRootError> {
        if plain.len() != MATERIAL_LEN {
            return Err(CellRootError::Db(PgError::Query(format!(
                "corrupt sealed cell-authority root for cell {cell_id}: unsealed to {} bytes \
                 (expected {MATERIAL_LEN})",
                plain.len()
            ))));
        }
        let mut ed25519_seed = [0u8; KEY_LEN];
        let mut mac_key = [0u8; KEY_LEN];
        ed25519_seed.copy_from_slice(&plain[..KEY_LEN]);
        mac_key.copy_from_slice(&plain[KEY_LEN..]);
        Ok(CellRootMaterial {
            ed25519_seed,
            mac_key,
        })
    }

    async fn read_sealed_root(&self) -> Result<Option<([u8; NONCE_LEN], Vec<u8>)>, PgError> {
        let row = sqlx::query("SELECT nonce, ciphertext FROM cell_token_root WHERE cell_id = $1")
            .bind(&self.cell_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(row.map(|r| {
            let nonce: Vec<u8> = r.get("nonce");
            let ciphertext: Vec<u8> = r.get("ciphertext");
            (nonce_from(&nonce), ciphertext)
        }))
    }

    async fn insert_sealed_root_if_absent(
        &self,
        nonce: &[u8; NONCE_LEN],
        ciphertext: &[u8],
    ) -> Result<(), PgError> {
        sqlx::query(
            "INSERT INTO cell_token_root (cell_id, nonce, ciphertext) VALUES ($1, $2, $3) \
             ON CONFLICT (cell_id) DO NOTHING",
        )
        .bind(&self.cell_id)
        .bind(nonce.as_slice())
        .bind(ciphertext)
        .execute(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }
}

fn random_key() -> [u8; KEY_LEN] {
    let key = Aes256Gcm::generate_key(OsRng);
    let mut bytes = [0u8; KEY_LEN];
    bytes.copy_from_slice(key.as_slice());
    bytes
}

fn nonce_from(bytes: &[u8]) -> [u8; NONCE_LEN] {
    let mut n = [0u8; NONCE_LEN];
    let len = bytes.len().min(NONCE_LEN);
    n[..len].copy_from_slice(&bytes[..len]);
    n
}
