//! Tenant/region-scoped durable Issue store (R4.4 data-plane increment).
//!
//! Every operation derives its scope from a verified [`Principal`], checks object authorization
//! before touching PostgreSQL, and executes through [`SubstrateProvider::with_tenant_tx`] so FORCE
//! RLS is a second, independent tenant boundary. Free-text titles are sealed under the creator's
//! per-subject DEK before the insert; only nonce/ciphertext/`pii_key_ref` rest in PostgreSQL.
//!
//! This module intentionally does not provide an allow-all production authorizer or an in-memory
//! store. The edge composition must inject the live Identity/ReBAC implementation.
//!
//! ## Atomic authorization visibility across the Issues -> Identity boundary
//!
//! Identity owns `rebac_tuple`; Issues never writes it directly and does not assume two service
//! databases can share a transaction. Creation therefore uses a durable, fail-closed saga:
//!
//! 1. one Issues transaction inserts an **invisible** issue row, its exact `parent_project` tuple
//!    intent in `issue_authz_binding`, and `issue.issue.authorization_requested` in the outbox;
//! 2. a retryable worker calls [`IssueTupleWriter`] (the production adapter is Identity's scoped
//!    `TupleStore::write_tuples`) to idempotently install the tuple;
//! 3. only after that succeeds does one Issues transaction mark the binding `active` and co-emit
//!    the externally visible `issue.issue.created` event.
//!
//! Every read joins an `active` binding. Thus a crash can leave internal pending state, but it can
//! never expose an issue without its authorization tuple; retry converges without duplicate visible
//! creation. This is the honest outbox/Saga equivalent of atomicity at a cross-service boundary.

use crate::api::{
    decode_issue_page_cursor, encode_issue_page_cursor, normalize_issue_key_prefix, IssueListState,
};
use crate::dek::{decrypt_free_text, encrypt_free_text, IssueFreeText};
use crate::events::{ISSUE_AUTHORIZATION_REQUESTED, ISSUE_CLOSED, ISSUE_CREATED};
use myelin_events::{
    derive_envelope, Actor, AggregateKey, DataRole, EmitContext, EventDraft, EventEnvelope,
    EventId, EventType, IdMinter, Timestamp, UlidMinter, Visibility,
};
use myelin_identity::{ColRef, Principal, PrincipalKind, RelName, SetExpr, Zookie};
use myelin_storage::encryption::{EncryptedColumn, SubjectId};
use myelin_storage::kms::{KmsEngine, PiiKeyRef, NONCE_LEN};
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::{SubstrateProvider, TenantScope};
use myelin_tenancy::ArtifactRef;
use sqlx::types::Uuid;
use sqlx::Row;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Maximum issue title bytes accepted at the durable boundary.
pub const MAX_TITLE_BYTES: usize = 512;
/// Maximum page size; callers cannot request an unbounded tenant scan.
pub const MAX_PAGE_SIZE: u32 = 100;
/// Maximum materialised Identity allow-set accepted before any database transaction is opened.
/// Larger sets must use Identity's push-down/filter representation rather than allocating a huge
/// `ANY(uuid[])` parameter at the Issues boundary.
pub const MAX_AUTHORIZED_ISSUE_IDS: usize = 10_000;

/// Object permission checked before an issue read or transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssuePermission {
    /// Read/decrypt one issue.
    View,
    /// Durably transition one issue to the completed category.
    Close,
}

/// Leak-free authorization result for a list query. `Ids` is pushed into SQL before rows/counts are
/// read; it is never applied as a post-filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VisibleIssues {
    /// No issue is reachable; the store returns an empty page without querying the issue table.
    None,
    /// Every issue in the principal's verified tenant/region is reachable (explicit tenant-wide grant).
    All,
    /// A bounded, pre-authorized issue-id set to conjoin with the tenant-scoped SQL query.
    Ids(Vec<String>),
    /// Frozen effective-permission pushdown. PgIssueStore accepts only the exact `issue.id IN
    /// issue:view` shape and binds it to its durable source/applied projection watermark.
    Filter { set_expr: SetExpr },
}

impl VisibleIssues {
    /// The only production list shape. Subject, tenant, and region remain derived from the verified
    /// principal; callers cannot smuggle them into this expression.
    pub fn effective_issue_view_filter() -> Self {
        Self::Filter {
            set_expr: SetExpr::InRelation {
                relation: RelName("view".into()),
                via_column: ColRef {
                    table: "issue".into(),
                    column: "id".into(),
                },
            },
        }
    }
}

/// Result of atomically rebuilding one tenant/region effective `issue:view` projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueViewProjectionRevision {
    pub revision: i64,
    pub effective_grants: u64,
}

/// Required production authorization seam. The live implementation is Identity/ReBAC; there is no
/// permissive default. Every method receives the verified principal, never a body/path tenant.
pub trait IssueAuthorizer: Send + Sync {
    /// Authorize creation under the addressed project object before encryption/counter/row mutation.
    fn may_create(&self, principal: &Principal, project_id: &str) -> bool;
    /// Strong project-view check before disclosing a creator-scoped authorization receipt.
    fn may_view_project(&self, principal: &Principal, project_id: &str) -> bool {
        self.may_create(principal, project_id)
    }
    /// Authorize one issue object before its row is read or mutated.
    fn may_access(
        &self,
        principal: &Principal,
        issue_id: &str,
        permission: IssuePermission,
    ) -> bool;
    /// Produce the leak-free prefilter for list. Errors fail closed and are surfaced loudly.
    fn visible_issues(&self, principal: &Principal) -> Result<VisibleIssues, String>;
}

/// A create proposal. Scope and creator are deliberately absent: both come from the verified
/// principal passed separately to [`PgIssueStore::create`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateIssue {
    /// Opaque project UUID; checked as `project:<uuid>` by the production authorizer.
    pub project_id: String,
    /// Opaque issue-type UUID.
    pub type_id: String,
    /// Human-key prefix (`ENG`, `OPS`, …), 2–10 uppercase ASCII bytes.
    pub prefix: String,
    /// Free-text title, encrypted before the database transaction.
    pub title: String,
}

/// Durable receipt for an issue creation that is staged but deliberately not visible yet.
/// Callers may return this as an asynchronous-creation receipt; they must not render the title or
/// claim the issue is readable until [`PgIssueStore::reconcile_authorization`] succeeds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueCreationReceipt {
    pub id: String,
    pub key: String,
    pub project_id: String,
    pub authorization_request_event_id: String,
}

/// Creator-scoped status of the durable authorization saga. Failure internals are deliberately
/// absent; retryable failures remain represented as `Pending`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueAuthorizationStatus {
    Pending(IssueCreationReceipt),
    Active(StoredIssue),
}

/// The exact Identity relationship intent committed with the pending issue row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueAuthorizationBinding {
    pub issue_id: String,
    pub project_id: String,
    pub issue_object: String,
    pub project_userset: String,
    pub relation: String,
    pub request_event_id: String,
    pub created_event_id: String,
    pub state: IssueAuthorizationState,
    pub zookie: Option<Zookie>,
    pub attempts: u32,
}

/// Durable bootstrap state. Only `Active` rows participate in any Issue read query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueAuthorizationState {
    Pending,
    Active,
}

/// Scoped Identity/ReBAC tuple-write port used by the retry worker.
///
/// Implementations MUST make `ensure_parent_project` idempotent for the binding identity. The live
/// adapter should call Identity's scoped `TupleStore::write_tuples` with an `Add` of
/// `issue:<uuid>#parent_project@(project:<uuid>#view)`. A repeated `Add` must return success and a
/// zookie; it must not widen the relation or write another tenant's partition.
pub trait IssueTupleWriter: Send + Sync {
    fn ensure_parent_project<'a>(
        &'a self,
        scope: &'a TenantScope,
        actor: &'a Principal,
        binding: &'a IssueAuthorizationBinding,
    ) -> Pin<Box<dyn Future<Output = Result<Zookie, String>> + Send + 'a>>;
}

/// Result of one reconciliation attempt. `newly_activated=false` means another worker already won
/// the activation race; the returned issue is the same active row and no second `issue.created`
/// event was emitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueAuthorizationOutcome {
    pub issue: StoredIssue,
    pub zookie: Zookie,
    pub newly_activated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BootstrapFailure {
    EmptyZookie,
    TupleWriteFailed,
}

impl BootstrapFailure {
    fn code(self) -> &'static str {
        match self {
            BootstrapFailure::EmptyZookie => "identity_empty_zookie",
            BootstrapFailure::TupleWriteFailed => "identity_tuple_write_failed",
        }
    }
}

/// Bounded, filter-bound keyset page request. Product UX defaults to open work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuePageRequest {
    pub state: IssueListState,
    /// Normalized uppercase issue-key prefix only; titles remain encrypted and are never searched.
    pub key: Option<String>,
    /// Requested rows, `1..=100`.
    pub limit: u32,
    /// Exclusive opaque compound cursor (`updated_at DESC, id DESC`) bound to state/key.
    pub cursor: Option<String>,
}

impl IssuePageRequest {
    /// Validate the default open-work page request.
    pub fn new(limit: u32, cursor: Option<String>) -> Result<Self, IssueStoreError> {
        Self::filtered(IssueListState::Open, None, limit, cursor)
    }

    /// Validate a bounded authoritative list request, normalizing the key filter and strictly
    /// decoding a filter-bound cursor before any database operation.
    pub fn filtered(
        state: IssueListState,
        key: Option<String>,
        limit: u32,
        cursor: Option<String>,
    ) -> Result<Self, IssueStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(IssueStoreError::BadInput(format!(
                "page limit must be between 1 and {MAX_PAGE_SIZE}"
            )));
        }
        let key = match key {
            Some(value) => Some(normalize_issue_key_prefix(&value).ok_or_else(|| {
                IssueStoreError::BadInput("key must be a bounded ASCII issue-key prefix".into())
            })?),
            None => None,
        };
        if let Some(value) = cursor.as_deref() {
            decode_issue_page_cursor(value, state, key.as_deref())
                .map_err(|reason| IssueStoreError::BadInput(reason.into()))?;
        }
        Ok(Self {
            state,
            key,
            limit,
            cursor,
        })
    }
}

/// Decrypted issue view returned only after authorization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredIssue {
    pub id: String,
    pub key: String,
    pub project_id: String,
    pub state: String,
    pub state_category: String,
    pub title: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// A bounded issue page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuePage {
    pub items: Vec<StoredIssue>,
    pub next_cursor: Option<String>,
    pub limit: u32,
}

/// Loud typed store failure. No variant contains plaintext title or key material.
#[derive(Debug, PartialEq, Eq)]
pub enum IssueStoreError {
    BadInput(String),
    /// Uniform object denial/absence; callers must map this to the same 404 envelope.
    NotFound,
    AuthorizationUnavailable(String),
    Storage(String),
    Crypto(String),
}

impl core::fmt::Display for IssueStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            IssueStoreError::BadInput(reason) => write!(f, "invalid issue request: {reason}"),
            IssueStoreError::NotFound => f.write_str("issue not found"),
            IssueStoreError::AuthorizationUnavailable(reason) => {
                write!(f, "issue authorization unavailable: {reason}")
            }
            IssueStoreError::Storage(reason) => write!(f, "durable issue store fault: {reason}"),
            IssueStoreError::Crypto(reason) => write!(f, "issue encryption fault: {reason}"),
        }
    }
}

impl std::error::Error for IssueStoreError {}

/// Real PostgreSQL issue store. It is constructible only with a concrete authorization seam and a
/// KMS engine; no in-memory or permissive production constructor exists.
#[derive(Clone)]
pub struct PgIssueStore<A: IssueAuthorizer> {
    provider: SubstrateProvider,
    kms: Arc<KmsEngine>,
    authorizer: A,
    minter: Arc<dyn IdMinter>,
}

impl<A: IssueAuthorizer> PgIssueStore<A> {
    pub fn new(provider: SubstrateProvider, kms: Arc<KmsEngine>, authorizer: A) -> Self {
        Self::with_minter(provider, kms, authorizer, Arc::new(UlidMinter::new()))
    }

    /// Construct with an explicit event-id source. Production roots should normally use [`Self::new`];
    /// this seam exists so tests can pin the two IDs staged by one creation attempt.
    pub fn with_minter(
        provider: SubstrateProvider,
        kms: Arc<KmsEngine>,
        authorizer: A,
        minter: Arc<dyn IdMinter>,
    ) -> Self {
        Self {
            provider,
            kms,
            authorizer,
            minter,
        }
    }

    fn scope(&self, principal: &Principal) -> Result<TenantScope, IssueStoreError> {
        let configured_region = &self.provider.config().region;
        if principal.region.as_str() != configured_region {
            return Err(IssueStoreError::NotFound);
        }
        Ok(TenantScope::from_verified_token(
            principal,
            principal.region.clone(),
        ))
    }

    /// Authorize project creation and atomically stage the invisible row + exact Identity tuple
    /// intent + authorization-request outbox event. No product read can observe the row until the
    /// tuple has been durably written and [`Self::reconcile_authorization`] activates it.
    pub async fn create(
        &self,
        principal: &Principal,
        proposal: CreateIssue,
    ) -> Result<IssueCreationReceipt, IssueStoreError> {
        self.create_inner(principal, proposal, false).await
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Failure-injection seam for the live rollback proof: run the exact production transaction,
    /// then return an error after staging its outbox row so PostgreSQL must roll back the issue,
    /// binding, prefix allocation, and event together.
    pub async fn create_then_abort_for_test(
        &self,
        principal: &Principal,
        proposal: CreateIssue,
    ) -> Result<IssueCreationReceipt, IssueStoreError> {
        self.create_inner(principal, proposal, true).await
    }

    async fn create_inner(
        &self,
        principal: &Principal,
        proposal: CreateIssue,
        abort_after_outbox: bool,
    ) -> Result<IssueCreationReceipt, IssueStoreError> {
        let scope = self.scope(principal)?;
        validate_create(&proposal)?;
        if !self.authorizer.may_create(principal, &proposal.project_id) {
            return Err(IssueStoreError::NotFound);
        }

        let tenant = principal.tenant.clone();
        // The title is controller-authored free text, so its erasure subject is the verified creator.
        // For an agent acting on behalf of a human, Identity's explicit `on_behalf_of` subject owns
        // the title DEK; otherwise the stable opaque creator principal does. This gives a reachable
        // individual crypto-shred lever without fabricating a reporter identity. A title that names
        // some unrelated third party remains the platform-wide third-party-content residual; no
        // single row key can infer every person mentioned inside opaque free text.
        let subject = SubjectId::new(title_dek_subject(principal));
        let sealed = encrypt_free_text(
            &self.kms,
            &principal.region,
            &tenant,
            &subject,
            IssueFreeText::Title,
            proposal.title.as_bytes(),
        )
        .map_err(|e| IssueStoreError::Crypto(e.to_string()))?;

        let tenant_id = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let project_id = parse_uuid("project_id", &proposal.project_id)?;
        let type_id = parse_uuid("type_id", &proposal.type_id)?;
        let issue_id = Uuid::new_v4();
        let prefix = proposal.prefix;
        let created_by = principal.principal_id.0.clone();
        let nonce = sealed.nonce.to_vec();
        let ciphertext = sealed.ciphertext;
        let key_ref = sealed.key_ref.to_uri();
        let issue_object = issue_object(issue_id);
        let project_userset = project_userset(project_id);
        // Mint both canonical event IDs before staging. Their persisted values are the saga's
        // idempotency keys; retries never derive or replace either envelope identity.
        let request_event_id: EventId = self.minter.mint().into();
        let created_event_id: EventId = self.minter.mint().into();
        let request_envelope = authorization_request_envelope(
            principal,
            issue_id,
            project_id,
            &issue_object,
            &project_userset,
            request_event_id.clone(),
        );
        let aggregate = request_envelope.aggregate.0.clone();
        let request_event_id_text = request_event_id.0.clone();
        let created_event_id_text = created_event_id.0.clone();

        let row = self
            .provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "WITH allocated AS (\
                           INSERT INTO prefix_counter (tenant_id, region, prefix, high_water, block_size) \
                           VALUES ($1, $2, $3, 1, 1) \
                           ON CONFLICT (tenant_id, prefix) DO UPDATE \
                             SET high_water = prefix_counter.high_water + 1 \
                           RETURNING high_water\
                         ) \
                         INSERT INTO issue (\
                           tenant_id, region, id, key, prefix, type_id, type_rank, state, \
                           state_category, reporter, project_id, rank, title, title_nonce, \
                           title_ciphertext, created_by_principal, pii_key_ref, \
                           contains_personal_data, version\
                         ) \
                         SELECT $1, $2, $10, $3 || '-' || high_water::text, $3, $4, \
                           0, 'Todo', 'unstarted', NULL, $5, \
                           '0|' || lpad(high_water::text, 20, '0'), '<encrypted>', $6, $7, $8, $9, \
                           true, 1 \
                         FROM allocated \
                         RETURNING id, key, project_id, state, state_category, title_nonce, \
                           title_ciphertext, pii_key_ref, version, created_at::text, updated_at::text",
                    )
                    .bind(&tenant_id)
                    .bind(&region)
                    .bind(&prefix)
                    .bind(type_id)
                    .bind(project_id)
                    .bind(nonce)
                    .bind(ciphertext)
                    .bind(created_by)
                    .bind(key_ref)
                    .bind(issue_id)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;

                    sqlx::query(
                        "INSERT INTO issue_authz_binding (\
                           tenant_id, region, issue_id, project_id, issue_object, project_userset, \
                           relation, request_event_id, created_event_id, state\
                         ) VALUES ($1, $2, $3, $4, $5, $6, 'parent_project', $7, $8, 'pending')",
                    )
                    .bind(&tenant_id)
                    .bind(&region)
                    .bind(issue_id)
                    .bind(project_id)
                    .bind(&issue_object)
                    .bind(&project_userset)
                    .bind(&request_event_id_text)
                    .bind(&created_event_id_text)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;

                    PgRelay::co_commit_in_tx(&mut *conn, &aggregate, &request_envelope).await?;
                    if abort_after_outbox {
                        return Err(myelin_storage::PgError::Query(
                            "injected crash after authorization-request outbox stage".into(),
                        ));
                    }
                    Ok(row)
                })
            })
            .await
            .map_err(|e| IssueStoreError::Storage(e.to_string()))?;
        Ok(IssueCreationReceipt {
            id: row.get::<Uuid, _>("id").to_string(),
            key: row.get("key"),
            project_id: row.get::<Uuid, _>("project_id").to_string(),
            authorization_request_event_id: request_event_id.0,
        })
    }

    /// Reconcile one pending issue with Identity's scoped tuple writer.
    ///
    /// The Identity call intentionally happens outside the Issues transaction: holding a database
    /// lock across an RPC would be unsafe and still would not make two databases atomic. Instead,
    /// the exact `Add` is idempotent and activation is a compare-and-set under `FOR UPDATE`. If this
    /// process dies after Identity commits but before activation, the row remains invisible; a retry
    /// repeats the `Add`, then activates once. Concurrent workers may both call Identity, but only
    /// one can emit `issue.issue.created`.
    pub async fn reconcile_authorization<W: IssueTupleWriter>(
        &self,
        worker: &Principal,
        issue_id: &str,
        writer: &W,
    ) -> Result<IssueAuthorizationOutcome, IssueStoreError> {
        self.reconcile_inner(worker, issue_id, writer, false).await
    }

    /// Return a bounded, partition-scoped restart-recovery batch. A worker repeatedly scans this
    /// list and calls [`Self::reconcile_authorization`]; legacy rows without a binding are excluded
    /// and remain fail-closed until an explicit rollout backfill stages a reviewed tuple intent.
    pub async fn pending_authorization_ids(
        &self,
        worker: &Principal,
        limit: u32,
    ) -> Result<Vec<String>, IssueStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(IssueStoreError::BadInput(format!(
                "pending authorization limit must be between 1 and {MAX_PAGE_SIZE}"
            )));
        }
        let scope = self.scope(worker)?;
        let tenant_id = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let rows = self
            .provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "SELECT issue_id FROM issue_authz_binding \
                         WHERE tenant_id = $1 AND region = $2 AND state = 'pending' \
                         ORDER BY created_at, issue_id LIMIT $3",
                    )
                    .bind(&tenant_id)
                    .bind(&region)
                    .bind(i64::from(limit))
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))
                })
            })
            .await
            .map_err(|e| IssueStoreError::Storage(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|row| row.get::<Uuid, _>("issue_id").to_string())
            .collect())
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Failure-injection seam for the restart proof: Identity commits the idempotent tuple, then the
    /// simulated process dies before Issues activates the binding.
    pub async fn reconcile_then_crash_for_test<W: IssueTupleWriter>(
        &self,
        worker: &Principal,
        issue_id: &str,
        writer: &W,
    ) -> Result<IssueAuthorizationOutcome, IssueStoreError> {
        self.reconcile_inner(worker, issue_id, writer, true).await
    }

    async fn reconcile_inner<W: IssueTupleWriter>(
        &self,
        worker: &Principal,
        issue_id: &str,
        writer: &W,
        stop_after_tuple: bool,
    ) -> Result<IssueAuthorizationOutcome, IssueStoreError> {
        let scope = self.scope(worker)?;
        let id = parse_uuid("issue_id", issue_id)?;
        // Preflight the complete durable intent before crossing the database boundary. The tuple
        // writer receives only canonical fields derived from the issue row, never mutable copies
        // from issue_authz_binding.
        let binding = self
            .load_validated_authorization_binding(&scope, id)
            .await?;

        if binding.state == IssueAuthorizationState::Active {
            let zookie = binding.zookie.clone().ok_or_else(|| {
                IssueStoreError::Storage("active authorization binding has no zookie".into())
            })?;
            let issue = self.load_active_issue(&scope, &worker.region, id).await?;
            return Ok(IssueAuthorizationOutcome {
                issue,
                zookie,
                newly_activated: false,
            });
        }

        let zookie = match writer.ensure_parent_project(&scope, worker, &binding).await {
            Ok(zookie) if !zookie.0.trim().is_empty() => zookie,
            Ok(_) => {
                let reason = "Identity returned an empty zookie".to_string();
                self.record_bootstrap_failure(&scope, id, BootstrapFailure::EmptyZookie)
                    .await?;
                return Err(IssueStoreError::AuthorizationUnavailable(reason));
            }
            Err(_reason) => {
                self.record_bootstrap_failure(&scope, id, BootstrapFailure::TupleWriteFailed)
                    .await?;
                return Err(IssueStoreError::AuthorizationUnavailable(
                    "Identity tuple write failed".into(),
                ));
            }
        };

        if stop_after_tuple {
            return Err(IssueStoreError::Storage(
                "injected crash after Identity tuple commit and before Issues activation".into(),
            ));
        }

        let tenant_id = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let worker_region = worker.region.clone();
        let zookie_for_tx = zookie.clone();
        let (row, committed_zookie, newly_activated) = self
            .provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    let locked_sql = format!(
                        "SELECT b.issue_id, b.state AS authz_state, b.zookie AS authz_zookie, \
                                b.project_id AS binding_project_id, b.issue_object, \
                                b.project_userset, b.relation, b.request_event_id, \
                                b.created_event_id, b.attempts, i.created_by_principal, \
                                o.envelope AS request_envelope, {columns} \
                         FROM issue_authz_binding b \
                         JOIN issue i ON i.tenant_id = b.tenant_id AND i.region = b.region \
                                           AND i.id = b.issue_id \
                         JOIN outbox o ON o.event_id = b.request_event_id \
                         WHERE b.tenant_id = $1 AND b.region = $2 AND b.issue_id = $3 \
                           AND i.tenant_id = $1 AND i.region = $2 \
                         FOR UPDATE OF b, i, o",
                        columns = SELECT_COLUMNS_QUALIFIED
                    );
                    let tenant_id_locked_query = sqlx::query(&locked_sql);
                    let locked = tenant_id_locked_query
                        .bind(&tenant_id)
                        .bind(&region)
                        .bind(id)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?
                        .ok_or_else(|| {
                            myelin_storage::PgError::Query(
                                "authorization binding disappeared before activation".into(),
                            )
                        })?;

                    let (binding, request) =
                        validated_binding_from_row(&locked, &tenant_id, &region, id)
                            .map_err(myelin_storage::PgError::Query)?;
                    if binding.state == IssueAuthorizationState::Active {
                        let committed = binding.zookie.clone().ok_or_else(|| {
                            myelin_storage::PgError::Query(
                                "active authorization binding has no zookie".into(),
                            )
                        })?;
                        return Ok((locked, committed, false));
                    }
                    let request = request.ok_or_else(|| {
                        myelin_storage::PgError::Query(
                            "pending authorization binding lost its request provenance".into(),
                        )
                    })?;
                    let created = issue_created_envelope(
                        EventId(binding.created_event_id.clone()),
                        id,
                        &locked.get::<String, _>("key"),
                        Uuid::parse_str(&binding.project_id).map_err(|_| {
                            myelin_storage::PgError::Query(
                                "validated binding carried a malformed project id".into(),
                            )
                        })?,
                        &zookie_for_tx,
                        &request,
                    );

                    let changed = sqlx::query(
                        "UPDATE issue_authz_binding \
                         SET state = 'active', zookie = $4, attempts = attempts + 1, \
                             last_error = NULL, activated_at = now() \
                         WHERE tenant_id = $1 AND region = $2 AND issue_id = $3 \
                           AND state = 'pending'",
                    )
                    .bind(&tenant_id)
                    .bind(&region)
                    .bind(id)
                    .bind(&zookie_for_tx.0)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                    if changed.rows_affected() != 1 {
                        return Err(myelin_storage::PgError::Query(
                            "authorization activation compare-and-set lost unexpectedly".into(),
                        ));
                    }
                    let aggregate = created.aggregate.0.clone();
                    PgRelay::co_commit_in_tx(&mut *conn, &aggregate, &created).await?;
                    Ok((locked, zookie_for_tx, true))
                })
            })
            .await
            .map_err(|e| IssueStoreError::Storage(e.to_string()))?;

        Ok(IssueAuthorizationOutcome {
            issue: decode_row(&self.kms, &worker_region, row)?,
            zookie: committed_zookie,
            newly_activated,
        })
    }

    async fn load_validated_authorization_binding(
        &self,
        scope: &TenantScope,
        issue_id: Uuid,
    ) -> Result<IssueAuthorizationBinding, IssueStoreError> {
        let tenant_id = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let row = self
            .provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "SELECT b.issue_id, b.project_id AS binding_project_id, b.issue_object, \
                                b.project_userset, b.relation, b.request_event_id, \
                                b.created_event_id, b.state AS authz_state, \
                                b.zookie AS authz_zookie, b.attempts, i.project_id, \
                                i.created_by_principal, o.envelope AS request_envelope \
                         FROM issue_authz_binding b \
                         JOIN issue i ON i.tenant_id = b.tenant_id AND i.region = b.region \
                                          AND i.id = b.issue_id \
                         LEFT JOIN outbox o ON o.event_id = b.request_event_id \
                         WHERE b.tenant_id = $1 AND b.region = $2 AND b.issue_id = $3 \
                           AND i.tenant_id = $1 AND i.region = $2",
                    )
                    .bind(&tenant_id)
                    .bind(&region)
                    .bind(issue_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))
                })
            })
            .await
            .map_err(|e| IssueStoreError::Storage(e.to_string()))?
            .ok_or(IssueStoreError::NotFound)?;
        validated_binding_from_row(
            &row,
            scope.tenant().as_str(),
            scope.region().as_str(),
            issue_id,
        )
        .map(|(binding, _)| binding)
        .map_err(IssueStoreError::Storage)
    }

    async fn record_bootstrap_failure(
        &self,
        scope: &TenantScope,
        issue_id: Uuid,
        reason: BootstrapFailure,
    ) -> Result<(), IssueStoreError> {
        let tenant_id = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let reason = reason.code().to_string();
        self.provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "UPDATE issue_authz_binding \
                         SET attempts = attempts + 1, last_error = $4 \
                         WHERE tenant_id = $1 AND region = $2 AND issue_id = $3 \
                           AND state = 'pending'",
                    )
                    .bind(&tenant_id)
                    .bind(&region)
                    .bind(issue_id)
                    .bind(reason)
                    .execute(&mut *conn)
                    .await
                    .map(|_| ())
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))
                })
            })
            .await
            .map_err(|e| IssueStoreError::Storage(e.to_string()))
    }

    async fn load_active_issue(
        &self,
        scope: &TenantScope,
        region: &myelin_tenancy::Region,
        issue_id: Uuid,
    ) -> Result<StoredIssue, IssueStoreError> {
        let tenant_id = scope.tenant().0.clone();
        let region_id = scope.region().0.clone();
        let row = self
            .provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    select_one(conn, &tenant_id, &region_id, issue_id)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))
                })
            })
            .await
            .map_err(|e| IssueStoreError::Storage(e.to_string()))?
            .ok_or(IssueStoreError::NotFound)?;
        decode_row(&self.kms, region, row)
    }

    /// Read and decrypt one issue only after the object-level `view` decision allows it.
    pub async fn view(
        &self,
        principal: &Principal,
        issue_id: &str,
    ) -> Result<StoredIssue, IssueStoreError> {
        let scope = self.scope(principal)?;
        let id = parse_uuid("issue_id", issue_id)?;
        if !self
            .authorizer
            .may_access(principal, issue_id, IssuePermission::View)
        {
            return Err(IssueStoreError::NotFound);
        }
        let tenant_id = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let row = self
            .provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    select_one(conn, &tenant_id, &region, id)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))
                })
            })
            .await
            .map_err(|e| IssueStoreError::Storage(e.to_string()))?
            .ok_or(IssueStoreError::NotFound)?;
        decode_row(&self.kms, &principal.region, row)
    }

    /// Resolve a creator-scoped asynchronous authorization request. Tenant, region, and exact
    /// creator are applied in SQL before a row is returned; strong project view gates both states,
    /// and an active response additionally passes the ordinary strong issue-view authorization.
    pub async fn authorization_status(
        &self,
        principal: &Principal,
        request_event_id: &str,
    ) -> Result<IssueAuthorizationStatus, IssueStoreError> {
        let scope = self.scope(principal)?;
        if !is_canonical_request_event_id(request_event_id) {
            return Err(IssueStoreError::BadInput(
                "authorization request id must be a canonical ULID".into(),
            ));
        }
        let tenant_id = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let creator = principal.principal_id.0.clone();
        let request_id = request_event_id.to_string();
        let row = self
            .provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    sqlx::query(AUTHORIZATION_STATUS_SQL)
                        .bind(&tenant_id)
                        .bind(&region)
                        .bind(&request_id)
                        .bind(&creator)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
                })
            })
            .await
            .map_err(|error| IssueStoreError::Storage(error.to_string()))?
            .ok_or(IssueStoreError::NotFound)?;
        let issue_id = row.get::<Uuid, _>("id").to_string();
        let project_id = row.get::<Uuid, _>("project_id").to_string();
        if !self.authorizer.may_view_project(principal, &project_id) {
            return Err(IssueStoreError::NotFound);
        }
        match row.get::<String, _>("authorization_state").as_str() {
            "pending" => Ok(IssueAuthorizationStatus::Pending(IssueCreationReceipt {
                id: issue_id,
                key: row.get("key"),
                project_id,
                authorization_request_event_id: request_event_id.to_string(),
            })),
            "active" => {
                if !self
                    .authorizer
                    .may_access(principal, &issue_id, IssuePermission::View)
                {
                    return Err(IssueStoreError::NotFound);
                }
                decode_row(&self.kms, &principal.region, row).map(IssueAuthorizationStatus::Active)
            }
            _ => Err(IssueStoreError::Storage(
                "authorization binding carried an unsupported state".into(),
            )),
        }
    }

    /// Leak-free list over the durable effective-permission projection.
    ///
    /// The one SQL statement returns a projection-status sentinel plus only rows joined to the
    /// exact ready revision. Missing, pending, rebuilding, or revision-mismatched state therefore
    /// returns `AuthorizationUnavailable`; it is never confused with an authorized empty page.
    pub async fn list(
        &self,
        principal: &Principal,
        page: IssuePageRequest,
    ) -> Result<IssuePage, IssueStoreError> {
        let scope = self.scope(principal)?;
        let set_expr = normalize_visible(
            self.authorizer
                .visible_issues(principal)
                .map_err(IssueStoreError::AuthorizationUnavailable)?,
        )?;
        validate_issue_view_filter(&set_expr)?;
        let cursor = page
            .cursor
            .as_deref()
            .map(|value| {
                decode_issue_page_cursor(value, page.state, page.key.as_deref())
                    .map_err(|reason| IssueStoreError::BadInput(reason.into()))
            })
            .transpose()?;
        let cursor_micros = cursor.as_ref().map(|value| value.updated_at_micros);
        let cursor_id = cursor
            .as_ref()
            .map(|value| parse_uuid("cursor issue id", &value.issue_id))
            .transpose()?;
        let fetch_limit = i64::from(page.limit) + 1;
        let tenant_id = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let principal_id = principal.principal_id.0.clone();
        let state = page.state.as_str().to_string();
        let key = page.key.clone();
        let rows = self
            .provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    sqlx::query(LIST_EFFECTIVE_ISSUE_VIEW_SQL)
                        .bind(&tenant_id)
                        .bind(&region)
                        .bind(&principal_id)
                        .bind(cursor_micros)
                        .bind(cursor_id)
                        .bind(&state)
                        .bind(&key)
                        .bind(fetch_limit)
                        .fetch_all(&mut *conn)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))
                })
            })
            .await
            .map_err(|e| IssueStoreError::Storage(e.to_string()))?;

        let mut rows = rows.into_iter();
        let sentinel = rows.next().ok_or_else(|| {
            IssueStoreError::Storage("issue visibility query omitted its status sentinel".into())
        })?;
        let status: String = sentinel.get("projection_status");
        if status != "ready" {
            return Err(IssueStoreError::AuthorizationUnavailable(format!(
                "effective issue:view projection is {status}"
            )));
        }
        let rows: Vec<_> = rows.collect();
        let has_more = rows.len() > page.limit as usize;
        let mut items = Vec::with_capacity(rows.len().min(page.limit as usize));
        let mut next_position = None;
        for row in rows.into_iter().take(page.limit as usize) {
            next_position = Some((
                row.get::<i64, _>("updated_at_micros"),
                row.get::<Uuid, _>("id").to_string(),
            ));
            items.push(decode_row(&self.kms, &principal.region, row)?);
        }
        let next_cursor = if has_more {
            next_position
                .map(|(updated_at_micros, issue_id)| {
                    encode_issue_page_cursor(
                        page.state,
                        page.key.as_deref(),
                        updated_at_micros,
                        &issue_id,
                    )
                    .map_err(|reason| IssueStoreError::Storage(reason.into()))
                })
                .transpose()?
        } else {
            None
        };
        Ok(IssuePage {
            items,
            next_cursor,
            limit: page.limit,
        })
    }

    /// Rebuild the effective `issue:view` rows for one verified tenant/region.
    ///
    /// The projection state row is locked before the source is read and remains locked through
    /// publication. Tuple and issue triggers take the same row lock when advancing the source
    /// revision, so a concurrent mutation either precedes this snapshot or commits afterward and
    /// leaves it pending. A crash rolls the delete/insert/status transition back atomically.
    pub async fn rebuild_effective_issue_view(
        &self,
        worker: &Principal,
    ) -> Result<IssueViewProjectionRevision, IssueStoreError> {
        self.rebuild_effective_issue_view_inner(worker, None).await
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Deterministic race seam: pause only after the projection state row is locked.
    pub async fn rebuild_effective_issue_view_paused_for_test(
        &self,
        worker: &Principal,
        pause: std::time::Duration,
        locked: Arc<tokio::sync::Notify>,
    ) -> Result<IssueViewProjectionRevision, IssueStoreError> {
        self.rebuild_effective_issue_view_inner(worker, Some((pause, locked)))
            .await
    }

    async fn rebuild_effective_issue_view_inner(
        &self,
        worker: &Principal,
        pause_after_lock: Option<(std::time::Duration, Arc<tokio::sync::Notify>)>,
    ) -> Result<IssueViewProjectionRevision, IssueStoreError> {
        let scope = self.scope(worker)?;
        let tenant_id = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        self.provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO authz_projection_state \
                           (tenant_id, region, projection, source_revision, applied_revision, status) \
                         VALUES ($1, $2, 'issue:view', 1, 0, 'pending') \
                         ON CONFLICT (tenant_id, region, projection) DO NOTHING",
                    )
                    .bind(&tenant_id)
                    .bind(&region)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;

                    let revision: i64 = sqlx::query_scalar(
                        "SELECT source_revision FROM authz_projection_state \
                         WHERE tenant_id = $1 AND region = $2 AND projection = 'issue:view' \
                         FOR UPDATE",
                    )
                    .bind(&tenant_id)
                    .bind(&region)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;

                    if let Some((pause, locked)) = pause_after_lock {
                        locked.notify_one();
                        tokio::time::sleep(pause).await;
                    }

                    sqlx::query(
                        "UPDATE authz_projection_state SET status = 'rebuilding', rebuilt_at = NULL \
                         WHERE tenant_id = $1 AND region = $2 AND projection = 'issue:view'",
                    )
                    .bind(&tenant_id)
                    .bind(&region)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;

                    let unsupported_sql = format!(
                        "{ISSUE_VIEW_WALK_CTE} \
                         SELECT object_id || '#' || relation FROM walk \
                         WHERE NOT supported OR depth >= 16 ORDER BY depth, object_id LIMIT 1"
                    );
                    let unsupported: Option<String> = sqlx::query_scalar(&unsupported_sql)
                        .bind(&tenant_id)
                        .bind(&region)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                    if let Some(node) = unsupported {
                        return Err(myelin_storage::PgError::Query(format!(
                            "unsupported or over-depth userset in issue:view rebuild: {node}"
                        )));
                    }

                    sqlx::query(
                        "DELETE FROM issue_authz_visible \
                         WHERE tenant_id = $1 AND region = $2 AND projection = 'issue:view'",
                    )
                    .bind(&tenant_id)
                    .bind(&region)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;

                    let insert_sql = format!(
                        "{ISSUE_VIEW_WALK_CTE} {ISSUE_VIEW_MEMBERS_CTE} \
                         INSERT INTO issue_authz_visible \
                           (tenant_id, region, projection, subject, permission, object_type, \
                            object_id, revision) \
                         SELECT $1, $2, 'issue:view', subject, 'view', 'issue', issue_id, $3 \
                         FROM effective"
                    );
                    let inserted = sqlx::query(&insert_sql)
                        .bind(&tenant_id)
                        .bind(&region)
                        .bind(revision)
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?
                        .rows_affected();

                    let published = sqlx::query(
                        "UPDATE authz_projection_state \
                         SET applied_revision = $3, status = 'ready', rebuilt_at = now() \
                         WHERE tenant_id = $1 AND region = $2 AND projection = 'issue:view' \
                           AND source_revision = $3",
                    )
                    .bind(&tenant_id)
                    .bind(&region)
                    .bind(revision)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                    if published.rows_affected() != 1 {
                        return Err(myelin_storage::PgError::Query(
                            "issue:view source revision changed during locked rebuild".into(),
                        ));
                    }
                    Ok(IssueViewProjectionRevision {
                        revision,
                        effective_grants: inserted,
                    })
                })
            })
            .await
            .map_err(|error| match error.to_string().contains("unsupported or over-depth userset") {
                true => IssueStoreError::AuthorizationUnavailable(error.to_string()),
                false => IssueStoreError::Storage(error.to_string()),
            })
    }

    /// Durable readiness/lag probe for the bounded reconciler. `None` is absent state and therefore
    /// unavailable; `Some(0)` is serveable only when status is also `ready`.
    pub async fn effective_issue_view_lag(
        &self,
        worker: &Principal,
    ) -> Result<Option<i64>, IssueStoreError> {
        let scope = self.scope(worker)?;
        let tenant_id = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        self.provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT source_revision, applied_revision, status \
                         FROM authz_projection_state \
                         WHERE tenant_id = $1 AND region = $2 AND projection = 'issue:view'",
                    )
                    .bind(&tenant_id)
                    .bind(&region)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                    Ok(row.map(|row| {
                        let source: i64 = row.get("source_revision");
                        let applied: i64 = row.get("applied_revision");
                        let status: String = row.get("status");
                        if status == "ready" && source == applied {
                            0
                        } else {
                            source.saturating_sub(applied).max(1)
                        }
                    }))
                })
            })
            .await
            .map_err(|error| IssueStoreError::Storage(error.to_string()))
    }

    /// Object-authorize, lock the row, and durably transition it to `completed`. Repeated closes are
    /// idempotent: an already-completed row is returned without another version bump.
    pub async fn close(
        &self,
        principal: &Principal,
        issue_id: &str,
    ) -> Result<StoredIssue, IssueStoreError> {
        let scope = self.scope(principal)?;
        let id = parse_uuid("issue_id", issue_id)?;
        if !self
            .authorizer
            .may_access(principal, issue_id, IssuePermission::Close)
        {
            return Err(IssueStoreError::NotFound);
        }
        let tenant_id = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let actor = principal.clone();
        // Mint before opening the transaction, just like create. An idempotent no-op close may
        // discard this ID; a state-changing close persists it exactly once with the row update.
        let closed_event_id: EventId = self.minter.mint().into();
        let row = self
            .provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    let current = select_one_for_update(conn, &tenant_id, &region, id)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                    let Some(current) = current else {
                        return Ok(None);
                    };
                    if current.get::<String, _>("state_category") == "completed" {
                        return Ok(Some(current));
                    }
                    let previous_state = current.get::<String, _>("state");
                    let row = sqlx::query(&format!(
                        "UPDATE issue SET state = 'Done', state_category = 'completed', \
                         state_changed_at = now(), updated_at = now(), version = version + 1 \
                         WHERE tenant_id = $1 AND region = $2 AND id = $3 \
                           AND EXISTS (SELECT 1 FROM issue_authz_binding b \
                                       WHERE b.tenant_id = $1 AND b.region = $2 AND b.issue_id = $3 \
                                         AND b.state = 'active') \
                         RETURNING {}",
                        SELECT_COLUMNS
                    ))
                    .bind(&tenant_id)
                    .bind(&region)
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                    let Some(row) = row else {
                        return Ok(None);
                    };
                    let envelope = issue_closed_envelope(
                        &actor,
                        id,
                        row.get::<String, _>("key").as_str(),
                        previous_state.as_str(),
                        closed_event_id,
                    );
                    let aggregate = envelope.aggregate.0.clone();
                    PgRelay::co_commit_in_tx(&mut *conn, &aggregate, &envelope).await?;
                    Ok(Some(row))
                })
            })
            .await
            .map_err(|e| IssueStoreError::Storage(e.to_string()))?
            .ok_or(IssueStoreError::NotFound)?;
        decode_row(&self.kms, &principal.region, row)
    }
}

const SELECT_COLUMNS: &str = "id, key, project_id, state, state_category, title_nonce, \
title_ciphertext, pii_key_ref, version, created_at::text, updated_at::text";
const SELECT_COLUMNS_QUALIFIED: &str = "i.id, i.key, i.project_id, i.state, i.state_category, \
i.title_nonce, i.title_ciphertext, i.pii_key_ref, i.version, i.created_at::text, i.updated_at::text";
const AUTHORIZATION_STATUS_SQL: &str = r#"
SELECT i.id, i.key, i.project_id, i.state, i.state_category,
       i.title_nonce, i.title_ciphertext, i.pii_key_ref, i.version,
       i.created_at::text AS created_at, i.updated_at::text AS updated_at,
       b.state AS authorization_state
FROM issue_authz_binding b
JOIN issue i
  ON i.tenant_id = b.tenant_id AND i.region = b.region AND i.id = b.issue_id
 AND b.project_id = i.project_id
 AND b.issue_object = 'issue:' || i.id::text
 AND b.project_userset = 'project:' || i.project_id::text || '#view'
 AND b.relation = 'parent_project'
WHERE b.tenant_id = $1 AND b.region = $2 AND b.request_event_id = $3
  AND i.created_by_principal = $4 AND i.deleted_at IS NULL AND NOT i.archived
"#;
const LIST_EFFECTIVE_ISSUE_VIEW_SQL: &str = r#"
WITH projection AS MATERIALIZED (
  SELECT source_revision, applied_revision, status
  FROM authz_projection_state
  WHERE tenant_id = $1 AND region = $2 AND projection = 'issue:view'
),
gate AS MATERIALIZED (
  SELECT source_revision AS revision
  FROM projection
  WHERE status = 'ready' AND applied_revision = source_revision
),
authorized AS (
  SELECT i.id, i.key, i.project_id, i.state, i.state_category,
         i.title_nonce, i.title_ciphertext, i.pii_key_ref, i.version,
         i.created_at::text AS created_at, i.updated_at::text AS updated_at,
         floor(extract(epoch from i.updated_at) * 1000000)::bigint AS updated_at_micros
  FROM gate g
  JOIN issue_authz_visible av
    ON av.tenant_id = $1 AND av.region = $2
   AND av.projection = 'issue:view' AND av.subject = $3
   AND av.permission = 'view' AND av.object_type = 'issue'
   AND av.revision = g.revision
  JOIN issue i
    ON i.tenant_id = av.tenant_id AND i.region = av.region
   AND i.id::text = av.object_id
  JOIN issue_authz_binding b
    ON b.tenant_id = i.tenant_id AND b.region = i.region
   AND b.issue_id = i.id AND b.state = 'active'
   AND b.project_id = i.project_id
   AND b.issue_object = 'issue:' || i.id::text
   AND b.project_userset = 'project:' || i.project_id::text || '#view'
   AND b.relation = 'parent_project'
  WHERE i.tenant_id = $1 AND i.region = $2
    AND i.deleted_at IS NULL AND NOT i.archived
    AND ($4::bigint IS NULL OR
         (i.updated_at, i.id) < (to_timestamp($4::double precision / 1000000.0), $5))
    AND ($6 = 'all'
         OR ($6 = 'open' AND i.state_category IN ('unstarted', 'started'))
         OR ($6 = 'closed' AND i.state_category IN ('completed', 'cancelled')))
    AND ($7::text IS NULL OR i.key LIKE $7 || '%')
  ORDER BY i.updated_at DESC, i.id DESC
  LIMIT $8
)
SELECT 0::int AS sort_key,
       CASE
         WHEN EXISTS (SELECT 1 FROM gate) THEN 'ready'
         WHEN NOT EXISTS (SELECT 1 FROM projection) THEN 'missing'
         WHEN (SELECT status FROM projection) = 'ready' THEN 'stale'
         ELSE (SELECT status FROM projection)
       END::text AS projection_status,
       NULL::uuid AS id, NULL::text AS key, NULL::uuid AS project_id,
       NULL::text AS state, NULL::text AS state_category,
       NULL::bytea AS title_nonce, NULL::bytea AS title_ciphertext,
       NULL::text AS pii_key_ref, NULL::bigint AS version,
       NULL::text AS created_at, NULL::text AS updated_at,
       NULL::bigint AS updated_at_micros
UNION ALL
SELECT 1, 'ready', id, key, project_id, state, state_category,
       title_nonce, title_ciphertext, pii_key_ref, version, created_at, updated_at,
       updated_at_micros
FROM authorized
ORDER BY sort_key ASC, updated_at_micros DESC NULLS FIRST, id DESC NULLS FIRST
"#;

/// Exact served list statement, exposed for production-plan verification against live PostgreSQL.
/// Callers must not execute it outside the scoped store path; the Edge integration test only
/// prefixes it with `EXPLAIN` and binds the same typed parameters.
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub fn authoritative_issue_list_sql() -> &'static str {
    LIST_EFFECTIVE_ISSUE_VIEW_SQL
}

/// Resolve the fixed core org→team→project hierarchy plus direct-relation usersets. A userset that
/// leaves those supported group types is surfaced as `supported=false`; rebuild then fails closed
/// instead of silently under-materializing an unknown permission rewrite.
const ISSUE_VIEW_WALK_CTE: &str = r#"
WITH RECURSIVE roots(issue_id, arm, object_id, relation, depth, path, supported) AS (
  SELECT i.id::text, root.arm,
         CASE WHEN root.arm = 'base'
              THEN split_part(b.project_userset, '#', 1)
              ELSE b.issue_object END,
         root.relation, 0,
         ARRAY[(CASE WHEN root.arm = 'base'
                     THEN split_part(b.project_userset, '#', 1)
                     ELSE b.issue_object END) || '#' || root.relation]::text[],
         true
  FROM issue i
  JOIN issue_authz_binding b
    ON b.tenant_id = i.tenant_id AND b.region = i.region
   AND b.issue_id = i.id AND b.state = 'active'
  CROSS JOIN (VALUES
      ('base'::text, 'view'::text),
      ('confidential'::text, 'confidential'::text),
      ('grant'::text, 'confidential_grant'::text)
  ) AS root(arm, relation)
  WHERE i.tenant_id = $1 AND i.region = $2 AND i.deleted_at IS NULL
),
walk(issue_id, arm, object_id, relation, depth, path, supported) AS (
  SELECT issue_id, arm, object_id, relation, depth, path, supported FROM roots
  UNION ALL
  SELECT w.issue_id, w.arm, child.object_id, child.relation, w.depth + 1,
         w.path || (child.object_id || '#' || child.relation), child.supported
  FROM walk w
  CROSS JOIN LATERAL (
    SELECT split_part(t.subject, '#', 1) AS object_id,
           split_part(t.subject, '#', 2) AS relation,
           split_part(t.subject, ':', 1) IN ('org', 'team', 'project')
             AND split_part(t.subject, '#', 2) <> '' AS supported
    FROM rebac_tuple t
    WHERE t.tenant_id = $1 AND t.region = $2
      AND t.object_id = w.object_id AND t.relation = w.relation
      AND position('#' IN t.subject) > 0
      AND NOT (w.relation = 'view'
               AND split_part(w.object_id, ':', 1) IN ('org', 'team', 'project'))

    UNION ALL

    SELECT w.object_id, direct.relation, true
    FROM (VALUES
      ('project'::text, 'reader'::text),
      ('project'::text, 'writer'::text),
      ('team'::text, 'member'::text),
      ('org'::text, 'member'::text),
      ('org'::text, 'admin'::text)
    ) AS direct(object_type, relation)
    WHERE w.relation = 'view'
      AND split_part(w.object_id, ':', 1) = direct.object_type

    UNION ALL

    SELECT split_part(t.subject, '#', 1), split_part(t.subject, '#', 2),
           split_part(t.subject, ':', 1) IN ('org', 'team', 'project')
             AND split_part(t.subject, '#', 2) <> ''
    FROM (VALUES
      ('project'::text, 'parent_team'::text),
      ('team'::text, 'parent_org'::text)
    ) AS inherited(object_type, tupleset)
    JOIN rebac_tuple t
      ON t.tenant_id = $1 AND t.region = $2
     AND t.object_id = w.object_id AND t.relation = inherited.tupleset
    WHERE w.relation = 'view'
      AND split_part(w.object_id, ':', 1) = inherited.object_type
      AND position('#' IN t.subject) > 0
      AND split_part(t.subject, '#', 2) = 'view'
  ) AS child
  WHERE w.supported AND w.depth < 16
    AND NOT ((child.object_id || '#' || child.relation) = ANY(w.path))
)
"#;

const ISSUE_VIEW_MEMBERS_CTE: &str = r#"
, members(issue_id, arm, subject) AS (
  SELECT w.issue_id, w.arm, t.subject
  FROM walk w
  JOIN rebac_tuple t
    ON t.tenant_id = $1 AND t.region = $2
   AND t.object_id = w.object_id AND t.relation = w.relation
  WHERE w.supported AND position('#' IN t.subject) = 0
    AND NOT (w.relation = 'view'
             AND split_part(w.object_id, ':', 1) IN ('org', 'team', 'project'))
),
effective(issue_id, subject) AS (
  (SELECT issue_id, subject FROM members WHERE arm = 'base'
   EXCEPT
   SELECT issue_id, subject FROM members WHERE arm = 'confidential')
  UNION
  SELECT issue_id, subject FROM members WHERE arm = 'grant'
)
"#;

async fn select_one(
    conn: &mut sqlx::PgConnection,
    tenant_id: &str,
    region: &str,
    id: Uuid,
) -> Result<Option<sqlx::postgres::PgRow>, sqlx::Error> {
    let query_sql = format!(
        "SELECT {SELECT_COLUMNS_QUALIFIED} FROM issue i \
         JOIN issue_authz_binding b \
           ON b.tenant_id = i.tenant_id AND b.region = i.region AND b.issue_id = i.id \
              AND b.state = 'active' \
         WHERE i.tenant_id = $1 AND i.region = $2 AND b.tenant_id = $1 AND b.region = $2 \
           AND i.id = $3 AND i.deleted_at IS NULL"
    );
    let tenant_id_query = sqlx::query(&query_sql);
    tenant_id_query
        .bind(tenant_id)
        .bind(region)
        .bind(id)
        .fetch_optional(conn)
        .await
}

async fn select_one_for_update(
    conn: &mut sqlx::PgConnection,
    tenant_id: &str,
    region: &str,
    id: Uuid,
) -> Result<Option<sqlx::postgres::PgRow>, sqlx::Error> {
    let query_sql = format!(
        "SELECT {SELECT_COLUMNS_QUALIFIED} FROM issue i \
         JOIN issue_authz_binding b \
           ON b.tenant_id = i.tenant_id AND b.region = i.region AND b.issue_id = i.id \
              AND b.state = 'active' \
         WHERE i.tenant_id = $1 AND i.region = $2 AND b.tenant_id = $1 AND b.region = $2 \
           AND i.id = $3 AND i.deleted_at IS NULL FOR UPDATE OF i"
    );
    let tenant_id_query = sqlx::query(&query_sql);
    tenant_id_query
        .bind(tenant_id)
        .bind(region)
        .bind(id)
        .fetch_optional(conn)
        .await
}

fn validated_binding_from_row(
    row: &sqlx::postgres::PgRow,
    tenant: &str,
    region: &str,
    issue_id: Uuid,
) -> Result<(IssueAuthorizationBinding, Option<EventEnvelope>), String> {
    if row.get::<Uuid, _>("issue_id") != issue_id {
        return Err("authorization binding issue id does not match the requested issue".into());
    }
    let project_id = row.get::<Uuid, _>("project_id");
    if row.get::<Uuid, _>("binding_project_id") != project_id {
        return Err("authorization binding project does not match its issue".into());
    }
    let canonical_issue_object = issue_object(issue_id);
    let canonical_project_userset = project_userset(project_id);
    if row.get::<String, _>("issue_object") != canonical_issue_object
        || row.get::<String, _>("project_userset") != canonical_project_userset
        || row.get::<String, _>("relation") != "parent_project"
    {
        return Err("authorization binding is not canonical for its issue".into());
    }
    let request_event_id: String = row.get("request_event_id");
    let created_event_id: String = row.get("created_event_id");
    if !is_canonical_request_event_id(&request_event_id)
        || !is_canonical_request_event_id(&created_event_id)
    {
        return Err("authorization binding carries a malformed durable event id".into());
    }
    let created_by: String = row
        .try_get("created_by_principal")
        .map_err(|_| "issue creator is absent from authorization binding preflight".to_string())?;
    if created_by.trim().is_empty() {
        return Err("issue creator is empty in authorization binding preflight".into());
    }

    let state = match row.get::<String, _>("authz_state").as_str() {
        "pending" => IssueAuthorizationState::Pending,
        "active" => IssueAuthorizationState::Active,
        other => return Err(format!("unknown authorization binding state `{other}`")),
    };
    let attempts: i32 = row.get("attempts");
    let attempts = u32::try_from(attempts)
        .map_err(|_| "authorization binding has a negative attempt count".to_string())?;
    let zookie = row.get::<Option<String>, _>("authz_zookie").map(Zookie);
    if state == IssueAuthorizationState::Active
        && zookie
            .as_ref()
            .is_none_or(|value| value.0.trim().is_empty())
    {
        return Err("active authorization binding has no nonempty zookie".into());
    }
    let request = if state == IssueAuthorizationState::Pending {
        let request_value: serde_json::Value = row.try_get("request_envelope").map_err(|_| {
            "pending authorization binding has no request outbox envelope".to_string()
        })?;
        let request: EventEnvelope = serde_json::from_value(request_value)
            .map_err(|error| format!("decode authorization request envelope: {error}"))?;
        validate_authorization_request(
            &request,
            tenant,
            region,
            issue_id,
            project_id,
            &canonical_issue_object,
            &canonical_project_userset,
            "parent_project",
            &request_event_id,
            &created_by,
        )?;
        Some(request)
    } else {
        None
    };
    Ok((
        IssueAuthorizationBinding {
            issue_id: issue_id.to_string(),
            project_id: project_id.to_string(),
            issue_object: canonical_issue_object,
            project_userset: canonical_project_userset,
            relation: "parent_project".into(),
            request_event_id,
            created_event_id,
            state,
            zookie,
            attempts,
        },
        request,
    ))
}

fn issue_object(issue_id: Uuid) -> String {
    format!("issue:{issue_id}")
}

fn project_userset(project_id: Uuid) -> String {
    format!("project:{project_id}#view")
}

fn issue_subject(tenant: &str, issue_id: Uuid) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant}/issue/issue/{issue_id}"))
}

fn authorization_request_envelope(
    actor: &Principal,
    issue_id: Uuid,
    project_id: Uuid,
    issue_object: &str,
    project_userset: &str,
    event_id: EventId,
) -> EventEnvelope {
    let timestamp = now_rfc3339();
    derive_envelope(
        EventDraft {
            type_: EventType(ISSUE_AUTHORIZATION_REQUESTED.into()),
            subject: issue_subject(actor.tenant.as_str(), issue_id),
            aggregate: AggregateKey(format!("issue:{issue_id}")),
            payload: serde_json::json!({
                "issue_id": issue_id.to_string(),
                "project_id": project_id.to_string(),
                "issue_object": issue_object,
                "relation": "parent_project",
                "project_userset": project_userset,
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        EmitContext {
            event_id,
            tenant: actor.tenant.clone(),
            region: actor.region.clone(),
            actor: Actor(actor.clone()),
            schema_ver: 1,
            occurred_at: timestamp.clone(),
            recorded_at: timestamp,
            caused_by: None,
        },
        None,
    )
}

fn issue_created_envelope(
    event_id: EventId,
    issue_id: Uuid,
    key: &str,
    project_id: Uuid,
    zookie: &Zookie,
    request: &EventEnvelope,
) -> EventEnvelope {
    derive_envelope(
        EventDraft {
            type_: EventType(ISSUE_CREATED.into()),
            subject: issue_subject(request.tenant.as_str(), issue_id),
            aggregate: AggregateKey(format!("issue:{issue_id}")),
            payload: serde_json::json!({
                "issue_id": issue_id.to_string(),
                "issue_key": key,
                "project_id": project_id.to_string(),
                "authorization_zookie": zookie.0,
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            // IDs and refs only: the encrypted title is not copied into this envelope.
            contains_personal_data: false,
            pii_key_ref: None,
        },
        EmitContext {
            event_id,
            tenant: request.tenant.clone(),
            region: request.region.clone(),
            actor: request.actor.clone(),
            schema_ver: 1,
            occurred_at: request.occurred_at.clone(),
            recorded_at: now_rfc3339(),
            caused_by: request.caused_by.clone(),
        },
        Some(request),
    )
}

/// References-only close event committed in the exact transaction that changes the issue row.
/// The encrypted title never enters the outbox. A repeated close never calls this helper because
/// the locked row's completed category returns early, preserving one event for one transition.
fn issue_closed_envelope(
    actor: &Principal,
    issue_id: Uuid,
    key: &str,
    previous_state: &str,
    event_id: EventId,
) -> EventEnvelope {
    let timestamp = now_rfc3339();
    derive_envelope(
        EventDraft {
            type_: EventType(ISSUE_CLOSED.into()),
            subject: issue_subject(actor.tenant.as_str(), issue_id),
            aggregate: AggregateKey(format!("issue:{issue_id}")),
            payload: serde_json::json!({
                "issue_id": issue_id.to_string(),
                "issue_key": key,
                "from": previous_state,
                "to": "Done",
                "category": "completed",
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        EmitContext {
            event_id,
            tenant: actor.tenant.clone(),
            region: actor.region.clone(),
            actor: Actor(actor.clone()),
            schema_ver: 1,
            occurred_at: timestamp.clone(),
            recorded_at: timestamp,
            caused_by: None,
        },
        None,
    )
}

/// Fail closed if the persisted request envelope no longer describes the exact tuple intent and
/// partition staged with the issue. Activation never trusts worker-supplied provenance.
#[allow(clippy::too_many_arguments)]
fn validate_authorization_request(
    request: &EventEnvelope,
    tenant: &str,
    region: &str,
    issue_id: Uuid,
    project_id: Uuid,
    issue_object: &str,
    project_userset: &str,
    relation: &str,
    request_event_id: &str,
    created_by_principal: &str,
) -> Result<(), String> {
    let expected_subject = issue_subject(tenant, issue_id);
    let expected_aggregate = AggregateKey(format!("issue:{issue_id}"));
    let expected_issue_id = issue_id.to_string();
    let expected_project_id = project_id.to_string();
    let payload = &request.payload;
    let valid = request.type_.0 == ISSUE_AUTHORIZATION_REQUESTED
        && request.event_id.0 == request_event_id
        && request.tenant.as_str() == tenant
        && request.region.as_str() == region
        && request.actor.0.tenant.as_str() == tenant
        && request.actor.0.region.as_str() == region
        && request.actor.0.principal_id.0 == created_by_principal
        && request.subject == expected_subject
        && request.aggregate == expected_aggregate
        && !request.contains_personal_data
        && request.pii_key_ref.is_none()
        && payload.get("issue_id").and_then(serde_json::Value::as_str)
            == Some(expected_issue_id.as_str())
        && payload
            .get("project_id")
            .and_then(serde_json::Value::as_str)
            == Some(expected_project_id.as_str())
        && payload
            .get("issue_object")
            .and_then(serde_json::Value::as_str)
            == Some(issue_object)
        && payload
            .get("project_userset")
            .and_then(serde_json::Value::as_str)
            == Some(project_userset)
        && payload.get("relation").and_then(serde_json::Value::as_str) == Some(relation);
    if valid {
        Ok(())
    } else {
        Err("authorization request envelope does not match staged issue binding".into())
    }
}

fn now_rfc3339() -> Timestamp {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Timestamp(format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z"))
}

fn decode_row(
    kms: &KmsEngine,
    region: &myelin_tenancy::Region,
    row: sqlx::postgres::PgRow,
) -> Result<StoredIssue, IssueStoreError> {
    let nonce: Vec<u8> = row
        .try_get("title_nonce")
        .map_err(|_| IssueStoreError::Crypto("title nonce is absent".into()))?;
    let nonce: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| IssueStoreError::Crypto("title nonce has invalid length".into()))?;
    let ciphertext: Vec<u8> = row
        .try_get("title_ciphertext")
        .map_err(|_| IssueStoreError::Crypto("title ciphertext is absent".into()))?;
    let key_ref: String = row
        .try_get("pii_key_ref")
        .map_err(|_| IssueStoreError::Crypto("title key reference is absent".into()))?;
    let key_ref = PiiKeyRef::parse(&key_ref)
        .ok_or_else(|| IssueStoreError::Crypto("title key reference is malformed".into()))?;
    let opened = decrypt_free_text(
        kms,
        region,
        &EncryptedColumn {
            key_ref,
            nonce,
            ciphertext,
        },
    )
    .map_err(|e| IssueStoreError::Crypto(e.to_string()))?;
    let title = String::from_utf8(opened)
        .map_err(|_| IssueStoreError::Crypto("decrypted title is not UTF-8".into()))?;
    Ok(StoredIssue {
        id: row.get::<Uuid, _>("id").to_string(),
        key: row.get("key"),
        project_id: row.get::<Uuid, _>("project_id").to_string(),
        state: row.get("state"),
        state_category: row.get("state_category"),
        title,
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn validate_create(proposal: &CreateIssue) -> Result<(), IssueStoreError> {
    parse_uuid("project_id", &proposal.project_id)?;
    parse_uuid("type_id", &proposal.type_id)?;
    if proposal.title.is_empty() || proposal.title.len() > MAX_TITLE_BYTES {
        return Err(IssueStoreError::BadInput(format!(
            "title must contain 1..={MAX_TITLE_BYTES} bytes"
        )));
    }
    if proposal.prefix.len() < 2
        || proposal.prefix.len() > 10
        || !proposal
            .prefix
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(IssueStoreError::BadInput(
            "prefix must be 2..=10 uppercase ASCII letters/digits".into(),
        ));
    }
    Ok(())
}

fn title_dek_subject(principal: &Principal) -> String {
    match &principal.kind {
        PrincipalKind::Agent {
            on_behalf_of: Some(subject),
            ..
        } => subject.0.clone(),
        _ => principal.principal_id.0.clone(),
    }
}

fn normalize_visible(visible: VisibleIssues) -> Result<SetExpr, IssueStoreError> {
    match visible {
        VisibleIssues::Filter { set_expr } => Ok(set_expr),
        VisibleIssues::None | VisibleIssues::All | VisibleIssues::Ids(_) => {
            Err(IssueStoreError::AuthorizationUnavailable(
                "durable issue lists require the frozen effective issue:view filter".into(),
            ))
        }
    }
}

fn validate_issue_view_filter(set_expr: &SetExpr) -> Result<(), IssueStoreError> {
    let expected = match VisibleIssues::effective_issue_view_filter() {
        VisibleIssues::Filter { set_expr } => set_expr,
        _ => unreachable!("constructor always returns a filter"),
    };
    if set_expr != &expected {
        return Err(IssueStoreError::AuthorizationUnavailable(
            "Identity returned a non-frozen issue visibility expression".into(),
        ));
    }
    Ok(())
}

fn parse_uuid(field: &str, value: &str) -> Result<Uuid, IssueStoreError> {
    Uuid::parse_str(value).map_err(|_| IssueStoreError::BadInput(format!("{field} must be a UUID")))
}

/// Canonical uppercase Crockford ULID shape used by durable event ids.
pub fn is_canonical_request_event_id(value: &str) -> bool {
    value.len() == 26
        && value.as_bytes()[0] <= b'7'
        && value.bytes().all(|byte| {
            byte.is_ascii_digit()
                || matches!(
                    byte,
                    b'A'..=b'H' | b'J'..=b'K' | b'M'..=b'N' | b'P'..=b'T' | b'V'..=b'Z'
                )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalId, PrincipalStatus, RuntimeRef};
    use myelin_tenancy::{Region, TenantId};

    #[test]
    fn pagination_is_bounded_and_cursor_is_typed() {
        assert!(IssuePageRequest::new(1, None).is_ok());
        assert!(IssuePageRequest::new(MAX_PAGE_SIZE, None).is_ok());
        assert!(IssuePageRequest::new(0, None).is_err());
        assert!(IssuePageRequest::new(MAX_PAGE_SIZE + 1, None).is_err());
        assert!(IssuePageRequest::new(10, Some("not-a-uuid".into())).is_err());
        let cursor = encode_issue_page_cursor(
            IssueListState::Closed,
            Some("eng-"),
            1_700_000_000_123_456,
            "11111111-1111-1111-1111-111111111111",
        )
        .unwrap();
        let request = IssuePageRequest::filtered(
            IssueListState::Closed,
            Some("eng-".into()),
            10,
            Some(cursor.clone()),
        )
        .unwrap();
        assert_eq!(request.key.as_deref(), Some("ENG-"));
        assert!(IssuePageRequest::filtered(
            IssueListState::Open,
            Some("ENG-".into()),
            10,
            Some(cursor),
        )
        .is_err());
        assert!(LIST_EFFECTIVE_ISSUE_VIEW_SQL.contains("ORDER BY i.updated_at DESC, i.id DESC"));
        assert!(LIST_EFFECTIVE_ISSUE_VIEW_SQL.contains("(i.updated_at, i.id) <"));
        assert!(LIST_EFFECTIVE_ISSUE_VIEW_SQL.contains("AND NOT i.archived"));
        assert!(LIST_EFFECTIVE_ISSUE_VIEW_SQL.contains("b.issue_object = 'issue:' || i.id::text"));
        assert!(!LIST_EFFECTIVE_ISSUE_VIEW_SQL.contains("title LIKE"));
    }

    #[test]
    fn authorization_status_id_and_binding_lookup_are_strict() {
        assert!(is_canonical_request_event_id("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
        for invalid in [
            "01arz3ndektsv4rrffq69g5fav",
            "81ARZ3NDEKTSV4RRFFQ69G5FAV",
            "01ARZ3NDEKTSV4RRFFQ69G5FAI",
            "01ARZ3NDEKTSV4RRFFQ69G5FA",
            "01ARZ3NDEKTSV4RRFFQ69G5FAV/",
        ] {
            assert!(
                !is_canonical_request_event_id(invalid),
                "accepted `{invalid}`"
            );
        }
        for required in [
            "b.request_event_id = $3",
            "i.created_by_principal = $4",
            "b.project_id = i.project_id",
            "b.issue_object = 'issue:' || i.id::text",
            "b.project_userset = 'project:' || i.project_id::text || '#view'",
            "b.relation = 'parent_project'",
            "AND NOT i.archived",
        ] {
            assert!(
                AUTHORIZATION_STATUS_SQL.contains(required),
                "status lookup omitted `{required}`"
            );
        }
        for forbidden in ["outbox", "attempts", "last_error", "on_behalf_of"] {
            assert!(
                !AUTHORIZATION_STATUS_SQL.contains(forbidden),
                "status lookup exposed `{forbidden}`"
            );
        }
    }

    #[test]
    fn create_validation_refuses_unbounded_free_text_and_malformed_ids() {
        let valid = CreateIssue {
            project_id: "11111111-1111-1111-1111-111111111111".into(),
            type_id: "22222222-2222-2222-2222-222222222222".into(),
            prefix: "ENG".into(),
            title: "bounded title".into(),
        };
        assert!(validate_create(&valid).is_ok());
        let mut bad = valid.clone();
        bad.title = "x".repeat(MAX_TITLE_BYTES + 1);
        assert!(validate_create(&bad).is_err());
        bad = valid.clone();
        bad.prefix = "eng".into();
        assert!(validate_create(&bad).is_err());
        bad = valid;
        bad.project_id = "path-tenant-smuggling".into();
        assert!(validate_create(&bad).is_err());
    }

    #[test]
    fn only_the_frozen_effective_filter_reaches_sql() {
        let frozen = normalize_visible(VisibleIssues::effective_issue_view_filter()).unwrap();
        assert!(validate_issue_view_filter(&frozen).is_ok());
        assert!(normalize_visible(VisibleIssues::All).is_err());
        assert!(validate_issue_view_filter(&SetExpr::All).is_err());
    }

    #[test]
    fn title_dek_subject_uses_explicit_on_behalf_of_else_stable_creator() {
        let human = Principal::new(
            TenantId::from_token("acme"),
            Region::new("fr-par"),
            PrincipalId("human:ada".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        assert_eq!(title_dek_subject(&human), "human:ada");
        let agent = Principal::new(
            TenantId::from_token("acme"),
            Region::new("fr-par"),
            PrincipalId("agent:reviewer".into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("runtime:1".into()),
                on_behalf_of: Some(PrincipalId("human:ada".into())),
            },
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        assert_eq!(title_dek_subject(&agent), "human:ada");
    }

    #[test]
    fn activation_event_reuses_persisted_ulid_and_original_request_provenance_without_pii() {
        let creator = Principal::new(
            TenantId::from_token("acme"),
            Region::new("fr-par"),
            PrincipalId("human:creator".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let issue_id = Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap();
        let project_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let issue_object = issue_object(issue_id);
        let userset = project_userset(project_id);
        let minter = UlidMinter::new();
        let request_id: EventId = minter.mint().into();
        let created_id: EventId = minter.mint().into();
        let request = authorization_request_envelope(
            &creator,
            issue_id,
            project_id,
            &issue_object,
            &userset,
            request_id.clone(),
        );

        validate_authorization_request(
            &request,
            "acme",
            "fr-par",
            issue_id,
            project_id,
            &issue_object,
            &userset,
            "parent_project",
            &request_id.0,
            &creator.principal_id.0,
        )
        .unwrap();
        let created = issue_created_envelope(
            created_id.clone(),
            issue_id,
            "ENG-1",
            project_id,
            &Zookie("zookie:1".into()),
            &request,
        );

        assert_eq!(request.event_id.0.len(), 26);
        assert_eq!(created.event_id, created_id);
        assert_eq!(created.event_id.0.len(), 26);
        assert_eq!(created.actor, Actor(creator));
        assert_eq!(created.tenant, request.tenant);
        assert_eq!(created.region, request.region);
        assert_eq!(created.occurred_at, request.occurred_at);
        assert_eq!(created.causation_id, Some(request_id));
        assert!(!created.contains_personal_data);
        assert_eq!(created.pii_key_ref, None);
    }

    #[test]
    fn activation_rejects_request_partition_actor_or_tuple_tampering() {
        let creator = Principal::new(
            TenantId::from_token("acme"),
            Region::new("fr-par"),
            PrincipalId("human:creator".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let issue_id = Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap();
        let project_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let object = issue_object(issue_id);
        let userset = project_userset(project_id);
        let request = authorization_request_envelope(
            &creator,
            issue_id,
            project_id,
            &object,
            &userset,
            UlidMinter::new().mint().into(),
        );
        let request_event_id = request.event_id.0.clone();
        let validate = |candidate: &EventEnvelope| {
            validate_authorization_request(
                candidate,
                "acme",
                "fr-par",
                issue_id,
                project_id,
                &object,
                &userset,
                "parent_project",
                &request_event_id,
                &creator.principal_id.0,
            )
        };

        let mut tampered = request.clone();
        tampered.region = Region::new("us-east");
        assert!(validate(&tampered).is_err());
        tampered = request.clone();
        tampered.actor.0.tenant = TenantId::from_token("other");
        assert!(validate(&tampered).is_err());
        tampered = request;
        tampered.payload["project_userset"] = serde_json::json!("project:other#view");
        assert!(validate(&tampered).is_err());
    }

    #[test]
    fn close_event_is_canonical_references_only_and_actor_scoped() {
        let actor = Principal::new(
            TenantId::from_token("acme"),
            Region::new("fr-par"),
            PrincipalId("human:closer".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let issue_id = Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap();
        let event_id: EventId = UlidMinter::new().mint().into();
        let closed =
            issue_closed_envelope(&actor, issue_id, "ENG-7", "In Review", event_id.clone());

        assert_eq!(closed.event_id, event_id);
        assert_eq!(closed.type_.0, ISSUE_CLOSED);
        assert_eq!(closed.tenant, actor.tenant);
        assert_eq!(closed.region, actor.region);
        assert_eq!(closed.actor, Actor(actor));
        assert_eq!(closed.aggregate.0, format!("issue:{issue_id}"));
        assert_eq!(closed.payload["issue_key"], "ENG-7");
        assert_eq!(closed.payload["from"], "In Review");
        assert_eq!(closed.payload["category"], "completed");
        assert!(!closed.contains_personal_data);
        assert!(closed.pii_key_ref.is_none());
        assert!(!closed.payload.to_string().contains("title"));
    }
}
