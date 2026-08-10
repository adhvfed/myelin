use super::{CreateIssue, IssueStoreError};
use myelin_identity::Principal;
use myelin_storage::{ContentHash, PgError};
use sqlx::types::Uuid;
use sqlx::{PgConnection, Row};

const MAX_CALLER_KEY_BYTES: usize = 256;

#[derive(Clone, Debug)]
pub(super) struct CreateIdentity {
    storage_nonce: String,
    request_hash: String,
}

pub(super) enum CreateClaim {
    Acquired,
    Existing(ExistingIssueCreation),
    Conflict,
}

pub(super) struct ExistingIssueCreation {
    pub id: Uuid,
    pub key: String,
    pub project_id: Uuid,
    pub request_event_id: String,
}

impl CreateIdentity {
    pub fn new(
        actor: &Principal,
        caller_key: &str,
        proposal: &CreateIssue,
    ) -> Result<Self, IssueStoreError> {
        if caller_key.is_empty()
            || caller_key.len() > MAX_CALLER_KEY_BYTES
            || !caller_key.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(IssueStoreError::BadInput(format!(
                "issue create idempotency key must contain 1..={MAX_CALLER_KEY_BYTES} printable ASCII bytes"
            )));
        }
        Ok(Self {
            storage_nonce: digest(&[
                b"myelin.issue.create.idempotency.v1",
                actor.tenant.0.as_bytes(),
                actor.region.0.as_bytes(),
                actor.principal_id.0.as_bytes(),
                caller_key.as_bytes(),
            ]),
            request_hash: digest(&[
                b"myelin.issue.create.request.v1",
                proposal.project_id.as_bytes(),
                proposal.type_id.as_bytes(),
                proposal.prefix.as_bytes(),
                proposal.title.as_bytes(),
            ]),
        })
    }
}

pub(super) async fn claim(
    connection: &mut PgConnection,
    tenant_id: &str,
    region: &str,
    identity: &CreateIdentity,
) -> Result<CreateClaim, PgError> {
    let acquired = sqlx::query_scalar::<_, String>(
        "INSERT INTO issue_create_idempotency (\
           tenant_id, region, storage_nonce, request_hash, status\
         ) VALUES ($1, $2, $3, $4, 'pending') \
         ON CONFLICT (tenant_id, region, storage_nonce) DO NOTHING \
         RETURNING storage_nonce",
    )
    .bind(tenant_id)
    .bind(region)
    .bind(&identity.storage_nonce)
    .bind(&identity.request_hash)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;
    if acquired.is_some() {
        return Ok(CreateClaim::Acquired);
    }

    let existing = sqlx::query(
        "SELECT d.request_hash, d.status, d.issue_id, i.key, i.project_id, \
                b.request_event_id \
         FROM issue_create_idempotency d \
         LEFT JOIN issue i \
           ON i.tenant_id = d.tenant_id AND i.region = d.region AND i.id = d.issue_id \
         LEFT JOIN issue_authz_binding b \
           ON b.tenant_id = i.tenant_id AND b.region = i.region AND b.issue_id = i.id \
         WHERE d.tenant_id = $1 AND d.region = $2 AND d.storage_nonce = $3",
    )
    .bind(tenant_id)
    .bind(region)
    .bind(&identity.storage_nonce)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?
    .ok_or_else(|| PgError::Query("issue create claim disappeared during retry".into()))?;

    if existing.get::<String, _>("request_hash") != identity.request_hash {
        return Ok(CreateClaim::Conflict);
    }
    if existing.get::<String, _>("status") != "created" {
        return Err(PgError::Query(
            "issue create claim is not resumable from its durable state".into(),
        ));
    }
    Ok(CreateClaim::Existing(ExistingIssueCreation {
        id: required(&existing, "issue_id")?,
        key: required(&existing, "key")?,
        project_id: required(&existing, "project_id")?,
        request_event_id: required(&existing, "request_event_id")?,
    }))
}

pub(super) async fn complete(
    connection: &mut PgConnection,
    tenant_id: &str,
    region: &str,
    identity: &CreateIdentity,
    issue_id: Uuid,
) -> Result<(), PgError> {
    let updated = sqlx::query(
        "UPDATE issue_create_idempotency SET issue_id = $4, status = 'created' \
         WHERE tenant_id = $1 AND region = $2 AND storage_nonce = $3 \
           AND request_hash = $5 AND status = 'pending' AND issue_id IS NULL",
    )
    .bind(tenant_id)
    .bind(region)
    .bind(&identity.storage_nonce)
    .bind(issue_id)
    .bind(&identity.request_hash)
    .execute(&mut *connection)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;
    if updated.rows_affected() != 1 {
        return Err(PgError::Query(
            "issue create claim did not advance from pending to created".into(),
        ));
    }
    Ok(())
}

fn required<T>(row: &sqlx::postgres::PgRow, column: &str) -> Result<T, PgError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get::<Option<T>, _>(column)
        .map_err(|error| PgError::Query(error.to_string()))?
        .ok_or_else(|| PgError::Query(format!("completed issue create has no `{column}`")))
}

fn digest(parts: &[&[u8]]) -> String {
    let mut bytes = Vec::new();
    for part in parts {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    ContentHash::blake3(&bytes).to_multihash_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_tenancy::{Region, TenantId};

    fn actor(tenant: &str, principal: &str) -> Principal {
        Principal::new(
            TenantId(tenant.into()),
            Region("fr-par".into()),
            PrincipalId(principal.into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn proposal(title: &str) -> CreateIssue {
        CreateIssue {
            project_id: "11111111-1111-1111-1111-111111111111".into(),
            type_id: "22222222-2222-2222-2222-222222222222".into(),
            prefix: "ENG".into(),
            title: title.into(),
        }
    }

    #[test]
    fn identity_is_stable_scoped_secret_free_and_request_bound() {
        let first = CreateIdentity::new(&actor("acme", "agent:a"), "private-retry", &proposal("A"))
            .unwrap();
        let replay =
            CreateIdentity::new(&actor("acme", "agent:a"), "private-retry", &proposal("A"))
                .unwrap();
        let changed =
            CreateIdentity::new(&actor("acme", "agent:a"), "private-retry", &proposal("B"))
                .unwrap();
        let other_actor =
            CreateIdentity::new(&actor("acme", "agent:b"), "private-retry", &proposal("A"))
                .unwrap();
        assert_eq!(first.storage_nonce, replay.storage_nonce);
        assert_eq!(first.request_hash, replay.request_hash);
        assert_eq!(first.storage_nonce, changed.storage_nonce);
        assert_ne!(first.request_hash, changed.request_hash);
        assert_ne!(first.storage_nonce, other_actor.storage_nonce);
        assert!(!first.storage_nonce.contains("private"));
        assert!(!first.request_hash.contains('A'));
    }

    #[test]
    fn caller_key_is_bounded_before_hashing() {
        for key in ["", "contains space", &"x".repeat(MAX_CALLER_KEY_BYTES + 1)] {
            assert!(matches!(
                CreateIdentity::new(&actor("acme", "agent:a"), key, &proposal("A")),
                Err(IssueStoreError::BadInput(_))
            ));
        }
    }
}
