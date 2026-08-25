use super::{IssueAuthorizer, IssueStoreError, PgIssueStore};
use myelin_identity::Principal;
use myelin_storage::PgError;
use sqlx::{PgConnection, Row};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueViewProjectionRevision {
    pub revision: i64,
    pub effective_grants: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueViewRebuildOutcome {
    Published(IssueViewProjectionRevision),
    Superseded {
        attempted_revision: i64,
        current_revision: i64,
    },
}

impl<A: IssueAuthorizer> PgIssueStore<A> {
    pub async fn rebuild_effective_issue_view(
        &self,
        worker: &Principal,
    ) -> Result<IssueViewRebuildOutcome, IssueStoreError> {
        self.rebuild_effective_issue_view_inner(worker, None).await
    }

    #[cfg(any(test, feature = "test-support"))]
    pub async fn rebuild_effective_issue_view_paused_before_publish_for_test(
        &self,
        worker: &Principal,
        pause: std::time::Duration,
        snapshot_staged: Arc<tokio::sync::Notify>,
    ) -> Result<IssueViewRebuildOutcome, IssueStoreError> {
        self.rebuild_effective_issue_view_inner(worker, Some((pause, snapshot_staged)))
            .await
    }

    async fn rebuild_effective_issue_view_inner(
        &self,
        worker: &Principal,
        pause_before_publish: Option<(std::time::Duration, Arc<tokio::sync::Notify>)>,
    ) -> Result<IssueViewRebuildOutcome, IssueStoreError> {
        let scope = self.scope(worker)?;
        let tenant_id = scope.tenant().0.clone();
        let region = scope.region().0.clone();

        self.provider
            .with_tenant_tx(&tenant_id.clone(), move |connection| {
                Box::pin(async move {
                    let snapshot_revision =
                        stage_visibility_snapshot(connection, &tenant_id, &region).await?;

                    if let Some((pause, snapshot_staged)) = pause_before_publish {
                        snapshot_staged.notify_one();
                        tokio::time::sleep(pause).await;
                    }

                    publish_visibility_snapshot(connection, &tenant_id, &region, snapshot_revision)
                        .await
                })
            })
            .await
            .map_err(|error| projection_error(error.to_string()))
    }

    pub async fn effective_issue_view_lag(
        &self,
        worker: &Principal,
    ) -> Result<Option<i64>, IssueStoreError> {
        let scope = self.scope(worker)?;
        let tenant_id = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        self.provider
            .with_tenant_tx(&tenant_id.clone(), move |connection| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT source_revision, applied_revision, status \
                         FROM authz_projection_state \
                         WHERE tenant_id = $1 AND region = $2 AND projection = 'issue:view'",
                    )
                    .bind(&tenant_id)
                    .bind(&region)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(query_error)?;
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
}

async fn stage_visibility_snapshot(
    connection: &mut PgConnection,
    tenant_id: &str,
    region: &str,
) -> Result<i64, PgError> {
    ensure_projection_state(connection, tenant_id, region).await?;

    let revision = sqlx::query_scalar(
        "SELECT source_revision FROM authz_projection_state \
         WHERE tenant_id = $1 AND region = $2 AND projection = 'issue:view'",
    )
    .bind(tenant_id)
    .bind(region)
    .fetch_one(&mut *connection)
    .await
    .map_err(query_error)?;

    let stage_walk_sql = format!(
        "CREATE TEMP TABLE issue_view_rebuild_walk ON COMMIT DROP AS \
         {ISSUE_VIEW_WALK_CTE} \
         SELECT issue_id, arm, object_id, relation, depth, supported \
         FROM walk"
    );
    sqlx::query(&stage_walk_sql)
        .bind(tenant_id)
        .bind(region)
        .execute(&mut *connection)
        .await
        .map_err(query_error)?;

    reject_unsupported_usersets(connection).await?;

    let stage_effective_sql = format!(
        "CREATE TEMP TABLE issue_view_rebuild_effective ON COMMIT DROP AS \
         WITH walk AS MATERIALIZED (\
           SELECT issue_id, arm, object_id, relation, supported \
           FROM pg_temp.issue_view_rebuild_walk\
         ) \
         {ISSUE_VIEW_MEMBERS_CTE} \
         SELECT issue_id, subject FROM effective"
    );
    sqlx::query(&stage_effective_sql)
        .bind(tenant_id)
        .bind(region)
        .execute(&mut *connection)
        .await
        .map_err(query_error)?;

    Ok(revision)
}

async fn ensure_projection_state(
    connection: &mut PgConnection,
    tenant_id: &str,
    region: &str,
) -> Result<(), PgError> {
    sqlx::query(
        "INSERT INTO authz_projection_state \
           (tenant_id, region, projection, source_revision, applied_revision, status) \
         VALUES ($1, $2, 'issue:view', 1, 0, 'pending') \
         ON CONFLICT (tenant_id, region, projection) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(region)
    .execute(&mut *connection)
    .await
    .map_err(query_error)?;
    Ok(())
}

async fn reject_unsupported_usersets(connection: &mut PgConnection) -> Result<(), PgError> {
    let unsupported: Option<String> = sqlx::query_scalar(
        "SELECT object_id || '#' || relation \
           FROM pg_temp.issue_view_rebuild_walk \
          WHERE NOT supported OR depth >= 16 \
          ORDER BY depth, object_id LIMIT 1",
    )
    .fetch_optional(&mut *connection)
    .await
    .map_err(query_error)?;

    match unsupported {
        Some(node) => Err(PgError::Query(format!(
            "unsupported or over-depth userset in issue:view rebuild: {node}"
        ))),
        None => Ok(()),
    }
}

async fn publish_visibility_snapshot(
    connection: &mut PgConnection,
    tenant_id: &str,
    region: &str,
    snapshot_revision: i64,
) -> Result<IssueViewRebuildOutcome, PgError> {
    let current_revision: i64 = sqlx::query_scalar(
        "SELECT source_revision FROM authz_projection_state \
         WHERE tenant_id = $1 AND region = $2 AND projection = 'issue:view' \
         FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(region)
    .fetch_one(&mut *connection)
    .await
    .map_err(query_error)?;

    if current_revision != snapshot_revision {
        return Ok(IssueViewRebuildOutcome::Superseded {
            attempted_revision: snapshot_revision,
            current_revision,
        });
    }

    mark_projection_rebuilding(connection, tenant_id, region).await?;
    replace_visible_grants(connection, tenant_id, region, snapshot_revision).await
}

async fn mark_projection_rebuilding(
    connection: &mut PgConnection,
    tenant_id: &str,
    region: &str,
) -> Result<(), PgError> {
    sqlx::query(
        "UPDATE authz_projection_state SET status = 'rebuilding', rebuilt_at = NULL \
         WHERE tenant_id = $1 AND region = $2 AND projection = 'issue:view'",
    )
    .bind(tenant_id)
    .bind(region)
    .execute(&mut *connection)
    .await
    .map_err(query_error)?;
    Ok(())
}

async fn replace_visible_grants(
    connection: &mut PgConnection,
    tenant_id: &str,
    region: &str,
    revision: i64,
) -> Result<IssueViewRebuildOutcome, PgError> {
    sqlx::query(
        "DELETE FROM issue_authz_visible \
         WHERE tenant_id = $1 AND region = $2 AND projection = 'issue:view'",
    )
    .bind(tenant_id)
    .bind(region)
    .execute(&mut *connection)
    .await
    .map_err(query_error)?;

    let effective_grants = sqlx::query(
        "INSERT INTO issue_authz_visible \
           (tenant_id, region, projection, subject, permission, object_type, \
            object_id, revision) \
         SELECT $1, $2, 'issue:view', subject, 'view', 'issue', issue_id, $3 \
         FROM pg_temp.issue_view_rebuild_effective",
    )
    .bind(tenant_id)
    .bind(region)
    .bind(revision)
    .execute(&mut *connection)
    .await
    .map_err(query_error)?
    .rows_affected();

    let published = sqlx::query(
        "UPDATE authz_projection_state \
         SET applied_revision = $3, status = 'ready', rebuilt_at = now() \
         WHERE tenant_id = $1 AND region = $2 AND projection = 'issue:view' \
           AND source_revision = $3",
    )
    .bind(tenant_id)
    .bind(region)
    .bind(revision)
    .execute(&mut *connection)
    .await
    .map_err(query_error)?;

    if published.rows_affected() != 1 {
        return Err(PgError::Query(
            "issue:view source revision changed during locked rebuild".into(),
        ));
    }

    Ok(IssueViewRebuildOutcome::Published(
        IssueViewProjectionRevision {
            revision,
            effective_grants,
        },
    ))
}

fn query_error(error: sqlx::Error) -> PgError {
    PgError::Query(error.to_string())
}

fn projection_error(reason: String) -> IssueStoreError {
    if reason.contains("unsupported or over-depth userset") {
        IssueStoreError::AuthorizationUnavailable(reason)
    } else {
        IssueStoreError::Storage(reason)
    }
}

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
      AND (t.expires_at IS NULL OR t.expires_at > CURRENT_TIMESTAMP)
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
     AND (t.expires_at IS NULL OR t.expires_at > CURRENT_TIMESTAMP)
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
   AND (t.expires_at IS NULL OR t.expires_at > CURRENT_TIMESTAMP)
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
