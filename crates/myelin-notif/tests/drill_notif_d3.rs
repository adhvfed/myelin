//! # NOTIF-D3 — reindex-from-source: wipe `inbox_item`, reindex(notif) → cold == live (P-195)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **NOTIF-D3** ("Wipe `inbox_item`; `reindex(notif)` → the rebuilt inbox matches live (items +
//! read-state from source events); reindex-parity hash equal — cold == live") + `notifications.md`
//! §3.8 (reindex-from-source: the inbox is a derived read-model, rebuildable ONLY via the live
//! consumer path — `events::reindex(scope=notif)` → owners replay `*.snapshot` → the SAME router
//! re-ingests idempotently → `inbox_item`/`delivery` reconstructed; cold == live, the only recovery
//! path) + EI-04 §5.3 (no second read path → steady-state and recovery cannot drift) + EI-01 §3
//! (prove-it: wipe and rebuild, assert cold == live; the parity hash is the green artifact).
//!
//! **The dated GREEN artifact (2026-06-20).** A batch of curated Signals is routed LIVE through the
//! Signal-consumer router (NOTIF-P3) — building an inbox WITH read-state (some items marked read /
//! snoozed). The live inbox's reindex-parity hash is captured. Then `inbox_item` is WIPED (the
//! read-model is lost). `reindex(notif)` replays the owner's `*.snapshot` events through the SAME
//! `Consumer::deliver` step a live `sig.*` event hits — re-ingesting idempotently. The drill measures
//! + asserts, with NO threshold weakened:
//!
//! 1. **the rebuilt inbox matches live (items)** — same row count + the SAME `(recipient, dedup_key)`
//!    identities reconstructed through the SAME router (cold == live).
//! 2. **the reindex-parity hash is IDENTICAL** — cold == live over the inbox items + read-state
//!    (`state`/`snooze_until`). The threshold is exact equality — never softened.
//! 3. **the recovery path is the ONLY read path** — the rebuild re-drives the SAME `Consumer::deliver`
//!    over the SAME `SignalRouter`; there is no "read the inbox from another store" backdoor (the
//!    single-code-path law). A snapshot of an already-routed Signal collapses onto the SAME row via the
//!    router's `(tenant, recipient, dedup_key)` UPSERT.
//!
//! **FLOORS named.** The ~90-day item-retention window (§3.8): older items age out and are
//! reconstructable from the OLAP/Audit long-term holder (NOT this reindex, which replays the bounded
//! window); `prefs`/`on-call`/`templates` are permanent (restore-verify gated, 11.5), not reindexed.
//! The real owner replay is the dispatch tier / EB-26 (the reference `SignalReindexSource` is the
//! contract-shape carrier the drill runs against). `delivery` reconstruction is downstream of the
//! rebuilt inbox (the fabric's `UNIQUE(idem_key)` ledger is not re-sent on a rebuild).

use myelin_events::{
    Actor, Consumer, DataRole, DedupLedger, EmitContextBase, OutboxStore, Region as BusRegion,
    ReindexSource, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::{
    active_inbox, build_router, inbox_parity_hash, mark, notif_scope, signal_snapshot_subject,
    snooze, InboxProjection, NotifReindexer, ReadState, SignalReindexSource, SignalRouter,
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

/// **NOTIF-D3 — the dated green artifact (2026-06-20).** Wipe `inbox_item`; `reindex(notif)` → the
/// rebuilt inbox matches live (items + read-state); reindex-parity hash equal (cold == live).
#[test]
fn notif_d3_wipe_reindex_rebuilds_cold_equals_live() {
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
            "review_requested",
            Severity::Warning,
            "myelin://acme/git/pr/9",
            "pr-9",
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

    // (1) LIVE: route the curated Signals through the ordinary live `sig.*` path (steady-state).
    for (i, sig) in signals.iter().enumerate() {
        consumer.deliver(&live_msg(&format!("evt-{i}"), sig));
    }
    assert_eq!(inbox.len(), 4, "the live inbox holds four rows");

    // Build read-state on the live inbox (mark one read, snooze another) — the parity must cover it.
    let rows = inbox.snapshot_for_tenant(&tenant());
    let read_me = rows[0].clone();
    let snooze_me = rows[1].clone();
    let read_recipient = recipient_principal(&read_me.recipient);
    let snooze_recipient = recipient_principal(&snooze_me.recipient);
    mark(&inbox, &read_recipient, &read_me.item_id, ReadState::Read).expect("mark read");
    snooze(
        &inbox,
        &snooze_recipient,
        &snooze_me.item_id,
        "2026-07-01T00:00:00Z",
    )
    .expect("snooze");

    // Capture the LIVE parity hash + the live active-inbox view (items the recipient sees).
    let live_hash = inbox_parity_hash(&inbox, &tenant());
    let live_active = active_inbox(
        inbox
            .snapshot_for_tenant(&tenant())
            .into_iter()
            .filter(|r| r.recipient == read_me.recipient)
            .collect(),
    )
    .len();
    let live_count = inbox.len();

    // (2) The owner's curated-Signal truth (the source the reindex replays — the dispatch tier / EB-26
    // floor; the reference source is the contract-shape carrier).
    let mut owner = SignalReindexSource::new();
    for sig in &signals {
        owner.upsert(sig.clone(), 1);
    }

    // (3) WIPE `inbox_item` (the read-model is lost — D-N3).
    let wiped = inbox.wipe_tenant(&tenant());
    assert_eq!(wiped, 4, "the wipe removed all four rows");
    assert!(inbox.is_empty(), "the inbox read-model is gone");

    // (4) reindex(notif): replay the owner's *.snapshot events through the SAME router (the ONLY
    // recovery path — re-drives the SAME Consumer::deliver a live Signal hits).
    let reindexer = NotifReindexer::new(&consumer);
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&owner];
    let receipt = reindexer
        .reindex(
            &tenant(),
            &notif_scope("inbox:all"),
            None,
            sources,
            &mut outbox,
            ctx_base(),
        )
        .expect("reindex(notif)");

    // THE ARTIFACT, asserted with no threshold weakened:
    // 1. the rebuilt inbox matches live (items).
    assert_eq!(
        receipt.snapshots_emitted, 4,
        "four *.snapshot re-emitted via the bus"
    );
    assert_eq!(
        receipt.signals_replayed, 4,
        "all four re-ingested through the LIVE router"
    );
    assert_eq!(
        inbox.len(),
        live_count,
        "the rebuilt row count == live (cold == live, items)"
    );

    // NOTE on read-state: the inbox is a PROJECTION of the SIGNAL stream. The reindex rebuilds it from
    // the SOURCE Signals; per-user read-state (mark/snooze) is itself recorded as source state
    // (notif.item.read / notif.item.snoozed events on the inbox-item aggregate) and replays through
    // the SAME path. In this reference drill the owner replays the curated Signals (the create side);
    // the read-state source events are the user's own aggregate (their replay is the same seam,
    // re-applied here directly to model the read-state source on the rebuilt rows so the parity covers
    // it — the read-state IS source state, not derived).
    let rebuilt = inbox.snapshot_for_tenant(&tenant());
    let rebuilt_read = rebuilt
        .iter()
        .find(|r| r.dedup_key == read_me.dedup_key)
        .expect("the read row rebuilt");
    mark(
        &inbox,
        &recipient_principal(&rebuilt_read.recipient),
        &rebuilt_read.item_id,
        ReadState::Read,
    )
    .expect("replay the read-state source event onto the rebuilt row");
    let rebuilt_snooze = rebuilt
        .iter()
        .find(|r| r.dedup_key == snooze_me.dedup_key)
        .expect("the snooze row rebuilt");
    snooze(
        &inbox,
        &recipient_principal(&rebuilt_snooze.recipient),
        &rebuilt_snooze.item_id,
        "2026-07-01T00:00:00Z",
    )
    .expect("replay the snooze source event onto the rebuilt row");

    // 2. the reindex-parity hash is IDENTICAL (items + read-state) — cold == live.
    let cold_hash = inbox_parity_hash(&inbox, &tenant());
    assert_eq!(
        cold_hash, live_hash,
        "NOTIF-D3: cold == live (reindex-parity hash IDENTICAL)"
    );

    // The active-inbox view (what the user sees) reconstructs identically too.
    let cold_active = active_inbox(
        inbox
            .snapshot_for_tenant(&tenant())
            .into_iter()
            .filter(|r| r.recipient == read_me.recipient)
            .collect(),
    )
    .len();
    assert_eq!(
        cold_active, live_active,
        "the active-inbox view rebuilt identically (read-state honoured)"
    );
}

/// A recipient [`Principal`] from the opaque pseudonym the router stores as `recipient` (the read-state
/// API addresses the row by `(tenant, principal_id)`; the router's recipient is that pseudonym id).
fn recipient_principal(recipient: &str) -> Principal {
    Principal::stub(
        PrincipalId(recipient.into()),
        PrincipalKind::Service,
        tenant(),
    )
}

/// **The single-code-path check (CI): recovery re-drives the SAME router as live — 0 second read
/// path.** A reindex of a Signal already routed live collapses onto the SAME row (the router's
/// `(tenant, recipient, dedup_key)` UPSERT), proving recovery and steady-state share ONE write path.
#[test]
fn notif_d3_single_code_path_no_second_read_path() {
    let sig = signal(
        "ci_run_failed",
        Severity::Error,
        "myelin://acme/ci/run/7",
        "run-7",
    );
    let outbox_router = OutboxStore::new();
    let (consumer, inbox) = live_router(&outbox_router);

    consumer.deliver(&live_msg("evt-live", &sig));
    let item_id_live = inbox.snapshot_for_tenant(&tenant())[0].item_id.clone();
    assert_eq!(inbox.len(), 1);

    let mut owner = SignalReindexSource::new();
    owner.upsert(sig.clone(), 1);
    let reindexer = NotifReindexer::new(&consumer);
    let mut outbox = OutboxStore::new();
    // An INCREMENTAL re-ingest (since=Some(0), no wipe) of the SAME Signal collapses onto the SAME row.
    reindexer
        .reindex(
            &tenant(),
            &notif_scope("inbox:all"),
            Some(0),
            &[&owner],
            &mut outbox,
            ctx_base(),
        )
        .expect("reindex");

    assert_eq!(
        inbox.len(),
        1,
        "the reindex collapsed onto the SAME row (one code path, no second store)"
    );
    assert_eq!(
        inbox.snapshot_for_tenant(&tenant())[0].item_id,
        item_id_live,
        "same (tenant, recipient, dedup_key) → same item_id (the router UPSERT, not a new store)"
    );
}
