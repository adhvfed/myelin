use std::future::Future;
use std::sync::Arc;

use myelin_events::{Actor, CausedBy, EmitContextBase, IdMinter, UlidMinter};
use myelin_gdpr::ErasureMethod;
use myelin_identity::Principal;
use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, SubjectId};
use myelin_storage::kms::{KekId, KeyClass, KmsEngine, PiiKeyRef, NONCE_LEN};
use myelin_storage::{
    HotTables, Migration, MigrationPhase, Migrations, SubstrateProvider, TenantScope,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;

use crate::clock::system_clock_reading;
use crate::core::{Oid as CoreOid, RepoLoc};
use crate::durable::{DurableError, DurableGitRepo};
use crate::events::{
    event_actor_pseudonym, pseudonymized_event_principal, GIT_PR_MERGED, GIT_PR_OPENED,
    GIT_PR_UPDATED, GIT_REVIEW_SUBMITTED,
};
use crate::lifecycle::{BranchProtectionRuleset, PrState};
use crate::pg_pr_event::co_commit_event;
use crate::pr_list_pagination::PrListPage;
use crate::pr_store::{
    accepted_merge_update_seq, ensure_pr_record_size, evaluate_merge, DurablePrStore, MergeAttempt,
    MergeEval, PrCrossListQuery, PrCrossListRecord, PrCrossListSlice, PrListCounts, PrListQuery,
    PrListSlice, PrRecord,
};
use crate::receive_pack::{
    CrashPoint, InMemoryObjectDb, Oid as PushOid, ProposedRefUpdate, PushOutcome, PushProvenance,
    PushSession, Pusher, RefName, RefStore,
};

mod check_admission;
use check_admission::overlay_projected_checks;
mod list_queries;
use list_queries::{pr_cross_list_page_sql, pr_list_page_sql, validate_cross_visible_slugs};

pub const GIT_PR_TABLE: &str = "git_pr";
pub const GIT_PR_COUNTER_TABLE: &str = "git_pr_counter";
pub const GIT_PR_COMMAND_TABLE: &str = "git_pr_command";
const PR_BATCH_MAX_COORDINATES: usize = 10_000;
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

pub const CREATE_GIT_PR_REPO_UPDATED_LIST_INDEX_DDL: &str = r#"
CREATE INDEX CONCURRENTLY IF NOT EXISTS git_pr_repo_updated_list_idx
  ON git_pr (
    tenant_id, region, repo_slug,
    ((record->>'updated_at')::bigint) DESC NULLS LAST,
    number DESC
  )
"#;

pub const CREATE_GIT_PR_REPO_STATE_UPDATED_LIST_INDEX_DDL: &str = r#"
CREATE INDEX CONCURRENTLY IF NOT EXISTS git_pr_repo_state_updated_list_idx
  ON git_pr (
    tenant_id, region, repo_slug, (record->>'state'),
    ((record->>'updated_at')::bigint) DESC NULLS LAST,
    number DESC
  )
"#;

pub const CREATE_GIT_PR_REPO_STATE_CREATED_LIST_INDEX_DDL: &str = r#"
CREATE INDEX CONCURRENTLY IF NOT EXISTS git_pr_repo_state_created_list_idx
  ON git_pr (tenant_id, region, repo_slug, (record->>'state'), number DESC)
"#;

pub const CREATE_GIT_PR_CROSS_UPDATED_LIST_INDEX_DDL: &str = r#"
CREATE INDEX CONCURRENTLY IF NOT EXISTS git_pr_cross_updated_list_idx
  ON git_pr (
    tenant_id, region,
    ((record->>'updated_at')::bigint) DESC NULLS LAST,
    number DESC, repo_slug ASC
  )
"#;

pub const CREATE_GIT_PR_CROSS_CREATED_LIST_INDEX_DDL: &str = r#"
CREATE INDEX CONCURRENTLY IF NOT EXISTS git_pr_cross_created_list_idx
  ON git_pr (tenant_id, region, number DESC, repo_slug ASC)
"#;

pub const CREATE_GIT_PR_REVIEWS_GIN_INDEX_DDL: &str = r#"
CREATE INDEX CONCURRENTLY IF NOT EXISTS git_pr_reviews_gin_idx
  ON git_pr USING gin ((record->'reviews') jsonb_path_ops)
"#;

pub const CREATE_GIT_PR_AUTHOR_UPDATED_LIST_INDEX_DDL: &str = r#"
CREATE INDEX CONCURRENTLY IF NOT EXISTS git_pr_author_updated_list_idx
  ON git_pr (
    tenant_id, region, (record->>'author_pseudonym'),
    ((record->>'updated_at')::bigint) DESC NULLS LAST,
    number DESC, repo_slug ASC
  )
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
        Migration::phased(
            "git_0007_pr_repo_updated_list_index",
            CREATE_GIT_PR_REPO_UPDATED_LIST_INDEX_DDL,
            MigrationPhase::Expand,
            GIT_PR_TABLE,
        ),
        Migration::phased(
            "git_0008_pr_repo_state_updated_list_index",
            CREATE_GIT_PR_REPO_STATE_UPDATED_LIST_INDEX_DDL,
            MigrationPhase::Expand,
            GIT_PR_TABLE,
        ),
        Migration::phased(
            "git_0009_pr_repo_state_created_list_index",
            CREATE_GIT_PR_REPO_STATE_CREATED_LIST_INDEX_DDL,
            MigrationPhase::Expand,
            GIT_PR_TABLE,
        ),
        Migration::phased(
            "git_0010_pr_cross_updated_list_index",
            CREATE_GIT_PR_CROSS_UPDATED_LIST_INDEX_DDL,
            MigrationPhase::Expand,
            GIT_PR_TABLE,
        ),
        Migration::phased(
            "git_0011_pr_cross_created_list_index",
            CREATE_GIT_PR_CROSS_CREATED_LIST_INDEX_DDL,
            MigrationPhase::Expand,
            GIT_PR_TABLE,
        ),
        Migration::phased(
            "git_0012_pr_reviews_gin_index",
            CREATE_GIT_PR_REVIEWS_GIN_INDEX_DDL,
            MigrationPhase::Expand,
            GIT_PR_TABLE,
        ),
        Migration::phased(
            "git_0013_pr_author_updated_list_index",
            CREATE_GIT_PR_AUTHOR_UPDATED_LIST_INDEX_DDL,
            MigrationPhase::Expand,
            GIT_PR_TABLE,
        ),
    ])
}

pub fn git_pr_hot_tables() -> HotTables {
    HotTables::declare([GIT_PR_TABLE, GIT_PR_COUNTER_TABLE, GIT_PR_COMMAND_TABLE])
}

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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MergeIntent {
    pub operation_id: String,
    pub actor_subject_id: String,
    #[serde(default = "untrusted_merge_provenance")]
    pub ref_update_provenance: PushProvenance,
    pub base_ref: String,
    pub expected_old_oid: String,
    pub head_oid: String,
    pub head_repo_slug: String,
}

fn untrusted_merge_provenance() -> PushProvenance {
    PushProvenance::Agent
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingMerge {
    pub repo_slug: String,
    pub number: u64,
    pub intent: MergeIntent,
}

struct MergeAdmission {
    intent: MergeIntent,
    command_hash: String,
    ctx: PrOperationContext,
    ruleset: BranchProtectionRuleset,
    project_checks: bool,
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

enum MergeLedgerState {
    Absent,
    Pending,
    Terminal(MergeAttempt),
}

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

impl PrMutation {
    fn command_kind(&self) -> &'static str {
        match self {
            Self::ReportChecks { .. } => "report-checks",
            Self::SubmitReview(_) => "submit-review",
            Self::EndorseContexts(_) => "endorse-contexts",
            Self::Touch => "touch",
        }
    }

    fn event_type(&self) -> &'static str {
        match self {
            Self::SubmitReview(_) => GIT_REVIEW_SUBMITTED,
            Self::ReportChecks { .. } | Self::EndorseContexts(_) | Self::Touch => GIT_PR_UPDATED,
        }
    }

    pub fn apply_to_at(self, record: &mut PrRecord, updated_at_unix: i64) {
        match self {
            Self::ReportChecks {
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
            Self::SubmitReview(review) => record.reviews.push(review),
            Self::EndorseContexts(contexts) => {
                for context in contexts {
                    if !record.endorsed_contexts.contains(&context) {
                        record.endorsed_contexts.push(context);
                    }
                }
            }
            Self::Touch => {}
        }
        record.updated_at = Some(updated_at_unix);
    }
}

#[derive(Clone)]
struct PrOperationContext {
    emit: EmitContextBase,
    unix_seconds: i64,
}

struct PrMutationCommand {
    operation_id: PrOperationId,
    actor_subject_id: String,
    payload_hash: String,
    context: PrOperationContext,
}

struct PrCommandIdentity<'a> {
    operation_id: &'a PrOperationId,
    actor_subject_id: &'a str,
    command_kind: &'a str,
    payload_hash: &'a str,
    expected_number: Option<i64>,
}

impl<'a> PrCommandIdentity<'a> {
    fn merge(
        operation_id: &'a PrOperationId,
        actor_subject_id: &'a str,
        payload_hash: &'a str,
        number: i64,
    ) -> Self {
        Self {
            operation_id,
            actor_subject_id,
            command_kind: "merge",
            payload_hash,
            expected_number: Some(number),
        }
    }
}

struct SealedPrRecord {
    record: serde_json::Value,
    title: EncryptedColumn,
    body: Option<EncryptedColumn>,
}

#[derive(Clone, PartialEq, Eq)]
struct VerifiedRepoScope {
    tenant_id: TenantId,
    region: Region,
    loc: RepoLoc,
}

#[derive(Clone)]
struct VerifiedCellScope {
    tenant_id: TenantId,
    region: Region,
}

impl VerifiedCellScope {
    fn new(scope: &TenantScope, provider_region: &str) -> Result<Self, DurableError> {
        if scope.region().0 != provider_region || scope.tenant().0.is_empty() {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        Ok(Self {
            tenant_id: scope.tenant().clone(),
            region: scope.region().clone(),
        })
    }
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

    fn scoped_cell(&self, scope: &TenantScope) -> Result<VerifiedCellScope, DurableError> {
        VerifiedCellScope::new(scope, &self.provider.config().region)
    }

    fn operation_context(
        &self,
        scope: &TenantScope,
        principal: &Principal,
    ) -> Result<PrOperationContext, DurableError> {
        if principal.tenant != *scope.tenant() || principal.region != *scope.region() {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        if scope.region().0 != self.provider.config().region {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        let clock = system_clock_reading()
            .map_err(|error| DurableError::Io(format!("Git PR clock unavailable: {error}")))?;
        let timestamp = clock.timestamp();
        let event_principal = pseudonymized_event_principal(scope.tenant().as_str(), principal);
        Ok(PrOperationContext {
            emit: EmitContextBase {
                tenant: scope.tenant().clone(),
                region: scope.region().clone(),
                actor: Actor(event_principal),
                schema_ver: 1,
                occurred_at: timestamp.clone(),
                recorded_at: timestamp,
                caused_by: None,
            },
            unix_seconds: clock.unix_seconds(),
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

    /// Read an exact, bounded set of pull requests from one tenant cell.
    ///
    /// This primitive makes no access decision. Callers must authorize every returned record before
    /// exposing it. Missing records are omitted, and duplicate coordinates are read once.
    pub fn get_many(
        &self,
        scope: &TenantScope,
        coordinates: &[(String, u64)],
    ) -> Result<Vec<(String, PrRecord)>, DurableError> {
        let cell = self.scoped_cell(scope)?;
        let coordinates = normalize_pr_batch_coordinates(coordinates)?;
        if coordinates.is_empty() {
            return Ok(Vec::new());
        }
        let (repos, numbers): (Vec<_>, Vec<_>) = coordinates.into_iter().unzip();
        let provider = self.provider.clone();
        let kms = self.kms.clone();
        let tenant_id = cell.tenant_id;
        let region = cell.region;
        let transaction_tenant = tenant_id.0.clone();
        let crypto_region = region.clone();
        let expected_tenant = tenant_id.0.clone();
        let rows = self
            .block_on(async move {
                provider
                    .with_tenant_tx(&transaction_tenant.clone(), move |conn| {
                        Box::pin(async move {
                            let read_sql = format!(
                                "SELECT pr.repo_slug AS requested_repo_slug, {PR_RECORD_COLUMNS} \
                                 FROM git_pr AS pr \
                                 JOIN unnest($3::text[], $4::bigint[]) \
                                   AS requested(repo_slug, number) \
                                   ON pr.repo_slug=requested.repo_slug \
                                  AND pr.number=requested.number \
                                 WHERE pr.tenant_id=$1 AND pr.region=$2 \
                                 ORDER BY pr.repo_slug ASC, pr.number ASC"
                            );
                            sqlx::query(&read_sql)
                                .bind(&tenant_id.0)
                                .bind(&region.0)
                                .bind(&repos)
                                .bind(&numbers)
                                .fetch_all(&mut *conn)
                                .await
                                .map_err(|_| pg_query("read PR batch"))
                        })
                    })
                    .await
            })
            .map_err(pg_error)?;
        rows.into_iter()
            .map(|row| {
                let repo = row
                    .try_get("requested_repo_slug")
                    .map_err(|_| DurableError::Io("PR batch repository malformed".into()))?;
                let record = decode_record(&kms, &crypto_region, &expected_tenant, row)?;
                Ok((repo, record))
            })
            .collect()
    }

    pub fn list_bounded(
        &self,
        scope: &TenantScope,
        repo: &str,
        maximum_records: usize,
        maximum_bytes: usize,
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
        let record_limit = i64::try_from(maximum_records).unwrap_or(i64::MAX);
        let byte_limit = i64::try_from(maximum_bytes).unwrap_or(i64::MAX);
        let rows = self
            .block_on(async move {
                provider
                    .with_tenant_tx(&transaction_tenant.clone(), move |conn| {
                        Box::pin(async move {
                            let summary = sqlx::query(
                                "SELECT count(*) AS record_count, \
                                 COALESCE(sum(pg_column_size(git_pr)), 0)::bigint AS total_bytes \
                                 FROM git_pr WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3",
                            )
                            .bind(&tenant_id.0)
                            .bind(&region.0)
                            .bind(&loc.repo)
                            .fetch_one(&mut *conn)
                            .await
                            .map_err(|_| pg_query("measure PR list"))?;
                            let record_count: i64 = summary
                                .try_get("record_count")
                                .map_err(|_| pg_query("decode PR list count"))?;
                            let total_bytes: i64 = summary
                                .try_get("total_bytes")
                                .map_err(|_| pg_query("decode PR list size"))?;
                            if record_count > record_limit {
                                return Err(myelin_storage::PgError::Query(
                                    "pull request list limit exceeded: record count".into(),
                                ));
                            }
                            if total_bytes > byte_limit {
                                return Err(myelin_storage::PgError::Query(
                                    "pull request list limit exceeded: serialized bytes".into(),
                                ));
                            }
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

    pub fn list_page(
        &self,
        scope: &TenantScope,
        repo: &str,
        query: &PrListQuery,
    ) -> Result<PrListSlice, DurableError> {
        query.validate()?;
        let target = self.scoped_target(scope, repo)?;
        let provider = self.provider.clone();
        let kms = self.kms.clone();
        let tenant_id = target.tenant_id;
        let region = target.region;
        let loc = target.loc;
        let transaction_tenant = tenant_id.0.clone();
        let crypto_region = region.clone();
        let expected_tenant = tenant_id.0.clone();
        let fetch_limit = i64::try_from(query.limit)
            .map_err(|_| DurableError::Git("pull request page limit is too large".into()))?;
        let sql = pr_list_page_sql(query);
        let viewer = query.viewer_pseudonym.clone();
        let page_selector = query.page.clone();
        let rows = self
            .block_on(async move {
                provider
                    .with_tenant_tx(&transaction_tenant.clone(), move |conn| {
                        Box::pin(async move {
                            let statement = sqlx::query(&sql)
                                .bind(&tenant_id.0)
                                .bind(&region.0)
                                .bind(&loc.repo)
                                .bind(viewer);
                            let statement = match &page_selector {
                                PrListPage::Initial => statement.bind(fetch_limit),
                                PrListPage::LegacyOffset(offset) => statement
                                    .bind(
                                        i64::try_from(*offset)
                                            .map_err(|_| pg_query("page PR list"))?,
                                    )
                                    .bind(fetch_limit),
                                PrListPage::Keyset(cursor) => statement
                                    .bind(cursor.key().updated_at)
                                    .bind(
                                        i64::try_from(cursor.key().number)
                                            .map_err(|_| pg_query("page PR list"))?,
                                    )
                                    .bind(fetch_limit),
                            };
                            statement
                                .fetch_all(&mut *conn)
                                .await
                                .map_err(|_| pg_query("page PR list"))
                        })
                    })
                    .await
            })
            .map_err(pg_error)?;
        let summary = rows
            .first()
            .ok_or_else(|| DurableError::Io("PR list summary is missing".into()))?;
        let count = |column: &'static str| -> Result<usize, DurableError> {
            let value: i64 = summary
                .try_get(column)
                .map_err(|_| DurableError::Io("PR list count is malformed".into()))?;
            usize::try_from(value)
                .map_err(|_| DurableError::Io("PR list count is malformed".into()))
        };
        let counts = PrListCounts {
            open: count("open_count")?,
            merged: count("merged_count")?,
            closed: count("closed_count")?,
            all: count("all_count")?,
            yours: count("yours_count")?,
            needs_review: count("needs_review_count")?,
        };
        let has_newer: bool = summary
            .try_get("has_newer")
            .map_err(|_| DurableError::Io("PR list navigation flags are malformed".into()))?;
        let has_older: bool = summary
            .try_get("has_older")
            .map_err(|_| DurableError::Io("PR list navigation flags are malformed".into()))?;
        let mut records = Vec::with_capacity(query.limit);
        for row in rows {
            let page_number: Option<i64> = row
                .try_get("page_number")
                .map_err(|_| DurableError::Io("PR list page row is malformed".into()))?;
            if page_number.is_some() {
                records.push(decode_record(&kms, &crypto_region, &expected_tenant, row)?);
            }
        }
        Ok(PrListSlice {
            records,
            counts,
            total: counts.filtered_total(query.state),
            offset: query.page.display_offset(),
            has_newer,
            has_older,
        })
    }

    pub fn list_cross_page(
        &self,
        scope: &TenantScope,
        visible_slugs: &[String],
        query: &PrCrossListQuery,
    ) -> Result<PrCrossListSlice, DurableError> {
        query.validate()?;
        let cell = self.scoped_cell(scope)?;
        validate_cross_visible_slugs(visible_slugs)?;
        if visible_slugs.is_empty() {
            return Ok(PrCrossListSlice {
                records: Vec::new(),
                total: 0,
                offset: query.page.display_offset(),
                has_newer: false,
                has_older: false,
            });
        }
        let provider = self.provider.clone();
        let kms = self.kms.clone();
        let tenant_id = cell.tenant_id;
        let region = cell.region;
        let transaction_tenant = tenant_id.0.clone();
        let crypto_region = region.clone();
        let expected_tenant = tenant_id.0.clone();
        let slugs = visible_slugs.to_vec();
        let fetch_limit = i64::try_from(query.limit)
            .map_err(|_| DurableError::Git("pull request page limit is too large".into()))?;
        let sql = pr_cross_list_page_sql(query);
        let viewer = query.viewer_pseudonym.clone();
        let page_selector = query.page.clone();
        let rows = self
            .block_on(async move {
                provider
                    .with_tenant_tx(&transaction_tenant.clone(), move |conn| {
                        Box::pin(async move {
                            let statement = sqlx::query(&sql)
                                .bind(&tenant_id.0)
                                .bind(&region.0)
                                .bind(&slugs)
                                .bind(viewer);
                            let statement =
                                match &page_selector {
                                    PrListPage::Initial => statement.bind(fetch_limit),
                                    PrListPage::LegacyOffset(offset) => statement
                                        .bind(i64::try_from(*offset).map_err(|_| {
                                            pg_query("page cross-repository PR list")
                                        })?)
                                        .bind(fetch_limit),
                                    PrListPage::Keyset(cursor) => statement
                                        .bind(cursor.key().updated_at)
                                        .bind(i64::try_from(cursor.key().number).map_err(|_| {
                                            pg_query("page cross-repository PR list")
                                        })?)
                                        .bind(cursor.key().repo_slug.as_deref().ok_or_else(
                                            || pg_query("page cross-repository PR list"),
                                        )?)
                                        .bind(fetch_limit),
                                };
                            statement
                                .fetch_all(&mut *conn)
                                .await
                                .map_err(|_| pg_query("page cross-repository PR list"))
                        })
                    })
                    .await
            })
            .map_err(pg_error)?;
        let summary = rows.first().ok_or_else(|| {
            DurableError::Io("cross-repository PR list summary is missing".into())
        })?;
        let total: i64 = summary
            .try_get("bucket_count")
            .map_err(|_| DurableError::Io("cross-repository PR list count is malformed".into()))?;
        let total = usize::try_from(total)
            .map_err(|_| DurableError::Io("cross-repository PR list count is malformed".into()))?;
        let has_newer: bool = summary.try_get("has_newer").map_err(|_| {
            DurableError::Io("cross-repository PR navigation flags are malformed".into())
        })?;
        let has_older: bool = summary.try_get("has_older").map_err(|_| {
            DurableError::Io("cross-repository PR navigation flags are malformed".into())
        })?;
        let mut records = Vec::with_capacity(query.limit);
        for row in rows {
            let repo_slug: Option<String> = row.try_get("page_repo_slug").map_err(|_| {
                DurableError::Io("cross-repository PR page row is malformed".into())
            })?;
            if let Some(repo_slug) = repo_slug {
                records.push(PrCrossListRecord {
                    repo_slug,
                    record: decode_record(&kms, &crypto_region, &expected_tenant, row)?,
                });
            }
        }
        Ok(PrCrossListSlice {
            records,
            total,
            offset: query.page.display_offset(),
            has_newer,
            has_older,
        })
    }

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
        let payload_hash = open_payload_hash(&record)?;
        let ctx = self.operation_context(scope, principal)?;
        record.created_at = Some(ctx.unix_seconds);
        record.updated_at = Some(ctx.unix_seconds);
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
        let payload_hash = open_payload_hash(&record)?;
        let ctx = self.operation_context(scope, principal)?;
        record.created_at = Some(ctx.unix_seconds);
        record.updated_at = Some(ctx.unix_seconds);
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
        ctx: PrOperationContext,
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
                        let command = PrCommandIdentity {
                            operation_id: &operation_id,
                            actor_subject_id: &actor_subject_id,
                            command_kind: "open",
                            payload_hash: &payload_hash,
                            expected_number: None,
                        };
                        if let Some(replayed) =
                            replay_command(conn, &loc, &command, &kms, &crypto_region).await?
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
                        co_commit_event(conn, minter, ctx.emit, &loc, &record, GIT_PR_OPENED, None).await?;
                        record_command(conn, &loc, &command, &record).await?;
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

    fn mutate(
        &self,
        scope: &TenantScope,
        repo: &str,
        number: u64,
        command: PrMutationCommand,
        mutation: PrMutation,
    ) -> Result<PrRecord, DurableError> {
        let loc = self.scoped_loc(scope, repo)?;
        let number_db = db_number(number)?;
        let command_kind = mutation.command_kind();
        let event_type = mutation.event_type();
        let PrMutationCommand {
            operation_id,
            actor_subject_id,
            payload_hash,
            context,
        } = command;
        let provider = self.provider.clone();
        let minter = self.minter.clone();
        let kms = self.kms.clone();
        let crypto_region = Region(loc.region.clone());
        self.block_on(async move {
            provider
                .with_tenant_tx(&loc.tenant.clone(), move |conn| {
                    Box::pin(async move {
                        lock_operation(conn, &loc, &operation_id).await?;
                        let command = PrCommandIdentity {
                            operation_id: &operation_id,
                            actor_subject_id: &actor_subject_id,
                            command_kind,
                            payload_hash: &payload_hash,
                            expected_number: Some(number_db),
                        };
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
                        if let Some(replayed) =
                            replay_command(conn, &loc, &command, &kms, &crypto_region).await?
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
                        mutation.apply_to_at(&mut record, context.unix_seconds);
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
                        co_commit_event(
                            conn,
                            minter,
                            context.emit,
                            &loc,
                            &record,
                            event_type,
                            None,
                        )
                        .await?;
                        record_command(conn, &loc, &command, &record).await?;
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
        let payload_hash = payload_hash(&(number, &mutation))?;
        let context = self.operation_context(scope, principal)?;
        self.mutate(
            scope,
            repo,
            number,
            PrMutationCommand {
                operation_id: operation_id.clone(),
                actor_subject_id,
                payload_hash,
                context,
            },
            mutation,
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
                            &PrCommandIdentity::merge(
                                &operation_id,
                                &actor_subject_id,
                                &command_hash,
                                number_db,
                            ),
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
                            &PrCommandIdentity::merge(
                                &operation_id,
                                &actor_subject_id,
                                &command_hash,
                                number_db,
                            ),
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
                                    &PrCommandIdentity::merge(
                                        &operation_id,
                                        &actor_subject_id,
                                        &command_hash,
                                        number_db,
                                    ),
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
        ref_update_provenance: PushProvenance,
        project_checks: bool,
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
                || intent.ref_update_provenance != ref_update_provenance
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
                .ok_or_else(|| {
                    DurableError::Io("pending merge disappeared during recovery".into())
                });
        }
        let ctx = self.operation_context(scope, principal)?;
        let record = self
            .get(scope, repo, number)?
            .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))?;
        let command_intent = pending.clone().unwrap_or_else(|| MergeIntent {
            operation_id: operation_id.as_str().to_owned(),
            actor_subject_id: actor_subject_id.clone(),
            ref_update_provenance,
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
        let default_ref = RefName::new(target_repo.default_branch_ref()?);
        let ruleset =
            policy_store.effective_ruleset_for(target_loc, &record.base_ref, &default_ref)?;

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
            ref_update_provenance,
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
            MergeAdmission {
                intent: intent.clone(),
                command_hash: command_hash.clone(),
                ctx: ctx.clone(),
                ruleset,
                project_checks,
            },
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

        let mut ctx = self.operation_context(scope, recovery_principal)?;
        ctx.emit.caused_by = Some(CausedBy(format!(
            "git-pr-operation:{}",
            intent.operation_id
        )));
        let base = RefName::new(intent.base_ref.clone());
        let actual = ref_store.try_tip(&base)?;
        let attempt = if actual.as_ref().map(|oid| oid.0.as_str()) == Some(intent.head_oid.as_str())
        {
            let update_seq = target_repo.ref_generation(&base.0)?;
            self.finalize_merge(scope, repo, number, &intent, &command_hash, update_seq, ctx)?
        } else {
            let expected_matches = actual.as_ref().map(|oid| oid.0.as_str())
                == Some(intent.expected_old_oid.as_str())
                || (actual.is_none() && intent.expected_old_oid == PushOid::zero().0);
            if !expected_matches {
                self.cancel_merge(scope, repo, number, &intent, &command_hash, actual, ctx)?
            } else {
                let merger_pseudonym =
                    event_actor_pseudonym(scope.tenant().as_str(), &intent.actor_subject_id);
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
        ctx: PrOperationContext,
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
            pusher: Pusher::new(merger_pseudonym, intent.ref_update_provenance),
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

    pub fn list_pending_merges_bounded(
        &self,
        scope: &TenantScope,
        maximum_records: usize,
        maximum_bytes: usize,
    ) -> Result<Vec<PendingMerge>, DurableError> {
        if scope.region().0 != self.provider.config().region {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        let provider = self.provider.clone();
        let tenant = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let fetch_limit = i64::try_from(maximum_records.saturating_add(1)).unwrap_or(i64::MAX);
        let byte_limit = i64::try_from(maximum_bytes).unwrap_or(i64::MAX);
        let rows = self
            .block_on(async move {
                provider
                    .with_tenant_tx(&tenant.clone(), move |conn| {
                        Box::pin(async move {
                            sqlx::query(
                                "WITH pending AS (
                                   SELECT repo_slug,number,merge_intent,
                                          pg_column_size(merge_intent)::bigint AS intent_bytes
                                     FROM git_pr
                                    WHERE tenant_id=$1 AND region=$2
                                      AND merge_intent IS NOT NULL
                                      AND record->>'state' <> 'Merged'
                                    ORDER BY repo_slug,number
                                    LIMIT $3
                                 ), measured AS (
                                   SELECT repo_slug,number,merge_intent,
                                          sum(intent_bytes) OVER (
                                            ORDER BY repo_slug,number
                                          )::bigint AS aggregate_bytes
                                     FROM pending
                                 )
                                 SELECT repo_slug,number,
                                        CASE WHEN aggregate_bytes <= $4
                                             THEN merge_intent ELSE NULL END AS merge_intent,
                                        aggregate_bytes
                                   FROM measured ORDER BY repo_slug,number",
                            )
                            .bind(&tenant)
                            .bind(&region)
                            .bind(fetch_limit)
                            .bind(byte_limit)
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
        if rows.len() > maximum_records {
            return Err(DurableError::Git(
                "pending merge recovery limit exceeded: record count".into(),
            ));
        }
        rows.into_iter()
            .map(|row| {
                let repo_slug: String = row
                    .try_get("repo_slug")
                    .map_err(|_| DurableError::Io("pending PR merge row malformed".into()))?;
                let number: i64 = row
                    .try_get("number")
                    .map_err(|_| DurableError::Io("pending PR merge row malformed".into()))?;
                let aggregate_bytes: i64 = row
                    .try_get("aggregate_bytes")
                    .map_err(|_| DurableError::Io("pending PR merge row malformed".into()))?;
                if aggregate_bytes > byte_limit {
                    return Err(DurableError::Git(
                        "pending merge recovery limit exceeded: serialized bytes".into(),
                    ));
                }
                let value: Option<serde_json::Value> = row
                    .try_get("merge_intent")
                    .map_err(|_| DurableError::Io("pending PR merge row malformed".into()))?;
                let intent = serde_json::from_value(value.ok_or_else(|| {
                    DurableError::Git(
                        "pending merge recovery limit exceeded: serialized bytes".into(),
                    )
                })?)
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
        ctx: PrOperationContext,
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
                            &PrCommandIdentity::merge(
                                &operation_id,
                                &actor_subject_id,
                                &command_hash,
                                number_db,
                            ),
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
                            ctx.emit,
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
        admission: MergeAdmission,
    ) -> Result<Option<MergeAttempt>, DurableError> {
        let MergeAdmission {
            intent,
            command_hash,
            ctx,
            ruleset,
            project_checks,
        } = admission;
        validate_merge_intent(&intent)?;
        let intent_json = serde_json::to_value(&intent)
            .map_err(|e| DurableError::Git(format!("encode merge intent: {e}")))?;
        let operation_id = PrOperationId::from_stored_digest(&intent.operation_id)?;
        let actor_subject_id = intent.actor_subject_id.clone();
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
                            &PrCommandIdentity::merge(
                                &operation_id,
                                &actor_subject_id,
                                &command_hash,
                                number_db,
                            ),
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
                        let mut record = decode_record(&kms, &crypto_region, &loc.tenant, row)
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
                        if project_checks {
                            let repo_ref = format!("myelin://{}/git/repo/{}", loc.tenant, loc.repo);
                            crate::check_status_store::lock_check_admission(
                                conn,
                                &loc.tenant,
                                &loc.region,
                                &repo_ref,
                                &record.head_oid,
                            )
                            .await
                            .map_err(|_| pg_query("lock merge check admission"))?;
                            let rows = crate::check_status_store::rows_for_commit_in_tx(
                                conn,
                                &loc.tenant,
                                &loc.region,
                                &repo_ref,
                                &record.head_oid,
                            )
                            .await
                            .map_err(|_| pg_query("read merge check projection"))?;
                            overlay_projected_checks(&mut record, rows);
                        }
                        let evaluation = evaluate_merge(&ruleset, &record)
                            .map_err(|_| pg_query("merge policy input refused"))?;
                        if !evaluation.admitted() {
                            let result = MergeCommandResult::Blocked { evaluation };
                            insert_merge_command(
                                conn,
                                &loc,
                                &PrCommandIdentity::merge(
                                    &operation_id,
                                    &actor_subject_id,
                                    &command_hash,
                                    number_db,
                                ),
                                "completed",
                                Some(&result),
                            )
                            .await?;
                            return Ok(Some(result.into_attempt()));
                        }
                        insert_merge_command(
                            conn,
                            &loc,
                            &PrCommandIdentity::merge(
                                &operation_id,
                                &actor_subject_id,
                                &command_hash,
                                number_db,
                            ),
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
                            ctx.emit,
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
        ctx: PrOperationContext,
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
                match merge_ledger_state(
                    conn,
                    &loc,
                    &PrCommandIdentity::merge(
                        &operation_id,
                        &actor_subject_id,
                        &command_hash,
                        number_db,
                    ),
                ).await? {
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
                record.updated_at = Some(ctx.unix_seconds);
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
                co_commit_event(conn, minter, ctx.emit, &loc, &record, GIT_PR_MERGED, Some(&expected)).await?;
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

async fn replay_command(
    conn: &mut sqlx::PgConnection,
    loc: &RepoLoc,
    command: &PrCommandIdentity<'_>,
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
    .bind(command.operation_id.as_str())
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
    if stored_actor != command.actor_subject_id
        || stored_kind != command.command_kind
        || stored_hash != command.payload_hash
    {
        return Err(myelin_storage::PgError::Query(
            "PR operation id was reused for a different command".into(),
        ));
    }
    let number: i64 = row
        .try_get("pr_number")
        .map_err(|_| myelin_storage::PgError::Query("PR command row malformed".into()))?;
    if command
        .expected_number
        .is_some_and(|expected| expected != number)
    {
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
    command: &PrCommandIdentity<'_>,
    record: &PrRecord,
) -> Result<(), myelin_storage::PgError> {
    let result = command_projection(record)
        .map_err(|_| myelin_storage::PgError::Query("encode PR command result failed".into()))?;
    let number = db_number(record.number)
        .map_err(|_| myelin_storage::PgError::Query("PR command result number invalid".into()))?;
    if command
        .expected_number
        .is_some_and(|expected| expected != number)
    {
        return Err(myelin_storage::PgError::Query(
            "PR command target changed before persistence".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO git_pr_command
         (tenant_id,region,repo_slug,operation_id,actor_subject_id,command_kind,payload_hash,pr_number,status,result)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'completed',$9)",
    )
    .bind(&loc.tenant)
    .bind(&loc.region)
    .bind(&loc.repo)
    .bind(command.operation_id.as_str())
    .bind(command.actor_subject_id)
    .bind(command.command_kind)
    .bind(command.payload_hash)
    .bind(number)
    .bind(result)
    .execute(&mut *conn)
    .await
    .map_err(|_| myelin_storage::PgError::Query("persist PR command failed".into()))?;
    Ok(())
}

async fn merge_ledger_state(
    conn: &mut sqlx::PgConnection,
    loc: &RepoLoc,
    command: &PrCommandIdentity<'_>,
) -> Result<MergeLedgerState, myelin_storage::PgError> {
    let row = sqlx::query(
        "SELECT repo_slug,actor_subject_id,command_kind,payload_hash,pr_number,status,result
           FROM git_pr_command
          WHERE tenant_id=$1 AND region=$2 AND operation_id=$3",
    )
    .bind(&loc.tenant)
    .bind(&loc.region)
    .bind(command.operation_id.as_str())
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
    if stored_actor != command.actor_subject_id
        || stored_kind != command.command_kind
        || stored_hash != command.payload_hash
        || command.expected_number != Some(stored_number)
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

async fn insert_merge_command(
    conn: &mut sqlx::PgConnection,
    loc: &RepoLoc,
    command: &PrCommandIdentity<'_>,
    status: &str,
    result: Option<&MergeCommandResult>,
) -> Result<(), myelin_storage::PgError> {
    if command.command_kind != "merge" {
        return Err(pg_query("persist non-merge command through merge ledger"));
    }
    let pr_number = command
        .expected_number
        .ok_or_else(|| pg_query("persist merge command without a PR target"))?;
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
    .bind(command.operation_id.as_str())
    .bind(command.actor_subject_id)
    .bind(command.payload_hash)
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
    let encoded = serde_json::to_vec(record)
        .map_err(|_| DurableError::Io("encode PR record failed".into()))?;
    ensure_pr_record_size(encoded.len())?;
    kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()))
        .map_err(|_| DurableError::Io("PR free-text encryption failed".into()))?;
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

/// Hash only the caller's durable open intent.
///
/// Edge stamps a fresh observation time onto every request. Those timestamps belong in the PR
/// record, but they must not make a retry with the same idempotency key look like a different
/// command after the wall clock advances.
fn open_payload_hash(record: &PrRecord) -> Result<String, DurableError> {
    let mut intent = record.clone();
    intent.number = 0;
    intent.created_at = None;
    intent.updated_at = None;
    payload_hash(&intent)
}

fn merge_payload_hash(number: u64, intent: &MergeIntent) -> Result<String, DurableError> {
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
    if message.contains("pull request list limit exceeded: record count") {
        DurableError::Git("pull request list limit exceeded: record count".into())
    } else if message.contains("pull request list limit exceeded: serialized bytes") {
        DurableError::Git("pull request list limit exceeded: serialized bytes".into())
    } else if message.contains("not found") {
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

fn normalize_pr_batch_coordinates(
    coordinates: &[(String, u64)],
) -> Result<Vec<(String, i64)>, DurableError> {
    if coordinates.len() > PR_BATCH_MAX_COORDINATES {
        return Err(DurableError::Git(format!(
            "at most {PR_BATCH_MAX_COORDINATES} pull requests may be read at once"
        )));
    }
    let mut normalized = Vec::with_capacity(coordinates.len());
    for (repo, number) in coordinates {
        crate::coordinate::RepositorySlug::parse(repo)
            .map_err(|_| DurableError::Git("PR batch repository slug is malformed".into()))?;
        if *number == 0 {
            return Err(DurableError::Git(
                "PR batch numbers must be positive".into(),
            ));
        }
        normalized.push((repo.clone(), db_number(*number)?));
    }
    normalized.sort_unstable();
    normalized.dedup();
    Ok(normalized)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "integration")]
    use crate::events::{GIT_PR_HEAD_TRIGGER_SCHEMA_V2, GIT_PR_SYNCHRONIZED};
    #[cfg(feature = "integration")]
    use crate::pr_store::{PrListBucket, PrListSort, PrListState};

    #[test]
    fn accepted_merge_requires_one_exact_nonzero_move_witness() {
        let base = RefName::new("refs/heads/main");
        let head = PushOid::new("0123456789012345678901234567890123456789");

        assert_eq!(
            accepted_merge_update_seq(&[(base.clone(), head.clone(), 7)], &base, &head).unwrap(),
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
    fn pull_request_batches_are_canonical_bounded_and_deduplicated() {
        assert!(normalize_pr_batch_coordinates(&[]).unwrap().is_empty());
        assert_eq!(
            normalize_pr_batch_coordinates(&[
                ("team/api".into(), 2),
                ("core".into(), 1),
                ("team/api".into(), 2),
            ])
            .unwrap(),
            [("core".into(), 1), ("team/api".into(), 2)]
        );
        for malformed in [
            vec![("contains a space".into(), 1)],
            vec![("core".into(), 0)],
            vec![("core".into(), u64::MAX)],
        ] {
            assert!(normalize_pr_batch_coordinates(&malformed).is_err());
        }
        assert!(normalize_pr_batch_coordinates(&vec![
            ("core".into(), 1);
            PR_BATCH_MAX_COORDINATES + 1
        ])
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
        assert!(CREATE_GIT_PR_REPO_UPDATED_LIST_INDEX_DDL.contains("DESC NULLS LAST"));
        assert!(CREATE_GIT_PR_REPO_UPDATED_LIST_INDEX_DDL.contains("tenant_id, region, repo_slug"));
        assert!(CREATE_GIT_PR_REPO_STATE_UPDATED_LIST_INDEX_DDL.contains("(record->>'state')"));
        assert!(CREATE_GIT_PR_REPO_STATE_CREATED_LIST_INDEX_DDL.contains("number DESC"));
        assert!(CREATE_GIT_PR_CROSS_UPDATED_LIST_INDEX_DDL.contains("repo_slug ASC"));
        assert!(CREATE_GIT_PR_CROSS_CREATED_LIST_INDEX_DDL.contains("number DESC, repo_slug ASC"));
        assert!(CREATE_GIT_PR_REVIEWS_GIN_INDEX_DDL.contains("jsonb_path_ops"));
        assert!(CREATE_GIT_PR_AUTHOR_UPDATED_LIST_INDEX_DDL.contains("author_pseudonym"));
        assert!(!CREATE_GIT_PR_DDL.contains("title text"));
        assert!(CREATE_GIT_PR_COUNTER_DDL.contains("PRIMARY KEY (tenant_id, region, repo_slug)"));
    }

    #[test]
    fn migration_set_is_forward_only_and_declares_mutated_tables_hot() {
        let migrations = git_pr_migrations();
        assert_eq!(migrations.0.len(), 13);
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
    fn postgres_record_sealing_enforces_the_shared_size_ceiling() {
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
        record.head_repo_slug = "core".into();
        record.author_subject_id = "principal-123".into();
        record.body_md = Some("x".repeat(crate::pr_store::PR_RECORD_MAX_BYTES));
        assert!(matches!(
            seal_pr_record(&kms, Region("fr-par".into()), &tenant, &record),
            Err(DurableError::Git(message))
                if message == "pull request record limit exceeded: serialized bytes"
        ));
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
            ref_update_provenance: PushProvenance::NonAgent,
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
    fn a_pr_mutation_uses_the_operations_exact_clock_reading() {
        let pull_request = crate::lifecycle::PullRequest::open(
            1,
            "refs/heads/main",
            "refs/heads/feature",
            "author@acme.noreply",
            false,
        );
        let mut record = PrRecord::open(&pull_request, "a".repeat(40));

        PrMutation::Touch.apply_to_at(&mut record, 1_722_470_400);

        assert_eq!(record.updated_at, Some(1_722_470_400));
    }

    #[test]
    fn legacy_pending_merges_resume_with_fail_closed_agent_provenance() {
        let operation = PrOperationId::parse("legacy-merge").unwrap();
        let legacy = serde_json::json!({
            "operation_id": operation.as_str(),
            "actor_subject_id": "principal-123",
            "base_ref": "refs/heads/main",
            "expected_old_oid": "a".repeat(40),
            "head_oid": "b".repeat(40),
            "head_repo_slug": "fork"
        });

        let intent: MergeIntent = serde_json::from_value(legacy).unwrap();
        assert_eq!(intent.ref_update_provenance, PushProvenance::Agent);
    }

    #[test]
    fn open_command_hash_survives_a_retry_in_a_later_second() {
        let pull_request = crate::lifecycle::PullRequest::open(
            0,
            "refs/heads/main",
            "refs/heads/feature",
            "agent@acme.noreply",
            true,
        );
        let mut first = PrRecord::open(&pull_request, "a".repeat(40));
        first.title = "Repair the release".into();
        first.head_repo_slug = "core".into();
        first.created_at = Some(100);
        first.updated_at = Some(100);
        let mut retry = first.clone();
        retry.created_at = Some(101);
        retry.updated_at = Some(101);

        assert_eq!(
            open_payload_hash(&first).unwrap(),
            open_payload_hash(&retry).unwrap(),
            "server observation time is not caller intent"
        );
        retry.title = "A different change".into();
        assert_ne!(
            open_payload_hash(&first).unwrap(),
            open_payload_hash(&retry).unwrap(),
            "a genuinely different open intent still conflicts"
        );
    }

    #[test]
    fn merge_command_hash_is_stable_across_nonempty_base_crash_recovery() {
        let operation = PrOperationId::parse("merge-1").unwrap();
        let mut intent = MergeIntent {
            operation_id: operation.as_str().into(),
            actor_subject_id: "principal-123".into(),
            ref_update_provenance: PushProvenance::NonAgent,
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
        bootstrap
            .migrate(
                &crate::check_status_store::check_status_migrations(),
                &crate::check_status_store::check_status_hot_tables(),
            )
            .await
            .expect("Git check projection migrations");
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

        let suffix = format!(
            "{}-{}",
            std::process::id(),
            system_clock_reading().unwrap().unix_seconds()
        );
        let tenant_a = format!("git-pr-a-{suffix}");
        let tenant_b = format!("git-pr-b-{suffix}");
        let region = cfg.region.clone();
        let actor = principal(&tenant_a, &region, &format!("subject-{suffix}"));
        let actor_b = principal(&tenant_b, &region, &format!("subject-b-{suffix}"));
        let scope_a = TenantScope::from_verified_token(&actor, actor.region.clone());
        let scope_b = TenantScope::from_verified_token(&actor_b, actor_b.region.clone());
        let repo = "core";

        let abort_op = PrOperationId::parse("abort-open").unwrap();
        let mut abort_record = record(&"1".repeat(40), "abort-secret", repo);
        abort_record.body_md = Some("Closes ENG-41".into());
        assert!(store
            .open_then_abort_for_test(&scope_a, repo, abort_record, &abort_op, &actor,)
            .is_err());
        assert!(store
            .list_bounded(
                &scope_a,
                repo,
                10,
                10 * crate::pr_store::PR_RECORD_MAX_BYTES,
            )
            .unwrap()
            .is_empty());
        let abort_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM outbox WHERE envelope->>'tenant'=$1 AND envelope->>'type_'=$2",
        )
        .bind(&tenant_a)
        .bind(GIT_PR_OPENED)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(abort_events, 0, "aborted open emitted no ghost event");
        let abort_edges: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM outbox WHERE envelope->>'tenant'=$1 AND envelope->>'type_'=$2",
        )
        .bind(&tenant_a)
        .bind(crate::typed_edges::REFS_EDGE_CREATED)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(abort_edges, 0, "aborted open emitted no ghost edge");

        let projection_slug = format!("projection-admission-{suffix}");
        let projection_loc = RepoLoc::new(&tenant_a, &region, &projection_slug);
        let projection_root =
            std::env::temp_dir().join(format!("myelin-pg-pr-projection-{suffix}"));
        let projection_policy = DurablePrStore::rooted(&projection_root);
        projection_policy
            .put_protection(
                &projection_loc,
                &crate::pr_store::BranchProtectionConfig {
                    rulesets: vec![BranchProtectionRuleset {
                        ref_pattern: "refs/heads/main".into(),
                        required_contexts: vec!["ci/build".into()],
                        required_approvals: 0,
                        require_codeowner_review: false,
                        require_conversation_resolution: false,
                        allow_force_push: false,
                    }],
                },
            )
            .unwrap();
        let projection_open_op = PrOperationId::parse("open-projection-admission").unwrap();
        let mut legacy_green = record(&"f".repeat(40), "legacy green", &projection_slug);
        legacy_green.green_contexts.push("build".into());
        let projected_pr = store
            .open(
                &scope_a,
                &projection_slug,
                legacy_green,
                &projection_open_op,
                &actor,
            )
            .unwrap();
        let projection_op = PrOperationId::parse("merge-projection-admission").unwrap();
        let projection_intent = MergeIntent {
            operation_id: projection_op.as_str().into(),
            actor_subject_id: actor.principal_id.0.clone(),
            ref_update_provenance: PushProvenance::NonAgent,
            base_ref: projected_pr.base_ref.clone(),
            expected_old_oid: PushOid::zero().0,
            head_oid: projected_pr.head_oid.clone(),
            head_repo_slug: projection_slug.clone(),
        };
        let projection_hash = merge_payload_hash(projected_pr.number, &projection_intent).unwrap();
        let projection_ctx = store.operation_context(&scope_a, &actor).unwrap();
        assert!(matches!(
            store
                .begin_merge(
                    &scope_a,
                    &projection_slug,
                    projected_pr.number,
                    MergeAdmission {
                        intent: projection_intent,
                        command_hash: projection_hash,
                        ctx: projection_ctx,
                        ruleset: projection_policy
                            .effective_ruleset_for(
                                &projection_loc,
                                &projected_pr.base_ref,
                                &RefName::new("refs/heads/main"),
                            )
                            .unwrap(),
                        project_checks: true,
                    },
                )
                .unwrap(),
            Some(MergeAttempt::Blocked(_))
        ));
        assert!(
            store
                .pending_merge_intent(&scope_a, &projection_slug, projected_pr.number)
                .unwrap()
                .is_none(),
            "a blocked projection decision never freezes a merge intent"
        );

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
        let opened_at = crate::clock::clock_reading_from_unix(
            opened
                .created_at
                .expect("an open records its observation time"),
        )
        .unwrap()
        .timestamp()
        .0;
        assert_eq!(
            opened.updated_at, opened.created_at,
            "an open has one record observation time"
        );
        let event_times: (String, String) = sqlx::query_as(
            "SELECT envelope->>'occurred_at',envelope->>'recorded_at'
               FROM outbox
              WHERE aggregate=$1 AND envelope->>'tenant'=$2 AND envelope->>'type_'=$3",
        )
        .bind(format!("git/pr/{repo}:{}", opened.number))
        .bind(&tenant_a)
        .bind(GIT_PR_OPENED)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            event_times,
            (opened_at.clone(), opened_at),
            "one observation stamps the opened record and its event"
        );
        assert!(store
            .open(
                &scope_a,
                repo,
                record(&"2".repeat(40), "different-title", repo),
                &open_op,
                &actor,
            )
            .is_err());

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

        let exact_batch = store
            .get_many(
                &scope_a,
                &[
                    (projection_slug.clone(), projected_pr.number),
                    (repo.into(), opened.number),
                    (repo.into(), opened.number),
                    (repo.into(), 10_000),
                ],
            )
            .unwrap();
        assert_eq!(
            exact_batch.len(),
            2,
            "duplicates and missing PRs are omitted"
        );
        assert!(exact_batch.iter().any(|(batch_repo, record)| {
            batch_repo == repo && record.number == opened.number && record.title == "private-title"
        }));
        assert!(exact_batch.iter().any(|(batch_repo, record)| {
            batch_repo == &projection_slug
                && record.number == projected_pr.number
                && record.title == "legacy green"
        }));
        assert!(store
            .get_many(
                &scope_b,
                &[
                    (repo.into(), opened.number),
                    (projection_slug.clone(), projected_pr.number)
                ],
            )
            .unwrap()
            .is_empty());

        let page_query = PrListQuery::new(
            PrListState::All,
            PrListSort::Created,
            0,
            3,
            "author@tenant.noreply",
        )
        .unwrap();
        let page = store.list_page(&scope_a, repo, &page_query).unwrap();
        assert_eq!(
            page.records
                .iter()
                .map(|record| record.number)
                .collect::<Vec<_>>(),
            [9, 8, 7]
        );
        assert!(page.has_older);
        assert_eq!(page.total, 9);
        assert_eq!(page.counts.all, 9);
        assert_eq!(page.counts.open, 9);
        assert_eq!(page.counts.yours, 9);

        let empty_query = PrListQuery::new(
            PrListState::Merged,
            PrListSort::Created,
            999,
            3,
            "author@tenant.noreply",
        )
        .unwrap();
        let empty = store.list_page(&scope_a, repo, &empty_query).unwrap();
        assert!(empty.records.is_empty());
        assert_eq!(empty.total, 0);
        assert_eq!(empty.counts.all, 9, "empty pages retain exact full badges");

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
            "SELECT count(*) FROM outbox
              WHERE envelope->>'tenant'=$1
                AND (envelope->>'type_' LIKE 'git.pr.%' OR envelope->>'type_'=$2)",
        )
        .bind(&tenant_a)
        .bind(GIT_REVIEW_SUBMITTED)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            lifecycle_events, 12,
            "10 opens (including projection admission) + 2 mutations, exactly once"
        );

        let assigned_reviewer = "assigned-reviewer@tenant.noreply";
        store
            .apply_mutation(
                &scope_a,
                repo,
                opened.number,
                PrMutation::SubmitReview(crate::pr_store::ReviewRecord {
                    reviewer_pseudonym: assigned_reviewer.into(),
                    state: crate::lifecycle::ReviewState::Requested,
                    is_agent: false,
                }),
                &PrOperationId::parse("request-assigned-review").unwrap(),
                &actor,
            )
            .unwrap();
        let reviewer_page_query = PrListQuery::new(
            PrListState::All,
            PrListSort::Created,
            0,
            10,
            assigned_reviewer,
        )
        .unwrap();
        assert_eq!(
            store
                .list_page(&scope_a, repo, &reviewer_page_query)
                .unwrap()
                .counts
                .needs_review,
            1
        );
        let reviewer_cross_query = PrCrossListQuery::initial(
            PrListBucket::NeedsReview,
            PrListSort::Created,
            10,
            assigned_reviewer,
        )
        .unwrap();
        assert_eq!(
            store
                .list_cross_page(&scope_a, &[repo.into()], &reviewer_cross_query)
                .unwrap()
                .records
                .len(),
            1
        );

        store
            .apply_mutation(
                &scope_a,
                repo,
                opened.number,
                PrMutation::SubmitReview(crate::pr_store::ReviewRecord {
                    reviewer_pseudonym: assigned_reviewer.into(),
                    state: crate::lifecycle::ReviewState::Submitted(
                        crate::lifecycle::ReviewVerdict::Approve,
                    ),
                    is_agent: false,
                }),
                &PrOperationId::parse("complete-assigned-review").unwrap(),
                &actor,
            )
            .unwrap();
        assert_eq!(
            store
                .list_page(&scope_a, repo, &reviewer_page_query)
                .unwrap()
                .counts
                .needs_review,
            0,
            "the latest decision closes the earlier requested-review work item"
        );
        assert!(store
            .list_cross_page(&scope_a, &[repo.into()], &reviewer_cross_query)
            .unwrap()
            .records
            .is_empty());

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
                ref_update_provenance: PushProvenance::NonAgent,
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
            let privacy_ctx = store.operation_context(&scope_a, &event_agent).unwrap();
            let serialized_actor = serde_json::to_string(&privacy_ctx.emit.actor).unwrap();
            for raw in [
                "agent:raw-event-subject",
                "runtime://raw-pg-worker/session",
                "human:raw-pg-delegator",
            ] {
                assert!(
                    !serialized_actor.contains(raw),
                    "PG event actor leaked {raw}"
                );
            }
            let ctx = store.operation_context(&scope_a, &actor).unwrap();
            assert!(store
                .begin_merge(
                    &scope_a,
                    slug,
                    opened.number,
                    MergeAdmission {
                        intent: intent.clone(),
                        command_hash: hash.clone(),
                        ctx: ctx.clone(),
                        ruleset: policy_store
                            .effective_ruleset_for(
                                &loc,
                                &opened.base_ref,
                                &RefName::new("refs/heads/main"),
                            )
                            .unwrap(),
                        project_checks: false,
                    },
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
            assert!(
                store
                    .list_pending_merges_bounded(&scope_a, 0, 1024 * 1024)
                    .is_err(),
                "pending recovery record cap plus one is rejected"
            );
            assert!(
                store.list_pending_merges_bounded(&scope_a, 100, 0).is_err(),
                "pending recovery byte cap plus one is rejected"
            );
            assert!(store
                .list_pending_merges_bounded(&scope_a, 100, 1024 * 1024)
                .unwrap()
                .iter()
                .any(|pending| pending.repo_slug == slug && pending.number == opened.number));

            let ref_store = open_ref_store(target.clone(), slug, ctx.emit);
            if slug == "merge-before" {
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
                        PushProvenance::NonAgent,
                        false,
                    )
                    .unwrap();
                assert_eq!(first, retry, "cancelled command replays deterministically");
            }
            assert!(!store
                .list_pending_merges_bounded(&scope_a, 100, 1024 * 1024)
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
            let wire_rows: Vec<(String, i64, Option<i64>)> = sqlx::query_as(
                "SELECT envelope->>'type_',(envelope->>'schema_ver')::bigint, \
                        (envelope->'payload'->>'head_generation')::bigint \
                   FROM outbox WHERE aggregate=$1 AND envelope->>'tenant'=$2 ORDER BY seq",
            )
            .bind(&aggregate)
            .bind(&tenant_a)
            .fetch_all(&admin)
            .await
            .unwrap();
            let generations = wire_rows
                .iter()
                .filter_map(|(kind, schema_ver, generation)| {
                    if matches!(kind.as_str(), GIT_PR_OPENED | GIT_PR_SYNCHRONIZED) {
                        assert_eq!(
                            *schema_ver,
                            i64::from(GIT_PR_HEAD_TRIGGER_SCHEMA_V2),
                            "head-trigger events advertise the required generation shape"
                        );
                        Some(generation.expect("v2 head-trigger event carries its generation"))
                    } else {
                        assert_eq!(*schema_ver, 1, "unrelated PR event lineage stays v1");
                        assert_eq!(
                            *generation, None,
                            "unrelated PR events do not claim head-ordering authority"
                        );
                        None
                    }
                })
                .collect::<Vec<_>>();
            assert!(
                generations.iter().all(|generation| *generation > 0)
                    && generations.windows(2).all(|pair| pair[0] < pair[1]),
                "head-trigger events carry positive serialized git_pr.version in commit order: {generations:?}"
            );
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

        assert!(kms
            .destroy_dek(&DekId::new(
                TenantId(tenant_a.clone()),
                KeyClass::Subject(actor.principal_id.0.clone()),
            ))
            .expect("destroy the pull-request actor's durable DEK"));
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
        std::fs::remove_dir_all(projection_root).ok();
    }
}
