//! Tenant/region-scoped durable Issue store (R4.4 data-plane increment).
//!
//! Every operation derives its scope from a verified [`Principal`], checks object authorization
//! before touching PostgreSQL, and executes through [`SubstrateProvider::with_tenant_tx`] so FORCE
//! RLS is a second, independent tenant boundary. Free-text titles are sealed under the creator's
//! per-subject DEK before the insert; only nonce/ciphertext/`pii_key_ref` rest in PostgreSQL.
//!
//! This module intentionally does not provide an allow-all production authorizer or an in-memory
//! store. The edge composition must inject the live Identity/ReBAC implementation. Until Identity
//! exposes an atomic issue-row + `issue#parent_project` tuple bootstrap seam, this store is not
//! mounted at the public edge: exposing a route without that tuple would either orphan the row or
//! bypass object authorization.

use crate::dek::{decrypt_free_text, encrypt_free_text, IssueFreeText};
use myelin_identity::{Principal, PrincipalKind};
use myelin_storage::encryption::{EncryptedColumn, SubjectId};
use myelin_storage::kms::{KmsEngine, PiiKeyRef, NONCE_LEN};
use myelin_storage::{SubstrateProvider, TenantScope};
use sqlx::types::Uuid;
use sqlx::Row;
use std::collections::BTreeSet;
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
}

#[derive(Debug, PartialEq, Eq)]
enum QueryVisibility {
    None,
    All,
    Ids(Vec<Uuid>),
}

/// Required production authorization seam. The live implementation is Identity/ReBAC; there is no
/// permissive default. Every method receives the verified principal, never a body/path tenant.
pub trait IssueAuthorizer: Send + Sync {
    /// Authorize creation under the addressed project object before encryption/counter/row mutation.
    fn may_create(&self, principal: &Principal, project_id: &str) -> bool;
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

/// Bounded keyset page request. The cursor is an issue UUID from a prior page, never SQL text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuePageRequest {
    /// Requested rows, `1..=100`.
    pub limit: u32,
    /// Exclusive UUID cursor.
    pub cursor: Option<String>,
}

impl IssuePageRequest {
    /// Validate a bounded page request. Invalid/oversized limits and malformed cursors fail loudly.
    pub fn new(limit: u32, cursor: Option<String>) -> Result<Self, IssueStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(IssueStoreError::BadInput(format!(
                "page limit must be between 1 and {MAX_PAGE_SIZE}"
            )));
        }
        if let Some(value) = cursor.as_deref() {
            parse_uuid("cursor", value)?;
        }
        Ok(Self { limit, cursor })
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
pub struct PgIssueStore<A: IssueAuthorizer> {
    provider: SubstrateProvider,
    kms: Arc<KmsEngine>,
    authorizer: A,
}

impl<A: IssueAuthorizer> PgIssueStore<A> {
    pub fn new(provider: SubstrateProvider, kms: Arc<KmsEngine>, authorizer: A) -> Self {
        Self {
            provider,
            kms,
            authorizer,
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

    /// Authorize project creation, seal title, atomically allocate a human key and insert the row.
    pub async fn create(
        &self,
        principal: &Principal,
        proposal: CreateIssue,
    ) -> Result<StoredIssue, IssueStoreError> {
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
        let prefix = proposal.prefix;
        let created_by = principal.principal_id.0.clone();
        let nonce = sealed.nonce.to_vec();
        let ciphertext = sealed.ciphertext;
        let key_ref = sealed.key_ref.to_uri();

        let row = self
            .provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    sqlx::query(
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
                         SELECT $1, $2, gen_random_uuid(), $3 || '-' || high_water::text, $3, $4, \
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
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))
                })
            })
            .await
            .map_err(|e| IssueStoreError::Storage(e.to_string()))?;
        decode_row(&self.kms, &principal.region, row)
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
        let row = self
            .provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    select_one(conn, id)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))
                })
            })
            .await
            .map_err(|e| IssueStoreError::Storage(e.to_string()))?
            .ok_or(IssueStoreError::NotFound)?;
        decode_row(&self.kms, &principal.region, row)
    }

    /// Leak-free list: obtain the Identity prefilter first, then conjoin it inside the SQL query.
    pub async fn list(
        &self,
        principal: &Principal,
        page: IssuePageRequest,
    ) -> Result<IssuePage, IssueStoreError> {
        let scope = self.scope(principal)?;
        let visible = normalize_visible(
            self.authorizer
                .visible_issues(principal)
                .map_err(IssueStoreError::AuthorizationUnavailable)?,
        )?;
        if visible == QueryVisibility::None {
            return Ok(IssuePage {
                items: Vec::new(),
                next_cursor: None,
                limit: page.limit,
            });
        }
        let cursor = page
            .cursor
            .as_deref()
            .map(|v| parse_uuid("cursor", v))
            .transpose()?;
        let fetch_limit = i64::from(page.limit) + 1;
        let tenant_id = scope.tenant().0.clone();
        let rows = self
            .provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    let result = match visible {
                        QueryVisibility::None => unreachable!("handled before the transaction"),
                        QueryVisibility::All => {
                            sqlx::query(&format!("{} ORDER BY id ASC LIMIT $2", SELECT_BASE))
                                .bind(cursor)
                                .bind(fetch_limit)
                                .fetch_all(&mut *conn)
                                .await
                        }
                        QueryVisibility::Ids(ids) => {
                            sqlx::query(&format!(
                                "{} AND id = ANY($2) ORDER BY id ASC LIMIT $3",
                                SELECT_BASE
                            ))
                            .bind(cursor)
                            .bind(ids)
                            .bind(fetch_limit)
                            .fetch_all(&mut *conn)
                            .await
                        }
                    };
                    result.map_err(|e| myelin_storage::PgError::Query(e.to_string()))
                })
            })
            .await
            .map_err(|e| IssueStoreError::Storage(e.to_string()))?;

        let has_more = rows.len() > page.limit as usize;
        let mut items = Vec::with_capacity(rows.len().min(page.limit as usize));
        for row in rows.into_iter().take(page.limit as usize) {
            items.push(decode_row(&self.kms, &principal.region, row)?);
        }
        let next_cursor = has_more
            .then(|| items.last().map(|item| item.id.clone()))
            .flatten();
        Ok(IssuePage {
            items,
            next_cursor,
            limit: page.limit,
        })
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
        let row = self
            .provider
            .with_tenant_tx(&tenant_id.clone(), move |conn| {
                Box::pin(async move {
                    let current = select_one_for_update(conn, id)
                        .await
                        .map_err(|e| myelin_storage::PgError::Query(e.to_string()))?;
                    let Some(current) = current else {
                        return Ok(None);
                    };
                    if current.get::<String, _>("state_category") == "completed" {
                        return Ok(Some(current));
                    }
                    sqlx::query(&format!(
                        "UPDATE issue SET state = 'Done', state_category = 'completed', \
                         state_changed_at = now(), updated_at = now(), version = version + 1 \
                         WHERE id = $1 RETURNING {}",
                        SELECT_COLUMNS
                    ))
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::PgError::Query(e.to_string()))
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
const SELECT_BASE: &str = "SELECT id, key, project_id, state, state_category, title_nonce, \
title_ciphertext, pii_key_ref, version, created_at::text, updated_at::text \
FROM issue WHERE deleted_at IS NULL AND ($1::uuid IS NULL OR id > $1)";

async fn select_one(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
) -> Result<Option<sqlx::postgres::PgRow>, sqlx::Error> {
    sqlx::query(&format!(
        "SELECT {SELECT_COLUMNS} FROM issue WHERE id = $1 AND deleted_at IS NULL"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await
}

async fn select_one_for_update(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
) -> Result<Option<sqlx::postgres::PgRow>, sqlx::Error> {
    sqlx::query(&format!(
        "SELECT {SELECT_COLUMNS} FROM issue WHERE id = $1 AND deleted_at IS NULL FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await
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

fn normalize_visible(visible: VisibleIssues) -> Result<QueryVisibility, IssueStoreError> {
    match visible {
        VisibleIssues::None => Ok(QueryVisibility::None),
        VisibleIssues::All => Ok(QueryVisibility::All),
        VisibleIssues::Ids(ids) => {
            if ids.len() > MAX_AUTHORIZED_ISSUE_IDS {
                return Err(IssueStoreError::AuthorizationUnavailable(format!(
                    "Identity materialised more than {MAX_AUTHORIZED_ISSUE_IDS} issue ids; a bounded push-down filter is required"
                )));
            }
            let mut unique = BTreeSet::new();
            for id in ids {
                let id = Uuid::parse_str(&id).map_err(|_| {
                    IssueStoreError::AuthorizationUnavailable(
                        "Identity returned a malformed issue UUID".into(),
                    )
                })?;
                unique.insert(id);
            }
            if unique.is_empty() {
                Ok(QueryVisibility::None)
            } else {
                Ok(QueryVisibility::Ids(unique.into_iter().collect()))
            }
        }
    }
}

fn parse_uuid(field: &str, value: &str) -> Result<Uuid, IssueStoreError> {
    Uuid::parse_str(value).map_err(|_| IssueStoreError::BadInput(format!("{field} must be a UUID")))
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
    fn materialised_visibility_is_bounded_deduplicated_and_typed_before_sql() {
        let id = "11111111-1111-1111-1111-111111111111".to_string();
        let normalized = normalize_visible(VisibleIssues::Ids(vec![id.clone(), id])).unwrap();
        assert!(matches!(normalized, QueryVisibility::Ids(ids) if ids.len() == 1));
        assert!(normalize_visible(VisibleIssues::Ids(vec!["not-a-uuid".into()])).is_err());
        assert!(normalize_visible(VisibleIssues::Ids(vec![
            "11111111-1111-1111-1111-111111111111".into();
            MAX_AUTHORIZED_ISSUE_IDS + 1
        ]))
        .is_err());
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
}
