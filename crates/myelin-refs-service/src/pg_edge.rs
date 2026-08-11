use std::sync::Arc;

use myelin_events::{
    consume, Backoff, Consumer, ConsumerName, ConsumerSpec, DedupLedger, EventEnvelope,
    EventHandler, HandleOutcome, HandlerTx, Reason, SubjectPattern, SubscribeError,
};
use myelin_storage::PiiKeyRef;
use myelin_tenancy::{Region, TenantId};
use sqlx::PgPool;

use crate::edge_builder::{edge_mutation, EdgeMutation, EdgeRow, ProjectError, RelClass};

const DATABASE_RETRY_SECONDS: u64 = 2;

#[derive(Clone)]
pub struct PgEdgeStore {
    pool: PgPool,
}

impl PgEdgeStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn inbound_live(
        &self,
        tenant: &TenantId,
        region: &Region,
        target_root: &myelin_events::ArtifactRef,
        limit: u32,
    ) -> Result<Vec<StoredBacklink>, myelin_storage::PgError> {
        self.inbound_live_after(tenant, region, target_root, None, limit)
            .await
    }

    pub async fn inbound_live_after(
        &self,
        tenant: &TenantId,
        region: &Region,
        target_root: &myelin_events::ArtifactRef,
        after_edge_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StoredBacklink>, myelin_storage::PgError> {
        let scope_tenant = tenant.0.clone();
        let scope_region = region.0.clone();
        let tenant_id = scope_tenant.clone();
        let region_id = scope_region.clone();
        let target = target_root.0.clone();
        let after_edge_id = after_edge_id.map(str::to_string);
        myelin_storage::with_tenant_tx(
            &self.pool,
            &scope_tenant,
            &scope_region,
            move |connection| {
                Box::pin(async move {
                    sqlx::query_as::<_, (String, String, String, String, String, String, String, String)>(
                        "SELECT edge_id, source, source_root, target, target_root, rel, rel_class, \
                                origin_actor
                           FROM edge
                          WHERE tenant_id = $1 AND region = $2 AND target_root = $3
                            AND NOT tombstoned
                            AND ($4::text IS NULL OR edge_id > $4)
                          ORDER BY edge_id
                          LIMIT $5",
                    )
                    .bind(tenant_id)
                    .bind(region_id)
                    .bind(target)
                    .bind(after_edge_id)
                    .bind(i64::from(limit))
                    .fetch_all(connection)
                    .await
                    .map(|rows| {
                        rows.into_iter()
                            .map(
                                |(
                                    edge_id,
                                    source,
                                    source_root,
                                    target,
                                    target_root,
                                    rel,
                                    rel_class,
                                    origin_actor,
                                )| StoredBacklink {
                                    edge_id,
                                    source,
                                    source_root,
                                    target,
                                    target_root,
                                    rel,
                                    rel_class,
                                    origin_actor,
                                },
                            )
                            .collect()
                    })
                    .map_err(|error| {
                        myelin_storage::PgError::Query(format!(
                            "read live inbound reference edges: {error}"
                        ))
                    })
                })
            },
        )
        .await
    }

    fn co_commit_project(
        &self,
        tx: &mut HandlerTx<'_>,
        event: &EventEnvelope,
        runtime: &tokio::runtime::Handle,
        cell_id: Option<&str>,
    ) -> Result<(), PgEdgeError> {
        let mutation = edge_mutation(event).map_err(PgEdgeError::Malformed)?;
        let connection = tx
            .connection::<sqlx::PgConnection>()
            .ok_or(PgEdgeError::NoCoCommitTransaction)?;
        tokio::task::block_in_place(|| {
            runtime.block_on(project_on_connection(connection, event, mutation, cell_id))
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredBacklink {
    pub edge_id: String,
    pub source: String,
    pub source_root: String,
    pub target: String,
    pub target_root: String,
    pub rel: String,
    pub rel_class: String,
    pub origin_actor: String,
}

#[derive(Clone)]
pub struct PgEdgeProjector {
    scope: ProjectorScope,
    region: Region,
    store: PgEdgeStore,
    runtime: tokio::runtime::Handle,
    subjects: Vec<SubjectPattern>,
}

#[derive(Clone)]
enum ProjectorScope {
    Tenant(TenantId),
    Cell(String),
}

impl PgEdgeProjector {
    fn new(
        scope: ProjectorScope,
        region: Region,
        store: PgEdgeStore,
        runtime: tokio::runtime::Handle,
        subjects: Vec<SubjectPattern>,
    ) -> Self {
        Self {
            scope,
            region,
            store,
            runtime,
            subjects,
        }
    }
}

impl EventHandler for PgEdgeProjector {
    fn subjects(&self) -> &[SubjectPattern] {
        &self.subjects
    }

    fn handle(&self, event: &EventEnvelope, tx: &mut HandlerTx<'_>) -> HandleOutcome {
        if event.region != self.region
            || matches!(&self.scope, ProjectorScope::Tenant(tenant) if event.tenant != *tenant)
        {
            return HandleOutcome::NonRetryable(Reason(
                "Refs event is outside the projector's tenant/region binding".into(),
            ));
        }
        if !event.type_.0.starts_with("refs.edge.") {
            return HandleOutcome::NonRetryable(Reason(
                "Refs projector received an event outside refs.edge.*".into(),
            ));
        }
        let cell_id = match &self.scope {
            ProjectorScope::Tenant(_) => None,
            ProjectorScope::Cell(cell_id) => Some(cell_id.as_str()),
        };
        match self
            .store
            .co_commit_project(tx, event, &self.runtime, cell_id)
        {
            Ok(()) => HandleOutcome::Done,
            Err(PgEdgeError::Malformed(error)) => HandleOutcome::NonRetryable(Reason(error.0)),
            Err(PgEdgeError::IdentityCollision(edge_id)) => HandleOutcome::NonRetryable(Reason(
                format!("reference edge identity collision for `{edge_id}`"),
            )),
            Err(
                PgEdgeError::TenantNotActiveInCell
                | PgEdgeError::NoCoCommitTransaction
                | PgEdgeError::Database,
            ) => HandleOutcome::Retry(Backoff {
                seconds: DATABASE_RETRY_SECONDS,
            }),
        }
    }
}

pub fn build_pg_edge_consumer(
    tenant: &TenantId,
    region: &Region,
    store: PgEdgeStore,
    dedup: DedupLedger,
    dead_letters: Arc<dyn myelin_events::DurableDeadLetter>,
    runtime: tokio::runtime::Handle,
) -> Result<Consumer<PgEdgeProjector>, SubscribeError> {
    let artifact_prefix = format!("myelin://{}/", tenant.0);
    let subjects = vec![SubjectPattern(artifact_prefix.clone())];
    let projector = PgEdgeProjector::new(
        ProjectorScope::Tenant(tenant.clone()),
        region.clone(),
        store,
        runtime,
        subjects,
    );
    build_consumer(projector, artifact_prefix, dedup, dead_letters)
}

pub fn build_pg_cell_edge_consumer(
    cell_id: &str,
    region: &Region,
    store: PgEdgeStore,
    dedup: DedupLedger,
    dead_letters: Arc<dyn myelin_events::DurableDeadLetter>,
    runtime: tokio::runtime::Handle,
) -> Result<Consumer<PgEdgeProjector>, SubscribeError> {
    let artifact_prefix = "myelin://".to_string();
    let subjects = vec![SubjectPattern(artifact_prefix.clone())];
    let projector = PgEdgeProjector::new(
        ProjectorScope::Cell(cell_id.to_string()),
        region.clone(),
        store,
        runtime,
        subjects,
    );
    build_consumer(projector, artifact_prefix, dedup, dead_letters)
}

fn build_consumer(
    projector: PgEdgeProjector,
    artifact_prefix: String,
    dedup: DedupLedger,
    dead_letters: Arc<dyn myelin_events::DurableDeadLetter>,
) -> Result<Consumer<PgEdgeProjector>, SubscribeError> {
    consume(
        ConsumerSpec::new(
            ConsumerName(crate::edge_builder::EDGE_BUILDER_CONSUMER.into()),
            &[artifact_prefix.as_str()],
        ),
        projector,
        dedup,
    )
    .map(|consumer| {
        consumer.with_dead_letter_sink(myelin_events::DeadLetterSink::durable(dead_letters))
    })
}

#[derive(Debug)]
enum PgEdgeError {
    Malformed(ProjectError),
    IdentityCollision(String),
    TenantNotActiveInCell,
    NoCoCommitTransaction,
    Database,
}

async fn project_on_connection(
    connection: &mut sqlx::PgConnection,
    event: &EventEnvelope,
    mutation: EdgeMutation,
    cell_id: Option<&str>,
) -> Result<(), PgEdgeError> {
    if let Some(cell_id) = cell_id {
        ensure_tenant_is_active_in_cell(connection, cell_id, &event.tenant).await?;
    }
    match mutation {
        EdgeMutation::Apply(rows) => {
            for row in rows {
                upsert(&mut *connection, event, &row).await?;
            }
            Ok(())
        }
        EdgeMutation::TombstoneIds(edge_ids) => {
            for edge_id in edge_ids {
                sqlx::query(
                    "UPDATE edge
                        SET tombstoned = true, origin_event = $4, created_at = $5::timestamptz
                      WHERE tenant_id = $1 AND region = $2 AND edge_id = $3
                        AND (created_at, origin_event) <= ($5::timestamptz, $4)",
                )
                .bind(&event.tenant.0)
                .bind(&event.region.0)
                .bind(edge_id)
                .bind(&event.event_id.0)
                .bind(&event.recorded_at.0)
                .execute(&mut *connection)
                .await
                .map_err(|_| PgEdgeError::Database)?;
            }
            Ok(())
        }
        EdgeMutation::Ignore => Ok(()),
    }
}

async fn ensure_tenant_is_active_in_cell(
    connection: &mut sqlx::PgConnection,
    cell_id: &str,
    tenant: &TenantId,
) -> Result<(), PgEdgeError> {
    let active = sqlx::query_scalar::<_, bool>(
        "SELECT active
           FROM local_tenant
          WHERE cell_id = $1 AND tenant_id = $2
          FOR SHARE",
    )
    .bind(cell_id)
    .bind(&tenant.0)
    .fetch_optional(connection)
    .await
    .map_err(|_| PgEdgeError::Database)?;
    match active {
        Some(true) => Ok(()),
        Some(false) | None => Err(PgEdgeError::TenantNotActiveInCell),
    }
}

async fn upsert(
    connection: &mut sqlx::PgConnection,
    event: &EventEnvelope,
    row: &EdgeRow,
) -> Result<(), PgEdgeError> {
    let dek_ref =
        PiiKeyRef::new(event.tenant.clone(), 0, myelin_storage::KeyClass::Tenant).to_uri();
    sqlx::query(
        "INSERT INTO edge (
             tenant_id, region, edge_id, source, source_root, target, target_root, rel,
             rel_class, origin_event, origin_actor, created_at, zookie, tombstoned, dek_ref
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12::timestamptz,$13,$14,$15)
         ON CONFLICT (tenant_id, region, source, target, rel) DO UPDATE SET
             source_root = EXCLUDED.source_root,
             target_root = EXCLUDED.target_root,
             rel_class = EXCLUDED.rel_class,
             origin_event = EXCLUDED.origin_event,
             origin_actor = EXCLUDED.origin_actor,
             created_at = EXCLUDED.created_at,
             zookie = EXCLUDED.zookie,
             tombstoned = EXCLUDED.tombstoned,
             dek_ref = EXCLUDED.dek_ref
         WHERE (edge.created_at, edge.origin_event)
            <= (EXCLUDED.created_at, EXCLUDED.origin_event)",
    )
    .bind(&event.tenant.0)
    .bind(&event.region.0)
    .bind(&row.edge_id)
    .bind(&row.source.0)
    .bind(&row.source_root.0)
    .bind(&row.target.0)
    .bind(&row.target_root.0)
    .bind(&row.rel)
    .bind(match row.rel_class {
        RelClass::Reference => "reference",
        RelClass::Lifecycle => "lifecycle",
    })
    .bind(&row.origin_event)
    .bind(&row.origin_actor)
    .bind(&event.recorded_at.0)
    .bind(&row.zookie)
    .bind(row.tombstoned)
    .bind(dek_ref)
    .execute(connection)
    .await
    .map(|_| ())
    .map_err(|error| {
        let identity_collision = error.as_database_error().is_some_and(|database| {
            database.code().is_some_and(|code| code == "23505")
                && database.constraint() == Some("edge_region_identity")
        });
        if identity_collision {
            PgEdgeError::IdentityCollision(row.edge_id.clone())
        } else {
            PgEdgeError::Database
        }
    })
}
