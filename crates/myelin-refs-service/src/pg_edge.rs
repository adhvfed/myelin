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
        let scope_tenant = tenant.0.clone();
        let scope_region = region.0.clone();
        let tenant_id = scope_tenant.clone();
        let region_id = scope_region.clone();
        let target = target_root.0.clone();
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
                          ORDER BY edge_id
                          LIMIT $4",
                    )
                    .bind(tenant_id)
                    .bind(region_id)
                    .bind(target)
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
    ) -> Result<(), PgEdgeError> {
        let mutation = edge_mutation(event).map_err(PgEdgeError::Malformed)?;
        let connection = tx
            .connection::<sqlx::PgConnection>()
            .ok_or(PgEdgeError::NoCoCommitTransaction)?;
        tokio::task::block_in_place(|| {
            runtime.block_on(project_on_connection(connection, event, mutation))
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
    tenant: TenantId,
    region: Region,
    store: PgEdgeStore,
    runtime: tokio::runtime::Handle,
    subjects: &'static [SubjectPattern],
}

impl PgEdgeProjector {
    fn new(
        tenant: TenantId,
        region: Region,
        store: PgEdgeStore,
        runtime: tokio::runtime::Handle,
        subjects: &'static [SubjectPattern],
    ) -> Self {
        Self {
            tenant,
            region,
            store,
            runtime,
            subjects,
        }
    }
}

impl EventHandler for PgEdgeProjector {
    fn subjects(&self) -> &'static [SubjectPattern] {
        self.subjects
    }

    fn handle(&self, event: &EventEnvelope, tx: &mut HandlerTx<'_>) -> HandleOutcome {
        if event.tenant != self.tenant || event.region != self.region {
            return HandleOutcome::NonRetryable(Reason(
                "Refs event is outside the projector's tenant/region binding".into(),
            ));
        }
        if !event.type_.0.starts_with("refs.edge.") {
            return HandleOutcome::NonRetryable(Reason(
                "Refs projector received an event outside refs.edge.*".into(),
            ));
        }
        match self.store.co_commit_project(tx, event, &self.runtime) {
            Ok(()) => HandleOutcome::Done,
            Err(PgEdgeError::Malformed(error)) => HandleOutcome::NonRetryable(Reason(error.0)),
            Err(PgEdgeError::NoCoCommitTransaction | PgEdgeError::Database) => {
                HandleOutcome::Retry(Backoff {
                    seconds: DATABASE_RETRY_SECONDS,
                })
            }
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
    let subjects: &'static [SubjectPattern] =
        Box::leak(vec![SubjectPattern(artifact_prefix.clone())].into_boxed_slice());
    let projector = PgEdgeProjector::new(tenant.clone(), region.clone(), store, runtime, subjects);
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
    NoCoCommitTransaction,
    Database,
}

async fn project_on_connection(
    connection: &mut sqlx::PgConnection,
    event: &EventEnvelope,
    mutation: EdgeMutation,
) -> Result<(), PgEdgeError> {
    match mutation {
        EdgeMutation::Upsert(row) => upsert(connection, event, &row).await,
        EdgeMutation::Tombstone { edge_id } => sqlx::query(
            "UPDATE edge
                    SET tombstoned = true, origin_event = $3
                  WHERE tenant_id = $1 AND edge_id = $2",
        )
        .bind(&event.tenant.0)
        .bind(edge_id)
        .bind(&event.event_id.0)
        .execute(connection)
        .await
        .map(|_| ())
        .map_err(|_| PgEdgeError::Database),
        EdgeMutation::Ignore => Ok(()),
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
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12::timestamptz,$13,false,$14)
         ON CONFLICT (tenant_id, edge_id) DO UPDATE SET
             region = EXCLUDED.region,
             source = EXCLUDED.source,
             source_root = EXCLUDED.source_root,
             target = EXCLUDED.target,
             target_root = EXCLUDED.target_root,
             rel = EXCLUDED.rel,
             rel_class = EXCLUDED.rel_class,
             origin_event = EXCLUDED.origin_event,
             origin_actor = EXCLUDED.origin_actor,
             created_at = EXCLUDED.created_at,
             zookie = EXCLUDED.zookie,
             tombstoned = false,
             dek_ref = EXCLUDED.dek_ref",
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
    .bind(dek_ref)
    .execute(connection)
    .await
    .map(|_| ())
    .map_err(|_| PgEdgeError::Database)
}
