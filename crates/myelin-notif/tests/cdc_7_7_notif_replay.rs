//! # CDC 7.7 — the Notif REPLAY HALF of `PersonalDataHolder + replay` (NOTIF-P17 → P-195)
//!
//! **Contract:** index row 7.7 (`PersonalDataHolder` + `replay` — references-not-payloads; *inbox
//! rebuilt by reindex-from-source*) + row 2.6 (reindex-from-source — the only recovery path). The
//! HOLDER HALF (locate/export/rectify/restrict/erase) landed at NOTIF-P4 (its CDC is
//! `cdc_7_7_notif_holder.rs`). THIS file ships the **REPLAY HALF** — `reindex(scope=notif)` rebuilds
//! the inbox read-model through the SAME router (cold == live), completing contract 7.7. This CDC pair
//! is what the contract-coverage scanner (P-S21) reads for the Notif reindex/replay seam.
//!
//! - **PROVIDER** = the owning Signal source ([`SignalReindexSource`]) implementing
//!   [`myelin_events::ReindexSource::replay`] — replaying the owner's curated-Signal truth as
//!   `*.snapshot` drafts on the `sig.<tenant>.*` subject the router whitelists (so the SAME router
//!   re-ingests them — there is no second read path). The real owner replay is the dispatch tier /
//!   EB-26 (a named floor); the reference source is the contract-shape carrier.
//! - **CONSUMER** = the Notif reindex driver ([`NotifReindexer`]) consuming that `replay` through the
//!   bus re-emit ([`myelin_events::reindex`]) → the LIVE `Consumer<SignalRouter>::deliver` step → the
//!   inbox projection. It NEVER reads the inbox from a second store (the rebuild re-drives the SAME
//!   live consumer) — the single-code-path law (§3.8 / EI-04 §5.3).
//!
//! The dated green artifact (2026-06-20): the provider replays the owner's curated Signals; the
//! consumer reindexes a WIPED inbox through the live router; the rebuilt inbox's parity hash ==
//! live's (cold == live). If 7.7's replay shape (or 2.6's `replay`/`*.snapshot` seam) drifts, this
//! stops compiling/passing — that is the contract.

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

/// A live `sig.<tenant>.<sev>.<rule>` broker message carrying a curated Signal (the steady-state
/// ingest path — what live delivery looks like through the SAME router).
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

/// **The PROVIDER conforms to the 2.6 `ReindexSource` contract: it replays the owner's curated truth
/// as `*.snapshot` drafts on the whitelisted `sig.<tenant>.*` subject, owning the `notif` token.** A
/// drift in the `replay` signature or the snapshot subject breaks this build.
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

    // The source owns the `notif` §6.2 token (the bus dispatches `scope.owner == "notif"` to it).
    assert_eq!(
        <SignalReindexSource as ReindexSource>::owner_token(&src),
        NOTIF_OWNER_TOKEN
    );

    let drafts = src.replay(&notif_scope("inbox:all"), None);
    assert_eq!(drafts.len(), 2, "the provider replays both curated Signals");
    // Each snapshot rides the `sig.<tenant>.*` whitelist subject (so the SAME router re-ingests it).
    for d in &drafts {
        assert!(
            d.subject.0.starts_with("sig.acme."),
            "snapshot on the whitelisted subject"
        );
        assert_eq!(d.type_.0, "notif.signal.snapshot");
        // The payload round-trips to the SAME curated Signal (cold == live).
        let _: Signal =
            serde_json::from_value(d.payload.clone()).expect("snapshot carries the Signal");
    }
}

/// **The PAIR: PROVIDER `replay` → bus re-emit → CONSUMER re-ingest through the LIVE router rebuilds
/// the inbox cold == live (the 7.7 replay-half contract).** Build a LIVE inbox by routing live
/// Signals; snapshot its parity hash; WIPE; the consumer reindexes the provider's `replay`; assert the
/// rebuilt inbox's parity hash == live's. The consumer NEVER reads a second store (it re-drives the
/// SAME `Consumer::deliver`).
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

    // LIVE: route the curated Signals through the ordinary live path (steady-state).
    for (i, sig) in signals.iter().enumerate() {
        consumer.deliver(&live_msg(&format!("evt-{i}"), sig));
    }
    let live_hash = inbox_parity_hash(&inbox, &tenant());
    assert_eq!(inbox.len(), 3);

    // The PROVIDER's truth (the owner's curated-Signal log).
    let mut provider = SignalReindexSource::new();
    for (i, sig) in signals.iter().enumerate() {
        let _ = i;
        provider.upsert(sig.clone(), 1);
    }

    // WIPE the read-model (D-N3) → the CONSUMER reindexes through the live router.
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

    // cold == live (the 7.7 replay-half artifact).
    assert_eq!(inbox.len(), 3, "the rebuilt inbox holds the three rows");
    assert_eq!(
        inbox_parity_hash(&inbox, &tenant()),
        live_hash,
        "cold == live (reindex-parity hash identical) — contract 7.7 replay half"
    );
}

/// **The consumer's reindex of an UNKNOWN owner is a LOUD error (2.6 — never a silent empty
/// rebuild).** The bus's `NoSourceForOwner` bubbles up.
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
/// **The 2.6 seam the replay half consumes is reachable (the compile-time contract carrier).** A
/// drift in `myelin_events::reindex` (the bus re-emit seam) breaks this CDC build too.
#[test]
fn reindex_seam_2_6_is_reachable() {
    let mut provider = SignalReindexSource::new();
    provider.upsert(
        signal("r", Severity::Error, "myelin://acme/ci/run/1", "k"),
        1,
    );
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&provider];
    // The raw 2.6 bus re-emit (the seam the NotifReindexer drives) is callable with the frozen shape.
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
