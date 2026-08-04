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

#[test]
fn provider_wfctx_journals_and_emits_through_the_one_outbox_path() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let mut ctx = begin(&outbox, journal.clone());

    ctx.activity(RetryPolicy::default_policy(), |_idem, _attempt| {
        Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
    })
    .expect("the activity runs");
    let emitted = ctx
        .emit(step_draft(), None)
        .expect("emit through the outbox");
    ctx.commit().expect("the journal + outbox co-commit");

    let hist = journal.history_for(&tenant(), "R1");
    assert_eq!(hist.len(), 1, "one history row journaled");
    assert_eq!(
        hist[0].command_id, "agent.run:0",
        "deterministic command_id from position"
    );
    assert_eq!(hist[0].kind, history_kind::ACTIVITY_COMPLETED);

    let row = outbox
        .row(&emitted)
        .expect("the emitted event is a committed outbox row");
    assert_eq!(
        row.envelope.type_,
        EventType("agent.run.step".into()),
        "the caller's type carries"
    );
    assert_eq!(
        row.subject,
        EvArtifactRef("myelin://acme/agent/run/R1".into()),
        "the caller's subject carries (hoisted for the broker subject)"
    );
    assert_eq!(
        row.aggregate,
        AggregateKey("run:R1".into()),
        "the per-aggregate ordering key carries"
    );
    assert_eq!(
        row.seq, 0,
        "first event for the aggregate is seq 0 (per-aggregate order)"
    );
}

#[test]
fn consumer_receives_the_emitted_event_with_carriage_and_causality() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let mut ctx = begin(&outbox, journal);
    let emitted = ctx.emit(step_draft(), None).expect("emit");
    ctx.commit().expect("co-commit");

    let envelope: EventEnvelope = outbox
        .row(&emitted)
        .expect("the consumer receives the committed event")
        .envelope;

    assert_eq!(envelope.type_, EventType("agent.run.step".into()));
    assert_eq!(
        envelope.tenant,
        tenant(),
        "the (tenant, region) partition key carries"
    );
    assert_eq!(envelope.region, Region("fr-par".into()));
    assert_eq!(envelope.depth, 0, "a root emit is depth 0 (BUS-5)");
    assert_eq!(
        envelope.correlation_id.0, envelope.event_id.0,
        "a root carries its own correlation (BUS-5, no second emit path forged it)"
    );
    assert_eq!(envelope.causation_id, None, "a root has no parent");
}

#[test]
fn caused_emit_inherits_provenance_from_the_parent() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let minter = minter();
    let mut ctx = begin_with(&outbox, journal, minter.clone());

    let root_id = ctx.emit(step_draft(), None).expect("root emit");
    ctx.commit().expect("co-commit the root");
    let root_env = outbox.row(&root_id).expect("root row").envelope;

    let mut ctx2 = begin_with(&outbox, WfJournal::new(), minter);
    let mut child_draft = step_draft();
    child_draft.type_ = EventType("agent.run.step.child".into());
    let child_id = ctx2
        .emit(child_draft, Some(&root_env))
        .expect("caused emit");
    ctx2.commit().expect("co-commit the child");
    let child_env = outbox.row(&child_id).expect("child row").envelope;

    assert_eq!(child_env.depth, 1, "caused event is depth parent+1");
    assert_eq!(
        child_env.causation_id,
        Some(root_id),
        "the immediate parent is the causation"
    );
    assert_eq!(
        child_env.correlation_id, root_env.correlation_id,
        "the ROOT correlation carries through the chain unchanged"
    );
}
