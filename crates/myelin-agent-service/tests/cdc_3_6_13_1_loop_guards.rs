//! # The consumer CDC for contracts 3.6 + 13.1 — the structural loop guards (AG-P12 → P-224)
//!
//! **Contracts (CONSUMED):**
//! - `planning/05-refined-shared-systems-architecture/contract-index.md` row **3.6** (the Bus
//!   reactive/**dispatch tier** — the structural loop guards, bounded dispatch, nested causality the
//!   guards read). The guards primarily LIVE in the Bus tier; the Fabric **re-enforces at apply time**
//!   (defence in depth, §5.5). This pins the agent-fabric CONSUMER of the 3.6 dispatch-tier causality:
//!   the guards read `actor.principal` (self-guard), `correlation_id` (shared-root tripwire), and
//!   `depth` (causal-depth ceiling) off the delivered [`EventEnvelope`] — the SAME causality the
//!   `OutboxTx::emit(draft, cause)` nested-causality emit (2.2) carries.
//! - row **13.1** (the frozen `myelin-content` inline ref nodes the reference gate keys on). This pins
//!   the CONSUMER of the 13.1 inline nodes: ONLY a structured [`InlineNode::ArtifactRefNode`]
//!   re-triggers a run; a [`InlineNode::Mention`] / [`InlineNode::Embed`] / raw text never does.
//!
//! **Owning architecture:** `agent-fabric.md` §5.5 (the five structural loop guards). The PROVIDER of
//! 3.6 is the Bus dispatch tier (a delivered `EventEnvelope` with its nested causality); the PROVIDER
//! of 13.1 is `myelin-content` (the three load-bearing inline nodes). This crate is the CONSUMER of
//! both — it ships no second envelope shape and no second inline-node taxonomy (EI-01 §7 coherence).
//!
//! ## What this pair pins (the consumer half)
//! - the guards read EXACTLY the 3.6/2.2 causality fields (`actor.principal`, `correlation_id`,
//!   `depth`) off the frozen envelope — a drift in those field names/types breaks this test;
//! - the reference gate keys on EXACTLY the frozen 13.1 `InlineNode` taxonomy — only `ArtifactRefNode`
//!   admits.

use myelin_agent_service::{AgentLoopGuards, GuardRefusal, GuardVerdict};
use myelin_content::InlineNode;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn agent(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt".into()),
            on_behalf_of: None,
        },
        tenant(),
    )
}

fn human(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

/// **PROVIDER (Bus dispatch tier, 3.6) — a delivered [`EventEnvelope`] carrying the nested causality
/// (2.2) the guards read.** The `actor`, `correlation_id`, and `depth` are the load-bearing fields; the
/// rest is the frozen 2.1 envelope shape.
fn delivered_dispatch(actor: Principal, correlation: &str, depth: u32) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("evt-{depth}")),
        type_: EventType("issues.comment.created".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(actor),
        subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
        aggregate: AggregateKey("agg-1".into()),
        causation_id: None,
        correlation_id: CorrelationId(correlation.into()),
        caused_by: None,
        depth,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
        payload: serde_json::json!({}),
    }
}

/// **3.6 CONSUMER — the self-guard reads `actor.principal` off the delivered envelope.** A delivered
/// dispatch whose `actor.principal` IS the agent (the agent's own emission re-arriving via the Bus) is
/// DROPPED; a dispatch from a human is admitted past the self-guard.
#[test]
fn cdc_3_6_self_guard_reads_actor_principal_off_envelope() {
    let guards = AgentLoopGuards::new(PrincipalId("agent-alice".into()));
    let ref_node =
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()));

    // the agent's OWN emission re-delivered by the Bus → the self-guard drops it.
    let own = delivered_dispatch(agent("agent-alice"), "corr", 0);
    let v = guards.admit_dispatch(&own.actor, &ref_node, &own.correlation_id.0, own.depth);
    assert_eq!(v, GuardVerdict::Drop(GuardRefusal::SelfTrigger));

    // a HUMAN's dispatch (a structured ref, shallow depth) → admitted past the self-guard.
    let human_ev = delivered_dispatch(human("user-bob"), "corr", 0);
    let v = guards.admit_dispatch(
        &human_ev.actor,
        &ref_node,
        &human_ev.correlation_id.0,
        human_ev.depth,
    );
    assert_eq!(
        v,
        GuardVerdict::Admit,
        "a human's dispatch is admitted (bounded by depth/tripwire)"
    );
}

/// **3.6 CONSUMER — the causal-depth ceiling reads `depth` off the delivered envelope (the 2.2 nested
/// causality).** A dispatch arriving at `depth == ceiling` whose child would be `ceiling + 1` is
/// dropped on the depth ceiling — the guard reads the envelope's `depth`, not a parallel counter.
#[test]
fn cdc_3_6_depth_ceiling_reads_envelope_depth() {
    let guards = AgentLoopGuards::new(PrincipalId("agent-alice".into()));
    let ceiling = guards.ceiling();
    let ref_node =
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()));
    let other = Actor(human("user-bob"));

    // a dispatch whose parent depth is already AT the ceiling → the child (ceiling + 1) is dropped.
    let deep = delivered_dispatch(human("user-bob"), "corr", ceiling);
    let v = guards.admit_dispatch(&deep.actor, &ref_node, &deep.correlation_id.0, deep.depth);
    assert_eq!(
        v,
        GuardVerdict::Drop(GuardRefusal::DepthCeiling),
        "the depth ceiling reads the envelope's depth (a child past {ceiling} is dropped)"
    );

    // a dispatch one below the ceiling → its child is admitted (the boundary is exact).
    let ok = delivered_dispatch(human("user-bob"), "corr", ceiling - 1);
    let v = guards.admit_dispatch(&other, &ref_node, &ok.correlation_id.0, ok.depth);
    assert_eq!(
        v,
        GuardVerdict::Admit,
        "a child exactly AT the ceiling is admitted"
    );
}

/// **13.1 CONSUMER — the reference gate keys on the frozen `InlineNode` taxonomy: ONLY
/// `ArtifactRefNode` re-triggers.** The three load-bearing 13.1 nodes are pinned: a structured
/// `ArtifactRefNode` admits; a `Mention` (explicit-dispatch) and an `Embed` (display) do NOT re-trigger
/// on the loop path. This is the agent-fabric CONSUMER of the content crate's PROVIDER taxonomy.
#[test]
fn cdc_13_1_reference_gate_keys_on_inline_node_taxonomy() {
    let guards = AgentLoopGuards::new(PrincipalId("agent-alice".into()));
    let other = Actor(human("user-bob"));

    // ArtifactRefNode (13.1) → the ONLY re-trigger → admitted.
    let ref_node =
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/knowledge/page/7".into()));
    assert_eq!(
        guards.admit_dispatch(&other, &ref_node, "corr", 0),
        GuardVerdict::Admit,
        "a structured artifact_ref node (13.1) re-triggers",
    );

    // Mention (13.1) → explicit-dispatch, NOT a loop re-trigger → dropped.
    let mention = InlineNode::Mention(agent("agent-alice"));
    assert_eq!(
        guards.admit_dispatch(&other, &mention, "corr", 0),
        GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
        "a mention (13.1) is explicit-dispatch, not a loop re-trigger",
    );

    // Embed (13.1) → display, NOT a re-trigger → dropped.
    let embed = InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/9".into()));
    assert_eq!(
        guards.admit_dispatch(&other, &embed, "corr", 0),
        GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
        "an embed (13.1) is display, not a re-trigger",
    );
}

/// **13.1 CONSUMER — raw typed text (NOT an `InlineNode` at all) NEVER re-triggers (the structural
/// no-typo-into-a-loop guarantee, §5.5).** The reference gate's raw-text path is the consumer assertion
/// that a plain string — which produces no 13.1 node — can never re-trigger.
#[test]
fn cdc_13_1_raw_text_is_not_an_inline_node_never_re_triggers() {
    let guards = AgentLoopGuards::new(PrincipalId("agent-alice".into()));
    for raw in [
        "@agent-alice loop",
        "myelin://acme/issues/issue/PROJ-1",
        "plain text",
        "",
    ] {
        assert_eq!(
            guards.reference_gate().admit_raw_text(raw),
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
            "raw text {raw:?} produces no 13.1 node → never re-triggers",
        );
    }
}
