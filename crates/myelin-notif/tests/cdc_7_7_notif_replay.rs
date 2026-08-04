use myelin_events::{
    Actor, Consumer, DataRole, DedupLedger, EmitContextBase, OutboxStore, Region as BusRegion,
    ReindexSource, SnapshotScope, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::{
    build_router, inbox_parity_hash, notif_scope, signal_snapshot_subject, InboxProjection,
    NotifReindexer, SignalReindexSource, SignalRouter, NOTIF_OWNER_TOKEN,
};
use myelin_query::signals::{DedupKey, RuleId, Severity, Signal, SignalState};
use myelin_refs::ArtifactRef;
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("platform".into()),
        PrincipalKind::Service,
        tenant(),
    )
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: BusRegion("fr-par".into()),
        actor: Actor(principal()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        caused_by: None,
    }
}

fn signal(rule: &str, severity: Severity, subject: &str, dedup: &str) -> Signal {
    Signal {
        rule_id: RuleId(rule.into()),
        tenant: tenant(),
        severity,
        dedup_key: DedupKey(dedup.into()),
        subject: ArtifactRef(subject.into()),
        count: 1,
        state: SignalState::Open,
        first_seen: "2026-06-20T00:00:00Z".into(),
        last_seen: "2026-06-20T00:00:00Z".into(),
    }
}

fn live_msg(id: &str, sig: &Signal) -> myelin_events::Message {
    use myelin_events::{
        AggregateKey, CorrelationId, EventEnvelope, EventId, EventType, Visibility,
    };
    let subject = signal_snapshot_subject(sig);
    let env = EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("signal.opened".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: BusRegion("fr-par".into()),
        actor: Actor(principal()),
        subject: ArtifactRef(subject.clone()),
        aggregate: AggregateKey(format!("signal:{}", sig.dedup_key.0)),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::to_value(sig).unwrap(),
    };
    myelin_events::Message {
        subject,
        envelope: env,
    }
}

fn live_router(outbox: &OutboxStore) -> (Consumer<SignalRouter>, InboxProjection) {
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();
    (consumer, inbox)
}

#[test]
fn provider_replays_notif_owned_snapshots_on_the_whitelisted_subject() {
    let mut src = SignalReindexSource::new();
    src.upsert(
        signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/1",
            "run-1",
        ),
        1,
    );
    src.upsert(
        signal(
            "deploy_ok",
            Severity::Info,
            "myelin://acme/ci/run/2",
            "run-2",
        ),
        1,
    );

    assert_eq!(
        <SignalReindexSource as ReindexSource>::owner_token(&src),
        NOTIF_OWNER_TOKEN
    );

    let drafts = src.replay(&notif_scope("inbox:all"), None);
    assert_eq!(drafts.len(), 2, "the provider replays both curated Signals");
    for d in &drafts {
        assert!(
            d.subject.0.starts_with("sig.acme."),
            "snapshot on the whitelisted subject"
        );
        assert_eq!(d.type_.0, "notif.signal.snapshot");
        let _: Signal =
            serde_json::from_value(d.payload.clone()).expect("snapshot carries the Signal");
    }
}

#[test]
fn pair_replay_reindex_rebuilds_inbox_cold_equals_live() {
    let signals = [
        signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/1",
            "run-1",
        ),
        signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/2",
            "run-2",
        ),
        signal(
            "deploy_ok",
            Severity::Info,
            "myelin://acme/ci/run/3",
            "run-3",
        ),
    ];
    let outbox_router = OutboxStore::new();
    let (consumer, inbox) = live_router(&outbox_router);

    for (i, sig) in signals.iter().enumerate() {
        consumer.deliver(&live_msg(&format!("evt-{i}"), sig));
    }
    let live_hash = inbox_parity_hash(&inbox, &tenant());
    assert_eq!(inbox.len(), 3);

    let mut provider = SignalReindexSource::new();
    for (i, sig) in signals.iter().enumerate() {
        let _ = i;
        provider.upsert(sig.clone(), 1);
    }

    inbox.wipe_tenant(&tenant());
    assert!(inbox.is_empty());
    let reindexer = NotifReindexer::new(&consumer);
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&provider];
    let receipt = reindexer
        .reindex(
            &tenant(),
            &notif_scope("inbox:all"),
            None,
            sources,
            &mut outbox,
            ctx_base(),
        )
        .expect("reindex");
    assert_eq!(
        receipt.signals_replayed, 3,
        "the consumer re-ingested three through the live router"
    );

    assert_eq!(inbox.len(), 3, "the rebuilt inbox holds the three rows");
    assert_eq!(
        inbox_parity_hash(&inbox, &tenant()),
        live_hash,
        "cold == live (reindex-parity hash identical) - contract 7.7 replay half"
    );
}

#[test]
fn consumer_reindex_of_unknown_owner_is_loud() {
    let provider = SignalReindexSource::new();
    let outbox_router = OutboxStore::new();
    let (consumer, _inbox) = live_router(&outbox_router);
    let reindexer = NotifReindexer::new(&consumer);
    let unknown = SnapshotScope::new("refs", "edge:all");
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&provider];
    assert!(reindexer
        .reindex(&tenant(), &unknown, None, sources, &mut outbox, ctx_base())
        .is_err());
}
#[test]
fn reindex_seam_2_6_is_reachable() {
    let mut provider = SignalReindexSource::new();
    provider.upsert(
        signal("r", Severity::Error, "myelin://acme/ci/run/1", "k"),
        1,
    );
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&provider];
    let r = myelin_events::reindex::reindex(
        &notif_scope("inbox:all"),
        None,
        sources,
        &mut outbox,
        ctx_base(),
    )
    .expect("the 2.6 bus re-emit seam");
    assert_eq!(r.snapshots_emitted, 1);
}
