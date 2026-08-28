use super::{parse_uuid, CreateIssue, IssueCreationReceipt, IssueStoreError};
use crate::import_source::SourceSystem;
use myelin_storage::{ContentHash, PgError};
use sqlx::types::Uuid;
use sqlx::{PgConnection, Row};

const MAX_SOURCE_ID_BYTES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportIssue {
    pub import_job_id: String,
    pub source: SourceSystem,
    pub source_id: String,
    pub issue: CreateIssue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportIssueReceipt {
    pub issue: IssueCreationReceipt,
    pub created: bool,
}

#[derive(Clone, Debug)]
pub(super) struct ImportIdentity {
    pub job_id: Uuid,
    pub source: SourceSystem,
    pub source_id: String,
    pub request_hash: String,
}

impl ImportIssue {
    pub fn validate(&self) -> Result<(), IssueStoreError> {
        self.identity()?;
        super::validate_create(&self.issue)
    }

    pub(super) fn into_parts(self) -> Result<(ImportIdentity, CreateIssue), IssueStoreError> {
        let identity = self.identity()?;
        Ok((identity, self.issue))
    }

    fn identity(&self) -> Result<ImportIdentity, IssueStoreError> {
        let job_id = parse_uuid("import_job_id", &self.import_job_id)?;
        if self.source_id.is_empty() || self.source_id.len() > MAX_SOURCE_ID_BYTES {
            return Err(IssueStoreError::BadInput(format!(
                "import source_id must contain 1..={MAX_SOURCE_ID_BYTES} bytes"
            )));
        }
        Ok(ImportIdentity {
            job_id,
            source: self.source,
            source_id: self.source_id.clone(),
            request_hash: issue_request_hash(&self.issue),
        })
    }
}

pub(super) enum ImportClaim {
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

pub(super) async fn claim(
    connection: &mut PgConnection,
    tenant_id: &str,
    region: &str,
    identity: &ImportIdentity,
) -> Result<ImportClaim, PgError> {
    let acquired = sqlx::query_scalar::<_, String>(
        "INSERT INTO import_map (\
           tenant_id, region, import_job, source, source_id, request_hash, myelin_kind, status\
         ) VALUES ($1, $2, $3, $4, $5, $6, 'issue', 'pending') \
         ON CONFLICT (tenant_id, region, import_job, source, source_id, myelin_kind) DO NOTHING \
         RETURNING source_id",
    )
    .bind(tenant_id)
    .bind(region)
    .bind(identity.job_id)
    .bind(identity.source.token())
    .bind(&identity.source_id)
    .bind(&identity.request_hash)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;
    if acquired.is_some() {
        return Ok(ImportClaim::Acquired);
    }

    let existing = sqlx::query(
        "SELECT m.request_hash, m.status, m.myelin_id, i.key, i.project_id, b.request_event_id \
         FROM import_map m \
         LEFT JOIN issue i \
           ON i.tenant_id = m.tenant_id AND i.region = m.region AND i.id = m.myelin_id \
         LEFT JOIN issue_authz_binding b \
           ON b.tenant_id = i.tenant_id AND b.region = i.region AND b.issue_id = i.id \
         WHERE m.tenant_id = $1 AND m.region = $2 AND m.import_job = $3 \
           AND m.source = $4 AND m.source_id = $5 AND m.myelin_kind = 'issue'",
    )
    .bind(tenant_id)
    .bind(region)
    .bind(identity.job_id)
    .bind(identity.source.token())
    .bind(&identity.source_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?
    .ok_or_else(|| PgError::Query("import claim disappeared during conflict resolution".into()))?;

    let stored_hash = existing
        .try_get::<Option<String>, _>("request_hash")
        .map_err(|error| PgError::Query(error.to_string()))?;
    if stored_hash
        .as_ref()
        .is_some_and(|request_hash| request_hash != &identity.request_hash)
    {
        return Ok(ImportClaim::Conflict);
    }
    let status: String = existing.get("status");
    if !matches!(status.as_str(), "created" | "wired" | "lossy") {
        return Err(PgError::Query(format!(
            "import claim is not resumable from state `{status}`"
        )));
    }
    let id = existing
        .try_get::<Option<Uuid>, _>("myelin_id")
        .map_err(|error| PgError::Query(error.to_string()))?
        .ok_or_else(|| PgError::Query("completed import claim has no Myelin id".into()))?;
    let key = existing
        .try_get("key")
        .map_err(|error| PgError::Query(error.to_string()))?;
    let project_id = existing
        .try_get("project_id")
        .map_err(|error| PgError::Query(error.to_string()))?;
    let request_event_id = existing
        .try_get("request_event_id")
        .map_err(|error| PgError::Query(error.to_string()))?;
    Ok(ImportClaim::Existing(ExistingIssueCreation {
        id,
        key,
        project_id,
        request_event_id,
    }))
}

pub(super) async fn complete(
    connection: &mut PgConnection,
    tenant_id: &str,
    region: &str,
    identity: &ImportIdentity,
    issue_id: Uuid,
) -> Result<(), PgError> {
    let updated = sqlx::query(
        "UPDATE import_map SET myelin_id = $6, status = 'created' \
         WHERE tenant_id = $1 AND region = $2 AND import_job = $3 \
           AND source = $4 AND source_id = $5 AND myelin_kind = 'issue' \
           AND request_hash = $7 AND status = 'pending' AND myelin_id IS NULL",
    )
    .bind(tenant_id)
    .bind(region)
    .bind(identity.job_id)
    .bind(identity.source.token())
    .bind(&identity.source_id)
    .bind(issue_id)
    .bind(&identity.request_hash)
    .execute(&mut *connection)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;
    if updated.rows_affected() != 1 {
        return Err(PgError::Query(
            "import claim did not advance from pending to created".into(),
        ));
    }
    Ok(())
}

fn issue_request_hash(issue: &CreateIssue) -> String {
    let mut bytes = Vec::new();
    for part in [
        b"myelin.issue.import.request.v1".as_slice(),
        issue.project_id.as_bytes(),
        issue.type_id.as_bytes(),
        issue.prefix.as_bytes(),
        issue.title.as_bytes(),
    ] {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    ContentHash::blake3(&bytes).to_multihash_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn import(source_id: String) -> ImportIssue {
        ImportIssue {
            import_job_id: "11111111-1111-1111-1111-111111111111".into(),
            source: SourceSystem::Linear,
            source_id,
            issue: CreateIssue {
                project_id: "22222222-2222-2222-2222-222222222222".into(),
                type_id: "33333333-3333-3333-3333-333333333333".into(),
                prefix: "ENG".into(),
                title: "Imported".into(),
            },
        }
    }

    #[test]
    fn import_identity_is_bounded_before_it_reaches_postgres() {
        let original = import("external-41".into());
        let original_identity = original.identity().unwrap();
        let mut corrected = original.clone();
        corrected.issue.title = "Corrected imported title".into();
        let corrected_identity = corrected.identity().unwrap();
        assert_ne!(
            original_identity.request_hash,
            corrected_identity.request_hash
        );
        assert!(!original_identity.request_hash.contains("Imported"));
        assert!(original.into_parts().is_ok());
        for source_id in [String::new(), "x".repeat(MAX_SOURCE_ID_BYTES + 1)] {
            assert!(matches!(
                import(source_id).into_parts(),
                Err(IssueStoreError::BadInput(_))
            ));
        }
        let mut malformed_job = import("external-41".into());
        malformed_job.import_job_id = "not-a-job".into();
        assert!(matches!(
            malformed_job.into_parts(),
            Err(IssueStoreError::BadInput(_))
        ));
    }
}
