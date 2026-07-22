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
    Actor, AggregateKey, ArtifactRef, CausedBy, DataRole, EmitContextBase, EventDraft, EventType,
    IdMinter, OutboxTransaction, OutboxTx, Timestamp, UlidMinter, Visibility,
};
use myelin_gdpr::ErasureMethod;
use myelin_identity::Principal;
use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, SubjectId};
use myelin_storage::kms::{KekId, KeyClass, KmsEngine, PiiKeyRef, NONCE_LEN};
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::{
    HotTables, Migration, MigrationPhase, Migrations, SubstrateProvider, TenantScope,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;

use crate::core::{Oid as CoreOid, RepoLoc};
use crate::durable::{DurableError, DurableGitRepo};
use crate::events::{
    event_actor_pseudonym, pseudonymized_event_principal, GIT_PR_MERGED, GIT_PR_OPENED,
    GIT_PR_UPDATED,
};
use crate::lifecycle::PrState;
use crate::pr_store::{evaluate_merge, DurablePrStore, MergeAttempt, MergeEval, PrRecord};
use crate::receive_pack::{
    CrashPoint, InMemoryObjectDb, Oid as PushOid, ProposedRefUpdate, PushOutcome, PushSession,
    Pusher, RefName, RefStore, RejectReason,
};

pub const GIT_PR_TABLE: &str = "git_pr";
pub const GIT_PR_COUNTER_TABLE: &str = "git_pr_counter";
pub const GIT_PR_COMMAND_TABLE: &str = "git_pr_command";
const PR_RECORD_COLUMNS: &str = "record, head_repo_slug, title_nonce, title_ciphertext, \
title_pii_key_ref, body_nonce, body_ciphertext, body_pii_key_ref, author_subject_id";

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
  author_subject_id text NOT NULL CHECK (length(author_subject_id) > 0),
  record jsonb NOT NULL,
  title_nonce bytea NOT NULL,
  title_ciphertext bytea NOT NULL,
  title_pii_key_ref text NOT NULL CHECK (length(title_pii_key_ref) > 0),
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
  CHECK (record ? 'author_subject_id'
         AND jsonb_typeof(record->'author_subject_id') = 'string'
         AND record->>'author_subject_id' = ''),
  CHECK (record ? 'title' AND jsonb_typeof(record->'title') = 'string'
         AND record->>'title' = ''),
  CHECK (record ? 'body_md' AND record->'body_md' = 'null'::jsonb),
  CHECK (octet_length(title_nonce) = 12),
  CHECK (octet_length(title_ciphertext) > 0),
  CHECK ((body_nonce IS NULL) = (body_ciphertext IS NULL)
         AND (body_nonce IS NULL) = (body_pii_key_ref IS NULL)),
  CHECK (body_nonce IS NULL OR octet_length(body_nonce) = 12),
  CHECK (body_ciphertext IS NULL OR octet_length(body_ciphertext) > 0),
  CHECK (body_pii_key_ref IS NULL OR length(body_pii_key_ref) > 0),
  FOREIGN KEY (tenant_id, region, repo_slug)
    REFERENCES git_pr_counter (tenant_id, region, repo_slug)
);
SELECT myelin_make_tenant_scoped('git_pr');
"#;

pub const CREATE_GIT_PR_HEAD_REPO_INDEX_DDL: &str = r#"
CREATE INDEX CONCURRENTLY IF NOT EXISTS git_pr_head_repo_idx
  ON git_pr (tenant_id, region, head_repo_slug, repo_slug, number)
"#;

pub const CREATE_GIT_PR_COMMAND_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS git_pr_command (
  tenant_id text NOT NULL CHECK (length(tenant_id) > 0),
  region text NOT NULL CHECK (length(region) > 0),
  repo_slug text NOT NULL CHECK (length(repo_slug) > 0),
  operation_id text NOT NULL CHECK (length(operation_id) BETWEEN 1 AND 128),
  actor_subject_id text NOT NULL CHECK (length(actor_subject_id) BETWEEN 1 AND 255),
  command_kind text NOT NULL CHECK (length(command_kind) > 0),
  payload_hash text NOT NULL CHECK (length(payload_hash) = 64),
  pr_number bigint NOT NULL CHECK (pr_number > 0),
  status text NOT NULL CHECK (status IN ('pending', 'completed', 'cancelled')),
  result jsonb,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, region, repo_slug, operation_id),
  CHECK ((status = 'pending') = (result IS NULL)),
  CHECK (result IS NULL OR jsonb_typeof(result) = 'object'),
  FOREIGN KEY (tenant_id, region, repo_slug, pr_number)
    REFERENCES git_pr (tenant_id, region, repo_slug, number)
);
SELECT myelin_make_tenant_scoped('git_pr_command');
"#;

pub const REQUIRE_GIT_PR_COMMAND_DIGEST_DDL: &str = r#"
ALTER TABLE git_pr_command
  ADD CONSTRAINT git_pr_command_operation_digest_check
  CHECK (length(operation_id) = 64 AND operation_id ~ '^[0-9a-f]{64}$');
"#;

/// Expand the command namespace from repository-local to tenant/region-global without rewriting
/// the hot table or changing its repository FK/primary key. `CONCURRENTLY` keeps writers live; a
/// pre-existing cross-repository collision fails deployment loudly and must be investigated rather
/// than arbitrarily discarded.
pub const CREATE_GIT_PR_COMMAND_OPERATION_SCOPE_INDEX_DDL: &str = r#"
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS git_pr_command_operation_scope_uidx
  ON git_pr_command (tenant_id, region, operation_id)
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
        Migration::plain_on(
            "git_0004_pr_command",
            CREATE_GIT_PR_COMMAND_DDL,
            GIT_PR_COMMAND_TABLE,
        ),
        Migration::plain_on(
            "git_0005_pr_command_digest_only",
            REQUIRE_GIT_PR_COMMAND_DIGEST_DDL,
            GIT_PR_COMMAND_TABLE,
        ),
        Migration::phased(
            "git_0006_pr_command_operation_scope_index",
            CREATE_GIT_PR_COMMAND_OPERATION_SCOPE_INDEX_DDL,
            MigrationPhase::Expand,
            GIT_PR_COMMAND_TABLE,
        ),
    ])
}

pub fn git_pr_hot_tables() -> HotTables {
    HotTables::declare([GIT_PR_TABLE, GIT_PR_COUNTER_TABLE, GIT_PR_COMMAND_TABLE])
}

/// Bounded request identity for response-lost retry safety. It is opaque, PII-free, and scoped by
/// `(tenant, region)` in the command table. Repository remains payload/target authority and a
/// cross-repository replay under the same operation id is rejected loudly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrOperationId(String);

impl PrOperationId {
    pub fn parse(value: &str) -> Result<Self, DurableError> {
        let value = value.trim();
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(DurableError::Git("invalid PR operation id".into()));
        }
        Ok(Self::digest_parts(
            "myelin.git.external-operation-id.v1",
            &[value.as_bytes()],
        ))
    }

    /// Derive a stable, PII-free operation identity without retaining any raw source material.
    pub fn derive(domain: &str, parts: &[&[u8]]) -> Result<Self, DurableError> {
        if domain.is_empty() || domain.len() > 128 || !domain.is_ascii() {
            return Err(DurableError::Git("invalid PR operation-id domain".into()));
        }
        Ok(Self::digest_parts(domain, parts))
    }

    fn from_stored_digest(value: &str) -> Result<Self, DurableError> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(DurableError::Git(
                "stored PR operation digest is malformed".into(),
            ));
        }
        Ok(Self(value.to_owned()))
    }

    fn digest_parts(domain: &str, parts: &[&[u8]]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(domain.as_bytes());
        hasher.update(&[0]);
        for part in parts {
            hasher.update(&(part.len() as u64).to_be_bytes());
            hasher.update(part);
        }
        Self(hasher.finalize().to_hex().to_string())
    }

    fn as_str(&self) -> &str {
        &self.0
    }

    pub fn digest(&self) -> &str {
        &self.0
    }
}

/// A pending ref mutation, persisted before the existing ref-CAS is attempted.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MergeIntent {
    pub operation_id: String,
    pub actor_subject_id: String,
    pub base_ref: String,
    pub expected_old_oid: String,
    pub head_oid: String,
    pub head_repo_slug: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingMerge {
    pub repo_slug: String,
    pub number: u64,
    pub intent: MergeIntent,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
enum MergeCommandResult {
    Merged {
        base_ref: String,
        new_oid: String,
        update_seq: u64,
    },
    Blocked {
        evaluation: MergeEval,
    },
    InvalidHead {
        reason: String,
    },
    RefRefused {
        base_ref: String,
        expected: String,
        actual: String,
    },
}

impl MergeCommandResult {
    fn into_attempt(self) -> MergeAttempt {
        match self {
            Self::Merged {
                base_ref,
                new_oid,
                update_seq,
            } => MergeAttempt::Merged {
                base_ref,
                new_oid,
                update_seq,
            },
            Self::Blocked { evaluation } => MergeAttempt::Blocked(evaluation),
            Self::InvalidHead { reason } => MergeAttempt::InvalidHead(reason),
            Self::RefRefused {
                base_ref,
                expected,
                actual,
            } => MergeAttempt::RefRefused(RejectReason::NonFastForward {
                ref_name: RefName::new(base_ref),
                expected: PushOid::new(expected),
                actual: PushOid::new(actual),
            }),
        }
    }
}

enum MergeLedgerState {
    Absent,
    Pending,
    Terminal(MergeAttempt),
}

/// Closed production mutation vocabulary. Callers cannot select an event type or overwrite an
/// arbitrary JSON document.
#[derive(Clone, Debug, serde::Serialize)]
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

/// Verified tenant, residency, and repository authority for one PR-store operation.
#[derive(Clone, PartialEq, Eq)]
struct VerifiedRepoScope {
    tenant_id: TenantId,
    region: Region,
    loc: RepoLoc,
}

impl VerifiedRepoScope {
    fn new(scope: &TenantScope, repo: &str, provider_region: &str) -> Result<Self, DurableError> {
        if scope.region().0 != provider_region {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        if scope.tenant().0.is_empty() || repo.is_empty() {
            return Err(DurableError::Git("empty PR repository scope".into()));
        }
        let tenant_id = scope.tenant().clone();
        let region = scope.region().clone();
        let loc = RepoLoc::new(tenant_id.0.clone(), region.0.clone(), repo);
        Ok(Self {
            tenant_id,
            region,
            loc,
        })
    }
}

impl core::fmt::Debug for VerifiedRepoScope {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VerifiedRepoScope")
            .field("tenant_id", &"<redacted>")
            .field("region", &"<redacted>")
            .field("repo", &"<redacted>")
            .finish()
    }
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
    ) -> Result<Self, DurableError> {
        provider
            .require_validated_runtime()
            .map_err(|_| DurableError::Io("PostgreSQL PR runtime capability refused".into()))?;
        if provider.config().region.trim().is_empty() {
            return Err(DurableError::Io(
                "PostgreSQL PR provider has no residency region".into(),
            ));
        }
        Ok(Self {
            provider,
            kms,
            runtime,
            minter: Arc::new(UlidMinter::new()),
        })
    }

    #[cfg(any(test, feature = "test-support"))]
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
        self.scoped_target(scope, repo).map(|target| target.loc)
    }

    fn scoped_target(
        &self,
        scope: &TenantScope,
        repo: &str,
    ) -> Result<VerifiedRepoScope, DurableError> {
        VerifiedRepoScope::new(scope, repo, &self.provider.config().region)
    }

    fn emit_context(
        &self,
        scope: &TenantScope,
        principal: &Principal,
    ) -> Result<EmitContextBase, DurableError> {
        if principal.tenant != *scope.tenant() || principal.region != *scope.region() {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        if scope.region().0 != self.provider.config().region {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        let timestamp = now_rfc3339();
        let event_principal =
            pseudonymized_event_principal(scope.tenant().as_str(), principal);
        Ok(EmitContextBase {
            tenant: scope.tenant().clone(),
            region: scope.region().clone(),
            actor: Actor(event_principal),
            schema_ver: 1,
            occurred_at: timestamp.clone(),
            recorded_at: timestamp,
            caused_by: None,
        })
    }

    pub fn get(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
    ) -> Result<Option<PrRecord>, DurableError> {
        let target = self.scoped_target(scope, repo)?;
        let number = db_number(number)?;
        let provider = self.provider.clone();
        let kms = self.kms.clone();
        let tenant_id = target.tenant_id;
        let region = target.region;
        let loc = target.loc;
        let transaction_tenant = tenant_id.0.clone();
        let crypto_region = region.clone();
        let expected_tenant = tenant_id.0.clone();
        self.block_on(async move {
            provider
                .with_tenant_tx(&transaction_tenant.clone(), move |conn| {
                    Box::pin(async move {
                        let read_sql = format!(
                            "SELECT {PR_RECORD_COLUMNS} FROM git_pr \
                             WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4"
                        );
                        sqlx::query(&read_sql)
                            .bind(&tenant_id.0)
                            .bind(&region.0)
                            .bind(&loc.repo)
                            .bind(number)
                            .fetch_optional(&mut *conn)
                            .await
                            .map_err(|_| pg_query("read PR"))
                    })
                })
                .await
        })
        .map_err(pg_error)?
        .map(|row| decode_record(&kms, &crypto_region, &expected_tenant, row))
        .transpose()
    }

    pub fn list(&self, scope: &TenantScope, repo: &str) -> Result<Vec<PrRecord>, DurableError> {
        self.list_bounded(scope, repo, usize::MAX)
    }

    /// Fetch at most one row beyond the caller's ceiling, so interactive list/count projections can
    /// detect a pathological repository without materializing its entire PR relation in process.
    pub fn list_bounded(
        &self,
        scope: &TenantScope,
        repo: &str,
        maximum_records: usize,
    ) -> Result<Vec<PrRecord>, DurableError> {
        let target = self.scoped_target(scope, repo)?;
        let provider = self.provider.clone();
        let kms = self.kms.clone();
        let tenant_id = target.tenant_id;
        let region = target.region;
        let loc = target.loc;
        let transaction_tenant = tenant_id.0.clone();
        let crypto_region = region.clone();
        let expected_tenant = tenant_id.0.clone();
        let fetch_limit = i64::try_from(maximum_records.saturating_add(1)).unwrap_or(i64::MAX);
        let rows = self
            .block_on(async move {
                provider
                    .with_tenant_tx(&transaction_tenant.clone(), move |conn| {
                        Box::pin(async move {
                            let list_sql = format!(
                                "SELECT {PR_RECORD_COLUMNS} FROM git_pr \
                                 WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 \
                                 ORDER BY number LIMIT $4"
                            );
                            sqlx::query(&list_sql)
                                .bind(&tenant_id.0)
                                .bind(&region.0)
                                .bind(&loc.repo)
                                .bind(fetch_limit)
                                .fetch_all(&mut *conn)
                                .await
                                .map_err(|_| pg_query("list PRs"))
                        })
                    })
                    .await
            })
            .map_err(pg_error)?;
        if rows.len() > maximum_records {
            return Err(DurableError::Git(
                "pull request list limit exceeded: record count".into(),
            ));
        }
        rows.into_iter()
            .map(|row| decode_record(&kms, &crypto_region, &expected_tenant, row))
            .collect()
    }

    /// Allocate the number, insert the row and stage `git.pr.opened` in one transaction.
    pub fn open(
        &self,
        scope: &TenantScope,
        repo: &str,
        mut record: PrRecord,
        operation_id: &PrOperationId,
        principal: &Principal,
    ) -> Result<PrRecord, DurableError> {
        record.author_subject_id = normalized_subject_id(principal)?;
        let actor_subject_id = record.author_subject_id.clone();
        record.number = 0;
        let payload_hash = payload_hash(&record)?;
        let ctx = self.emit_context(scope, principal)?;
        self.open_inner(
            scope,
            repo,
            record,
            operation_id.clone(),
            actor_subject_id,
            payload_hash,
            ctx,
            false,
        )
    }

    /// Failure-injection seam proving row and lifecycle event share one transaction.
    #[cfg(any(test, feature = "test-support"))]
    pub fn open_then_abort_for_test(
        &self,
        scope: &TenantScope,
        repo: &str,
        mut record: PrRecord,
        operation_id: &PrOperationId,
        principal: &Principal,
    ) -> Result<PrRecord, DurableError> {
        record.author_subject_id = normalized_subject_id(principal)?;
        let actor_subject_id = record.author_subject_id.clone();
        record.number = 0;
        let payload_hash = payload_hash(&record)?;
        let ctx = self.emit_context(scope, principal)?;
        self.open_inner(
            scope,
            repo,
            record,
            operation_id.clone(),
            actor_subject_id,
            payload_hash,
            ctx,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn open_inner(
        &self,
        scope: &TenantScope,
        repo: &str,
        mut record: PrRecord,
        operation_id: PrOperationId,
        actor_subject_id: String,
        payload_hash: String,
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
                        lock_operation(conn, &loc, &operation_id).await?;
                        if let Some(replayed) = replay_command(
                            conn,
                            &loc,
                            &operation_id,
                            &actor_subject_id,
                            "open",
                            &payload_hash,
                            None,
                            &kms,
                            &crypto_region,
                        )
                        .await?
                        {
                            return Ok(replayed);
                        }
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
                        .map_err(|_| pg_query("allocate PR number"))?;
                        record.number = number as u64;
                        let sealed = seal_pr_record(
                            &kms,
                            crypto_region,
                            &TenantId(loc.tenant.clone()),
                            &record,
                        )
                        .map_err(|_| pg_query("seal PR row"))?;
                        sqlx::query(
                            "INSERT INTO git_pr \
                             (tenant_id,region,repo_slug,number,head_repo_slug,author_subject_id,record,version, \
                              title_nonce,title_ciphertext,title_pii_key_ref,body_nonce, \
                              body_ciphertext,body_pii_key_ref) \
                             VALUES ($1,$2,$3,$4,$5,$6,$7,1,$8,$9,$10,$11,$12,$13)",
                        )
                        .bind(&loc.tenant)
                        .bind(&loc.region)
                        .bind(&loc.repo)
                        .bind(number)
                        .bind(&record.head_repo_slug)
                        .bind(&record.author_subject_id)
                        .bind(sealed.record)
                        .bind(sealed.title.nonce.to_vec())
                        .bind(sealed.title.ciphertext)
                        .bind(sealed.title.key_ref.to_uri())
                        .bind(sealed.body.as_ref().map(|column| column.nonce.to_vec()))
                        .bind(sealed.body.as_ref().map(|column| column.ciphertext.clone()))
                        .bind(sealed.body.as_ref().map(|column| column.key_ref.to_uri()))
                        .execute(&mut *conn)
                        .await
                        .map_err(|_| pg_query("insert PR"))?;
                        co_commit_event(conn, minter, ctx, &loc, &record, GIT_PR_OPENED, None).await?;
                        record_command(
                            conn,
                            &loc,
                            &operation_id,
                            &actor_subject_id,
                            "open",
                            &payload_hash,
                            &record,
                        )
                        .await?;
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
    #[allow(clippy::too_many_arguments)]
    fn mutate<F>(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        operation_id: PrOperationId,
        actor_subject_id: String,
        command_kind: &'static str,
        command_hash: String,
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
                        lock_operation(conn, &loc, &operation_id).await?;
                        let row = sqlx::query(&format!(
                            "SELECT {PR_RECORD_COLUMNS}, merge_intent FROM git_pr \
                             WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4 FOR UPDATE"
                        ))
                        .bind(&loc.tenant)
                        .bind(&loc.region)
                        .bind(&loc.repo)
                        .bind(number_db)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|_| pg_query("lock PR"))?
                        .ok_or_else(|| myelin_storage::PgError::Query(format!("PR #{number} not found")))?;
                        if let Some(replayed) = replay_command(
                            conn,
                            &loc,
                            &operation_id,
                            &actor_subject_id,
                            command_kind,
                            &command_hash,
                            Some(number_db),
                            &kms,
                            &crypto_region,
                        )
                        .await?
                        {
                            return Ok(replayed);
                        }
                        let pending_intent: Option<serde_json::Value> = row
                            .try_get("merge_intent")
                            .map_err(|_| pg_query("decode merge intent"))?;
                        if pending_intent.is_some() {
                            return Err(myelin_storage::PgError::Query(
                                "a merge operation is pending for this PR".into(),
                            ));
                        }
                        let mut record = decode_record(&kms, &crypto_region, &loc.tenant, row)
                            .map_err(|_| pg_query("decode PR"))?;
                        mutation(&mut record).map_err(|_| pg_query("apply PR mutation"))?;
                        let sealed = seal_pr_record(
                            &kms,
                            crypto_region.clone(),
                            &TenantId(loc.tenant.clone()),
                            &record,
                        )
                        .map_err(|_| pg_query("seal mutated PR"))?;
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
                        .map_err(|_| pg_query("update PR"))?;
                        co_commit_event(conn, minter, ctx, &loc, &record, event_type, None).await?;
                        record_command(
                            conn,
                            &loc,
                            &operation_id,
                            &actor_subject_id,
                            command_kind,
                            &command_hash,
                            &record,
                        )
                        .await?;
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
        mutation: PrMutation,
        operation_id: &PrOperationId,
        principal: &Principal,
    ) -> Result<PrRecord, DurableError> {
        let actor_subject_id = normalized_subject_id(principal)?;
        let command_kind = match &mutation {
            PrMutation::ReportChecks { .. } => "report-checks",
            PrMutation::SubmitReview(_) => "submit-review",
            PrMutation::EndorseContexts(_) => "endorse-contexts",
            PrMutation::Touch => "touch",
        };
        let command_hash = payload_hash(&(number, &mutation))?;
        let ctx = self.emit_context(scope, principal)?;
        self.mutate(
            scope,
            repo,
            number,
            operation_id.clone(),
            actor_subject_id,
            command_kind,
            command_hash,
            ctx,
            GIT_PR_UPDATED,
            move |record| {
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
            },
        )
    }

    fn merge_command_state(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        operation_id: &PrOperationId,
        actor_subject_id: &str,
        command_hash: &str,
    ) -> Result<MergeLedgerState, DurableError> {
        let loc = self.scoped_loc(scope, repo)?;
        let number_db = db_number(number)?;
        let provider = self.provider.clone();
        let operation_id = operation_id.clone();
        let actor_subject_id = actor_subject_id.to_owned();
        let command_hash = command_hash.to_owned();
        self.block_on(async move {
            provider
                .with_tenant_tx(&loc.tenant.clone(), move |conn| {
                    Box::pin(async move {
                        lock_operation(conn, &loc, &operation_id).await?;
                        merge_ledger_state(
                            conn,
                            &loc,
                            &operation_id,
                            &actor_subject_id,
                            &command_hash,
                            number_db,
                        )
                        .await
                    })
                })
                .await
        })
        .map_err(pg_error)
    }

    #[allow(clippy::too_many_arguments)]
    fn record_merge_terminal(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        operation_id: &PrOperationId,
        actor_subject_id: &str,
        command_hash: &str,
        status: &'static str,
        result: MergeCommandResult,
    ) -> Result<MergeAttempt, DurableError> {
        let loc = self.scoped_loc(scope, repo)?;
        let number_db = db_number(number)?;
        let provider = self.provider.clone();
        let operation_id = operation_id.clone();
        let actor_subject_id = actor_subject_id.to_owned();
        let command_hash = command_hash.to_owned();
        self.block_on(async move {
            provider
                .with_tenant_tx(&loc.tenant.clone(), move |conn| {
                    Box::pin(async move {
                        lock_operation(conn, &loc, &operation_id).await?;
                        match merge_ledger_state(
                            conn,
                            &loc,
                            &operation_id,
                            &actor_subject_id,
                            &command_hash,
                            number_db,
                        )
                        .await?
                        {
                            MergeLedgerState::Terminal(attempt) => Ok(attempt),
                            MergeLedgerState::Pending => {
                                Err(pg_query("merge command is pending a durable ref outcome"))
                            }
                            MergeLedgerState::Absent => {
                                insert_merge_command(
                                    conn,
                                    &loc,
                                    &operation_id,
                                    &actor_subject_id,
                                    &command_hash,
                                    number_db,
                                    status,
                                    Some(&result),
                                )
                                .await?;
                                Ok(result.into_attempt())
                            }
                        }
                    })
                })
                .await
        })
        .map_err(pg_error)
    }

    /// Execute the complete production merge protocol against the concrete durable ref adapter.
    /// A pending intent is recovered by inspecting the actual target tip; finalization is reachable
    /// only after the adapter proves that tip equals the locked head OID.
    #[allow(clippy::too_many_arguments)]
    pub fn merge_pr_durable(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        operation_id: &PrOperationId,
        principal: &Principal,
        policy_store: &DurablePrStore,
        target_loc: &RepoLoc,
        target_repo: &DurableGitRepo,
        source_repo: &DurableGitRepo,
        ref_store: &RefStore,
        merger_pseudonym: &str,
    ) -> Result<MergeAttempt, DurableError> {
        let loc = self.scoped_loc(scope, repo)?;
        if &loc != target_loc {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        let actor_subject_id = normalized_subject_id(principal)?;
        let pending = self.pending_merge_intent(scope, repo, number)?;
        if let Some(intent) = &pending {
            validate_merge_intent(intent)?;
            if intent.operation_id != operation_id.as_str()
                || intent.actor_subject_id != actor_subject_id
            {
                return Err(DurableError::Git(
                    "a different merge operation is already pending".into(),
                ));
            }
            return self
                .recover_pending_merge_target(
                    scope,
                    repo,
                    number,
                    principal,
                    target_loc,
                    target_repo,
                    ref_store,
                )?
                .ok_or_else(|| DurableError::Io("pending merge disappeared during recovery".into()));
        }
        let ctx = self.emit_context(scope, principal)?;
        let record = self
            .get(scope, repo, number)?
            .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))?;
        let command_intent = pending.clone().unwrap_or_else(|| MergeIntent {
            operation_id: operation_id.as_str().to_owned(),
            actor_subject_id: actor_subject_id.clone(),
            base_ref: record.base_ref.clone(),
            expected_old_oid: PushOid::zero().0,
            head_oid: record.head_oid.clone(),
            head_repo_slug: record.head_repo_slug.clone(),
        });
        let command_hash = merge_payload_hash(number, &command_intent)?;
        match self.merge_command_state(
            scope,
            repo,
            number,
            operation_id,
            &actor_subject_id,
            &command_hash,
        )? {
            MergeLedgerState::Terminal(attempt) => return Ok(attempt),
            MergeLedgerState::Pending if pending.is_none() => {
                return Err(DurableError::Io(
                    "pending merge command has no durable intent".into(),
                ));
            }
            _ => {}
        }
        if record.state == PrState::Merged {
            let update_seq = target_repo.ref_generation(&record.base_ref)?;
            return self.record_merge_terminal(
                scope,
                repo,
                number,
                operation_id,
                &actor_subject_id,
                &command_hash,
                "completed",
                MergeCommandResult::Merged {
                    base_ref: record.base_ref,
                    new_oid: record.head_oid,
                    update_seq,
                },
            );
        }
        let ruleset = policy_store.effective_ruleset_for(target_loc, &record.base_ref)?;
        let evaluation = evaluate_merge(&ruleset, &record)
            .map_err(|_| DurableError::Git("merge policy input refused".into()))?;
        if !evaluation.admitted() {
            return self.record_merge_terminal(
                scope,
                repo,
                number,
                operation_id,
                &actor_subject_id,
                &command_hash,
                "completed",
                MergeCommandResult::Blocked { evaluation },
            );
        }

        let source_ref = qualify_ref(&record.head_ref);
        let source_tip = source_repo.read_ref(&source_ref)?;
        if source_tip.as_ref().map(CoreOid::as_str) != Some(record.head_oid.as_str()) {
            return self.record_merge_terminal(
                scope,
                repo,
                number,
                operation_id,
                &actor_subject_id,
                &command_hash,
                "completed",
                MergeCommandResult::InvalidHead {
                    reason: "the authoritative source ref no longer equals the locked head OID"
                        .into(),
                },
            );
        }
        let head_core = CoreOid::new(record.head_oid.clone());
        if !target_repo.object_is_commit(&head_core) {
            return self.record_merge_terminal(
                scope,
                repo,
                number,
                operation_id,
                &actor_subject_id,
                &command_hash,
                "completed",
                MergeCommandResult::InvalidHead {
                    reason: "the locked head OID is not a commit in the target object database"
                        .into(),
                },
            );
        }

        let base = RefName::new(record.base_ref.clone());
        let actual_before = ref_store.try_tip(&base)?;
        if !target_repo.is_fast_forward(
            actual_before
                .as_ref()
                .map(|oid| CoreOid::new(oid.0.clone()))
                .as_ref(),
            &head_core,
        )? {
            return self.record_merge_terminal(
                scope,
                repo,
                number,
                operation_id,
                &actor_subject_id,
                &command_hash,
                "completed",
                MergeCommandResult::InvalidHead {
                    reason: "the locked head OID is not a fast-forward of the target ref".into(),
                },
            );
        }

        let intent = pending.unwrap_or_else(|| MergeIntent {
            operation_id: operation_id.as_str().to_owned(),
            actor_subject_id,
            base_ref: record.base_ref.clone(),
            expected_old_oid: actual_before
                .as_ref()
                .map(|oid| oid.0.clone())
                .unwrap_or_else(|| PushOid::zero().0),
            head_oid: record.head_oid.clone(),
            head_repo_slug: record.head_repo_slug.clone(),
        });
        validate_merge_intent(&intent)?;
        if let Some(attempt) = self.begin_merge(
            scope,
            repo,
            number,
            intent.clone(),
            &command_hash,
            ctx.clone(),
        )? {
            return Ok(attempt);
        }
        self.advance_pending_merge(
            scope,
            repo,
            number,
            &intent,
            &command_hash,
            ctx,
            target_repo,
            ref_store,
            merger_pseudonym,
        )
    }

    /// Recover one persisted merge using only the target repository, target ref adapter, durable
    /// intent, and command ledger. The source repository and its authorization are deliberately not
    /// inputs: admission already froze and persisted those facts before the ref-CAS crash window.
    ///
    /// `Ok(None)` means no pending intent exists. Recovery events use the supplied authenticated
    /// recovery principal, pseudonymized by [`Self::emit_context`], and explicitly link causality to
    /// the PII-free operation digest stored in the intent.
    #[allow(clippy::too_many_arguments)]
    pub fn recover_pending_merge_target(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        recovery_principal: &Principal,
        target_loc: &RepoLoc,
        target_repo: &DurableGitRepo,
        ref_store: &RefStore,
    ) -> Result<Option<MergeAttempt>, DurableError> {
        let loc = self.scoped_loc(scope, repo)?;
        if &loc != target_loc {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        let Some(intent) = self.pending_merge_intent(scope, repo, number)? else {
            return Ok(None);
        };
        validate_merge_intent(&intent)?;
        let operation_id = PrOperationId::from_stored_digest(&intent.operation_id)?;
        let command_hash = merge_payload_hash(number, &intent)?;
        match self.merge_command_state(
            scope,
            repo,
            number,
            &operation_id,
            &intent.actor_subject_id,
            &command_hash,
        )? {
            MergeLedgerState::Terminal(attempt) => return Ok(Some(attempt)),
            MergeLedgerState::Pending => {}
            MergeLedgerState::Absent => {
                return Err(DurableError::Io(
                    "pending merge has no command reservation".into(),
                ));
            }
        }

        let mut ctx = self.emit_context(scope, recovery_principal)?;
        ctx.caused_by = Some(CausedBy(format!(
            "git-pr-operation:{}",
            intent.operation_id
        )));
        let base = RefName::new(intent.base_ref.clone());
        let actual = ref_store.try_tip(&base)?;
        let attempt = if actual.as_ref().map(|oid| oid.0.as_str())
            == Some(intent.head_oid.as_str())
        {
            let update_seq = target_repo.ref_generation(&base.0)?;
            self.finalize_merge(
                scope,
                repo,
                number,
                &intent,
                &command_hash,
                update_seq,
                ctx,
            )?
        } else {
            let expected_matches = actual.as_ref().map(|oid| oid.0.as_str())
                == Some(intent.expected_old_oid.as_str())
                || (actual.is_none() && intent.expected_old_oid == PushOid::zero().0);
            if !expected_matches {
                self.cancel_merge(
                    scope,
                    repo,
                    number,
                    &intent,
                    &command_hash,
                    actual,
                    ctx,
                )?
            } else {
                let merger_pseudonym = event_actor_pseudonym(
                    scope.tenant().as_str(),
                    &intent.actor_subject_id,
                );
                self.advance_pending_merge(
                    scope,
                    repo,
                    number,
                    &intent,
                    &command_hash,
                    ctx,
                    target_repo,
                    ref_store,
                    &merger_pseudonym,
                )?
            }
        };
        Ok(Some(attempt))
    }

    #[allow(clippy::too_many_arguments)]
    fn advance_pending_merge(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        intent: &MergeIntent,
        command_hash: &str,
        ctx: EmitContextBase,
        target_repo: &DurableGitRepo,
        ref_store: &RefStore,
        merger_pseudonym: &str,
    ) -> Result<MergeAttempt, DurableError> {
        let base = RefName::new(intent.base_ref.clone());
        let actual = ref_store.try_tip(&base)?;
        if actual.as_ref().map(|oid| oid.0.as_str()) == Some(intent.head_oid.as_str()) {
            let update_seq = target_repo.ref_generation(&base.0)?;
            return self.finalize_merge(scope, repo, number, intent, command_hash, update_seq, ctx);
        }
        let expected_matches = actual.as_ref().map(|oid| oid.0.as_str())
            == Some(intent.expected_old_oid.as_str())
            || (actual.is_none() && intent.expected_old_oid == PushOid::zero().0);
        if !expected_matches {
            return self.cancel_merge(scope, repo, number, intent, command_hash, actual, ctx);
        }

        let head = PushOid::new(intent.head_oid.clone());
        let push = PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name: base.clone(),
                expected_old: PushOid::new(intent.expected_old_oid.clone()),
                new_oid: head.clone(),
                forced: false,
                commit_oids: vec![head.clone()],
            }],
            quarantine: Vec::new(),
            pusher: Pusher {
                pseudonym: merger_pseudonym.to_owned(),
                is_agent: false,
            },
        };
        match ref_store
            .receive(&push, &InMemoryObjectDb::new(), CrashPoint::None)
            .map_err(|_| DurableError::Git("merge ref advance failed".into()))?
        {
            PushOutcome::Accepted { moved, .. } => {
                if ref_store.try_tip(&base)?.as_ref().map(|oid| oid.0.as_str())
                    != Some(intent.head_oid.as_str())
                {
                    return Err(DurableError::Git(
                        "merge ref adapter did not verify the committed head".into(),
                    ));
                }
                let update_seq = accepted_merge_update_seq(&moved, &base, &head)?;
                self.finalize_merge(scope, repo, number, intent, command_hash, update_seq, ctx)
            }
            PushOutcome::Rejected(_reason) => {
                let actual = ref_store.try_tip(&base)?;
                if actual.as_ref().map(|oid| oid.0.as_str()) == Some(intent.head_oid.as_str()) {
                    let update_seq = target_repo.ref_generation(&base.0)?;
                    self.finalize_merge(scope, repo, number, intent, command_hash, update_seq, ctx)
                } else {
                    self.cancel_merge(scope, repo, number, intent, command_hash, actual, ctx)
                }
            }
            PushOutcome::Crashed(_) => Err(DurableError::Git(
                "merge ref advance requires recovery".into(),
            )),
        }
    }

    pub fn pending_merge_intent(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
    ) -> Result<Option<MergeIntent>, DurableError> {
        let loc = self.scoped_loc(scope, repo)?;
        let number = db_number(number)?;
        let provider = self.provider.clone();
        self.block_on(async move {
            provider
                .with_tenant_tx(&loc.tenant.clone(), move |conn| {
                    Box::pin(async move {
                        let value: Option<serde_json::Value> = sqlx::query_scalar(
                            "SELECT merge_intent FROM git_pr
                              WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4",
                        )
                        .bind(&loc.tenant)
                        .bind(&loc.region)
                        .bind(&loc.repo)
                        .bind(number)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|_| {
                            myelin_storage::PgError::Query("pending PR merge lookup failed".into())
                        })?
                        .flatten();
                        value
                            .map(|value| {
                                serde_json::from_value(value).map_err(|_| {
                                    myelin_storage::PgError::Query(
                                        "pending PR merge intent malformed".into(),
                                    )
                                })
                            })
                            .transpose()
                    })
                })
                .await
        })
        .map_err(pg_error)
    }

    /// Enumerate recovery work for the verified tenant/region only. Startup code then opens the
    /// exact target/source repositories and re-enters `merge_pr_durable`, which verifies actual tips.
    pub fn list_pending_merges(
        &self,
        scope: &TenantScope,
    ) -> Result<Vec<PendingMerge>, DurableError> {
        if scope.region().0 != self.provider.config().region {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        let provider = self.provider.clone();
        let tenant = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let rows = self
            .block_on(async move {
                provider
                    .with_tenant_tx(&tenant.clone(), move |conn| {
                        Box::pin(async move {
                            sqlx::query(
                                "SELECT repo_slug,number,merge_intent FROM git_pr
                                  WHERE tenant_id=$1 AND region=$2 AND merge_intent IS NOT NULL
                                    AND record->>'state' <> 'Merged'
                                  ORDER BY repo_slug,number",
                            )
                            .bind(&tenant)
                            .bind(&region)
                            .fetch_all(&mut *conn)
                            .await
                            .map_err(|_| {
                                myelin_storage::PgError::Query(
                                    "pending PR merge enumeration failed".into(),
                                )
                            })
                        })
                    })
                    .await
            })
            .map_err(pg_error)?;
        rows.into_iter()
            .map(|row| {
                let repo_slug: String = row
                    .try_get("repo_slug")
                    .map_err(|_| DurableError::Io("pending PR merge row malformed".into()))?;
                let number: i64 = row
                    .try_get("number")
                    .map_err(|_| DurableError::Io("pending PR merge row malformed".into()))?;
                let value: serde_json::Value = row
                    .try_get("merge_intent")
                    .map_err(|_| DurableError::Io("pending PR merge row malformed".into()))?;
                let intent = serde_json::from_value(value)
                    .map_err(|_| DurableError::Io("pending PR merge intent malformed".into()))?;
                Ok(PendingMerge {
                    repo_slug,
                    number: u64::try_from(number)
                        .map_err(|_| DurableError::Io("pending PR number invalid".into()))?,
                    intent,
                })
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn cancel_merge(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        intent: &MergeIntent,
        command_hash: &str,
        actual: Option<PushOid>,
        ctx: EmitContextBase,
    ) -> Result<MergeAttempt, DurableError> {
        let expected = serde_json::to_value(intent)
            .map_err(|_| DurableError::Git("encode merge intent failed".into()))?;
        let operation_id = PrOperationId::from_stored_digest(&intent.operation_id)?;
        let command_hash = command_hash.to_owned();
        let actor_subject_id = intent.actor_subject_id.clone();
        let result = MergeCommandResult::RefRefused {
            base_ref: intent.base_ref.clone(),
            expected: intent.expected_old_oid.clone(),
            actual: actual.unwrap_or_else(PushOid::zero).0,
        };
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
                        lock_operation(conn, &loc, &operation_id).await?;
                        match merge_ledger_state(
                            conn,
                            &loc,
                            &operation_id,
                            &actor_subject_id,
                            &command_hash,
                            number_db,
                        )
                        .await?
                        {
                            MergeLedgerState::Terminal(attempt) => return Ok(attempt),
                            MergeLedgerState::Absent => {
                                return Err(pg_query(
                                    "cancelled merge command reservation missing",
                                ));
                            }
                            MergeLedgerState::Pending => {}
                        }
                        let row = sqlx::query(&format!(
                            "SELECT {PR_RECORD_COLUMNS},merge_intent FROM git_pr
                              WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4
                              FOR UPDATE"
                        ))
                        .bind(&loc.tenant)
                        .bind(&loc.region)
                        .bind(&loc.repo)
                        .bind(number_db)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|_| {
                            myelin_storage::PgError::Query("cancel PR merge lookup failed".into())
                        })?
                        .ok_or_else(|| {
                            myelin_storage::PgError::Query("PR merge target not found".into())
                        })?;
                        let persisted: Option<serde_json::Value> =
                            row.try_get("merge_intent").map_err(|_| {
                                myelin_storage::PgError::Query(
                                    "persisted PR merge intent malformed".into(),
                                )
                            })?;
                        if persisted.as_ref() != Some(&expected) {
                            return Err(myelin_storage::PgError::Query(
                                "merge cancellation intent mismatch".into(),
                            ));
                        }
                        let record = decode_record(&kms, &crypto_region, &loc.tenant, row)
                            .map_err(|_| {
                                myelin_storage::PgError::Query(
                                    "cancel PR merge record unavailable".into(),
                                )
                            })?;
                        sqlx::query(
                            "UPDATE git_pr SET merge_intent=NULL,version=version+1,updated_at=now()
                              WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4",
                        )
                        .bind(&loc.tenant)
                        .bind(&loc.region)
                        .bind(&loc.repo)
                        .bind(number_db)
                        .execute(&mut *conn)
                        .await
                        .map_err(|_| {
                            myelin_storage::PgError::Query("cancel PR merge failed".into())
                        })?;
                        co_commit_event(
                            conn,
                            minter,
                            ctx,
                            &loc,
                            &record,
                            GIT_PR_UPDATED,
                            Some(&expected),
                        )
                        .await?;
                        let result_json = serde_json::to_value(&result)
                            .map_err(|_| pg_query("encode cancelled merge result"))?;
                        let completed = sqlx::query(
                            "UPDATE git_pr_command SET status='cancelled',result=$5
                              WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND operation_id=$4
                                AND status='pending'",
                        )
                        .bind(&loc.tenant)
                        .bind(&loc.region)
                        .bind(&loc.repo)
                        .bind(operation_id.as_str())
                        .bind(result_json)
                        .execute(&mut *conn)
                        .await
                        .map_err(|_| pg_query("complete cancelled merge command"))?;
                        if completed.rows_affected() != 1 {
                            return Err(pg_query("cancelled merge command completion lost"));
                        }
                        Ok(result.into_attempt())
                    })
                })
                .await
        })
        .map_err(pg_error)
    }

    fn begin_merge(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        intent: MergeIntent,
        command_hash: &str,
        ctx: EmitContextBase,
    ) -> Result<Option<MergeAttempt>, DurableError> {
        validate_merge_intent(&intent)?;
        let intent_json = serde_json::to_value(&intent)
            .map_err(|e| DurableError::Git(format!("encode merge intent: {e}")))?;
        let operation_id = PrOperationId::from_stored_digest(&intent.operation_id)?;
        let actor_subject_id = intent.actor_subject_id.clone();
        let command_hash = command_hash.to_owned();
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
                        lock_operation(conn, &loc, &operation_id).await?;
                        let row = sqlx::query(&format!(
                            "SELECT {PR_RECORD_COLUMNS},merge_intent FROM git_pr
                    WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4 FOR UPDATE"
                        ))
                        .bind(&loc.tenant)
                        .bind(&loc.region)
                        .bind(&loc.repo)
                        .bind(number_db)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|_| pg_query("lock merge PR"))?
                        .ok_or_else(|| pg_query("merge PR not found"))?;
                        let existing: Option<serde_json::Value> = row
                            .try_get("merge_intent")
                            .map_err(|_| pg_query("decode merge intent"))?;
                        match merge_ledger_state(
                            conn,
                            &loc,
                            &operation_id,
                            &actor_subject_id,
                            &command_hash,
                            number_db,
                        )
                        .await?
                        {
                            MergeLedgerState::Terminal(attempt) => return Ok(Some(attempt)),
                            MergeLedgerState::Pending => {
                                if existing.as_ref() != Some(&intent_json) {
                                    return Err(pg_query(
                                        "pending merge command and intent diverge",
                                    ));
                                }
                                return Ok(None);
                            }
                            MergeLedgerState::Absent => {}
                        }
                        if existing.is_some() {
                            return Err(pg_query("a different merge intent is already pending"));
                        }
                        let record = decode_record(&kms, &crypto_region, &loc.tenant, row)
                            .map_err(|_| pg_query("decode merge PR"))?;
                        if record.state == PrState::Merged
                            || record.head_oid != intent.head_oid
                            || record.head_repo_slug != intent.head_repo_slug
                            || record.base_ref != intent.base_ref
                        {
                            return Err(pg_query(
                                "merge intent diverges from locked PR provenance",
                            ));
                        }
                        insert_merge_command(
                            conn,
                            &loc,
                            &operation_id,
                            &actor_subject_id,
                            &command_hash,
                            number_db,
                            "pending",
                            None,
                        )
                        .await?;
                        sqlx::query(
                            "UPDATE git_pr SET merge_intent=$5,version=version+1,updated_at=now()
                    WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4",
                        )
                        .bind(&loc.tenant)
                        .bind(&loc.region)
                        .bind(&loc.repo)
                        .bind(number_db)
                        .bind(&intent_json)
                        .execute(&mut *conn)
                        .await
                        .map_err(|_| pg_query("persist merge intent"))?;
                        co_commit_event(
                            conn,
                            minter,
                            ctx,
                            &loc,
                            &record,
                            GIT_PR_UPDATED,
                            Some(&intent_json),
                        )
                        .await?;
                        Ok(None)
                    })
                })
                .await
        })
        .map_err(pg_error)
    }

    #[allow(clippy::too_many_arguments)]
    fn finalize_merge(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        intent: &MergeIntent,
        command_hash: &str,
        update_seq: u64,
        ctx: EmitContextBase,
    ) -> Result<MergeAttempt, DurableError> {
        validate_merge_intent(intent)?;
        let operation_id = PrOperationId::from_stored_digest(&intent.operation_id)?;
        let actor_subject_id = intent.actor_subject_id.clone();
        let command_hash = command_hash.to_owned();
        let expected = serde_json::to_value(intent)
            .map_err(|e| DurableError::Git(format!("encode merge intent: {e}")))?;
        let loc = self.scoped_loc(scope, repo)?;
        let number_db = db_number(number)?;
        let provider = self.provider.clone();
        let minter = self.minter.clone();
        let kms = self.kms.clone();
        let crypto_region = Region(loc.region.clone());
        self.block_on(async move {
            provider.with_tenant_tx(&loc.tenant.clone(), move |conn| Box::pin(async move {
                lock_operation(conn, &loc, &operation_id).await?;
                let row = sqlx::query(&format!("SELECT {PR_RECORD_COLUMNS}, merge_intent FROM git_pr WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4 FOR UPDATE"))
                    .bind(&loc.tenant).bind(&loc.region).bind(&loc.repo).bind(number_db)
                    .fetch_optional(&mut *conn).await.map_err(|_| pg_query("lock merge PR"))?
                    .ok_or_else(|| myelin_storage::PgError::Query(format!("PR #{number} not found")))?;
                match merge_ledger_state(conn, &loc, &operation_id, &actor_subject_id, &command_hash, number_db).await? {
                    MergeLedgerState::Terminal(attempt) => return Ok(attempt),
                    MergeLedgerState::Absent => return Err(pg_query("merge command reservation missing")),
                    MergeLedgerState::Pending => {}
                }
                let existing_intent: Option<serde_json::Value> = row.try_get("merge_intent").map_err(|_| pg_query("decode merge intent"))?;
                if existing_intent.as_ref() != Some(&expected) {
                    return Err(pg_query("merge finalize intent does not match the durable operation"));
                }
                let mut record = decode_record(&kms, &crypto_region, &loc.tenant, row).map_err(|_| pg_query("decode merge PR"))?;
                record.state = PrState::Merged;
                record.updated_at = Some(now_unix());
                let sealed = seal_pr_record(&kms, crypto_region.clone(), &TenantId(loc.tenant.clone()), &record)
                    .map_err(|_| pg_query("seal merge PR"))?;
                sqlx::query("UPDATE git_pr SET record=$5, version=version+1, merge_intent=$6, \
                    title_nonce=$7,title_ciphertext=$8,title_pii_key_ref=$9,body_nonce=$10, \
                    body_ciphertext=$11,body_pii_key_ref=$12,updated_at=now() \
                    WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4")
                    .bind(&loc.tenant).bind(&loc.region).bind(&loc.repo).bind(number_db)
                    .bind(sealed.record).bind(Option::<serde_json::Value>::None)
                    .bind(sealed.title.nonce.to_vec()).bind(sealed.title.ciphertext).bind(sealed.title.key_ref.to_uri())
                    .bind(sealed.body.as_ref().map(|column| column.nonce.to_vec()))
                    .bind(sealed.body.as_ref().map(|column| column.ciphertext.clone()))
                    .bind(sealed.body.as_ref().map(|column| column.key_ref.to_uri()))
                    .execute(&mut *conn).await.map_err(|_| pg_query("update merge PR"))?;
                co_commit_event(conn, minter, ctx, &loc, &record, GIT_PR_MERGED, Some(&expected)).await?;
                let result = MergeCommandResult::Merged {
                    base_ref: record.base_ref.clone(),
                    new_oid: record.head_oid.clone(),
                    update_seq,
                };
                let result_json = serde_json::to_value(&result).map_err(|_| pg_query("encode merged command result"))?;
                let updated = sqlx::query("UPDATE git_pr_command SET status='completed',result=$5
                    WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND operation_id=$4 AND status='pending'")
                    .bind(&loc.tenant).bind(&loc.region).bind(&loc.repo).bind(operation_id.as_str()).bind(result_json)
                    .execute(&mut *conn).await.map_err(|_| pg_query("complete merge command"))?;
                if updated.rows_affected() != 1 { return Err(pg_query("merge command completion lost")); }
                Ok(result.into_attempt())
            })).await
        }).map_err(pg_error)
    }
}

fn accepted_merge_update_seq(
    moved: &[(RefName, PushOid, u64)],
    expected_ref: &RefName,
    expected_oid: &PushOid,
) -> Result<u64, DurableError> {
    match moved {
        [(ref_name, oid, update_seq)]
            if ref_name == expected_ref && oid == expected_oid && *update_seq > 0 =>
        {
            Ok(*update_seq)
        }
        _ => Err(DurableError::Git(
            "merge ref adapter returned an invalid committed-move witness".into(),
        )),
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
    let safe_operation = operation.map(|value| {
        serde_json::json!({
            "operation_id": value.get("operation_id"),
            "base_ref": value.get("base_ref"),
            "expected_old_oid": value.get("expected_old_oid"),
            "head_oid": value.get("head_oid"),
            "head_repo_slug": value.get("head_repo_slug"),
        })
    });
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
                "operation": safe_operation,
            }),
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        None,
    )
    .map_err(|_| pg_query("stage PR lifecycle event"))?;
    let row = tx
        .into_staged_rows()
        .map_err(|_| pg_query("encode PR lifecycle event"))?
        .pop()
        .ok_or_else(|| myelin_storage::PgError::Query("missing PR envelope".into()))?;
    PgRelay::co_commit_in_tx(conn, &row.aggregate.0, &row.envelope).await
}

async fn lock_operation(
    conn: &mut sqlx::PgConnection,
    loc: &RepoLoc,
    operation_id: &PrOperationId,
) -> Result<(), myelin_storage::PgError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "git-pr-command:{}/{}:{}",
            loc.tenant,
            loc.region,
            operation_id.as_str()
        ))
        .execute(&mut *conn)
        .await
        .map_err(|_| myelin_storage::PgError::Query("PR command lock failed".into()))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn replay_command(
    conn: &mut sqlx::PgConnection,
    loc: &RepoLoc,
    operation_id: &PrOperationId,
    actor_subject_id: &str,
    command_kind: &str,
    expected_hash: &str,
    expected_number: Option<i64>,
    kms: &KmsEngine,
    region: &Region,
) -> Result<Option<PrRecord>, myelin_storage::PgError> {
    let row = sqlx::query(
        "SELECT repo_slug,actor_subject_id,command_kind,payload_hash,pr_number,status,result
           FROM git_pr_command
          WHERE tenant_id=$1 AND region=$2 AND operation_id=$3",
    )
    .bind(&loc.tenant)
    .bind(&loc.region)
    .bind(operation_id.as_str())
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| myelin_storage::PgError::Query("PR command replay lookup failed".into()))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let stored_repo: String = row
        .try_get("repo_slug")
        .map_err(|_| myelin_storage::PgError::Query("PR command row malformed".into()))?;
    if stored_repo != loc.repo {
        return Err(myelin_storage::PgError::Query(
            "PR operation id was reused for a different repository".into(),
        ));
    }
    let stored_actor: String = row
        .try_get("actor_subject_id")
        .map_err(|_| myelin_storage::PgError::Query("PR command row malformed".into()))?;
    let stored_kind: String = row
        .try_get("command_kind")
        .map_err(|_| myelin_storage::PgError::Query("PR command row malformed".into()))?;
    let stored_hash: String = row
        .try_get("payload_hash")
        .map_err(|_| myelin_storage::PgError::Query("PR command row malformed".into()))?;
    if stored_actor != actor_subject_id
        || stored_kind != command_kind
        || stored_hash != expected_hash
    {
        return Err(myelin_storage::PgError::Query(
            "PR operation id was reused for a different command".into(),
        ));
    }
    let number: i64 = row
        .try_get("pr_number")
        .map_err(|_| myelin_storage::PgError::Query("PR command row malformed".into()))?;
    if expected_number.is_some_and(|expected| expected != number) {
        return Err(myelin_storage::PgError::Query(
            "PR operation id was reused for a different command target".into(),
        ));
    }
    let status: String = row
        .try_get("status")
        .map_err(|_| myelin_storage::PgError::Query("PR command row malformed".into()))?;
    let historical: Option<serde_json::Value> = row
        .try_get("result")
        .map_err(|_| myelin_storage::PgError::Query("PR command row malformed".into()))?;
    if status != "completed" {
        return Err(myelin_storage::PgError::Query(
            "PR operation is reserved by a non-completed command".into(),
        ));
    }
    let historical = historical.ok_or_else(|| {
        myelin_storage::PgError::Query("PR completed command result is missing".into())
    })?;
    let current_row = sqlx::query(&format!(
        "SELECT {PR_RECORD_COLUMNS} FROM git_pr
          WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4"
    ))
    .bind(&loc.tenant)
    .bind(&loc.region)
    .bind(&loc.repo)
    .bind(number)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| myelin_storage::PgError::Query("PR command result lookup failed".into()))?
    .ok_or_else(|| myelin_storage::PgError::Query("PR command target not found".into()))?;
    let current = decode_record(kms, region, &loc.tenant, current_row)
        .map_err(|_| myelin_storage::PgError::Query("PR command result unavailable".into()))?;
    let mut result: PrRecord = serde_json::from_value(historical)
        .map_err(|_| myelin_storage::PgError::Query("PR command result malformed".into()))?;
    if !result.title.is_empty()
        || result.body_md.is_some()
        || !result.author_subject_id.is_empty()
        || result.number != number as u64
    {
        return Err(myelin_storage::PgError::Query(
            "PR command result projection violated".into(),
        ));
    }
    result.title = current.title;
    result.body_md = current.body_md;
    result.author_subject_id = current.author_subject_id;
    Ok(Some(result))
}

async fn record_command(
    conn: &mut sqlx::PgConnection,
    loc: &RepoLoc,
    operation_id: &PrOperationId,
    actor_subject_id: &str,
    command_kind: &str,
    payload_hash: &str,
    record: &PrRecord,
) -> Result<(), myelin_storage::PgError> {
    let result = command_projection(record)
        .map_err(|_| myelin_storage::PgError::Query("encode PR command result failed".into()))?;
    sqlx::query(
        "INSERT INTO git_pr_command
         (tenant_id,region,repo_slug,operation_id,actor_subject_id,command_kind,payload_hash,pr_number,status,result)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'completed',$9)",
    )
    .bind(&loc.tenant)
    .bind(&loc.region)
    .bind(&loc.repo)
    .bind(operation_id.as_str())
    .bind(actor_subject_id)
    .bind(command_kind)
    .bind(payload_hash)
    .bind(db_number(record.number).map_err(|_| {
        myelin_storage::PgError::Query("PR command result number invalid".into())
    })?)
    .bind(result)
    .execute(&mut *conn)
    .await
    .map_err(|_| myelin_storage::PgError::Query("persist PR command failed".into()))?;
    Ok(())
}

async fn merge_ledger_state(
    conn: &mut sqlx::PgConnection,
    loc: &RepoLoc,
    operation_id: &PrOperationId,
    actor_subject_id: &str,
    expected_hash: &str,
    expected_number: i64,
) -> Result<MergeLedgerState, myelin_storage::PgError> {
    let row = sqlx::query(
        "SELECT repo_slug,actor_subject_id,command_kind,payload_hash,pr_number,status,result
           FROM git_pr_command
          WHERE tenant_id=$1 AND region=$2 AND operation_id=$3",
    )
    .bind(&loc.tenant)
    .bind(&loc.region)
    .bind(operation_id.as_str())
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| pg_query("merge command lookup"))?;
    let Some(row) = row else {
        return Ok(MergeLedgerState::Absent);
    };
    let stored_repo: String = row
        .try_get("repo_slug")
        .map_err(|_| pg_query("decode merge command"))?;
    if stored_repo != loc.repo {
        return Err(myelin_storage::PgError::Query(
            "PR operation id was reused for a different repository".into(),
        ));
    }
    let stored_actor: String = row
        .try_get("actor_subject_id")
        .map_err(|_| pg_query("decode merge command"))?;
    let stored_kind: String = row
        .try_get("command_kind")
        .map_err(|_| pg_query("decode merge command"))?;
    let stored_hash: String = row
        .try_get("payload_hash")
        .map_err(|_| pg_query("decode merge command"))?;
    let stored_number: i64 = row
        .try_get("pr_number")
        .map_err(|_| pg_query("decode merge command"))?;
    if stored_actor != actor_subject_id
        || stored_kind != "merge"
        || stored_hash != expected_hash
        || stored_number != expected_number
    {
        return Err(myelin_storage::PgError::Query(
            "PR operation id was reused for a different command or target".into(),
        ));
    }
    let status: String = row
        .try_get("status")
        .map_err(|_| pg_query("decode merge command"))?;
    let result: Option<serde_json::Value> = row
        .try_get("result")
        .map_err(|_| pg_query("decode merge command"))?;
    if status == "pending" {
        if result.is_some() {
            return Err(pg_query("decode pending merge command"));
        }
        return Ok(MergeLedgerState::Pending);
    }
    if status != "completed" && status != "cancelled" {
        return Err(pg_query("decode terminal merge command"));
    }
    let result: MergeCommandResult = serde_json::from_value(
        result.ok_or_else(|| pg_query("terminal merge command result missing"))?,
    )
    .map_err(|_| pg_query("decode terminal merge command result"))?;
    Ok(MergeLedgerState::Terminal(result.into_attempt()))
}

#[allow(clippy::too_many_arguments)]
async fn insert_merge_command(
    conn: &mut sqlx::PgConnection,
    loc: &RepoLoc,
    operation_id: &PrOperationId,
    actor_subject_id: &str,
    payload_hash: &str,
    pr_number: i64,
    status: &str,
    result: Option<&MergeCommandResult>,
) -> Result<(), myelin_storage::PgError> {
    let result = result
        .map(serde_json::to_value)
        .transpose()
        .map_err(|_| pg_query("encode merge command result"))?;
    sqlx::query(
        "INSERT INTO git_pr_command
         (tenant_id,region,repo_slug,operation_id,actor_subject_id,command_kind,payload_hash,pr_number,status,result)
         VALUES ($1,$2,$3,$4,$5,'merge',$6,$7,$8,$9)",
    )
    .bind(&loc.tenant)
    .bind(&loc.region)
    .bind(&loc.repo)
    .bind(operation_id.as_str())
    .bind(actor_subject_id)
    .bind(payload_hash)
    .bind(pr_number)
    .bind(status)
    .bind(result)
    .execute(&mut *conn)
    .await
    .map_err(|_| pg_query("persist merge command"))?;
    Ok(())
}

fn seal_pr_record(
    kms: &KmsEngine,
    region: Region,
    tenant: &TenantId,
    record: &PrRecord,
) -> Result<SealedPrRecord, DurableError> {
    if record.author_subject_id.is_empty() {
        return Err(DurableError::Io(
            "PR author subject locator is missing".into(),
        ));
    }
    kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
    let cryptor = ColumnCryptor::new(kms, region);
    let subject = SubjectId::new(record.author_subject_id.clone());
    let erasure = ErasureMethod::CryptoShred("subject_dek".into());
    let title = cryptor
        .encrypt(tenant, Some(&subject), &erasure, record.title.as_bytes())
        .map_err(|_| DurableError::Io("PR free-text encryption failed".into()))?;
    let body = record
        .body_md
        .as_ref()
        .map(|body| {
            cryptor
                .encrypt(tenant, Some(&subject), &erasure, body.as_bytes())
                .map_err(|_| DurableError::Io("PR free-text encryption failed".into()))
        })
        .transpose()?;
    let mut projection = record.clone();
    projection.title.clear();
    projection.body_md = None;
    projection.author_subject_id.clear();
    let record = serde_json::to_value(projection)
        .map_err(|_| DurableError::Io("encode PR projection failed".into()))?;
    Ok(SealedPrRecord {
        record,
        title,
        body,
    })
}

fn command_projection(record: &PrRecord) -> Result<serde_json::Value, DurableError> {
    let mut projection = record.clone();
    projection.title.clear();
    projection.body_md = None;
    projection.author_subject_id.clear();
    serde_json::to_value(projection)
        .map_err(|_| DurableError::Io("encode PR command projection failed".into()))
}

fn payload_hash(value: &impl serde::Serialize) -> Result<String, DurableError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| DurableError::Git("encode PR command payload failed".into()))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn merge_payload_hash(number: u64, intent: &MergeIntent) -> Result<String, DurableError> {
    // `expected_old_oid` is an observation made by the server immediately before CAS, not request
    // identity. Excluding it keeps the command hash stable across response-loss/crash recovery.
    payload_hash(&(
        number,
        intent.base_ref.as_str(),
        intent.head_oid.as_str(),
        intent.head_repo_slug.as_str(),
    ))
}

fn decode_record(
    kms: &KmsEngine,
    region: &Region,
    expected_tenant: &str,
    row: sqlx::postgres::PgRow,
) -> Result<PrRecord, DurableError> {
    let value: serde_json::Value = row
        .try_get("record")
        .map_err(|_| DurableError::Io("PR row projection malformed".into()))?;
    let mut record: PrRecord = serde_json::from_value(value)
        .map_err(|_| DurableError::Io("PR row projection malformed".into()))?;
    if !record.author_subject_id.is_empty() {
        return Err(DurableError::Io(
            "PR projection contains a broad subject identifier".into(),
        ));
    }
    let author_subject_id: String = row
        .try_get("author_subject_id")
        .map_err(|_| DurableError::Io("PR subject locator malformed".into()))?;
    if author_subject_id.is_empty() {
        return Err(DurableError::Io(
            "PR author subject locator is missing".into(),
        ));
    }
    record.author_subject_id = author_subject_id;
    let head_repo: String = row
        .try_get("head_repo_slug")
        .map_err(|_| DurableError::Io("PR provenance malformed".into()))?;
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
    if record.author_subject_id.is_empty() {
        return Err(DurableError::Io(
            "PR author subject locator is missing".into(),
        ));
    }
    let title = encrypted_column(&row, "title", expected_tenant, &record.author_subject_id)?
        .ok_or_else(|| {
            DurableError::Io("PR title ciphertext is missing from authoritative row".into())
        })?;
    let cryptor = ColumnCryptor::new(kms, region.clone());
    record.title = String::from_utf8(
        cryptor
            .decrypt(&title)
            .map_err(|_| DurableError::Io("PR free text unavailable".into()))?,
    )
    .map_err(|_| DurableError::Io("decrypted PR title is not UTF-8".into()))?;
    if let Some(body) = encrypted_column(&row, "body", expected_tenant, &record.author_subject_id)?
    {
        record.body_md = Some(
            String::from_utf8(
                cryptor
                    .decrypt(&body)
                    .map_err(|_| DurableError::Io("PR free text unavailable".into()))?,
            )
            .map_err(|_| DurableError::Io("decrypted PR body is not UTF-8".into()))?,
        );
    }
    Ok(record)
}

fn encrypted_column(
    row: &sqlx::postgres::PgRow,
    prefix: &str,
    expected_tenant: &str,
    expected_subject: &str,
) -> Result<Option<EncryptedColumn>, DurableError> {
    let nonce_name = format!("{prefix}_nonce");
    let ciphertext_name = format!("{prefix}_ciphertext");
    let key_name = format!("{prefix}_pii_key_ref");
    let nonce: Option<Vec<u8>> = row
        .try_get(nonce_name.as_str())
        .map_err(|_| DurableError::Io("encrypted PR column malformed".into()))?;
    let ciphertext: Option<Vec<u8>> = row
        .try_get(ciphertext_name.as_str())
        .map_err(|_| DurableError::Io("encrypted PR column malformed".into()))?;
    let key_ref: Option<String> = row
        .try_get(key_name.as_str())
        .map_err(|_| DurableError::Io("encrypted PR column malformed".into()))?;
    match (nonce, ciphertext, key_ref) {
        (None, None, None) => Ok(None),
        (Some(nonce), Some(ciphertext), Some(key_ref)) => {
            let nonce: [u8; NONCE_LEN] = nonce
                .try_into()
                .map_err(|_| DurableError::Io(format!("{nonce_name} has invalid length")))?;
            let key_ref = PiiKeyRef::parse(&key_ref)
                .ok_or_else(|| DurableError::Io(format!("{key_name} is malformed")))?;
            if key_ref.tenant.as_str() != expected_tenant
                || key_ref.class != KeyClass::Subject(expected_subject.to_owned())
            {
                return Err(DurableError::Io(format!(
                    "{key_name} does not match the PR tenant and author subject"
                )));
            }
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
        DurableError::NotFound("pull request not found".into())
    } else if message.contains("different repository")
        || message.contains("different command")
        || message.contains("different merge")
        || message.contains("intent mismatch")
        || message.contains("pending")
    {
        DurableError::Git("PR operation id conflicts with durable state".into())
    } else {
        DurableError::Io("PostgreSQL PR store operation failed".into())
    }
}

fn pg_query(action: &'static str) -> myelin_storage::PgError {
    myelin_storage::PgError::Query(action.into())
}

fn db_number(number: u64) -> Result<i64, DurableError> {
    i64::try_from(number).map_err(|_| DurableError::Git("PR number exceeds bigint".into()))
}

fn qualify_ref(value: &str) -> String {
    if value.starts_with("refs/") {
        value.to_owned()
    } else {
        format!("refs/heads/{value}")
    }
}

fn validate_merge_intent(intent: &MergeIntent) -> Result<(), DurableError> {
    if PrOperationId::from_stored_digest(&intent.operation_id).is_err()
        || intent.actor_subject_id.is_empty()
        || intent.actor_subject_id.len() > 255
        || intent.base_ref.is_empty()
        || intent.base_ref.len() > 1024
        || intent.expected_old_oid.is_empty()
        || intent.head_oid.is_empty()
        || intent.head_repo_slug.is_empty()
        || intent.head_repo_slug.len() > 255
        || !valid_oid(&intent.expected_old_oid)
        || !valid_oid(&intent.head_oid)
    {
        return Err(DurableError::Git(
            "merge intent contains an empty authority field".into(),
        ));
    }
    Ok(())
}

fn valid_oid(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn normalized_subject_id(principal: &Principal) -> Result<String, DurableError> {
    let subject = principal.principal_id.0.trim();
    if subject.is_empty() || subject.len() > 255 {
        return Err(DurableError::Git(
            "verified principal has an invalid subject identifier".into(),
        ));
    }
    Ok(subject.to_owned())
}

fn now_rfc3339() -> Timestamp {
    let seconds = now_unix().max(0) as u64;
    let days = (seconds / 86_400) as i64;
    let remainder = seconds % 86_400;
    let (hour, minute, second) = (remainder / 3_600, (remainder % 3_600) / 60, remainder % 60);
    let shifted_days = days + 719_468;
    let era = shifted_days.div_euclid(146_097);
    let day_of_era = shifted_days.rem_euclid(146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    Timestamp(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
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
    fn accepted_merge_requires_one_exact_nonzero_move_witness() {
        let base = RefName::new("refs/heads/main");
        let head = PushOid::new("0123456789012345678901234567890123456789");

        assert_eq!(
            accepted_merge_update_seq(&[(base.clone(), head.clone(), 7)], &base, &head)
                .unwrap(),
            7
        );
        assert!(accepted_merge_update_seq(&[], &base, &head).is_err());
        assert!(
            accepted_merge_update_seq(&[(base.clone(), head.clone(), 0)], &base, &head).is_err()
        );
        assert!(accepted_merge_update_seq(
            &[(RefName::new("refs/heads/other"), head, 8)],
            &base,
            &PushOid::new("0123456789012345678901234567890123456789"),
        )
        .is_err());
    }

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
        assert!(CREATE_GIT_PR_DDL.contains("record ? 'title'"));
        assert!(CREATE_GIT_PR_DDL.contains("record ? 'body_md'"));
        assert!(CREATE_GIT_PR_DDL.contains("octet_length(title_nonce) = 12"));
        assert!(CREATE_GIT_PR_COMMAND_DDL.contains("actor_subject_id text NOT NULL"));
        assert!(
            CREATE_GIT_PR_COMMAND_DDL.contains("status IN ('pending', 'completed', 'cancelled')")
        );
        assert!(CREATE_GIT_PR_COMMAND_DDL.contains("(status = 'pending') = (result IS NULL)"));
        assert!(CREATE_GIT_PR_COMMAND_DDL
            .contains("PRIMARY KEY (tenant_id, region, repo_slug, operation_id)"));
        assert!(REQUIRE_GIT_PR_COMMAND_DIGEST_DDL.contains("length(operation_id) = 64"));
        assert!(REQUIRE_GIT_PR_COMMAND_DIGEST_DDL.contains("^[0-9a-f]{64}$"));
        assert!(CREATE_GIT_PR_COMMAND_OPERATION_SCOPE_INDEX_DDL
            .contains("CREATE UNIQUE INDEX CONCURRENTLY"));
        assert!(CREATE_GIT_PR_COMMAND_OPERATION_SCOPE_INDEX_DDL
            .contains("(tenant_id, region, operation_id)"));
        assert!(!CREATE_GIT_PR_DDL.contains("title text"));
        assert!(CREATE_GIT_PR_COUNTER_DDL.contains("PRIMARY KEY (tenant_id, region, repo_slug)"));
    }

    #[test]
    fn migration_set_is_forward_only_and_declares_mutated_tables_hot() {
        let migrations = git_pr_migrations();
        assert_eq!(migrations.0.len(), 6);
        for migration in &migrations.0 {
            let upper = migration.ddl.to_ascii_uppercase();
            assert!(!upper.contains("DROP TABLE"));
            assert!(!upper.contains("TRUNCATE"));
        }
        let hot = git_pr_hot_tables();
        assert!(hot.is_hot(GIT_PR_TABLE));
        assert!(hot.is_hot(GIT_PR_COUNTER_TABLE));
        assert!(hot.is_hot(GIT_PR_COMMAND_TABLE));
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
        record.author_subject_id = "principal-123".into();
        record.title = "private launch title".into();
        record.body_md = Some("private launch body".into());
        let sealed = seal_pr_record(&kms, Region("fr-par".into()), &tenant, &record)
            .expect("seal PR free text");
        let encoded = serde_json::to_string(&sealed.record).unwrap();
        assert!(!encoded.contains("private launch"));
        assert!(!encoded.contains("principal-123"));
        assert!(!sealed.title.contains_plaintext(record.title.as_bytes()));
        assert!(!sealed
            .body
            .as_ref()
            .unwrap()
            .contains_plaintext(record.body_md.as_ref().unwrap().as_bytes()));
        let command = serde_json::to_string(&command_projection(&record).unwrap()).unwrap();
        assert!(!command.contains("private launch"));
        assert!(!command.contains("principal-123"));
    }

    #[test]
    fn operation_ids_are_bounded_and_merge_intents_require_complete_authority() {
        assert!(PrOperationId::parse("retry-123").is_ok());
        assert!(PrOperationId::parse(" ").is_err());
        assert!(PrOperationId::parse(&"x".repeat(129)).is_err());
        let operation = PrOperationId::parse("merge-1").unwrap();
        assert_ne!(operation.as_str(), "merge-1");
        assert_eq!(operation.as_str().len(), 64);
        let mut intent = MergeIntent {
            operation_id: operation.as_str().into(),
            actor_subject_id: "principal-123".into(),
            base_ref: "refs/heads/main".into(),
            expected_old_oid: "a".repeat(40),
            head_oid: "b".repeat(40),
            head_repo_slug: "fork".into(),
        };
        assert!(validate_merge_intent(&intent).is_ok());
        intent.operation_id.clear();
        assert!(validate_merge_intent(&intent).is_err());
    }

    #[test]
    fn merge_command_hash_is_stable_across_nonempty_base_crash_recovery() {
        let operation = PrOperationId::parse("merge-1").unwrap();
        let mut intent = MergeIntent {
            operation_id: operation.as_str().into(),
            actor_subject_id: "principal-123".into(),
            base_ref: "refs/heads/main".into(),
            expected_old_oid: "a".repeat(40),
            head_oid: "b".repeat(40),
            head_repo_slug: "fork".into(),
        };
        let before = merge_payload_hash(41, &intent).expect("fresh command hash");
        intent.expected_old_oid = "c".repeat(40);
        let recovered = merge_payload_hash(41, &intent).expect("recovery command hash");
        assert_eq!(
            before, recovered,
            "the observed old tip is not request identity"
        );

        intent.head_oid = "d".repeat(40);
        assert_ne!(
            before,
            merge_payload_hash(41, &intent).expect("divergent command hash"),
            "locked PR provenance remains bound"
        );
    }

    #[test]
    fn verified_repo_scope_retains_types_without_disclosing_partition_authority() {
        use myelin_identity::{PrincipalId, PrincipalKind};

        let mut principal = Principal::stub(
            PrincipalId("subject-under-test".into()),
            PrincipalKind::Human,
            TenantId("tenant-secret".into()),
        );
        principal.region = Region("region-secret".into());
        let scope = TenantScope::from_verified_token(&principal, principal.region.clone());

        let target = VerifiedRepoScope::new(&scope, "repo-secret", "region-secret")
            .expect("matching verified scope");
        assert_eq!(target.tenant_id, TenantId("tenant-secret".into()));
        assert_eq!(target.region, Region("region-secret".into()));

        let debug = format!("{target:?}");
        for secret in ["tenant-secret", "region-secret", "repo-secret"] {
            assert!(!debug.contains(secret), "debug disclosed {secret}");
        }

        let mismatch = VerifiedRepoScope::new(&scope, "repo-secret", "provider-secret")
            .expect_err("cross-region scope must be rejected");
        assert!(matches!(mismatch, DurableError::NotFound(_)));
        let public_error = mismatch.to_string();
        for secret in [
            "tenant-secret",
            "region-secret",
            "repo-secret",
            "provider-secret",
        ] {
            assert!(
                !public_error.contains(secret),
                "scope error disclosed {secret}"
            );
        }
    }

    #[cfg(feature = "integration")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn live_pg_pr_boundary_proves_isolation_atomicity_idempotency_and_merge_recovery() {
        use myelin_config::{Mode, MyelinConfig};
        use myelin_events::{MonotonicMinter, OutboxStore};
        use myelin_identity::{PrincipalId, PrincipalKind, RuntimeRef};
        use myelin_storage::kms::DekId;
        use myelin_storage::PgBootstrap;

        fn principal(tenant: &str, region: &str, subject: &str) -> Principal {
            let mut principal = Principal::stub(
                PrincipalId(subject.into()),
                PrincipalKind::Human,
                TenantId(tenant.into()),
            );
            principal.region = Region(region.into());
            principal
        }

        fn record(head_oid: &str, title: &str, head_repo: &str) -> PrRecord {
            let pr = crate::lifecycle::PullRequest::open(
                0,
                "refs/heads/main",
                "refs/heads/feature",
                "author@tenant.noreply",
                false,
            );
            let mut record = PrRecord::open(&pr, head_oid);
            record.head_repo_slug = head_repo.into();
            record.title = title.into();
            record.body_md = Some(format!("body:{title}"));
            record
        }

        fn seed_repo(repo: &DurableGitRepo) -> (CoreOid, CoreOid) {
            let (base, _, _) = repo
                .build_file_commit(
                    "refs/heads/main",
                    "a.txt",
                    b"base\n",
                    "base",
                    "psn@tenant.noreply",
                    "psn@tenant.noreply",
                )
                .expect("build base");
            repo.update_ref_cas(
                "refs/heads/main",
                None,
                Some(&base),
                "seed base",
                "psn@tenant.noreply",
            )
            .expect("seed main");
            let (head, _, _) = repo
                .build_file_commit(
                    "refs/heads/main",
                    "a.txt",
                    b"head\n",
                    "head",
                    "psn@tenant.noreply",
                    "psn@tenant.noreply",
                )
                .expect("build head");
            repo.update_ref_cas(
                "refs/heads/feature",
                None,
                Some(&head),
                "seed feature",
                "psn@tenant.noreply",
            )
            .expect("seed feature");
            (base, head)
        }

        fn open_ref_store(repo: Arc<DurableGitRepo>, slug: &str, ctx: EmitContextBase) -> RefStore {
            RefStore::open_durable(
                repo,
                slug,
                ctx,
                OutboxStore::new(),
                Arc::new(MonotonicMinter::new()),
            )
        }

        let cfg = MyelinConfig::from_env(Mode::DevDefaults).expect("integration config");
        let handle = tokio::runtime::Handle::current();
        let kms = Arc::new(KmsEngine::new());

        // A raw/admin-connected provider never carries the validated runtime capability marker.
        let mut admin_cfg = cfg.clone();
        admin_cfg.database_url = cfg.database_migration_url.clone();
        let raw_admin = SubstrateProvider::connect(admin_cfg, 1)
            .await
            .expect("connect raw admin provider");
        assert!(PgPrStore::new(raw_admin.clone(), kms.clone(), handle.clone()).is_err());
        raw_admin.db_pool().close().await;

        let bootstrap = PgBootstrap::connect(cfg.clone(), 12)
            .await
            .expect("validate split database roles");
        bootstrap
            .migrate_foundation()
            .await
            .expect("foundation migrations");
        bootstrap
            .migrate(&git_pr_migrations(), &git_pr_hot_tables())
            .await
            .expect("Git PR migrations");
        assert!(bootstrap
            .verify_index_ready("missing_git_pr_index")
            .await
            .is_err());
        bootstrap
            .verify_index_ready("git_pr_head_repo_idx")
            .await
            .expect("concurrent provenance index ready");
        let provider = bootstrap
            .into_runtime()
            .await
            .expect("validated runtime handoff");
        let store = PgPrStore::with_minter(
            provider.clone(),
            kms.clone(),
            handle,
            Arc::new(UlidMinter::new()),
        );
        // @residency-cell-pinned: integration admin pool uses the validated region-specific DSN.
        let admin = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&cfg.database_migration_url)
            .await
            .expect("admin assertions pool");

        let suffix = format!("{}-{}", std::process::id(), now_unix());
        let tenant_a = format!("git-pr-a-{suffix}");
        let tenant_b = format!("git-pr-b-{suffix}");
        let region = cfg.region.clone();
        let actor = principal(&tenant_a, &region, &format!("subject-{suffix}"));
        let actor_b = principal(&tenant_b, &region, &format!("subject-b-{suffix}"));
        let scope_a = TenantScope::from_verified_token(&actor, actor.region.clone());
        let scope_b = TenantScope::from_verified_token(&actor_b, actor_b.region.clone());
        let repo = "core";

        // Aborting after event staging rolls back the row, counter allocation, command and outbox.
        let abort_op = PrOperationId::parse("abort-open").unwrap();
        assert!(store
            .open_then_abort_for_test(
                &scope_a,
                repo,
                record(&"1".repeat(40), "abort-secret", repo),
                &abort_op,
                &actor,
            )
            .is_err());
        assert!(store.list(&scope_a, repo).unwrap().is_empty());
        let abort_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM outbox WHERE envelope->>'tenant'=$1 AND envelope->>'type_'=$2",
        )
        .bind(&tenant_a)
        .bind(GIT_PR_OPENED)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(abort_events, 0, "aborted open emitted no ghost event");

        // Exact retry replays one historical result/event; divergent reuse is rejected.
        let open_op = PrOperationId::parse("open-1").unwrap();
        let opened = store
            .open(
                &scope_a,
                repo,
                record(&"2".repeat(40), "private-title", repo),
                &open_op,
                &actor,
            )
            .unwrap();
        let replayed = store
            .open(
                &scope_a,
                repo,
                record(&"2".repeat(40), "private-title", repo),
                &open_op,
                &actor,
            )
            .unwrap();
        assert_eq!(opened, replayed);
        assert!(store
            .open(
                &scope_a,
                repo,
                record(&"2".repeat(40), "different-title", repo),
                &open_op,
                &actor,
            )
            .is_err());

        // Concurrent opens allocate a gap-free committed range and never collide.
        let mut joins = Vec::new();
        for i in 0..8_u64 {
            let store = store.clone();
            let scope = scope_a.clone();
            let actor = actor.clone();
            joins.push(tokio::task::spawn_blocking(move || {
                let op = PrOperationId::parse(&format!("concurrent-open-{i}")).unwrap();
                store
                    .open(
                        &scope,
                        repo,
                        record(&format!("{:040x}", i + 10), &format!("title-{i}"), repo),
                        &op,
                        &actor,
                    )
                    .unwrap()
                    .number
            }));
        }
        let mut numbers = Vec::new();
        for join in joins {
            numbers.push(join.await.unwrap());
        }
        numbers.sort_unstable();
        assert_eq!(numbers, (2..=9).collect::<Vec<_>>());

        // Concurrent read/modify/write mutations retain both reviews; exact retry emits no duplicate.
        let mut mutation_joins = Vec::new();
        for i in 0..2_u64 {
            let store = store.clone();
            let scope = scope_a.clone();
            let actor = actor.clone();
            mutation_joins.push(tokio::task::spawn_blocking(move || {
                let op = PrOperationId::parse(&format!("review-{i}")).unwrap();
                store.apply_mutation(
                    &scope,
                    repo,
                    opened.number,
                    PrMutation::SubmitReview(crate::pr_store::ReviewRecord {
                        reviewer_pseudonym: format!("reviewer-{i}@tenant.noreply"),
                        state: crate::lifecycle::ReviewState::Submitted(
                            crate::lifecycle::ReviewVerdict::Approve,
                        ),
                        is_agent: false,
                    }),
                    &op,
                    &actor,
                )
            }));
        }
        for join in mutation_joins {
            join.await.unwrap().unwrap();
        }
        let mutated = store.get(&scope_a, repo, opened.number).unwrap().unwrap();
        assert_eq!(mutated.reviews.len(), 2, "row lock prevented a lost update");
        let replay_review = PrOperationId::parse("review-0").unwrap();
        store
            .apply_mutation(
                &scope_a,
                repo,
                opened.number,
                PrMutation::SubmitReview(crate::pr_store::ReviewRecord {
                    reviewer_pseudonym: "reviewer-0@tenant.noreply".into(),
                    state: crate::lifecycle::ReviewState::Submitted(
                        crate::lifecycle::ReviewVerdict::Approve,
                    ),
                    is_agent: false,
                }),
                &replay_review,
                &actor,
            )
            .unwrap();
        let lifecycle_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM outbox WHERE envelope->>'tenant'=$1 AND envelope->>'type_' LIKE 'git.pr.%'",
        )
        .bind(&tenant_a)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(lifecycle_events, 11, "9 opens + 2 mutations, exactly once");

        // Tenant RLS makes the same repo/number invisible; region mismatch fails before SQL.
        assert!(store.get(&scope_b, repo, opened.number).unwrap().is_none());
        let tenant_a_for_probe = tenant_a.clone();
        let repo_for_probe = repo.to_string();
        let rls_bypass_attempt: i64 = provider
            .with_tenant_tx(&tenant_b, move |conn| {
                Box::pin(async move {
                    sqlx::query_scalar(
                        "SELECT count(*) FROM git_pr
                          WHERE tenant_id=$1 AND repo_slug=$2",
                    )
                    .bind(tenant_a_for_probe)
                    .bind(repo_for_probe)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|_| myelin_storage::PgError::Query("RLS probe failed".into()))
                })
            })
            .await
            .unwrap();
        assert_eq!(
            rls_bypass_attempt, 0,
            "an explicit tenant-A predicate is still invisible under tenant-B RLS"
        );
        let mut wrong_region_actor = actor.clone();
        wrong_region_actor.region = Region("wrong-region".into());
        let wrong_region = TenantScope::from_verified_token(
            &wrong_region_actor,
            wrong_region_actor.region.clone(),
        );
        assert!(matches!(
            store.get(&wrong_region, repo, opened.number),
            Err(DurableError::NotFound(_))
        ));

        // The broad JSON projection and command result contain no plaintext or subject locator.
        let row = sqlx::query(
            "SELECT record::text AS record,title_ciphertext,author_subject_id FROM git_pr
              WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4",
        )
        .bind(&tenant_a)
        .bind(&region)
        .bind(repo)
        .bind(opened.number as i64)
        .fetch_one(&admin)
        .await
        .unwrap();
        let projection: String = row.try_get("record").unwrap();
        let ciphertext: Vec<u8> = row.try_get("title_ciphertext").unwrap();
        let subject: String = row.try_get("author_subject_id").unwrap();
        assert!(!projection.contains("private-title"));
        assert!(!projection.contains(&subject));
        assert!(!ciphertext
            .windows("private-title".len())
            .any(|w| w == b"private-title"));
        let command_result: String = sqlx::query_scalar(
            "SELECT result::text FROM git_pr_command
              WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND operation_id=$4",
        )
        .bind(&tenant_a)
        .bind(&region)
        .bind(repo)
        .bind(open_op.digest())
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(!command_result.contains("private-title"));
        assert!(!command_result.contains(&subject));
        let raw_operation_ids: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM git_pr_command
              WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3
                AND (operation_id IN ('open-1','review-0')
                     OR length(operation_id) <> 64
                     OR operation_id !~ '^[0-9a-f]{64}$')",
        )
        .bind(&tenant_a)
        .bind(&region)
        .bind(repo)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            raw_operation_ids, 0,
            "raw Idempotency-Key material never reaches durable command rows"
        );
        let leaking_envelopes: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM outbox
              WHERE envelope->>'tenant'=$1
                AND (envelope::text LIKE '%' || $2 || '%'
                     OR envelope::text LIKE '%private-title%')",
        )
        .bind(&tenant_a)
        .bind(&subject)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(leaking_envelopes, 0, "lifecycle envelopes remain PII-free");

        // Concrete durable ref crash recovery: admitted intent freezes policy/source authority.
        let root = std::env::temp_dir().join(format!("myelin-pg-pr-live-{suffix}"));
        let git_store = crate::durable::DurableGitStore::rooted(&root);
        let policy_store = DurablePrStore::rooted(&root);
        for slug in ["merge-before", "merge-after", "merge-cancel"] {
            let loc = RepoLoc::new(&tenant_a, &region, slug);
            let target = Arc::new(git_store.create_repo(&loc).unwrap());
            let (base, head) = seed_repo(&target);
            policy_store
                .put_protection(
                    &loc,
                    &crate::pr_store::BranchProtectionConfig {
                        rulesets: vec![crate::lifecycle::BranchProtectionRuleset {
                            ref_pattern: "refs/heads/main".into(),
                            required_contexts: vec![],
                            required_approvals: 0,
                            require_codeowner_review: false,
                            require_conversation_resolution: false,
                            allow_force_push: false,
                        }],
                    },
                )
                .unwrap();
            let op = PrOperationId::parse(&format!("merge-{slug}")).unwrap();
            let open_op = PrOperationId::parse(&format!("open-{slug}")).unwrap();
            let opened = store
                .open(
                    &scope_a,
                    slug,
                    record(head.as_str(), &format!("merge {slug}"), slug),
                    &open_op,
                    &actor,
                )
                .unwrap();
            let intent = MergeIntent {
                operation_id: op.as_str().into(),
                actor_subject_id: actor.principal_id.0.clone(),
                base_ref: "refs/heads/main".into(),
                expected_old_oid: base.0.clone(),
                head_oid: head.0.clone(),
                head_repo_slug: slug.into(),
            };
            let hash = merge_payload_hash(opened.number, &intent).unwrap();
            let mut event_agent = Principal::stub(
                PrincipalId("agent:raw-event-subject".into()),
                PrincipalKind::Agent {
                    runtime_ref: RuntimeRef("runtime://raw-pg-worker/session".into()),
                    on_behalf_of: Some(PrincipalId("human:raw-pg-delegator".into())),
                },
                scope_a.tenant().clone(),
            );
            event_agent.region = scope_a.region().clone();
            let privacy_ctx = store.emit_context(&scope_a, &event_agent).unwrap();
            let serialized_actor = serde_json::to_string(&privacy_ctx.actor).unwrap();
            for raw in [
                "agent:raw-event-subject",
                "runtime://raw-pg-worker/session",
                "human:raw-pg-delegator",
            ] {
                assert!(!serialized_actor.contains(raw), "PG event actor leaked {raw}");
            }
            let ctx = store.emit_context(&scope_a, &actor).unwrap();
            assert!(store
                .begin_merge(
                    &scope_a,
                    slug,
                    opened.number,
                    intent.clone(),
                    &hash,
                    ctx.clone()
                )
                .unwrap()
                .is_none());
            assert!(
                store
                    .apply_mutation(
                        &scope_a,
                        slug,
                        opened.number,
                        PrMutation::Touch,
                        &op,
                        &actor,
                    )
                    .is_err(),
                "the pending merge reservation owns the repo-wide operation id"
            );
            let unrelated_op = PrOperationId::parse(&format!("touch-while-{slug}")).unwrap();
            assert!(
                store
                    .apply_mutation(
                        &scope_a,
                        slug,
                        opened.number,
                        PrMutation::Touch,
                        &unrelated_op,
                        &actor,
                    )
                    .is_err(),
                "a durable merge intent freezes every ordinary PR mutation"
            );
            assert!(store
                .list_pending_merges(&scope_a)
                .unwrap()
                .iter()
                .any(|pending| pending.repo_slug == slug && pending.number == opened.number));

            let ref_store = open_ref_store(target.clone(), slug, ctx);
            if slug == "merge-before" {
                // Mutable policy/source become invalid after the durable admitted intent. Recovery
                // must still execute the exact locked CAS instead of wedging Pending.
                policy_store
                    .put_protection(&loc, &crate::pr_store::BranchProtectionConfig::default())
                    .unwrap();
                target
                    .update_ref_cas(
                        "refs/heads/feature",
                        Some(&head),
                        None,
                        "delete source after intent",
                        "psn@tenant.noreply",
                    )
                    .unwrap();
                assert!(matches!(
                    store
                        .recover_pending_merge_target(
                            &scope_a,
                            slug,
                            opened.number,
                            &actor,
                            &loc,
                            &target,
                            &ref_store,
                        )
                        .unwrap(),
                    Some(MergeAttempt::Merged { .. })
                ));
            } else if slug == "merge-after" {
                // Simulate crash after the concrete ref CAS but before PostgreSQL finalization.
                target
                    .update_ref_cas(
                        "refs/heads/main",
                        Some(&base),
                        Some(&head),
                        "crash-window CAS",
                        "psn@tenant.noreply",
                    )
                    .unwrap();
                target
                    .update_ref_cas(
                        "refs/heads/feature",
                        Some(&head),
                        None,
                        "delete source after CAS",
                        "psn@tenant.noreply",
                    )
                    .unwrap();
                assert!(matches!(
                    store
                        .recover_pending_merge_target(
                            &scope_a,
                            slug,
                            opened.number,
                            &actor,
                            &loc,
                            &target,
                            &ref_store,
                        )
                        .unwrap(),
                    Some(MergeAttempt::Merged { .. })
                ));
            } else {
                let (raced, _, _) = target
                    .build_file_commit(
                        "refs/heads/main",
                        "race.txt",
                        b"race\n",
                        "race",
                        "psn@tenant.noreply",
                        "psn@tenant.noreply",
                    )
                    .unwrap();
                target
                    .update_ref_cas(
                        "refs/heads/main",
                        Some(&base),
                        Some(&raced),
                        "race base",
                        "psn@tenant.noreply",
                    )
                    .unwrap();
                let first = store
                    .recover_pending_merge_target(
                        &scope_a,
                        slug,
                        opened.number,
                        &actor,
                        &loc,
                        &target,
                        &ref_store,
                    )
                    .unwrap()
                    .unwrap();
                assert!(matches!(first, MergeAttempt::RefRefused(_)));
                let retry = store
                    .merge_pr_durable(
                        &scope_a,
                        slug,
                        opened.number,
                        &op,
                        &actor,
                        &policy_store,
                        &loc,
                        &target,
                        &target,
                        &ref_store,
                        "merger@tenant.noreply",
                    )
                    .unwrap();
                assert_eq!(first, retry, "cancelled command replays deterministically");
            }
            assert!(!store
                .list_pending_merges(&scope_a)
                .unwrap()
                .iter()
                .any(|pending| pending.repo_slug == slug && pending.number == opened.number));
            let aggregate = format!("git/pr/{slug}:{}", opened.number);
            let event_rows: Vec<(String, i64)> = sqlx::query_as(
                "SELECT envelope->>'type_',count(*)
                   FROM outbox WHERE aggregate=$1 AND envelope->>'tenant'=$2
                  GROUP BY envelope->>'type_' ORDER BY envelope->>'type_'",
            )
            .bind(&aggregate)
            .bind(&tenant_a)
            .fetch_all(&admin)
            .await
            .unwrap();
            let count = |kind: &str| {
                event_rows
                    .iter()
                    .find_map(|(event, count)| (event == kind).then_some(*count))
                    .unwrap_or(0)
            };
            assert_eq!(count(GIT_PR_OPENED), 1);
            if slug == "merge-cancel" {
                assert_eq!(count(GIT_PR_UPDATED), 2, "begin + exact cancellation");
                assert_eq!(count(GIT_PR_MERGED), 0);
            } else {
                assert_eq!(count(GIT_PR_UPDATED), 1, "one durable merge intent");
                assert_eq!(count(GIT_PR_MERGED), 1, "one exact finalization");
            }
        }
        let terminal_leaks: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM git_pr_command
              WHERE tenant_id=$1 AND result IS NOT NULL
                AND (result::text LIKE '%' || $2 || '%'
                     OR result::text LIKE '%merge merge-before%'
                     OR result::text LIKE '%body:merge%')",
        )
        .bind(&tenant_a)
        .bind(&subject)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            terminal_leaks, 0,
            "terminal command projections remain PII-free"
        );
        let final_envelope_leaks: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM outbox
              WHERE envelope->>'tenant'=$1
                AND (envelope::text LIKE '%' || $2 || '%'
                     OR envelope::text LIKE '%private-title%'
                     OR envelope::text LIKE '%merge merge-before%')",
        )
        .bind(&tenant_a)
        .bind(&subject)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            final_envelope_leaks, 0,
            "all lifecycle envelopes remain PII-free"
        );

        // Subject crypto-shred makes every column under that exact canonical subject unavailable.
        assert!(kms.destroy_dek(&DekId::new(
            TenantId(tenant_a.clone()),
            KeyClass::Subject(actor.principal_id.0.clone()),
        )));
        assert!(store.get(&scope_a, repo, opened.number).is_err());

        sqlx::query("DELETE FROM outbox WHERE envelope->>'tenant'=$1")
            .bind(&tenant_a)
            .execute(&admin)
            .await
            .unwrap();
        for table in ["git_pr_command", "git_pr", "git_pr_counter"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE tenant_id=$1"))
                .bind(&tenant_a)
                .execute(&admin)
                .await
                .unwrap();
        }
        admin.close().await;
        std::fs::remove_dir_all(root).ok();
    }
}
