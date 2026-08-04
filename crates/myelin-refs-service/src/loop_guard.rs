use myelin_events::{ArtifactRef, EventEnvelope, EventId, OutboxTx, Result};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::emit::{emit_edges, EdgeRel};

pub const CAUSAL_DEPTH_CEILING: u32 = 12;

pub fn stamped_depth(content_event: &EventEnvelope) -> u32 {
    content_event.depth.saturating_add(1)
}

pub fn would_exceed_ceiling(content_event: &EventEnvelope, ceiling: u32) -> bool {
    stamped_depth(content_event) >= ceiling
}

pub fn is_retrigger_source(rel: EdgeRel) -> bool {
    match rel {
        EdgeRel::Links | EdgeRel::Embeds => true,
        EdgeRel::Mentions => false,
    }
}

pub fn target_is_structured_node(target: &ArtifactRef) -> bool {
    target.0.starts_with("myelin://") && target.0.len() > "myelin://".len()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuardDecision {
    Emitted {
        ids: Vec<EventId>,
        stamped_depth: u32,
    },
    CeilingParked {
        would_be_depth: u32,
    },
}

#[derive(Debug)]
pub struct RefsLoopGuard {
    causal_depth_max: Arc<AtomicU32>,
    ceiling_tripwire_firings: Arc<AtomicU64>,
    ceiling: u32,
}

impl Default for RefsLoopGuard {
    fn default() -> RefsLoopGuard {
        RefsLoopGuard::new()
    }
}

impl RefsLoopGuard {
    pub const CAUSAL_DEPTH_SIGNAL: &'static str = "bus.causal_depth_max";

    pub fn new() -> RefsLoopGuard {
        RefsLoopGuard::with_ceiling(CAUSAL_DEPTH_CEILING)
    }

    pub fn with_ceiling(ceiling: u32) -> RefsLoopGuard {
        RefsLoopGuard {
            causal_depth_max: Arc::new(AtomicU32::new(0)),
            ceiling_tripwire_firings: Arc::new(AtomicU64::new(0)),
            ceiling,
        }
    }

    pub fn ceiling(&self) -> u32 {
        self.ceiling
    }

    pub fn causal_depth_max(&self) -> u32 {
        self.causal_depth_max.load(Ordering::SeqCst)
    }

    pub fn ceiling_tripwire_firings(&self) -> u64 {
        self.ceiling_tripwire_firings.load(Ordering::SeqCst)
    }

    pub fn guarded_emit_edges(
        &self,
        tx: &mut dyn OutboxTx,
        source: &ArtifactRef,
        doc: &[myelin_content::InlineNode],
        content_event: &EventEnvelope,
    ) -> Result<GuardDecision> {
        let would_be_depth = stamped_depth(content_event);

        if would_exceed_ceiling(content_event, self.ceiling) {
            self.ceiling_tripwire_firings.fetch_add(1, Ordering::SeqCst);
            self.record_depth_max(would_be_depth);
            return Ok(GuardDecision::CeilingParked { would_be_depth });
        }

        let ids = emit_edges(tx, source, doc, content_event)?;
        self.record_depth_max(would_be_depth);
        Ok(GuardDecision::Emitted {
            ids,
            stamped_depth: would_be_depth,
        })
    }

    fn record_depth_max(&self, depth: u32) {
        let mut cur = self.causal_depth_max.load(Ordering::SeqCst);
        while depth > cur {
            match self.causal_depth_max.compare_exchange_weak(
                cur,
                depth,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(observed) => cur = observed,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit::extract_edges;
    use myelin_content::InlineNode;
    use myelin_events::{
        Actor, AggregateKey, CausedBy, CorrelationId, DataRole, EmitContextBase, EventType,
        IdMinter, MonotonicMinter, OutboxStore, Region, TenantId, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-7".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }
    fn source_doc() -> ArtifactRef {
        ArtifactRef("myelin://acme/chat/message/m1".into())
    }

    fn content_event(depth: u32) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("01J-content".into()),
            type_: EventType("chat.message.created".into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(principal()),
            subject: source_doc(),
            aggregate: AggregateKey("chat:message:m1".into()),
            causation_id: None,
            correlation_id: CorrelationId("01J-root-corr".into()),
            caused_by: Some(CausedBy("session:abc".into())),
            depth,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(principal()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn store_and_minter() -> (OutboxStore, Arc<dyn IdMinter>) {
        (
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
        )
    }

    fn one_link_doc() -> Vec<InlineNode> {
        vec![InlineNode::ArtifactRefNode(ArtifactRef(
            "myelin://acme/knowledge/page/7c2".into(),
        ))]
    }

    #[test]
    fn stamped_depth_is_content_plus_one() {
        assert_eq!(stamped_depth(&content_event(0)), 1);
        assert_eq!(stamped_depth(&content_event(3)), 4);
        assert_eq!(stamped_depth(&content_event(11)), 12);
    }

    #[test]
    fn stamped_depth_saturates_never_wraps() {
        assert_eq!(
            stamped_depth(&content_event(u32::MAX)),
            u32::MAX,
            "saturates, never wraps to 0"
        );
    }

    #[test]
    fn guarded_emit_stamps_every_edge_at_content_depth_plus_one() {
        let (store, minter) = store_and_minter();
        let guard = RefsLoopGuard::new();
        let content = content_event(3);

        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("chat message m1 written");
        let decision = guard
            .guarded_emit_edges(&mut tx, &source_doc(), &one_link_doc(), &content)
            .expect("emit ok");
        tx.commit().expect("commit ok");

        let ids = match decision {
            GuardDecision::Emitted { ids, stamped_depth } => {
                assert_eq!(
                    stamped_depth, 4,
                    "the +1 stamp: content depth 3 → edge depth 4"
                );
                ids
            }
            other => panic!("expected Emitted, got {other:?}"),
        };
        assert_eq!(ids.len(), 1, "one structured node → one edge");
        let row = store.row(&ids[0]).expect("committed edge row present");
        assert_eq!(
            row.envelope.depth, 4,
            "every emitted refs.edge.* carries content.depth + 1"
        );
        assert_eq!(
            row.envelope.correlation_id, content.correlation_id,
            "the correlation root carries (BUS-5)"
        );
        assert_eq!(
            row.envelope.causation_id.as_ref(),
            Some(&content.event_id),
            "causation = the content event"
        );
        assert_eq!(
            guard.causal_depth_max(),
            4,
            "bus.causal_depth_max recorded the +1 hop"
        );
        assert_eq!(
            guard.ceiling_tripwire_firings(),
            0,
            "below the ceiling → no tripwire"
        );
    }

    #[test]
    fn only_artifact_ref_and_embed_are_retrigger_sources() {
        assert!(
            is_retrigger_source(EdgeRel::Links),
            "artifact_ref node re-triggers"
        );
        assert!(
            is_retrigger_source(EdgeRel::Embeds),
            "embed node re-triggers"
        );
        assert!(
            !is_retrigger_source(EdgeRel::Mentions),
            "a mention notifies, it does not auto re-trigger (CHAT-1)"
        );
    }

    #[test]
    fn retrigger_source_targets_are_structured_nodes() {
        let edges = extract_edges(&source_doc(), &one_link_doc());
        assert_eq!(edges.len(), 1);
        assert!(is_retrigger_source(edges[0].rel));
        assert!(
            target_is_structured_node(&edges[0].target),
            "an artifact_ref edge's target is a structured myelin:// node"
        );
        assert!(
            !target_is_structured_node(&ArtifactRef("please do the thing @agent".into())),
            "raw text is not a structured node (cannot re-trigger)"
        );
    }

    #[test]
    fn would_exceed_ceiling_at_ceiling_minus_one() {
        let ceiling = CAUSAL_DEPTH_CEILING;
        assert!(!would_exceed_ceiling(&content_event(ceiling - 2), ceiling));
        assert!(would_exceed_ceiling(&content_event(ceiling - 1), ceiling));
        assert!(would_exceed_ceiling(&content_event(ceiling), ceiling));
    }

    #[test]
    fn ceiling_tripwire_fires_and_parks_zero_edges() {
        let (store, minter) = store_and_minter();
        let guard = RefsLoopGuard::new();
        let content = content_event(CAUSAL_DEPTH_CEILING);

        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("a deep reactive content write");
        let decision = guard
            .guarded_emit_edges(&mut tx, &source_doc(), &one_link_doc(), &content)
            .expect("guard ok");
        tx.commit().expect("commit ok");

        match decision {
            GuardDecision::CeilingParked { would_be_depth } => {
                assert_eq!(
                    would_be_depth,
                    CAUSAL_DEPTH_CEILING + 1,
                    "the parked edge would be over"
                );
            }
            other => panic!("expected CeilingParked, got {other:?}"),
        }
        assert_eq!(
            store.outbox_depth(),
            0,
            "the chain halts ≤ ceiling → 0 edges emitted"
        );
        assert_eq!(
            guard.ceiling_tripwire_firings(),
            1,
            "the tripwire fired exactly once"
        );
        assert_eq!(
            guard.causal_depth_max(),
            CAUSAL_DEPTH_CEILING + 1,
            "the deepest hop is recorded even on a park (observable, never silent)"
        );
    }

    #[test]
    fn a_climbing_chain_halts_at_the_ceiling() {
        let guard = RefsLoopGuard::with_ceiling(4);
        let mut emitted = 0u64;
        let mut parked = 0u64;
        for d in 0..=6u32 {
            let (store, minter) = store_and_minter();
            let mut tx = store.begin(minter, ctx_base());
            tx.stage_state_change("hop");
            let decision = guard
                .guarded_emit_edges(&mut tx, &source_doc(), &one_link_doc(), &content_event(d))
                .expect("guard ok");
            tx.commit().expect("commit ok");
            match decision {
                GuardDecision::Emitted { stamped_depth, .. } => {
                    emitted += 1;
                    assert!(
                        stamped_depth < 4,
                        "an emitted edge is strictly inside the ceiling"
                    );
                }
                GuardDecision::CeilingParked { .. } => parked += 1,
            }
        }
        assert_eq!(
            emitted, 3,
            "edges emitted only while strictly inside the ceiling"
        );
        assert_eq!(parked, 4, "every over-ceiling hop parked");
        assert!(
            guard.ceiling_tripwire_firings() >= 1,
            "the tripwire bounded the chain"
        );
        assert_eq!(
            guard.causal_depth_max(),
            7,
            "bus.causal_depth_max saw the deepest would-be hop"
        );
    }

    #[test]
    fn refs_ceiling_matches_the_frozen_ag6_number() {
        assert_eq!(
            CAUSAL_DEPTH_CEILING, 12,
            "the frozen AG-6 causal-depth ceiling"
        );
        assert_ne!(
            CAUSAL_DEPTH_CEILING, 16,
            "NOT the Refs traversal ceiling (REF-P13 / §4.4)"
        );
    }

    #[test]
    fn causal_depth_signal_name_is_frozen() {
        assert_eq!(RefsLoopGuard::CAUSAL_DEPTH_SIGNAL, "bus.causal_depth_max");
        assert_eq!(
            RefsLoopGuard::CAUSAL_DEPTH_SIGNAL,
            myelin_events::BusSignal::CausalDepthMax.metric_name(),
            "the guard feeds exactly the §4.11 #7 causal-depth survival signal (1.8)"
        );
    }
}
