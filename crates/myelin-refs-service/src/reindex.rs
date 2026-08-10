use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_events::{
    reindex as bus_reindex, snapshot_event_id, AggregateKey, ArtifactRef, DataRole,
    EmitContextBase, EventHandler, EventType, OutboxStore, ReindexError as BusReindexError,
    ReindexSource, SnapshotDraft, SnapshotScope, Visibility,
};
use myelin_tenancy::{Region, TenantId};

use crate::edge_builder::{EdgeProjection, RefsEdgeBuilder};
use crate::mirror::{reconverge, MirrorError, SyntheticTypedEvent};

pub const REFS_OWNER_TOKEN: &str = "refs";

pub const REFS_EDGE_SNAPSHOT_TYPE: &str = "refs.edge.snapshot";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEdge {
    pub aggregate: String,
    pub version: u64,
    pub source: ArtifactRef,
    pub target: ArtifactRef,
    pub rel: String,
    pub origin_actor: String,
    pub zookie: Option<String>,
}

pub struct RefsReindexSource {
    truth: BTreeMap<String, SourceEdge>,
}

impl RefsReindexSource {
    pub fn new() -> RefsReindexSource {
        RefsReindexSource {
            truth: BTreeMap::new(),
        }
    }

    pub fn record(&mut self, edge: SourceEdge) {
        self.truth.insert(edge.aggregate.clone(), edge);
    }

    pub fn erase(&mut self, aggregate: &str) -> bool {
        self.truth.remove(aggregate).is_some()
    }

    pub fn len(&self) -> usize {
        self.truth.len()
    }

    pub fn is_empty(&self) -> bool {
        self.truth.is_empty()
    }
}

impl Default for RefsReindexSource {
    fn default() -> RefsReindexSource {
        RefsReindexSource::new()
    }
}

impl ReindexSource for RefsReindexSource {
    fn owner_token(&self) -> &str {
        REFS_OWNER_TOKEN
    }

    fn replay(&self, _scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        self.truth
            .values()
            .filter(|e| since.is_none_or(|s| e.version > s))
            .map(|e| SnapshotDraft {
                aggregate: AggregateKey(e.aggregate.clone()),
                version: e.version,
                type_: EventType(REFS_EDGE_SNAPSHOT_TYPE.into()),
                subject: e.source.clone(),
                payload: serde_json::json!({
                    "source": e.source.0,
                    "target": e.target.0,
                    "rel": e.rel,
                    "zookie": e.zookie,
                    "origin_actor": e.origin_actor,
                }),
                data_role: DataRole::Processor,
                visibility: Visibility::Internal,
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReindexReceipt {
    pub parity_hash: String,
    pub snapshots_emitted: usize,
    pub snapshots_skipped_duplicate: usize,
    pub ingested: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReindexError {
    Bus(String),
    Poison(String),
    Mirror(String),
}

impl std::fmt::Display for ReindexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReindexError::Bus(e) => write!(f, "refs reindex: bus seam failed: {e}"),
            ReindexError::Poison(e) => write!(f, "refs reindex: poison snapshot: {e}"),
            ReindexError::Mirror(e) => write!(f, "refs reindex: typed reconvergence failed: {e}"),
        }
    }
}

impl std::error::Error for ReindexError {}

impl From<BusReindexError> for ReindexError {
    fn from(e: BusReindexError) -> ReindexError {
        ReindexError::Bus(e.to_string())
    }
}

impl From<MirrorError> for ReindexError {
    fn from(e: MirrorError) -> ReindexError {
        match e {
            MirrorError::UnknownRel(r) => {
                ReindexError::Mirror(format!("unknown lifecycle rel `{r}`"))
            }
        }
    }
}

#[derive(Clone)]
pub struct RefsReindexer {
    builder: RefsEdgeBuilder,
    reindex_parity: Arc<AtomicU64>,
}

impl RefsReindexer {
    pub const REINDEX_PARITY_SIGNAL: &'static str = "refs.reindex_parity";

    pub fn new(builder: RefsEdgeBuilder) -> RefsReindexer {
        RefsReindexer {
            builder,
            reindex_parity: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn builder(&self) -> &RefsEdgeBuilder {
        &self.builder
    }

    pub fn projection(&self) -> &EdgeProjection {
        self.builder.projection()
    }

    pub fn reindex_parity(&self) -> u64 {
        self.reindex_parity.load(Ordering::SeqCst)
    }

    pub fn reindex(
        &self,
        scope: &SnapshotScope,
        since: Option<u64>,
        source: &dyn ReindexSource,
        outbox: &mut OutboxStore,
        ctx_base: EmitContextBase,
    ) -> Result<ReindexReceipt, ReindexError> {
        let tenant = &ctx_base.tenant;
        let region = &ctx_base.region;
        let sources: &[&dyn ReindexSource] = &[source];
        let bus_receipt = bus_reindex(scope, since, sources, outbox, ctx_base.clone())?;

        if since.is_none() {
            self.projection().wipe_partition(tenant, region);
        }

        let drafts = source.replay(scope, since);
        let mut ingested = 0usize;
        for draft in &drafts {
            let id = snapshot_event_id(&draft.aggregate, draft.version);
            let row = outbox.row(&id).ok_or_else(|| {
                ReindexError::Bus(format!("snapshot row {} absent after emit", id.0))
            })?;
            match self
                .builder
                .handle(&row.envelope, &mut myelin_events::HandlerTx::none())
            {
                myelin_events::HandleOutcome::Done => ingested += 1,
                myelin_events::HandleOutcome::NonRetryable(myelin_events::Reason(r)) => {
                    return Err(ReindexError::Poison(r));
                }
                myelin_events::HandleOutcome::Retry(_)
                | myelin_events::HandleOutcome::DependencyUnavailable { .. } => {
                    return Err(ReindexError::Poison(format!(
                        "unexpected retryable outcome ingesting snapshot {}",
                        id.0
                    )));
                }
            }
        }

        let parity_hash = self.projection().parity_hash(tenant, region);
        Ok(ReindexReceipt {
            parity_hash,
            snapshots_emitted: bus_receipt.snapshots_emitted,
            snapshots_skipped_duplicate: bus_receipt.snapshots_skipped_duplicate,
            ingested,
        })
    }

    pub fn reconverge_typed(
        &self,
        tenant: &TenantId,
        region: &Region,
        typed_snapshot: &[SyntheticTypedEvent],
        covered_roots: &[ArtifactRef],
        reindex_event_id: &str,
    ) -> Result<(usize, usize), ReindexError> {
        Ok(reconverge(
            self.projection(),
            tenant,
            region,
            typed_snapshot,
            covered_roots,
            reindex_event_id,
        )?)
    }

    pub fn verify_parity(&self, live: &EdgeProjection, tenant: &TenantId, region: &Region) -> bool {
        let rebuilt = self.projection().parity_hash(tenant, region);
        let reference = live.parity_hash(tenant, region);
        let matched = rebuilt == reference;
        self.reindex_parity
            .store(u64::from(matched), Ordering::SeqCst);
        matched
    }
}

#[cfg(test)]
mod tests;
