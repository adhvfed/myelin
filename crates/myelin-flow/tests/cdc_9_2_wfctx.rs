//! # The CDC pair for the WfCtx core — contract 9.2 (the deterministic surface; PROVIDER half)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 9.2
//! (`WfCtx`: `activity`/`now`/`rand`/`emit` — the deterministic surface; the WRITE half is OWNED
//! by P-FLOW-04). Owning architecture: `durable-workflow.md` §5.1 (the surface), §4.5 (the outbox
//! seam — NO second emit path), §3.2 (`wf_history` as the journal source of truth).
//!
//! ## What this pair pins (the PROVIDER ↔ CONSUMER agreement of 9.2's write half)
//!
//! **9.2 PROVIDER (the WfCtx) — the agreement the workflow engine guarantees:**
//! - `activity` journals EXACTLY ONE `wf_history` row under its DETERMINISTIC `command_id`
//!   (`<wf_type>:<n>`) and records the BUS-2 `idem_token` in `wf_activity_attempt`;
//! - `emit` produces an outbox row whose envelope carries the caller's `type`/`subject`/`aggregate`
//!   and the causality the bus derives correct-by-construction — the SAME outbox emit-iff-committed
//!   carriage every subsystem's events use (NO second emit path, §4.5);
//! - the journal row and the outbox row CO-COMMIT in one transaction (FLOW-D5).
//!
//! **9.2 CONSUMER (a sibling subsystem, e.g. the bus relay + an indexer) — what it relies on:**
//! - it receives the emitted event PER-AGGREGATE ORDERED + at-least-once via the outbox/relay (the
//!   carriage the bus guarantees), and the journal it consumes on replay (P-FLOW-05) is keyed by
//!   the deterministic `command_id`.
//!
//! This pins the provider's promise NOW (the WfCtx write half); the replay-consumer LEG lands
//! P-FLOW-05. The pair proves the provider emits through the ONE outbox path with the frozen
//! envelope shape, not a private channel.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef as EvArtifactRef, CausedBy, DataRole, EmitContextBase,
    EventDraft, EventEnvelope, EventType, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    Visibility,
};
use myelin_flow::{history_kind, RetryPolicy, WfCtx, WfJournal};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

fn step_draft() -> EventDraft {
    EventDraft {
        type_: EventType("agent.run.step".into()),
        subject: EvArtifactRef("myelin://acme/agent/run/R1".into()),
        aggregate: AggregateKey("run:R1".into()),
        payload: serde_json::json!({ "step": "plan" }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

fn begin(outbox: &OutboxStore, journal: WfJournal) -> WfCtx {
    begin_with(outbox, journal, minter())
}

fn begin_with(outbox: &OutboxStore, journal: WfJournal, minter: Arc<dyn IdMinter>) -> WfCtx {
    WfCtx::begin(
        outbox,
        minter,
        journal,
        ctx_base(),
        "R1",
        "agent.run",
        "2026-06-21T00:00:00Z",
        42,
    )
}

/// **PROVIDER side of 9.2 (the WfCtx journals an activity + emits its event through the outbox).**
/// The provider's promise: one deterministic-`command_id` `wf_history` row + an outbox row whose
/// envelope carries the caller's `type`/`subject`/`aggregate`, co-committed in one transaction.
#[test]
fn provider_wfctx_journals_and_emits_through_the_one_outbox_path() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let mut ctx = begin(&outbox, journal.clone());

    ctx.activity(RetryPolicy::default_policy(), |_idem, _attempt| {
        Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
    })
    .expect("the activity runs");
    let emitted = ctx.emit(step_draft(), None).expect("emit through the outbox");
    ctx.commit().expect("the journal + outbox co-commit");

    // PROVIDER promise #1: exactly one journal row under the deterministic command_id (§3.2).
    let hist = journal.history_for(&tenant(), "R1");
    assert_eq!(hist.len(), 1, "one history row journaled");
    assert_eq!(hist[0].command_id, "agent.run:0", "deterministic command_id from position");
    assert_eq!(hist[0].kind, history_kind::ACTIVITY_COMPLETED);

    // PROVIDER promise #2: the emit went through the ONE outbox path with the frozen envelope shape.
    let row = outbox.row(&emitted).expect("the emitted event is a committed outbox row");
    assert_eq!(row.envelope.type_, EventType("agent.run.step".into()), "the caller's type carries");
    assert_eq!(
        row.subject,
        EvArtifactRef("myelin://acme/agent/run/R1".into()),
        "the caller's subject carries (hoisted for the broker subject)"
    );
    assert_eq!(row.aggregate, AggregateKey("run:R1".into()), "the per-aggregate ordering key carries");
    assert_eq!(row.seq, 0, "first event for the aggregate is seq 0 (per-aggregate order)");
}

/// **CONSUMER side of 9.2 (a sibling subsystem receives the emitted event).** A consumer fixture
/// (modeling the bus relay → an indexer) reads the committed outbox row's envelope and relies on:
/// the caller-authored carriage (`type`/`subject`/`aggregate`) + the bus-derived causality
/// (a ROOT emit carries its own correlation at depth 0, BUS-5). It does NOT see the journal — the
/// journal is the engine's private source of truth; the consumer sees only the emitted event.
#[test]
fn consumer_receives_the_emitted_event_with_carriage_and_causality() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let mut ctx = begin(&outbox, journal);
    let emitted = ctx.emit(step_draft(), None).expect("emit");
    ctx.commit().expect("co-commit");

    // The CONSUMER fixture: it reads the delivered envelope (here, the committed outbox row's body).
    let envelope: EventEnvelope = outbox
        .row(&emitted)
        .expect("the consumer receives the committed event")
        .envelope;

    // carriage the provider authored.
    assert_eq!(envelope.type_, EventType("agent.run.step".into()));
    assert_eq!(envelope.tenant, tenant(), "the (tenant, region) partition key carries");
    assert_eq!(envelope.region, Region("fr-par".into()));
    // causality derived correct-by-construction (a ROOT emit): carries its own correlation, depth 0.
    assert_eq!(envelope.depth, 0, "a root emit is depth 0 (BUS-5)");
    assert_eq!(
        envelope.correlation_id.0, envelope.event_id.0,
        "a root carries its own correlation (BUS-5, no second emit path forged it)"
    );
    assert_eq!(envelope.causation_id, None, "a root has no parent");
}

/// **The provider's emit is CAUSALITY-derived for a caused event too (BUS-5).** A workflow step
/// emitted as a reaction to a parent envelope sets `causation_id = parent.id`,
/// `correlation_id = parent.correlation`, `depth = parent.depth + 1` — the same correct-by-
/// construction derivation every subsystem's emit gets (the WfCtx does not fork the causality).
#[test]
fn caused_emit_inherits_provenance_from_the_parent() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    // ONE shared minter across both contexts (so the second emit gets a DISTINCT ULID — modeling
    // the service-wide id source; two fresh minters would collide on the first id).
    let minter = minter();
    let mut ctx = begin_with(&outbox, journal, minter.clone());

    let root_id = ctx.emit(step_draft(), None).expect("root emit");
    ctx.commit().expect("co-commit the root");
    let root_env = outbox.row(&root_id).expect("root row").envelope;

    // a second step emitted as a reaction to the root.
    let mut ctx2 = begin_with(&outbox, WfJournal::new(), minter);
    let mut child_draft = step_draft();
    child_draft.type_ = EventType("agent.run.step.child".into());
    let child_id = ctx2.emit(child_draft, Some(&root_env)).expect("caused emit");
    ctx2.commit().expect("co-commit the child");
    let child_env = outbox.row(&child_id).expect("child row").envelope;

    assert_eq!(child_env.depth, 1, "caused event is depth parent+1");
    assert_eq!(child_env.causation_id, Some(root_id), "the immediate parent is the causation");
    assert_eq!(
        child_env.correlation_id, root_env.correlation_id,
        "the ROOT correlation carries through the chain unchanged"
    );
}
