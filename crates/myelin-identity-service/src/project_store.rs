use chrono::{DateTime, SecondsFormat, Utc};
use myelin_events::{
    derive_envelope, Actor, AggregateKey, DataRole, EmitContext, EventDraft, EventId, EventType,
    IdMinter, Timestamp, UlidMinter, Visibility,
};
use myelin_identity::{Principal, PrincipalStatus, IDENTITY_PROJECT_CREATED};
use myelin_storage::{PgError, PgStore, SubstrateProvider};
use myelin_tenancy::ArtifactRef;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

pub const PROJECT_WRITER_RELATION: &str = "writer";
pub const MAX_PROJECT_NAME_BYTES: usize = 100;
pub const MAX_PROJECT_PREFIX_BYTES: usize = 10;
pub const VISIBLE_PROJECTS_CTE: &str = r#"
WITH RECURSIVE visible_project(object_id) AS (
  SELECT object_id FROM rebac_tuple
   WHERE tenant_id = $1 AND region = $2 AND subject = $3
     AND ((split_part(object_id, ':', 1) = 'org'
             AND relation IN ('member', 'admin'))
       OR (split_part(object_id, ':', 1) = 'team'
             AND relation = 'member')
       OR (split_part(object_id, ':', 1) = 'project'
             AND relation IN ('reader', 'writer')))
  UNION
  SELECT edge.object_id
    FROM rebac_tuple edge
    JOIN visible_project parent
      ON edge.subject = parent.object_id || '#view'
   WHERE edge.tenant_id = $1 AND edge.region = $2
     AND ((split_part(edge.object_id, ':', 1) = 'team'
             AND edge.relation = 'parent_org'
             AND split_part(parent.object_id, ':', 1) = 'org')
       OR (split_part(edge.object_id, ':', 1) = 'project'
             AND edge.relation = 'parent_team'
             AND split_part(parent.object_id, ':', 1) = 'team'))
)
"#;
const MAX_PROJECT_LIST_ROWS: u32 = 101;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub issue_prefix: String,
    pub default_issue_type_id: String,
    pub created_by: String,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProject {
    pub name: String,
    pub issue_prefix: String,
    pub client_nonce: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectCreation {
    pub project: Project,
    pub created: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectMetadataRegistration {
    pub project: Project,
    pub registered: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    BadInput(String),
    NotFound,
    Conflict(String),
    Storage(String),
}

impl core::fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProjectError::BadInput(reason) => write!(formatter, "invalid project: {reason}"),
            ProjectError::NotFound => formatter.write_str("project not found"),
            ProjectError::Conflict(reason) => write!(formatter, "project conflict: {reason}"),
            ProjectError::Storage(reason) => write!(formatter, "project storage failed: {reason}"),
        }
    }
}

impl std::error::Error for ProjectError {}

#[derive(Clone)]
pub struct PgProjectStore {
    provider: SubstrateProvider,
    event_ids: Arc<dyn IdMinter>,
}

impl PgProjectStore {
    pub fn new(provider: SubstrateProvider) -> Self {
        Self::with_minter(provider, Arc::new(UlidMinter::new()))
    }

    pub fn with_minter(provider: SubstrateProvider, event_ids: Arc<dyn IdMinter>) -> Self {
        Self {
            provider,
            event_ids,
        }
    }

    pub async fn create(
        &self,
        actor: &Principal,
        proposal: NewProject,
    ) -> Result<ProjectCreation, ProjectError> {
        validate_new_project(&proposal)?;
        self.require_local_region(actor)?;

        let tenant = actor.tenant.0.clone();
        let region = actor.region.0.clone();
        let project_id = Uuid::new_v4();
        let default_issue_type_id = Uuid::new_v4();
        let created_at = Utc::now();
        let event = project_created_event(
            actor,
            project_id,
            default_issue_type_id,
            &proposal.issue_prefix,
            EventId(self.event_ids.mint().0),
            created_at,
        );
        let actor_id = actor.principal_id.0.clone();
        let proposed_name = proposal.name.clone();
        let proposed_prefix = proposal.issue_prefix.clone();
        let client_nonce = proposal.client_nonce.clone();

        let outcome = self
            .provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    if let Some(existing) =
                        project_by_nonce(conn, &tenant, &region, &client_nonce).await?
                    {
                        return Ok(CreateTx::Existing(existing));
                    }

                    let inserted = sqlx::query(
                        "INSERT INTO identity_project (\
                           tenant_id, region, project_id, name, issue_prefix, \
                           default_issue_type_id, created_by, client_nonce, created_at\
                         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
                         ON CONFLICT DO NOTHING \
                         RETURNING project_id, name, issue_prefix, default_issue_type_id, \
                                   created_by, created_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(project_id)
                    .bind(&proposed_name)
                    .bind(&proposed_prefix)
                    .bind(default_issue_type_id)
                    .bind(&actor_id)
                    .bind(&client_nonce)
                    .bind(created_at)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("insert identity project"))?;

                    let Some(row) = inserted else {
                        if let Some(existing) =
                            project_by_nonce(conn, &tenant, &region, &client_nonce).await?
                        {
                            return Ok(CreateTx::Existing(existing));
                        }
                        return Ok(CreateTx::PrefixConflict);
                    };
                    let project = project_from_row(&row)?;
                    let object = format!("project:{project_id}");
                    PgStore::insert_tuple_on_conn(
                        conn,
                        &tenant,
                        &region,
                        &object,
                        PROJECT_WRITER_RELATION,
                        &actor_id,
                    )
                    .await?;
                    myelin_storage::pgrelay::PgRelay::co_commit_in_tx(
                        conn,
                        &event.aggregate.0,
                        &event,
                    )
                    .await?;
                    Ok(CreateTx::Created(project))
                })
            })
            .await
            .map_err(|error| ProjectError::Storage(error.to_string()))?;

        match outcome {
            CreateTx::Created(project) => Ok(ProjectCreation {
                project,
                created: true,
            }),
            CreateTx::Existing(project)
                if project.name == proposal.name
                    && project.issue_prefix == proposal.issue_prefix =>
            {
                Ok(ProjectCreation {
                    project,
                    created: false,
                })
            }
            CreateTx::Existing(_) => Err(ProjectError::Conflict(
                "that idempotency key was already used for a different project".into(),
            )),
            CreateTx::PrefixConflict => Err(ProjectError::Conflict(format!(
                "issue prefix `{}` is already assigned to another project",
                proposal.issue_prefix
            ))),
        }
    }

    pub async fn ensure_existing_project_metadata(
        &self,
        actor: &Principal,
        project_id: &str,
        name: &str,
        issue_prefix: &str,
        default_issue_type_id: &str,
    ) -> Result<ProjectMetadataRegistration, ProjectError> {
        self.require_local_region(actor)?;
        let project_id = parse_canonical_uuid(project_id, "project id")?;
        let default_issue_type_id =
            parse_canonical_uuid(default_issue_type_id, "default issue type id")?;
        let client_nonce = format!("existing-project:{project_id}");
        let proposal = NewProject {
            name: name.to_string(),
            issue_prefix: issue_prefix.to_string(),
            client_nonce: client_nonce.clone(),
        };
        validate_new_project(&proposal)?;

        let tenant = actor.tenant.0.clone();
        let region = actor.region.0.clone();
        let actor_id = actor.principal_id.0.clone();
        let name = name.to_string();
        let issue_prefix = issue_prefix.to_string();
        let created_at = Utc::now();
        let outcome = self
            .provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let inserted = sqlx::query(
                        "INSERT INTO identity_project (\
                           tenant_id, region, project_id, name, issue_prefix, \
                           default_issue_type_id, created_by, client_nonce, created_at\
                         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
                         ON CONFLICT DO NOTHING \
                         RETURNING project_id, name, issue_prefix, default_issue_type_id, \
                                   created_by, created_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(project_id)
                    .bind(&name)
                    .bind(&issue_prefix)
                    .bind(default_issue_type_id)
                    .bind(&actor_id)
                    .bind(&client_nonce)
                    .bind(created_at)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("register existing project metadata"))?;
                    if let Some(row) = inserted {
                        return Ok(ProjectMetadataTx::Registered(project_from_row(&row)?));
                    }
                    Ok(
                        match project_by_id(conn, &tenant, &region, project_id).await? {
                            Some(project) => ProjectMetadataTx::Existing(project),
                            None => ProjectMetadataTx::Conflict,
                        },
                    )
                })
            })
            .await
            .map_err(|error| ProjectError::Storage(error.to_string()))?;

        let (project, registered) = match outcome {
            ProjectMetadataTx::Registered(project) => (project, true),
            ProjectMetadataTx::Existing(project)
                if project.name == proposal.name
                    && project.issue_prefix == proposal.issue_prefix
                    && project.default_issue_type_id == default_issue_type_id.to_string() =>
            {
                (project, false)
            }
            ProjectMetadataTx::Existing(_) => {
                return Err(ProjectError::Conflict(
                    "the existing project metadata differs from the bootstrap contract".into(),
                ))
            }
            ProjectMetadataTx::Conflict => {
                return Err(ProjectError::Conflict(format!(
                    "issue prefix `{}` is already assigned to another project",
                    proposal.issue_prefix
                )))
            }
        };
        Ok(ProjectMetadataRegistration {
            project,
            registered,
        })
    }

    pub async fn get(&self, actor: &Principal, project_id: &str) -> Result<Project, ProjectError> {
        self.require_local_region(actor)?;
        let project_id = parse_project_id(project_id)?;
        let tenant = actor.tenant.0.clone();
        let region = actor.region.0.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT project_id, name, issue_prefix, default_issue_type_id, \
                                created_by, created_at \
                           FROM identity_project \
                          WHERE tenant_id = $1 AND region = $2 AND project_id = $3",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(project_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("read identity project"))?;
                    row.as_ref().map(project_from_row).transpose()
                })
            })
            .await
            .map_err(|error| ProjectError::Storage(error.to_string()))?
            .ok_or(ProjectError::NotFound)
    }

    pub async fn list_visible(
        &self,
        actor: &Principal,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Project>, ProjectError> {
        self.require_local_region(actor)?;
        if limit == 0 || limit > MAX_PROJECT_LIST_ROWS {
            return Err(ProjectError::BadInput(
                "project store row limit must be between 1 and 101".into(),
            ));
        }
        if actor.status != PrincipalStatus::Active {
            return Ok(Vec::new());
        }
        let cursor = cursor.map(parse_project_id).transpose()?;
        let tenant = actor.tenant.0.clone();
        let region = actor.region.0.clone();
        let subject = actor.principal_id.0.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let query = format!(
                        "{VISIBLE_PROJECTS_CTE} \
                         SELECT project.project_id, project.name, project.issue_prefix, \
                                project.default_issue_type_id, project.created_by, \
                                project.created_at \
                           FROM identity_project project \
                           JOIN visible_project visible \
                             ON visible.object_id = 'project:' || project.project_id::text \
                          WHERE project.tenant_id = $1 AND project.region = $2 \
                            AND ($4::uuid IS NULL OR project.project_id < $4) \
                          ORDER BY project.project_id DESC LIMIT $5"
                    );
                    let rows = sqlx::query(&query)
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&subject)
                        .bind(cursor)
                        .bind(i64::from(limit))
                        .fetch_all(&mut *conn)
                        .await
                        .map_err(query_error("list visible identity projects"))?;
                    rows.iter().map(project_from_row).collect()
                })
            })
            .await
            .map_err(|error| ProjectError::Storage(error.to_string()))
    }

    fn require_local_region(&self, actor: &Principal) -> Result<(), ProjectError> {
        if actor.region.0 != self.provider.config().region {
            return Err(ProjectError::NotFound);
        }
        Ok(())
    }
}

enum CreateTx {
    Created(Project),
    Existing(Project),
    PrefixConflict,
}

enum ProjectMetadataTx {
    Registered(Project),
    Existing(Project),
    Conflict,
}

async fn project_by_id(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    project_id: Uuid,
) -> Result<Option<Project>, PgError> {
    let row = sqlx::query(
        "SELECT project_id, name, issue_prefix, default_issue_type_id, created_by, created_at \
           FROM identity_project \
          WHERE tenant_id = $1 AND region = $2 AND project_id = $3",
    )
    .bind(tenant)
    .bind(region)
    .bind(project_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(query_error("read identity project"))?;
    row.as_ref().map(project_from_row).transpose()
}

async fn project_by_nonce(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    nonce: &str,
) -> Result<Option<Project>, PgError> {
    let row = sqlx::query(
        "SELECT project_id, name, issue_prefix, default_issue_type_id, created_by, created_at \
           FROM identity_project \
          WHERE tenant_id = $1 AND region = $2 AND client_nonce = $3",
    )
    .bind(tenant)
    .bind(region)
    .bind(nonce)
    .fetch_optional(&mut *conn)
    .await
    .map_err(query_error("read project idempotency record"))?;
    row.as_ref().map(project_from_row).transpose()
}

fn project_from_row(row: &sqlx::postgres::PgRow) -> Result<Project, PgError> {
    Ok(Project {
        id: row
            .try_get::<Uuid, _>("project_id")
            .map_err(decode_error("project_id"))?
            .to_string(),
        name: row.try_get("name").map_err(decode_error("project name"))?,
        issue_prefix: row
            .try_get("issue_prefix")
            .map_err(decode_error("project issue prefix"))?,
        default_issue_type_id: row
            .try_get::<Uuid, _>("default_issue_type_id")
            .map_err(decode_error("default issue type"))?
            .to_string(),
        created_by: row
            .try_get("created_by")
            .map_err(decode_error("project creator"))?,
        created_at: row
            .try_get::<DateTime<Utc>, _>("created_at")
            .map_err(decode_error("project creation time"))?
            .to_rfc3339_opts(SecondsFormat::Micros, true),
    })
}

fn query_error(context: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("{context}: {error}"))
}

fn decode_error(context: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("decode {context}: {error}"))
}

pub fn validate_new_project(project: &NewProject) -> Result<(), ProjectError> {
    if project.name.is_empty()
        || project.name.len() > MAX_PROJECT_NAME_BYTES
        || project.name.trim() != project.name
        || project.name.chars().any(char::is_control)
    {
        return Err(ProjectError::BadInput(format!(
            "name must contain 1..={MAX_PROJECT_NAME_BYTES} bytes, without surrounding whitespace or control characters"
        )));
    }
    if project.issue_prefix.len() < 2
        || project.issue_prefix.len() > MAX_PROJECT_PREFIX_BYTES
        || !project
            .issue_prefix
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(ProjectError::BadInput(
            "issue prefix must be 2..=10 uppercase ASCII letters/digits".into(),
        ));
    }
    if project.client_nonce.is_empty()
        || project.client_nonce.len() > 128
        || !project
            .client_nonce
            .bytes()
            .all(|byte| byte.is_ascii_graphic())
    {
        return Err(ProjectError::BadInput(
            "client nonce must be 1..=128 ASCII-graphic bytes".into(),
        ));
    }
    Ok(())
}

pub fn project_ref(tenant: &str, project_id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant}/identity/project/{project_id}"))
}

fn project_created_event(
    actor: &Principal,
    project_id: Uuid,
    default_issue_type_id: Uuid,
    issue_prefix: &str,
    event_id: EventId,
    now: DateTime<Utc>,
) -> myelin_events::EventEnvelope {
    let project_id = project_id.to_string();
    let timestamp = Timestamp(now.to_rfc3339_opts(SecondsFormat::Micros, true));
    derive_envelope(
        EventDraft {
            type_: EventType(IDENTITY_PROJECT_CREATED.into()),
            subject: project_ref(&actor.tenant.0, &project_id),
            aggregate: AggregateKey(format!("identity:project:{project_id}")),
            payload: serde_json::json!({
                "project_id": project_id,
                "issue_prefix": issue_prefix,
                "default_issue_type_id": default_issue_type_id.to_string(),
                "creator_grant": {
                    "relation": PROJECT_WRITER_RELATION,
                    "subject": actor.principal_id.0,
                },
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

fn parse_project_id(value: &str) -> Result<Uuid, ProjectError> {
    parse_canonical_uuid(value, "project id")
}

fn parse_canonical_uuid(value: &str, field: &str) -> Result<Uuid, ProjectError> {
    let parsed = Uuid::parse_str(value)
        .map_err(|_| ProjectError::BadInput(format!("{field} must be a canonical UUID")))?;
    if parsed.to_string() != value {
        return Err(ProjectError::BadInput(format!(
            "{field} must be a canonical UUID"
        )));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(name: &str, prefix: &str) -> NewProject {
        NewProject {
            name: name.into(),
            issue_prefix: prefix.into(),
            client_nonce: "request-v1-safe".into(),
        }
    }

    #[test]
    fn project_inputs_are_small_canonical_and_human_readable() {
        assert!(validate_new_project(&proposal("Developer experience", "DX")).is_ok());
        for invalid in [
            proposal("", "DX"),
            proposal(" padded", "DX"),
            proposal("line\nbreak", "DX"),
            proposal("Developer experience", "dX"),
            proposal("Developer experience", "X"),
            proposal("Developer experience", "TOO-LONG-KEY"),
        ] {
            assert!(
                validate_new_project(&invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn project_refs_are_total_and_parse_canonically() {
        let reference = project_ref("acme", "11111111-1111-1111-1111-111111111111");
        assert_eq!(
            reference.0,
            "myelin://acme/identity/project/11111111-1111-1111-1111-111111111111"
        );
        assert_eq!(myelin_refs::parse(&reference.0).unwrap(), reference);
    }
}
