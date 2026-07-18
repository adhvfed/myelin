//! PostgreSQL authority for pull-request lifecycle state (GT-003b).
//!
//! The two tables are always scoped by the verified `(tenant_id, region)` pair. `git_pr_counter`
//! allocates per-target-repository numbers in the same transaction that inserts `git_pr`, so aborted
//! opens consume no number and concurrent committed opens are contiguous. `git_pr.version` is the
//! optimistic identity exposed to update code; production mutations additionally lock the row before
//! deriving the next JSON document, preventing read/modify/write lost updates.

use std::future::Future;
use std::sync::Arc;

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType, IdMinter,
    OutboxTransaction, OutboxTx, UlidMinter, Visibility,
};
use myelin_gdpr::ErasureMethod;
use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, SubjectId};
use myelin_storage::kms::{KekId, KmsEngine, PiiKeyRef, NONCE_LEN};
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::{HotTables, Migration, Migrations, SubstrateProvider, TenantScope};
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;

use crate::core::RepoLoc;
use crate::durable::DurableError;
use crate::events::{GIT_PR_MERGED, GIT_PR_OPENED, GIT_PR_UPDATED};
use crate::lifecycle::PrState;
use crate::pr_store::PrRecord;

pub const GIT_PR_TABLE: &str = "git_pr";
pub const GIT_PR_COUNTER_TABLE: &str = "git_pr_counter";
const PR_RECORD_COLUMNS: &str = "record, head_repo_slug, title_nonce, title_ciphertext, \
title_pii_key_ref, body_nonce, body_ciphertext, body_pii_key_ref";

pub const CREATE_GIT_PR_COUNTER_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS git_pr_counter (
  tenant_id text NOT NULL CHECK (length(tenant_id) > 0),
  region text NOT NULL CHECK (length(region) > 0),
  repo_slug text NOT NULL CHECK (length(repo_slug) > 0),
  high_water bigint NOT NULL CHECK (high_water >= 0),
  PRIMARY KEY (tenant_id, region, repo_slug)
);
SELECT myelin_make_tenant_scoped('git_pr_counter');
"#;

pub const CREATE_GIT_PR_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS git_pr (
  tenant_id text NOT NULL CHECK (length(tenant_id) > 0),
  region text NOT NULL CHECK (length(region) > 0),
  repo_slug text NOT NULL CHECK (length(repo_slug) > 0),
  number bigint NOT NULL CHECK (number > 0),
  head_repo_slug text NOT NULL CHECK (length(head_repo_slug) > 0),
  record jsonb NOT NULL,
  title_nonce bytea NOT NULL,
  title_ciphertext bytea NOT NULL,
  title_pii_key_ref text NOT NULL,
  body_nonce bytea,
  body_ciphertext bytea,
  body_pii_key_ref text,
  version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
  merge_intent jsonb,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, region, repo_slug, number),
  CHECK (jsonb_typeof(record) = 'object'),
  CHECK (record ? 'number' AND jsonb_typeof(record->'number') = 'number'),
  CHECK ((record->>'number')::bigint = number),
  CHECK (record ? 'head_repo_slug' AND jsonb_typeof(record->'head_repo_slug') = 'string'),
  CHECK (record->>'head_repo_slug' = head_repo_slug),
  CHECK (record->>'title' = ''),
  CHECK (record->'body_md' = 'null'::jsonb),
  CHECK ((body_nonce IS NULL) = (body_ciphertext IS NULL)
         AND (body_nonce IS NULL) = (body_pii_key_ref IS NULL)),
  FOREIGN KEY (tenant_id, region, repo_slug)
    REFERENCES git_pr_counter (tenant_id, region, repo_slug)
);
SELECT myelin_make_tenant_scoped('git_pr');
"#;

pub const CREATE_GIT_PR_HEAD_REPO_INDEX_DDL: &str = r#"
CREATE INDEX CONCURRENTLY IF NOT EXISTS git_pr_head_repo_idx
  ON git_pr (tenant_id, region, head_repo_slug, repo_slug, number)
"#;

pub fn git_pr_migrations() -> Migrations {
    Migrations::of([
        Migration::plain_on(
            "git_0001_pr_counter",
            CREATE_GIT_PR_COUNTER_DDL,
            GIT_PR_COUNTER_TABLE,
        ),
        Migration::plain_on("git_0002_pr", CREATE_GIT_PR_DDL, GIT_PR_TABLE),
        Migration::plain_on(
            "git_0003_pr_head_repo_index",
            CREATE_GIT_PR_HEAD_REPO_INDEX_DDL,
            GIT_PR_TABLE,
        ),
    ])
}

pub fn git_pr_hot_tables() -> HotTables {
    HotTables::declare([GIT_PR_TABLE, GIT_PR_COUNTER_TABLE])
}

/// A pending ref mutation, persisted before the existing ref-CAS is attempted.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MergeIntent {
    pub operation_id: String,
    pub base_ref: String,
    pub expected_old_oid: String,
    pub head_oid: String,
    pub head_repo_slug: String,
}

/// Closed production mutation vocabulary. Callers cannot select an event type or overwrite an
/// arbitrary JSON document.
#[derive(Clone, Debug)]
pub enum PrMutation {
    ReportChecks {
        green_contexts: Option<Vec<String>>,
        fork_unendorsed_contexts: Option<Vec<String>>,
        codeowner_review_satisfied: Option<bool>,
        outstanding_conversations: Option<u32>,
    },
    SubmitReview(crate::pr_store::ReviewRecord),
    EndorseContexts(Vec<String>),
    Touch,
}

struct SealedPrRecord {
    record: serde_json::Value,
    title: EncryptedColumn,
    body: Option<EncryptedColumn>,
}

/// Provider-only PostgreSQL PR authority. It exposes no raw pool constructor.
#[derive(Clone)]
pub struct PgPrStore {
    provider: SubstrateProvider,
    kms: Arc<KmsEngine>,
    runtime: tokio::runtime::Handle,
    minter: Arc<dyn IdMinter>,
}

impl PgPrStore {
    pub fn new(
        provider: SubstrateProvider,
        kms: Arc<KmsEngine>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            provider,
            kms,
            runtime,
            minter: Arc::new(UlidMinter::new()),
        }
    }

    #[cfg(any(test, feature = "integration"))]
    pub fn with_minter(
        provider: SubstrateProvider,
        kms: Arc<KmsEngine>,
        runtime: tokio::runtime::Handle,
        minter: Arc<dyn IdMinter>,
    ) -> Self {
        Self {
            provider,
            kms,
            runtime,
            minter,
        }
    }

    fn block_on<F: Future>(&self, future: F) -> F::Output {
        tokio::task::block_in_place(|| self.runtime.block_on(future))
    }

    fn scoped_loc(&self, scope: &TenantScope, repo: &str) -> Result<RepoLoc, DurableError> {
        if scope.region().0 != self.provider.config().region {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        if scope.tenant().0.is_empty() || repo.is_empty() {
            return Err(DurableError::Git("empty PR repository scope".into()));
        }
        Ok(RepoLoc::new(
            scope.tenant().0.clone(),
            scope.region().0.clone(),
            repo,
        ))
    }

    pub fn get(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
    ) -> Result<Option<PrRecord>, DurableError> {
        let loc = self.scoped_loc(scope, repo)?;
        let number = db_number(number)?;
        let provider = self.provider.clone();
        let kms = self.kms.clone();
        let region = Region(loc.region.clone());
        self.block_on(async move {
            provider
                .with_tenant_tx(&loc.tenant.clone(), move |conn| {
                    Box::pin(async move {
                        sqlx::query(&format!(
                            "SELECT {PR_RECORD_COLUMNS} FROM git_pr \
                             WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4"
                        ))
                        .bind(&loc.tenant)
                        .bind(&loc.region)
                        .bind(&loc.repo)
                        .bind(number)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))
                    })
                })
                .await
        })
        .map_err(pg_error)?
        .map(|row| decode_record(&kms, &region, row))
        .transpose()
    }

    pub fn list(&self, scope: &TenantScope, repo: &str) -> Result<Vec<PrRecord>, DurableError> {
        let loc = self.scoped_loc(scope, repo)?;
        let provider = self.provider.clone();
        let kms = self.kms.clone();
        let region = Region(loc.region.clone());
        let rows = self
            .block_on(async move {
                provider
                    .with_tenant_tx(&loc.tenant.clone(), move |conn| {
                        Box::pin(async move {
                            sqlx::query(&format!(
                                "SELECT {PR_RECORD_COLUMNS} FROM git_pr \
                                 WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 ORDER BY number"
                            ))
                            .bind(&loc.tenant)
                            .bind(&loc.region)
                            .bind(&loc.repo)
                            .fetch_all(&mut *conn)
                            .await
                            .map_err(|e| myelin_storage::PgError::Query(e.to_string()))
                        })
                    })
                    .await
            })
            .map_err(pg_error)?;
        rows.into_iter()
            .map(|row| decode_record(&kms, &region, row))
            .collect()
    }

    /// Allocate the number, insert the row and stage `git.pr.opened` in one transaction.
    pub fn open(
        &self,
        scope: &TenantScope,
        repo: &str,
        mut record: PrRecord,
        ctx: EmitContextBase,
        abort_after_event: bool,
    ) -> Result<PrRecord, DurableError> {
        let loc = self.scoped_loc(scope, repo)?;
        if record.head_repo_slug.is_empty() {
            return Err(DurableError::Git(
                "production PR row requires explicit head repository provenance".into(),
            ));
        }
        let provider = self.provider.clone();
        let minter = self.minter.clone();
        let kms = self.kms.clone();
        let crypto_region = Region(loc.region.clone());
        self.block_on(async move {
            provider
                .with_tenant_tx(&loc.tenant.clone(), move |conn| {
                    Box::pin(async move {
                        let number: i64 = sqlx::query_scalar(
                            "INSERT INTO git_pr_counter (tenant_id,region,repo_slug,high_water) \
                             VALUES ($1,$2,$3,1) ON CONFLICT (tenant_id,region,repo_slug) \
                             DO UPDATE SET high_water=git_pr_counter.high_water+1 RETURNING high_water",
                        )
                        .bind(&loc.tenant)
                        .bind(&loc.region)
                        .bind(&loc.repo)
                        .fetch_one(&mut *conn)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                        record.number = number as u64;
                        let sealed = seal_pr_record(
                            &kms,
                            crypto_region,
                            &TenantId(loc.tenant.clone()),
                            &record,
                        )
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                        sqlx::query(
                            "INSERT INTO git_pr \
                             (tenant_id,region,repo_slug,number,head_repo_slug,record,version, \
                              title_nonce,title_ciphertext,title_pii_key_ref,body_nonce, \
                              body_ciphertext,body_pii_key_ref) \
                             VALUES ($1,$2,$3,$4,$5,$6,1,$7,$8,$9,$10,$11,$12)",
                        )
                        .bind(&loc.tenant)
                        .bind(&loc.region)
                        .bind(&loc.repo)
                        .bind(number)
                        .bind(&record.head_repo_slug)
                        .bind(sealed.record)
                        .bind(sealed.title.nonce.to_vec())
                        .bind(sealed.title.ciphertext)
                        .bind(sealed.title.key_ref.to_uri())
                        .bind(sealed.body.as_ref().map(|column| column.nonce.to_vec()))
                        .bind(sealed.body.as_ref().map(|column| column.ciphertext.clone()))
                        .bind(sealed.body.as_ref().map(|column| column.key_ref.to_uri()))
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                        co_commit_event(conn, minter, ctx, &loc, &record, GIT_PR_OPENED, None).await?;
                        if abort_after_event {
                            return Err(myelin_storage::PgError::Query(
                                "injected abort after PR event".into(),
                            ));
                        }
                        Ok(record)
                    })
                })
                .await
        })
        .map_err(pg_error)
    }

    /// Serialize a record mutation with `FOR UPDATE`, then co-commit its canonical lifecycle event.
    fn mutate<F>(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        ctx: EmitContextBase,
        event_type: &'static str,
        mutation: F,
    ) -> Result<PrRecord, DurableError>
    where
        F: FnOnce(&mut PrRecord) -> Result<(), DurableError> + Send + 'static,
    {
        let loc = self.scoped_loc(scope, repo)?;
        let number_db = db_number(number)?;
        let provider = self.provider.clone();
        let minter = self.minter.clone();
        let kms = self.kms.clone();
        let crypto_region = Region(loc.region.clone());
        self.block_on(async move {
            provider
                .with_tenant_tx(&loc.tenant.clone(), move |conn| {
                    Box::pin(async move {
                        let row = sqlx::query(&format!(
                            "SELECT {PR_RECORD_COLUMNS} FROM git_pr \
                             WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4 FOR UPDATE"
                        ))
                        .bind(&loc.tenant)
                        .bind(&loc.region)
                        .bind(&loc.repo)
                        .bind(number_db)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?
                        .ok_or_else(|| myelin_storage::PgError::Query(format!("PR #{number} not found")))?;
                        let mut record = decode_record(&kms, &crypto_region, row)
                            .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                        mutation(&mut record)
                            .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                        let sealed = seal_pr_record(
                            &kms,
                            crypto_region.clone(),
                            &TenantId(loc.tenant.clone()),
                            &record,
                        )
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                        sqlx::query(
                            "UPDATE git_pr SET record=$5, head_repo_slug=$6, version=version+1, \
                             title_nonce=$7,title_ciphertext=$8,title_pii_key_ref=$9, \
                             body_nonce=$10,body_ciphertext=$11,body_pii_key_ref=$12, \
                             updated_at=now() WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4",
                        )
                        .bind(&loc.tenant)
                        .bind(&loc.region)
                        .bind(&loc.repo)
                        .bind(number_db)
                        .bind(sealed.record)
                        .bind(&record.head_repo_slug)
                        .bind(sealed.title.nonce.to_vec())
                        .bind(sealed.title.ciphertext)
                        .bind(sealed.title.key_ref.to_uri())
                        .bind(sealed.body.as_ref().map(|column| column.nonce.to_vec()))
                        .bind(sealed.body.as_ref().map(|column| column.ciphertext.clone()))
                        .bind(sealed.body.as_ref().map(|column| column.key_ref.to_uri()))
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                        co_commit_event(conn, minter, ctx, &loc, &record, event_type, None).await?;
                        Ok(record)
                    })
                })
                .await
        })
        .map_err(pg_error)
    }

    pub fn apply_mutation(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        ctx: EmitContextBase,
        mutation: PrMutation,
    ) -> Result<PrRecord, DurableError> {
        self.mutate(scope, repo, number, ctx, GIT_PR_UPDATED, move |record| {
            match mutation {
                PrMutation::ReportChecks {
                    green_contexts,
                    fork_unendorsed_contexts,
                    codeowner_review_satisfied,
                    outstanding_conversations,
                } => {
                    if let Some(value) = green_contexts {
                        record.green_contexts = value;
                    }
                    if let Some(value) = fork_unendorsed_contexts {
                        record.fork_unendorsed_contexts = value;
                    }
                    if let Some(value) = codeowner_review_satisfied {
                        record.codeowner_review_satisfied = value;
                    }
                    if let Some(value) = outstanding_conversations {
                        record.outstanding_conversations = value;
                    }
                }
                PrMutation::SubmitReview(review) => record.reviews.push(review),
                PrMutation::EndorseContexts(contexts) => {
                    for context in contexts {
                        if !record.endorsed_contexts.contains(&context) {
                            record.endorsed_contexts.push(context);
                        }
                    }
                }
                PrMutation::Touch => {}
            }
            record.updated_at = Some(now_unix());
            Ok(())
        })
    }

    pub fn begin_merge(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        intent: MergeIntent,
        ctx: EmitContextBase,
    ) -> Result<PrRecord, DurableError> {
        let intent_json = serde_json::to_value(&intent)
            .map_err(|e| DurableError::Git(format!("encode merge intent: {e}")))?;
        let intent_for_event = intent.clone();
        self.mutate_with_intent(
            scope,
            repo,
            number,
            ctx,
            Some(intent_json),
            None,
            move |record| {
                if record.state == PrState::Merged {
                    return Err(DurableError::Git("PR is already merged".into()));
                }
                if record.head_oid != intent_for_event.head_oid
                    || record.head_repo_slug != intent_for_event.head_repo_slug
                    || record.base_ref != intent_for_event.base_ref
                {
                    return Err(DurableError::Git(
                        "merge intent diverges from locked PR provenance".into(),
                    ));
                }
                Ok(())
            },
        )
    }

    pub fn finalize_merge(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        intent: &MergeIntent,
        ctx: EmitContextBase,
    ) -> Result<PrRecord, DurableError> {
        if intent.operation_id.is_empty() {
            return Err(DurableError::Git("empty merge operation id".into()));
        }
        let expected = serde_json::to_value(intent)
            .map_err(|e| DurableError::Git(format!("encode merge intent: {e}")))?;
        self.mutate_with_intent(
            scope,
            repo,
            number,
            ctx,
            None,
            Some(expected),
            move |record| {
                if record.state != PrState::Merged {
                    record.state = PrState::Merged;
                    record.updated_at = Some(now_unix());
                }
                Ok(())
            },
        )
    }

    fn mutate_with_intent<F>(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        ctx: EmitContextBase,
        new_intent: Option<serde_json::Value>,
        expected_finalize_intent: Option<serde_json::Value>,
        mutation: F,
    ) -> Result<PrRecord, DurableError>
    where
        F: FnOnce(&mut PrRecord) -> Result<(), DurableError> + Send + 'static,
    {
        let event_type = if new_intent.is_some() {
            GIT_PR_UPDATED
        } else {
            GIT_PR_MERGED
        };
        let loc = self.scoped_loc(scope, repo)?;
        let number_db = db_number(number)?;
        let provider = self.provider.clone();
        let minter = self.minter.clone();
        let kms = self.kms.clone();
        let crypto_region = Region(loc.region.clone());
        self.block_on(async move {
            provider.with_tenant_tx(&loc.tenant.clone(), move |conn| Box::pin(async move {
                let row = sqlx::query(&format!("SELECT {PR_RECORD_COLUMNS}, merge_intent FROM git_pr WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4 FOR UPDATE"))
                    .bind(&loc.tenant).bind(&loc.region).bind(&loc.repo).bind(number_db)
                    .fetch_optional(&mut *conn).await.map_err(|e| myelin_storage::PgError::Query(e.to_string()))?
                    .ok_or_else(|| myelin_storage::PgError::Query(format!("PR #{number} not found")))?;
                let existing_intent: Option<serde_json::Value> = row.try_get("merge_intent").map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                if let (Some(existing), Some(requested)) = (&existing_intent, &new_intent) {
                    if existing != requested { return Err(myelin_storage::PgError::Query("a different merge intent is already pending".into())); }
                    return decode_record(&kms, &crypto_region, row).map_err(|e| myelin_storage::PgError::Query(e.to_string()));
                }
                let mut record = decode_record(&kms, &crypto_region, row).map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                let already_merged = record.state == PrState::Merged;
                if new_intent.is_none() && existing_intent.as_ref() != expected_finalize_intent.as_ref() {
                    return Err(myelin_storage::PgError::Query("merge finalize intent does not match the durable operation".into()));
                }
                mutation(&mut record).map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                if already_merged && new_intent.is_none() { return Ok(record); }
                let sealed = seal_pr_record(&kms, crypto_region.clone(), &TenantId(loc.tenant.clone()), &record)
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                let persisted_intent = new_intent.as_ref().or(existing_intent.as_ref());
                sqlx::query("UPDATE git_pr SET record=$5, version=version+1, merge_intent=$6, \
                    title_nonce=$7,title_ciphertext=$8,title_pii_key_ref=$9,body_nonce=$10, \
                    body_ciphertext=$11,body_pii_key_ref=$12,updated_at=now() \
                    WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4")
                    .bind(&loc.tenant).bind(&loc.region).bind(&loc.repo).bind(number_db)
                    .bind(sealed.record).bind(persisted_intent)
                    .bind(sealed.title.nonce.to_vec()).bind(sealed.title.ciphertext).bind(sealed.title.key_ref.to_uri())
                    .bind(sealed.body.as_ref().map(|column| column.nonce.to_vec()))
                    .bind(sealed.body.as_ref().map(|column| column.ciphertext.clone()))
                    .bind(sealed.body.as_ref().map(|column| column.key_ref.to_uri()))
                    .execute(&mut *conn).await.map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                co_commit_event(conn, minter, ctx, &loc, &record, event_type, persisted_intent).await?;
                Ok(record)
            })).await
        }).map_err(pg_error)
    }
}

async fn co_commit_event(
    conn: &mut sqlx::PgConnection,
    minter: Arc<dyn IdMinter>,
    ctx: EmitContextBase,
    loc: &RepoLoc,
    record: &PrRecord,
    event_type: &'static str,
    operation: Option<&serde_json::Value>,
) -> Result<(), myelin_storage::PgError> {
    let mut tx = OutboxTransaction::detached(minter, ctx);
    tx.emit(
        EventDraft {
            type_: EventType(event_type.into()),
            subject: ArtifactRef(format!(
                "myelin://{}/git/pr/{}:{}",
                loc.tenant, loc.repo, record.number
            )),
            aggregate: AggregateKey(format!("git/pr/{}:{}", loc.repo, record.number)),
            payload: serde_json::json!({
                "repo": loc.repo,
                "number": record.number,
                "base_ref": record.base_ref,
                "head_repo": record.head_repo_slug,
                "head_ref": record.head_ref,
                "head_oid": record.head_oid,
                "is_fork": record.head_repo_slug != loc.repo,
                "state": format!("{:?}", record.state).to_ascii_lowercase(),
                "operation": operation,
            }),
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        None,
    )
    .map_err(|e| myelin_storage::PgError::Query(e.0))?;
    let row = tx
        .into_staged_rows()
        .map_err(|e| myelin_storage::PgError::Query(e.0))?
        .pop()
        .ok_or_else(|| myelin_storage::PgError::Query("missing PR envelope".into()))?;
    PgRelay::co_commit_in_tx(conn, &row.aggregate.0, &row.envelope).await
}

fn seal_pr_record(
    kms: &KmsEngine,
    region: Region,
    tenant: &TenantId,
    record: &PrRecord,
) -> Result<SealedPrRecord, DurableError> {
    kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
    let cryptor = ColumnCryptor::new(kms, region);
    let subject = SubjectId::new(record.author_pseudonym.clone());
    let erasure = ErasureMethod::CryptoShred("subject_dek".into());
    let title = cryptor
        .encrypt(tenant, Some(&subject), &erasure, record.title.as_bytes())
        .map_err(|e| DurableError::Io(format!("encrypt PR title: {e:?}")))?;
    let body = record
        .body_md
        .as_ref()
        .map(|body| {
            cryptor
                .encrypt(tenant, Some(&subject), &erasure, body.as_bytes())
                .map_err(|e| DurableError::Io(format!("encrypt PR body: {e:?}")))
        })
        .transpose()?;
    let mut projection = record.clone();
    projection.title.clear();
    projection.body_md = None;
    let record = serde_json::to_value(projection)
        .map_err(|e| DurableError::Io(format!("encode PR projection: {e}")))?;
    Ok(SealedPrRecord {
        record,
        title,
        body,
    })
}

fn decode_record(
    kms: &KmsEngine,
    region: &Region,
    row: sqlx::postgres::PgRow,
) -> Result<PrRecord, DurableError> {
    let value: serde_json::Value = row
        .try_get("record")
        .map_err(|e| DurableError::Io(format!("decode PR json: {e}")))?;
    let mut record: PrRecord = serde_json::from_value(value)
        .map_err(|e| DurableError::Io(format!("parse PR json: {e}")))?;
    let head_repo: String = row
        .try_get("head_repo_slug")
        .map_err(|e| DurableError::Io(format!("decode PR provenance: {e}")))?;
    if record.head_repo_slug != head_repo {
        return Err(DurableError::Io(
            "PR JSON provenance diverges from authoritative column".into(),
        ));
    }
    if !record.title.is_empty() || record.body_md.is_some() {
        return Err(DurableError::Io(
            "PR projection contains plaintext free text".into(),
        ));
    }
    let title = encrypted_column(&row, "title")?.ok_or_else(|| {
        DurableError::Io("PR title ciphertext is missing from authoritative row".into())
    })?;
    let cryptor = ColumnCryptor::new(kms, region.clone());
    record.title = String::from_utf8(
        cryptor
            .decrypt(&title)
            .map_err(|e| DurableError::Io(format!("decrypt PR title: {e:?}")))?,
    )
    .map_err(|_| DurableError::Io("decrypted PR title is not UTF-8".into()))?;
    if let Some(body) = encrypted_column(&row, "body")? {
        record.body_md = Some(
            String::from_utf8(
                cryptor
                    .decrypt(&body)
                    .map_err(|e| DurableError::Io(format!("decrypt PR body: {e:?}")))?,
            )
            .map_err(|_| DurableError::Io("decrypted PR body is not UTF-8".into()))?,
        );
    }
    Ok(record)
}

fn encrypted_column(
    row: &sqlx::postgres::PgRow,
    prefix: &str,
) -> Result<Option<EncryptedColumn>, DurableError> {
    let nonce_name = format!("{prefix}_nonce");
    let ciphertext_name = format!("{prefix}_ciphertext");
    let key_name = format!("{prefix}_pii_key_ref");
    let nonce: Option<Vec<u8>> = row
        .try_get(nonce_name.as_str())
        .map_err(|e| DurableError::Io(format!("decode {nonce_name}: {e}")))?;
    let ciphertext: Option<Vec<u8>> = row
        .try_get(ciphertext_name.as_str())
        .map_err(|e| DurableError::Io(format!("decode {ciphertext_name}: {e}")))?;
    let key_ref: Option<String> = row
        .try_get(key_name.as_str())
        .map_err(|e| DurableError::Io(format!("decode {key_name}: {e}")))?;
    match (nonce, ciphertext, key_ref) {
        (None, None, None) => Ok(None),
        (Some(nonce), Some(ciphertext), Some(key_ref)) => {
            let nonce: [u8; NONCE_LEN] = nonce
                .try_into()
                .map_err(|_| DurableError::Io(format!("{nonce_name} has invalid length")))?;
            let key_ref = PiiKeyRef::parse(&key_ref)
                .ok_or_else(|| DurableError::Io(format!("{key_name} is malformed")))?;
            Ok(Some(EncryptedColumn {
                key_ref,
                nonce,
                ciphertext,
            }))
        }
        _ => Err(DurableError::Io(format!(
            "partial {prefix} encrypted-column tuple"
        ))),
    }
}

fn pg_error(error: impl std::fmt::Display) -> DurableError {
    let message = error.to_string();
    if message.contains("not found") {
        DurableError::NotFound(message)
    } else {
        DurableError::Io(message)
    }
}

fn db_number(number: u64) -> Result<i64, DurableError> {
    i64::try_from(number).map_err(|_| DurableError::Git("PR number exceeds bigint".into()))
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_partition_first_forced_rls_and_has_concurrency_anchors() {
        for ddl in [CREATE_GIT_PR_COUNTER_DDL, CREATE_GIT_PR_DDL] {
            assert!(ddl.contains("tenant_id text NOT NULL CHECK"));
            assert!(ddl.contains("region text NOT NULL CHECK"));
            assert!(ddl.contains("myelin_make_tenant_scoped"));
            assert!(!ddl.contains("DISABLE ROW LEVEL SECURITY"));
        }
        assert!(CREATE_GIT_PR_DDL.contains("head_repo_slug text NOT NULL"));
        assert!(CREATE_GIT_PR_DDL.contains("version bigint NOT NULL"));
        assert!(CREATE_GIT_PR_DDL.contains("merge_intent jsonb"));
        assert!(CREATE_GIT_PR_DDL.contains("title_ciphertext bytea NOT NULL"));
        assert!(CREATE_GIT_PR_DDL.contains("record->>'title' = ''"));
        assert!(CREATE_GIT_PR_DDL.contains("record->'body_md' = 'null'::jsonb"));
        assert!(!CREATE_GIT_PR_DDL.contains("title text"));
        assert!(CREATE_GIT_PR_COUNTER_DDL.contains("PRIMARY KEY (tenant_id, region, repo_slug)"));
    }

    #[test]
    fn migration_set_is_forward_only_and_declares_mutated_tables_hot() {
        let migrations = git_pr_migrations();
        assert_eq!(migrations.0.len(), 3);
        for migration in &migrations.0 {
            let upper = migration.ddl.to_ascii_uppercase();
            assert!(!upper.contains("DROP TABLE"));
            assert!(!upper.contains("TRUNCATE"));
        }
        let hot = git_pr_hot_tables();
        assert!(hot.is_hot(GIT_PR_TABLE));
        assert!(hot.is_hot(GIT_PR_COUNTER_TABLE));
        assert!(CREATE_GIT_PR_HEAD_REPO_INDEX_DDL.contains("CREATE INDEX CONCURRENTLY"));
        let mut runner = myelin_substrate::MigrationRunner::new();
        runner
            .run(&migrations, &hot)
            .expect("production boot validator admits the exact migration set");
    }

    #[test]
    fn free_text_projection_is_ciphertext_only() {
        let kms = KmsEngine::new();
        let tenant = TenantId("acme".into());
        let pr = crate::lifecycle::PullRequest::open(
            1,
            "refs/heads/main",
            "refs/heads/feature",
            "author@acme.noreply",
            false,
        );
        let mut record = PrRecord::open(&pr, "a".repeat(40));
        record.head_repo_slug = "fork".into();
        record.title = "private launch title".into();
        record.body_md = Some("private launch body".into());
        let sealed = seal_pr_record(&kms, Region("fr-par".into()), &tenant, &record)
            .expect("seal PR free text");
        let encoded = serde_json::to_string(&sealed.record).unwrap();
        assert!(!encoded.contains("private launch"));
        assert!(!sealed.title.contains_plaintext(record.title.as_bytes()));
        assert!(!sealed
            .body
            .as_ref()
            .unwrap()
            .contains_plaintext(record.body_md.as_ref().unwrap().as_bytes()));
    }
}
