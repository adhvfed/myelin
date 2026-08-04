use myelin_events::{EmitContextBase, EventHandler, OutboxStore, SnapshotScope};
use myelin_tenancy::{Region, TenantId};

use crate::edge_builder::{EdgeProjection, RefsEdgeBuilder};
use crate::mirror::{project_typed_event, LifecycleRel, SyntheticTypedEvent};
use crate::reindex::{
    RefsReindexSource, RefsReindexer, ReindexError, SourceEdge, REFS_OWNER_TOKEN,
};
use myelin_refs::ArtifactRef;

pub const WORLD_SCALE_FLEET_LOAD_FLOOR: &str =
    "REF-D4 at full 30x world-scale cardinality over the PgStore-backed edge index on real fleet \
     hardware (the ONE legitimate remaining floor); the byte-parity property + both-mirror \
     reconvergence are proven here over a deterministic scaled corpus";

pub const FIVE_PRODUCERS: [&str; 5] = ["git", "knowledge", "ci", "chat", "issue"];

#[derive(Clone, Debug)]
pub struct FiveProducerCorpus {
    pub reference_edges: Vec<SourceEdge>,
    pub page_parent_snapshot: Vec<SyntheticTypedEvent>,
    pub page_parent_roots: Vec<ArtifactRef>,
    pub issue_relation_snapshot: Vec<SyntheticTypedEvent>,
    pub issue_relation_roots: Vec<ArtifactRef>,
}

impl FiveProducerCorpus {
    pub fn reference_count(&self) -> usize {
        self.reference_edges.len()
    }

    pub fn mirror_event_count(&self) -> usize {
        self.page_parent_snapshot.len() + self.issue_relation_snapshot.len()
    }
}

pub fn build_full_scale_corpus(tenant: &str, scale: usize) -> FiveProducerCorpus {
    assert!(scale > 0, "the full-scale corpus must be non-empty");
    let reference_rels = ["mentions", "links", "embeds"];

    let mut reference_edges = Vec::with_capacity(FIVE_PRODUCERS.len() * scale);
    for (p_idx, producer) in FIVE_PRODUCERS.iter().enumerate() {
        for i in 0..scale {
            let rel = reference_rels[(p_idx + i) % reference_rels.len()];
            let source = format!("myelin://{tenant}/{producer}/artifact/{producer}-src-{i}");
            let target = format!("myelin://{tenant}/{producer}/artifact/{producer}-tgt-{i}");
            reference_edges.push(SourceEdge {
                aggregate: format!("refs.edge:{producer}:{i}"),
                version: 1,
                source: ArtifactRef(source),
                target: ArtifactRef(target),
                rel: rel.into(),
                origin_actor: format!("p-opaque-{producer}-{}", i % 7),
                zookie: Some(format!("zk-{producer}-{i}")),
            });
        }
    }

    let mut page_parent_snapshot = Vec::with_capacity(scale);
    let mut page_parent_roots = Vec::with_capacity(scale);
    for i in 0..scale {
        let parent = ArtifactRef(format!("myelin://{tenant}/knowledge/page/page-{}", i / 4));
        let child = ArtifactRef(format!("myelin://{tenant}/knowledge/page/page-{i}"));
        page_parent_snapshot.push(SyntheticTypedEvent {
            source: parent.clone(),
            target: child.clone(),
            rel: LifecycleRel::Parent,
            origin_event: format!("page_parent-{i}"),
            origin_actor: format!("p-opaque-knowledge-{}", i % 7),
            zookie: None,
        });
        page_parent_roots.push(strip_sub(&child));
        page_parent_roots.push(strip_sub(&parent));
    }

    let mut issue_relation_snapshot = Vec::with_capacity(scale);
    let mut issue_relation_roots = Vec::with_capacity(scale);
    for i in 0..scale {
        let rel = if i % 2 == 0 {
            LifecycleRel::Blocks
        } else {
            LifecycleRel::Relates
        };
        let source = ArtifactRef(format!("myelin://{tenant}/issue/issue/ENG-{i}"));
        let target = ArtifactRef(format!("myelin://{tenant}/issue/issue/ENG-{}", i + scale));
        issue_relation_snapshot.push(SyntheticTypedEvent {
            source: source.clone(),
            target: target.clone(),
            rel,
            origin_event: format!("issue_relation-{i}"),
            origin_actor: format!("p-opaque-issue-{}", i % 7),
            zookie: None,
        });
        issue_relation_roots.push(strip_sub(&target));
        issue_relation_roots.push(strip_sub(&source));
    }

    FiveProducerCorpus {
        reference_edges,
        page_parent_snapshot,
        page_parent_roots,
        issue_relation_snapshot,
        issue_relation_roots,
    }
}

fn strip_sub(r: &ArtifactRef) -> ArtifactRef {
    match r.0.split_once('#') {
        Some((root, _)) => ArtifactRef(root.to_string()),
        None => r.clone(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullScaleParityReport {
    pub parity_matched: bool,
    pub parity_hash: String,
    pub reference_edges: usize,
    pub mirror_events: usize,
    pub reference_ingested: usize,
    pub page_parent_reprojected: usize,
    pub issue_relation_reprojected: usize,
    pub reindex_parity_signal: u64,
}

impl FullScaleParityReport {
    pub fn is_ref_d4_full_scale_green(&self) -> bool {
        self.parity_matched
            && self.reindex_parity_signal == 1
            && self.page_parent_reprojected > 0
            && self.issue_relation_reprojected > 0
    }

    pub fn summary(&self) -> String {
        format!(
            "REF-D4 full-scale: rebuilt==live={} parity={} refs={} (ingested {}) mirrors={} \
             (page_parent reproj {}, issue_relation reproj {}) reindex_parity={}",
            self.parity_matched,
            self.parity_hash,
            self.reference_edges,
            self.reference_ingested,
            self.mirror_events,
            self.page_parent_reprojected,
            self.issue_relation_reprojected,
            self.reindex_parity_signal,
        )
    }
}

pub fn run_full_scale_reindex_parity(
    tenant: &TenantId,
    region: &Region,
    corpus: &FiveProducerCorpus,
    ctx_base: EmitContextBase,
) -> Result<FullScaleParityReport, ReindexError> {
    let live = RefsEdgeBuilder::new(EdgeProjection::new());
    let mut truth = RefsReindexSource::new();
    for edge in &corpus.reference_edges {
        truth.record(edge.clone());
        live.handle(&live_reference_event(tenant, region, edge, &ctx_base), &mut myelin_events::HandlerTx::none());
    }
    let live_proj = live.projection();
    for ev in &corpus.page_parent_snapshot {
        project_typed_event(live_proj, tenant, region, ev)?;
    }
    for ev in &corpus.issue_relation_snapshot {
        project_typed_event(live_proj, tenant, region, ev)?;
    }
    let live_snapshot = live_proj.clone();

    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    let scope = SnapshotScope::new(REFS_OWNER_TOKEN, "edge:all");
    let mut outbox = OutboxStore::new();
    let receipt = reindexer.reindex(&scope, None, &truth, &mut outbox, ctx_base)?;

    let (page_parent_reprojected, _) = reindexer.reconverge_typed(
        tenant,
        region,
        &corpus.page_parent_snapshot,
        &corpus.page_parent_roots,
        "reindex-full-scale-page-parent",
    )?;
    let (issue_relation_reprojected, _) = reindexer.reconverge_typed(
        tenant,
        region,
        &corpus.issue_relation_snapshot,
        &corpus.issue_relation_roots,
        "reindex-full-scale-issue-relation",
    )?;

    let parity_matched = reindexer.verify_parity(&live_snapshot, tenant, region);
    let parity_hash = reindexer.projection().parity_hash(tenant, region);

    Ok(FullScaleParityReport {
        parity_matched,
        parity_hash,
        reference_edges: corpus.reference_count(),
        mirror_events: corpus.mirror_event_count(),
        reference_ingested: receipt.ingested,
        page_parent_reprojected,
        issue_relation_reprojected,
        reindex_parity_signal: reindexer.reindex_parity(),
    })
}

fn live_reference_event(
    tenant: &TenantId,
    region: &Region,
    edge: &SourceEdge,
    ctx_base: &EmitContextBase,
) -> myelin_events::EventEnvelope {
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    myelin_events::EventEnvelope {
        event_id: EventId(format!("live-{}-{}", edge.aggregate, edge.version)),
        type_: EventType("refs.edge.created".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: region.clone(),
        actor: Actor(Principal::stub(
            PrincipalId(edge.origin_actor.clone()),
            PrincipalKind::Human,
            tenant.clone(),
        )),
        subject: edge.source.clone(),
        aggregate: AggregateKey(edge.aggregate.clone()),
        causation_id: None,
        correlation_id: CorrelationId(format!("live-{}", edge.aggregate)),
        caused_by: None,
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: ctx_base.occurred_at.clone(),
        recorded_at: ctx_base.recorded_at.clone(),
        payload: serde_json::json!({
            "source": edge.source.0,
            "target": edge.target.0,
            "rel": edge.rel,
            "zookie": edge.zookie,
            "origin_actor": edge.origin_actor,
        }),
    }
}

#[cfg(test)]
mod tests;
