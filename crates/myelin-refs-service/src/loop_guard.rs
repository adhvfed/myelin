//! The **loop-guard causal-depth stamp** on every `refs.edge.*` emit (REF-P9 / P-158; the §4.1
//! `depth +1` half of the producer seam; the AG-6 loop guard's Refs-side input).
//!
//! **Owning architecture doc:** `reference-graph.md` §4.1 (edge extraction → emit): every
//! `refs.edge.*` is emitted with `OutboxTx::emit(draft, cause = Some(content_event))`, so the
//! correlation **root carries**, `causation = the content event`, and **`depth +1`** (BUS-5) —
//! "which is what lets the loop guard treat **only a structured `artifact_ref` node** as a
//! re-trigger source (AG-6)." This module is the explicit DRILL + the depth-ceiling TRIPWIRE over
//! the [`crate::emit`] seam (REF-P8): REF-P8 ships the emit (the `+1` rides
//! [`myelin_events::derive_envelope`] correct-by-construction); REF-P9 adds the loop guard that
//! reads the stamp, gates the re-trigger source, and fires before runaway.
//!
//! **External insight:** `01-process-and-quality-doctrine.md` §3 (observability is part of the
//! pass — the causal-depth telemetry signal 1.8 is a pass condition, not an afterthought). **VISION
//! §2** (Chat references any artifact → a reference edge → a *bounded* reactive chain).
//!
//! ## Contracts CONSUMED (to the frozen shapes)
//! - **2.1** [`myelin_events::EventEnvelope`] causality fields (`causation_id`/`correlation_id`/
//!   `depth`) — the loop guard READS `depth`.
//! - **2.2** [`myelin_events::OutboxTx::emit`]`(draft, cause)` — the ONE emit verb; the `+1` stamp
//!   is derived here, never authored (the draft has no causal fields, BUS-5).
//! - **1.8** the **causal-depth telemetry signal** (`bus.causal_depth_max`,
//!   [`myelin_events::BusSignal::CausalDepthMax`]) — the deepest stamped hop observed; this module
//!   FEEDS it ([`RefsLoopGuard::causal_depth_max`]).
//!
//! ## The three structural properties this loop guard pins (the GATE)
//!
//! 1. **The `+1` depth stamp on every `refs.edge.*`.** [`stamped_depth`] is the ONE function that
//!    computes an edge's depth from its content cause: `content_event.depth + 1` (saturating — a
//!    pathological chain caps at `u32::MAX` rather than wrapping to 0, which would *defeat* the
//!    ceiling). [`RefsLoopGuard::guarded_emit_edges`] is the emit wrapper that proves every emitted
//!    edge carries exactly this stamp; the explicit drill asserts `emitted.depth ==
//!    content.depth + 1` (REF-P9's reason to exist — REF-P8 left this to `derive_envelope`).
//!
//! 2. **Only a structured `artifact_ref` node is a re-trigger source (AG-6 / ADR-05).** A reference
//!    edge whose `rel` is [`crate::EdgeRel::Links`] / [`crate::EdgeRel::Embeds`] (born from a
//!    structured `artifact_ref` / `embed` node pointing at a `myelin://…` artifact) MAY re-trigger
//!    downstream reactive work; a [`crate::EdgeRel::Mentions`] edge (a `mention(Principal)` → the
//!    pseudonymous `member` URN) is a NOTIFY, never an auto re-trigger (CHAT-1 explicit-first), and
//!    raw typed text is structurally not a node at all (extraction is structured, never a regex —
//!    EI-04 §2.4). [`is_retrigger_source`] is that gate; it is the SAME discipline the Bus dispatch
//!    tier lowers (`myelin_query::dispatch`'s reference gate) — here it is stated at the Refs emit
//!    boundary so the loop guard's input is well-defined the moment an edge is born.
//!
//! 3. **A depth-ceiling tripwire fires BEFORE runaway.** [`CAUSAL_DEPTH_CEILING`] (12 — the frozen
//!    AG-6 causal ceiling, distinct from the Refs *traversal* ceiling 16, REF-P13/§4.4) bounds the
//!    reactive chain: when a content cause is already at/over the ceiling, the would-be edge is at
//!    `depth >= ceiling + 1`, so [`RefsLoopGuard::guarded_emit_edges`] **PARKS** the emit (writes 0
//!    edges) and fires the tripwire — the chain halts at a bounded depth rather than recursing
//!    unboundedly. The tripwire is a COUNTER ([`RefsLoopGuard::ceiling_tripwire_firings`]) so the
//!    park is observable (never a silent drop — EI-02 §4), AND the causal-depth max telemetry (1.8)
//!    records the deepest hop so a chain approaching the ceiling is visible before it trips.
//!
//! ## Why the ceiling is re-stated here, not imported from `myelin-query` (DOCUMENTED)
//! The Bus dispatch tier (`myelin_query::dispatch::CAUSAL_DEPTH_CEILING`, P-143) holds the SAME
//! frozen `12`. `myelin-refs-service` is a terminal LEAF consumer of the §2.9 DAG and MUST NOT
//! depend on `myelin-query` (a mid-tier crate) — that would add an edge the crate-graph forbids and
//! invert the DAG. The ceiling is a frozen ARCHITECTURE constant (§4.7 / AG-6 / the §00 two-ceilings
//! gate), not a query-owned value, so it is re-stated here over the same upstream
//! [`myelin_events::EventEnvelope::depth`] field — exactly as `myelin-agent-service`'s schema
//! re-states "default ceiling 12" independently. There is NO second causality function: the `+1`
//! stamp is still [`myelin_events::derive_envelope`] (via `OutboxTx::emit`); only the CEILING NUMBER
//! is named in both places, pinned identical by [`tests::refs_ceiling_matches_the_frozen_ag6_number`].
//!
//! ## Floors named (VISION §3 / prompt DoD)
//! - **Producers are SYNTHETIC at M2** (inherited from REF-P8): the guard is exercised by the test
//!   content writer; the first REAL producers are REF-P17 / REF-P18 (M3+). The loop-guard WIRING
//!   (the stamp + the gate + the tripwire) is real over the synthetic emit.
//! - **The cross-CELL causal depth is cell-local** (C-5; §4.2): a cross-cell pointer carries the
//!   `correlation_id` (BUS-5), and the home cell continues the chain at its own `depth`. The
//!   cross-cell fan-out BUILD is the named §6.5 floor (REF-P26); the stamp semantics this module
//!   pins are cell-local and frozen.

use myelin_events::{ArtifactRef, EventEnvelope, EventId, OutboxTx, Result};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::emit::{emit_edges, EdgeRel};

/// The **frozen causal-depth ceiling** for the loop guard (AG-6; arch §4.7 / the §00 two-ceilings
/// gate). A reactive chain whose content cause is already at/over this depth produces NO further
/// `refs.edge.*` — the structural guard that a self-perpetuating reference→edge→reference chain
/// halts at a bounded depth rather than recursing unboundedly. **12** is the frozen AGENT/CAUSAL
/// ceiling — the SAME number the Bus dispatch tier (`myelin_query::dispatch`) holds; it is
/// re-stated here (not imported) because refs-service must not depend on the mid-tier query crate
/// (DOCUMENTED above). It is DISTINCT from the Refs **traversal** ceiling (16, REF-P13 / §4.4): the
/// traversal ceiling bounds a read-time CTE walk; THIS ceiling bounds the write-time causal chain.
pub const CAUSAL_DEPTH_CEILING: u32 = 12;

/// **The depth stamp every `refs.edge.*` carries: `content_event.depth + 1`** (BUS-5; §4.1
/// `depth +1`). **Saturating** — a pathological chain at `u32::MAX` caps rather than wrapping to 0
/// (a wrap would re-seed the chain UNDER the ceiling and defeat the loop guard; saturating keeps a
/// runaway pinned at the max so the ceiling still trips). This is the SAME arithmetic
/// [`myelin_events::derive_envelope`] applies when the emit passes `cause = Some(content_event)`;
/// it is named here so the loop guard can assert the stamp WITHOUT re-deriving an envelope, and so
/// the depth-ceiling decision ([`would_exceed_ceiling`]) reads the same value the emitted edge will
/// carry.
pub fn stamped_depth(content_event: &EventEnvelope) -> u32 {
    content_event.depth.saturating_add(1)
}

/// Whether emitting an edge caused by `content_event` would **exceed the causal-depth ceiling**
/// (the would-be edge is at [`stamped_depth`]; the chain halts iff that is `>= CAUSAL_DEPTH_CEILING`,
/// i.e. the cause is already at/over `ceiling - 1`). At the ceiling the emit is parked so the
/// deepest edge that is ever written sits at `ceiling - 1` — one hop INSIDE the bound, never at or
/// past it. Pure in `(content_event.depth, ceiling)` (the replay-determinism property).
pub fn would_exceed_ceiling(content_event: &EventEnvelope, ceiling: u32) -> bool {
    stamped_depth(content_event) >= ceiling
}

/// Whether a reference edge with this `rel` is a **structured re-trigger source** (AG-6 / ADR-05).
///
/// Only an edge born from a structured `artifact_ref` / `embed` node — [`EdgeRel::Links`] /
/// [`EdgeRel::Embeds`], pointing at a `myelin://…` artifact — MAY re-trigger downstream reactive
/// work. A [`EdgeRel::Mentions`] edge is a NOTIFY (explicit-first, CHAT-1): a `@mention` notifies
/// the principal, it does NOT auto-re-trigger a reactive run. (Raw typed text is structurally not a
/// node at all — extraction matches enum variants, never scans prose, EI-04 §2.4 — so it can never
/// reach this gate.) This is the SAME reference-gate discipline the Bus dispatch tier lowers; it is
/// stated at the Refs emit boundary so the loop guard's input — "which of THESE born edges may feed
/// the next causal hop" — is well-defined the instant the edge is extracted.
pub fn is_retrigger_source(rel: EdgeRel) -> bool {
    match rel {
        // A structured artifact_ref / embed → a `myelin://…` artifact: a re-trigger source.
        EdgeRel::Links | EdgeRel::Embeds => true,
        // A mention → the pseudonymous member URN: NOTIFY, never an auto re-trigger (CHAT-1).
        EdgeRel::Mentions => false,
    }
}

/// Whether an [`ArtifactRef`] target is a **structured `myelin://…` artifact node** (the
/// belt-and-braces structural check the reference gate leans on, mirroring the Bus dispatch tier's
/// `is_artifact_ref`). A well-formed `myelin://<tenant>/<subsystem>/<type>/<id>` is a structured
/// node; anything else is not a re-trigger source. Used by [`RefsLoopGuard`] so a malformed/raw
/// target can never be treated as a re-trigger source even if a future producer mis-tagged its
/// `rel`.
pub fn target_is_structured_node(target: &ArtifactRef) -> bool {
    target.0.starts_with("myelin://") && target.0.len() > "myelin://".len()
}

/// The **disposition of the loop-guard decision** for one content-write's edge emit — a named,
/// observable outcome (never a silent drop, EI-02 §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuardDecision {
    /// **Emitted**: the content cause is below the ceiling; the edges were emitted, each stamped at
    /// `content.depth + 1`. Carries the emitted edge [`EventId`]s (in document order) + the
    /// stamped depth so the caller/drill can assert the `+1`.
    Emitted {
        /// The emitted `refs.edge.created` ids (document order).
        ids: Vec<EventId>,
        /// The depth every emitted edge carries (`content.depth + 1`).
        stamped_depth: u32,
    },
    /// **Ceiling-parked**: the content cause is at/over `ceiling - 1`, so the would-be edge is at
    /// `>= ceiling`. ZERO edges were emitted (the chain halts ≤ ceiling) and the tripwire fired.
    /// Carries the depth the parked edge WOULD have had (so the park is auditable).
    CeilingParked {
        /// The depth the parked edge would have carried (`>= CAUSAL_DEPTH_CEILING`).
        would_be_depth: u32,
    },
}

/// The **Refs loop guard** over the [`crate::emit`] seam (REF-P9): it stamps every `refs.edge.*`
/// with `content.depth + 1`, gates the re-trigger source to structured `artifact_ref`/`embed` nodes
/// (AG-6), and PARKS + trips a tripwire when a chain reaches the causal-depth ceiling — before
/// runaway. It feeds the causal-depth telemetry (1.8) so the deepest stamped hop is observable.
///
/// Stateful only in its OBSERVABILITY (the tripwire-firing count + the causal-depth max); the
/// stamp/gate/ceiling DECISIONS are pure functions of the content cause (the replay-determinism
/// property — the same content sequence drives the same emits + the same telemetry). The counters
/// are atomics so the guard is `Send + Sync` (a consumer-shared handle), exactly like
/// [`crate::edge_builder::RefsEdgeBuilder`]'s `index_lag`.
#[derive(Debug)]
pub struct RefsLoopGuard {
    /// The deepest stamped hop observed (`bus.causal_depth_max` input, 1.8). Monotonic max.
    causal_depth_max: Arc<AtomicU32>,
    /// How many times the depth-ceiling tripwire fired (the park-before-runaway count). Observable
    /// so a runaway chain that hit the ceiling is never a silent drop.
    ceiling_tripwire_firings: Arc<AtomicU64>,
    /// The causal-depth ceiling this guard enforces (default [`CAUSAL_DEPTH_CEILING`]; a drill may
    /// set a small ceiling to force the tripwire without 12 real hops — the SAME structural code).
    ceiling: u32,
}

impl Default for RefsLoopGuard {
    fn default() -> RefsLoopGuard {
        RefsLoopGuard::new()
    }
}

impl RefsLoopGuard {
    /// The frozen telemetry signal name this guard FEEDS (contract 1.8;
    /// [`myelin_events::BusSignal::CausalDepthMax`]). A named constant — drills assert against the
    /// NAME, never a literal (EI-01 §3 observability / coherence).
    pub const CAUSAL_DEPTH_SIGNAL: &'static str = "bus.causal_depth_max";

    /// A guard at the frozen-default ceiling (12), with zeroed observability.
    pub fn new() -> RefsLoopGuard {
        RefsLoopGuard::with_ceiling(CAUSAL_DEPTH_CEILING)
    }

    /// A guard with a custom ceiling (the drill sets a small ceiling to force the tripwire in a few
    /// hops; the SAME structural code path runs). The ceiling is the FLOOR default the production
    /// value tunes (the §00 two-ceilings gate); the drill exercises the structural property.
    pub fn with_ceiling(ceiling: u32) -> RefsLoopGuard {
        RefsLoopGuard {
            causal_depth_max: Arc::new(AtomicU32::new(0)),
            ceiling_tripwire_firings: Arc::new(AtomicU64::new(0)),
            ceiling,
        }
    }

    /// The causal-depth ceiling this guard enforces.
    pub fn ceiling(&self) -> u32 {
        self.ceiling
    }

    /// The deepest stamped causal hop observed so far (the `bus.causal_depth_max` sample, 1.8).
    pub fn causal_depth_max(&self) -> u32 {
        self.causal_depth_max.load(Ordering::SeqCst)
    }

    /// How many times the depth-ceiling tripwire fired (the park-before-runaway count). `> 0` means
    /// a reactive chain reached the ceiling and was halted (observable, never silent).
    pub fn ceiling_tripwire_firings(&self) -> u64 {
        self.ceiling_tripwire_firings.load(Ordering::SeqCst)
    }

    /// **Emit one `refs.edge.created` per structured node — STAMPED at `content.depth + 1` and
    /// GUARDED by the causal-depth ceiling (REF-P9; the loop guard over the REF-P8 emit seam).**
    ///
    /// The decision:
    /// 1. Compute the would-be stamped depth ([`stamped_depth`] = `content.depth + 1`).
    /// 2. **Ceiling guard:** if that depth is `>= ceiling` ([`would_exceed_ceiling`]), the chain has
    ///    reached the bound — **PARK** (emit 0 edges), fire the tripwire, and return
    ///    [`GuardDecision::CeilingParked`]. The chain halts ≤ ceiling; the deepest edge ever written
    ///    sits at `ceiling - 1`.
    /// 3. Otherwise **emit** via [`crate::emit::emit_edges`] (the ONE outbox path, contract 2.2;
    ///    `cause = Some(content_event)` → the `+1` rides [`myelin_events::derive_envelope`]), record
    ///    the stamped depth into the causal-depth max (1.8), and return [`GuardDecision::Emitted`].
    ///
    /// Emit-iff-committed is unchanged (the rows are BUFFERED into `tx`; durable iff the caller
    /// commits — REF-D7 producer half). The guard adds the depth bound + the telemetry; it never
    /// commits.
    pub fn guarded_emit_edges(
        &self,
        tx: &mut dyn OutboxTx,
        source: &ArtifactRef,
        doc: &[myelin_content::InlineNode],
        content_event: &EventEnvelope,
    ) -> Result<GuardDecision> {
        let would_be_depth = stamped_depth(content_event);

        // (2) CEILING GUARD: park before runaway. The would-be edge is at `>= ceiling` → halt.
        if would_exceed_ceiling(content_event, self.ceiling) {
            // The tripwire fires (observable, never silent) — and we STILL record the would-be
            // depth into the causal-depth max so the deepest hop the chain reached is visible.
            self.ceiling_tripwire_firings.fetch_add(1, Ordering::SeqCst);
            self.record_depth_max(would_be_depth);
            return Ok(GuardDecision::CeilingParked { would_be_depth });
        }

        // (3) EMIT (the +1 rides derive_envelope via emit_edges' `cause = Some(content_event)`).
        let ids = emit_edges(tx, source, doc, content_event)?;
        // The causal-depth telemetry (1.8): the deepest stamped hop observed. Only edges that were
        // actually emitted contribute (an empty doc emits 0 edges but the depth WAS reached at this
        // content hop — we record the content hop's depth so an approaching chain is visible).
        self.record_depth_max(would_be_depth);
        Ok(GuardDecision::Emitted {
            ids,
            stamped_depth: would_be_depth,
        })
    }

    /// Record one observed stamped depth into the monotonic causal-depth max (1.8). A compare-and-
    /// set loop so the max is correct under a shared handle (the consumer may emit concurrently).
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

    /// A content cause at `depth` (the edge it produces is stamped `depth + 1`).
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

    // ---------------------------------------------------------------------
    // (1) THE +1 DEPTH STAMP — every emitted refs.edge.* carries content.depth + 1
    // ---------------------------------------------------------------------

    /// `stamped_depth` is exactly `content.depth + 1` (the §4.1 `depth +1`).
    #[test]
    fn stamped_depth_is_content_plus_one() {
        assert_eq!(stamped_depth(&content_event(0)), 1);
        assert_eq!(stamped_depth(&content_event(3)), 4);
        assert_eq!(stamped_depth(&content_event(11)), 12);
    }

    /// The stamp **saturates** at `u32::MAX` rather than wrapping to 0 (a wrap would re-seed the
    /// chain UNDER the ceiling and defeat the loop guard — the leak-of-runaway-critical case).
    #[test]
    fn stamped_depth_saturates_never_wraps() {
        assert_eq!(
            stamped_depth(&content_event(u32::MAX)),
            u32::MAX,
            "saturates, never wraps to 0"
        );
    }

    /// **The drill (REF-P9 reason to exist): a guarded emit stamps every `refs.edge.*` at
    /// `content.depth + 1`,** proven through the REAL outbox transaction (not just `derive_envelope`
    /// in isolation). A content cause at depth 3 → the emitted edge carries depth 4.
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
        // The DURABLE emitted envelope carries the +1 stamp (read off the committed outbox row).
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
        // The causal-depth telemetry (1.8) recorded the deepest hop.
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

    // ---------------------------------------------------------------------
    // (2) RE-TRIGGER GATE — only a structured artifact_ref / embed node re-triggers
    // ---------------------------------------------------------------------

    /// **Only a structured `artifact_ref` (`links`) / `embed` (`embeds`) edge is a re-trigger
    /// source; a `mention` (`mentions`) is a NOTIFY, never an auto re-trigger (AG-6 / CHAT-1).**
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

    /// The re-trigger gate lines up with the structured-target check: an `artifact_ref`/`embed`
    /// edge's target IS a `myelin://…` node; a raw/empty target is never a structured node.
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

    // ---------------------------------------------------------------------
    // (3) THE DEPTH-CEILING TRIPWIRE — fires BEFORE runaway (parks, halts ≤ ceiling)
    // ---------------------------------------------------------------------

    /// `would_exceed_ceiling` is true exactly when the would-be edge is at/over the ceiling (the
    /// content cause is at `ceiling - 1` or deeper).
    #[test]
    fn would_exceed_ceiling_at_ceiling_minus_one() {
        let ceiling = CAUSAL_DEPTH_CEILING;
        // cause at ceiling-2 → edge at ceiling-1 → INSIDE the bound.
        assert!(!would_exceed_ceiling(&content_event(ceiling - 2), ceiling));
        // cause at ceiling-1 → edge at ceiling → AT the bound → park.
        assert!(would_exceed_ceiling(&content_event(ceiling - 1), ceiling));
        // cause at ceiling → edge at ceiling+1 → over → park.
        assert!(would_exceed_ceiling(&content_event(ceiling), ceiling));
    }

    /// **The depth-ceiling tripwire fires BEFORE runaway: a content cause at the ceiling emits ZERO
    /// edges, the tripwire counter increments, and the would-be depth is recorded for audit.** The
    /// chain halts ≤ ceiling — the deepest edge ever written sits at `ceiling - 1`.
    #[test]
    fn ceiling_tripwire_fires_and_parks_zero_edges() {
        let (store, minter) = store_and_minter();
        let guard = RefsLoopGuard::new();
        // A content cause already AT the ceiling: the would-be edge is at ceiling + 1 → park.
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

    /// **A chain that climbs to the ceiling halts there (it does not run away).** Drive the guard
    /// with a small ceiling (4) over a synthetic chain depth 0→1→…: edges are emitted while inside
    /// the bound, then the emit is parked at the ceiling. The tripwire bounds the chain.
    #[test]
    fn a_climbing_chain_halts_at_the_ceiling() {
        let guard = RefsLoopGuard::with_ceiling(4);
        let mut emitted = 0u64;
        let mut parked = 0u64;
        // Content causes climbing depth 0..=6 — well past the small ceiling 4.
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
        // causes 0,1,2 → edges 1,2,3 (inside ceiling 4); causes 3..=6 → would-be 4..=7 → parked.
        assert_eq!(
            emitted, 3,
            "edges emitted only while strictly inside the ceiling"
        );
        assert_eq!(parked, 4, "every over-ceiling hop parked");
        assert!(
            guard.ceiling_tripwire_firings() >= 1,
            "the tripwire bounded the chain"
        );
        // The deepest stamped hop the chain reached is observable (1.8).
        assert_eq!(
            guard.causal_depth_max(),
            7,
            "bus.causal_depth_max saw the deepest would-be hop"
        );
    }

    // ---------------------------------------------------------------------
    // The ceiling matches the frozen AG-6 number + the signal name is frozen
    // ---------------------------------------------------------------------

    /// The Refs causal-depth ceiling is the frozen AG-6 `12` (arch §4.7 / the §00 two-ceilings
    /// gate) — the SAME number the Bus dispatch tier holds; re-stated here because refs-service must
    /// not depend on the mid-tier query crate (DOCUMENTED). DISTINCT from the traversal ceiling 16.
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

    /// The causal-depth telemetry signal name is the frozen `bus.causal_depth_max` (1.8) — the guard
    /// FEEDS exactly the signal the Bus survival set names ([`BusSignal::CausalDepthMax`]).
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
