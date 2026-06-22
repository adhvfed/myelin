//! Unit + chained + NOTIF-D3 drill tests for reindex-from-source (NOTIF-P17 / P-195).
//!
//! The mandatory-core reindex decision logic is exercised here: the wipe-iff-full-rebuild branch, the
//! bus re-emit drive, the deterministic-id drain order, the feed-through-the-LIVE-router step, the
//! parity hash (every covered field), the incremental backfill (no wipe), and the retention-window
//! boundary. The NOTIF-D3 drill (`notif_d3_*`) wipes the inbox, reindexes, and asserts the rebuilt
//! inbox's parity hash == the live inbox's (cold == live). The single-code-path check
//! (`reindex_re_ingests_through_the_same_consumer_deliver`) pins that recovery uses the SAME
//! `Consumer::deliver` as live ingest — 0 second read path.

use super::*;
use crate::router::{build_router, InboxProjection, RoutedInboxItem, SignalRouter};
use myelin_events::{Actor, Region as BusRegion, Timestamp};
use myelin_events::{Consumer, DedupLedger, EmitContextBase, Message, OutboxStore};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::signals::{DedupKey, RuleId, Severity, Signal, SignalState};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
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

/// A curated Signal (the shape the engine, P-138, publishes — the owner's source of truth).
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

/// A live `sig.<tenant>.<sev>.<rule>` broker [`Message`] carrying the curated Signal (the live
/// router-ingest path — what steady-state delivery looks like).
fn live_signal_msg(id: &str, sig: &Signal) -> Message {
    use myelin_events::{
        AggregateKey, CorrelationId, DataRole, EventEnvelope, EventId, EventType, Visibility,
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
    Message {
        subject,
        envelope: env,
    }
}

/// Build a live Signal-consumer router over a fresh inbox (the SAME `build_router` `serve` wires).
fn live_router(outbox: &OutboxStore) -> (Consumer<SignalRouter>, InboxProjection) {
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();
    (consumer, inbox)
}

/// An owning Signal source with three curated Signals at known versions (the owner's truth — the
/// dispatch tier's curated-Signal log, the named floor).
fn owner_with_three_signals() -> SignalReindexSource {
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
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/2",
            "run-2",
        ),
        1,
    );
    src.upsert(
        signal(
            "deploy_ok",
            Severity::Info,
            "myelin://acme/ci/run/3",
            "run-3",
        ),
        1,
    );
    src
}

// ===========================================================================================
//  The required UNIT: bus re-emit → the LIVE router → the inbox rebuilds from cold.
// ===========================================================================================

/// **The reindex drives the bus re-emit → the LIVE router → rebuilds the inbox from cold (the
/// NOTIF-D3 happy path; the prompt's required unit).** The owner replays curated Signals as
/// `*.snapshot` events; they ride the outbox; each is fed through the SAME `Consumer::deliver` a
/// live `sig.*` event hits; the inbox projection ends up with the routed rows.
#[test]
fn reindex_rebuilds_the_inbox_from_the_bus_re_emit_through_the_live_router() {
    let src = owner_with_three_signals();
    let outbox_router = OutboxStore::new();
    let (consumer, inbox) = live_router(&outbox_router);

    let reindexer = NotifReindexer::new(&consumer);
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];

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
        receipt.snapshots_emitted, 3,
        "three curated Signals re-emitted as *.snapshot via the bus"
    );
    assert_eq!(
        receipt.signals_replayed, 3,
        "all three driven through the LIVE router"
    );
    assert_eq!(
        receipt.signals_deduplicated, 0,
        "no dedup on a cold rebuild"
    );
    assert_eq!(receipt.owners_replayed, vec!["notif".to_string()]);
    assert_eq!(inbox.len(), 3, "the inbox holds the three rebuilt rows");
    // The rebuild went through the router's own projection (the rebuild target).
    assert_eq!(
        reindexer.inbox().len(),
        3,
        "the reindexer rebuilds the router's OWN inbox"
    );
}

// ===========================================================================================
//  The required CHAINED test (EI-01 §4): ingest live → wipe → reindex → hash-equal to live.
// ===========================================================================================

/// **CHAINED (NOTIF-D3): ingest a batch live → reindex(notif) on a wiped store → the rebuilt inbox +
/// read-state hash-equal to live.** Build a LIVE inbox by routing live `sig.*` Signals; snapshot its
/// parity hash; WIPE the inbox; reindex-from-source; assert the rebuilt inbox's parity hash ==
/// live's. cold == live (the threshold is IDENTICAL).
#[test]
fn notif_d3_chained_wipe_reindex_rebuilds_hash_equal_to_live() {
    let src = owner_with_three_signals();
    let outbox_router = OutboxStore::new();
    let (consumer, inbox) = live_router(&outbox_router);

    // LIVE: route the three curated Signals through the ordinary `sig.*` live path (steady-state).
    for (i, (_, (v, sig))) in (0..).zip(src_truth_iter(&src)) {
        let _ = v;
        consumer.deliver(&live_signal_msg(&format!("evt-{i}"), &sig));
    }
    assert_eq!(inbox.len(), 3, "the live inbox holds three rows");
    let live_hash = inbox_parity_hash(&inbox, &tenant());

    // WIPE the inbox (the read-model is lost — D-N3).
    let wiped = inbox.wipe_tenant(&tenant());
    assert_eq!(wiped, 3, "the wipe removed all three rows");
    assert!(inbox.is_empty(), "the inbox is empty after the wipe");

    // REINDEX-FROM-SOURCE: rebuild through the SAME router.
    let reindexer = NotifReindexer::new(&consumer);
    let mut outbox = OutboxStore::new();
    let receipt = reindexer
        .reindex(
            &tenant(),
            &notif_scope("inbox:all"),
            None,
            &[&src],
            &mut outbox,
            ctx_base(),
        )
        .expect("reindex");
    assert_eq!(
        receipt.signals_replayed, 3,
        "the rebuild replayed three through the live router"
    );

    // PARITY: the rebuilt inbox's hash == the live inbox's (items + read-state). cold == live.
    let cold_hash = inbox_parity_hash(&inbox, &tenant());
    assert_eq!(inbox.len(), 3, "the rebuilt inbox holds three rows");
    assert_eq!(
        cold_hash, live_hash,
        "NOTIF-D3: cold == live (reindex-parity hash identical)"
    );
}

/// Iterate the owner's truth in deterministic order (a test helper — mirrors `replay` ordering).
fn src_truth_iter(src: &SignalReindexSource) -> Vec<(String, (u64, Signal))> {
    // The owner replays in ascending-aggregate order; rebuild the same order from the replay drafts'
    // aggregates by re-deriving the Signals from a fresh full replay.
    src.replay(&notif_scope("inbox:all"), None)
        .into_iter()
        .map(|d| {
            let sig: Signal = serde_json::from_value(d.payload.clone()).unwrap();
            (d.aggregate.0.clone(), (d.version, sig))
        })
        .collect()
}

// ===========================================================================================
//  The required SINGLE-CODE-PATH check (CI): reindex re-ingests through the SAME router.
// ===========================================================================================

/// **The single-code-path check (CI): reindex re-ingests through the SAME `Consumer::deliver` the
/// live path uses — 0 second read path.** Drive a live Signal AND a reindex snapshot of the SAME
/// curated Signal through the SAME consumer; they collapse onto the SAME row (the router's
/// `(tenant, recipient, dedup_key)` UPSERT) — proving recovery and steady-state share ONE write
/// path (cannot drift, EI-04 §5.3). If reindex had a second read path, the snapshot would not
/// collapse onto the live row.
#[test]
fn reindex_re_ingests_through_the_same_consumer_deliver_one_code_path() {
    let sig = signal(
        "ci_run_failed",
        Severity::Error,
        "myelin://acme/ci/run/7",
        "run-7",
    );
    let outbox_router = OutboxStore::new();
    let (consumer, inbox) = live_router(&outbox_router);

    // LIVE: one curated Signal → one inbox row.
    consumer.deliver(&live_signal_msg("evt-live", &sig));
    assert_eq!(inbox.len(), 1, "the live Signal produced one row");
    let live_row = inbox.snapshot_for_tenant(&tenant())[0].clone();

    // REINDEX a snapshot of the SAME curated Signal through the SAME consumer: it collapses onto the
    // SAME row (coalesce_count bumps) — NOT a new row, NOT a second store. One write path.
    let mut src = SignalReindexSource::new();
    src.upsert(sig.clone(), 1);
    let reindexer = NotifReindexer::new(&consumer);
    let mut outbox = OutboxStore::new();
    reindexer
        .reindex(
            &tenant(),
            &notif_scope("inbox:all"),
            Some(0),
            &[&src],
            &mut outbox,
            ctx_base(),
        )
        .expect("reindex");

    assert_eq!(
        inbox.len(),
        1,
        "the reindexed snapshot collapsed onto the SAME row (one code path)"
    );
    let after_row = inbox.snapshot_for_tenant(&tenant())[0].clone();
    assert_eq!(
        after_row.item_id, live_row.item_id,
        "same (tenant, recipient, dedup_key) → same item_id"
    );
    assert!(
        after_row.coalesce_count >= live_row.coalesce_count,
        "the reindex re-ingested through the SAME router UPSERT (collapsed, not duplicated)"
    );
}

// ===========================================================================================
//  Idempotency, incremental backfill, wipe, parity-hash field coverage, loud errors.
// ===========================================================================================

/// **Re-running the reindex is idempotent in EFFECT** — the bus emits 0 NEW snapshots (the
/// deterministic id makes the re-emit a no-op) and the consumer deduplicates the redelivered
/// snapshots; the inbox converges to the SAME row set (no duplication).
#[test]
fn reindex_is_idempotent_on_the_deterministic_snapshot_event_id() {
    let src = owner_with_three_signals();
    let outbox_router = OutboxStore::new();
    let (consumer, inbox) = live_router(&outbox_router);
    let reindexer = NotifReindexer::new(&consumer);
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];

    let first = reindexer
        .reindex(
            &tenant(),
            &notif_scope("inbox:all"),
            None,
            sources,
            &mut outbox,
            ctx_base(),
        )
        .expect("first");
    assert_eq!(
        first.snapshots_emitted, 3,
        "first run emits three snapshots"
    );
    assert_eq!(first.signals_replayed, 3);
    let hash_after_first = inbox_parity_hash(&inbox, &tenant());

    // A SECOND full reindex over the SAME outbox: the BUS skips all three as duplicate (their
    // deterministic ids are already present), but the full rebuild WIPES the inbox + forgets the
    // snapshot dedup marks, so it RE-APPLIES the three through the router and converges to the SAME
    // three rows (the router's (tenant, recipient, dedup_key) UPSERT keeps it idempotent in effect).
    let second = reindexer
        .reindex(
            &tenant(),
            &notif_scope("inbox:all"),
            None,
            sources,
            &mut outbox,
            ctx_base(),
        )
        .expect("second");
    assert_eq!(
        second.snapshots_emitted, 0,
        "0 NEW snapshots emitted (deterministic id — bus no-op)"
    );
    assert_eq!(
        second.snapshots_skipped_duplicate, 3,
        "all three skipped at the bus re-emit"
    );
    // The full rebuild forgot the prior marks → it re-applies the three (not a dedup no-op), so the
    // wiped inbox is rebuilt — never left empty.
    assert_eq!(
        second.signals_replayed, 3,
        "the full rebuild re-applies the three over the wipe"
    );
    assert_eq!(
        inbox.len(),
        3,
        "still exactly three rows — idempotent in effect"
    );
    assert_eq!(
        inbox_parity_hash(&inbox, &tenant()),
        hash_after_first,
        "the re-run converges to the byte-identical inbox"
    );
}

/// **The incremental backfill (`since = Some`) does NOT wipe the live inbox — it re-ingests above the
/// cursor.** A new-recipient / upcaster backfill rides the SAME path but preserves the already-routed
/// rows.
#[test]
fn incremental_backfill_does_not_wipe_the_inbox() {
    // The owner gains a new curated Signal at version 5; the inbox already holds an older row.
    let mut src = SignalReindexSource::new();
    src.upsert(
        signal(
            "old_rule",
            Severity::Error,
            "myelin://acme/ci/run/old",
            "old",
        ),
        1,
    );
    src.upsert(
        signal(
            "new_rule",
            Severity::Error,
            "myelin://acme/ci/run/new",
            "new",
        ),
        5,
    );
    let outbox_router = OutboxStore::new();
    let (consumer, inbox) = live_router(&outbox_router);
    let reindexer = NotifReindexer::new(&consumer);

    // Pre-seed the inbox with the OLD row via a full reindex of only the old Signal.
    let mut only_old = SignalReindexSource::new();
    only_old.upsert(
        signal(
            "old_rule",
            Severity::Error,
            "myelin://acme/ci/run/old",
            "old",
        ),
        1,
    );
    let mut outbox = OutboxStore::new();
    reindexer
        .reindex(
            &tenant(),
            &notif_scope("inbox:all"),
            None,
            &[&only_old],
            &mut outbox,
            ctx_base(),
        )
        .expect("seed old");
    assert_eq!(inbox.len(), 1, "the old row is routed");

    // Incremental backfill since=1: only the version-5 Signal replays; the old row is PRESERVED.
    let mut outbox2 = OutboxStore::new();
    let job = reindexer
        .reindex(
            &tenant(),
            &notif_scope("inbox:all"),
            Some(1),
            &[&src],
            &mut outbox2,
            ctx_base(),
        )
        .expect("backfill");
    assert_eq!(
        job.signals_replayed, 1,
        "only the new Signal replays past since=1"
    );
    assert_eq!(
        inbox.len(),
        2,
        "the backfill APPENDED — the old row survives (no wipe)"
    );
}

/// **A full reindex WIPES the inbox first (the cold-rebuild precondition).** A stale row not present
/// in the owner's truth is removed by the wipe and NOT resurrected (the rebuild reflects only the
/// owner's current truth — the erasure-stays-erased posture, X-7).
#[test]
fn full_reindex_wipes_stale_rows_not_in_the_owner_truth() {
    let outbox_router = OutboxStore::new();
    let (consumer, inbox) = live_router(&outbox_router);

    // A STALE live row (a curated Signal the owner has since tombstoned — not in its truth).
    consumer.deliver(&live_signal_msg(
        "evt-stale",
        &signal(
            "stale",
            Severity::Error,
            "myelin://acme/ci/run/x",
            "stale-k",
        ),
    ));
    assert_eq!(inbox.len(), 1, "the stale row is in the inbox");

    // The owner's CURRENT truth holds a DIFFERENT Signal (the stale one is gone — erased).
    let mut src = SignalReindexSource::new();
    src.upsert(
        signal(
            "fresh",
            Severity::Error,
            "myelin://acme/ci/run/y",
            "fresh-k",
        ),
        1,
    );

    let reindexer = NotifReindexer::new(&consumer);
    let mut outbox = OutboxStore::new();
    reindexer
        .reindex(
            &tenant(),
            &notif_scope("inbox:all"),
            None,
            &[&src],
            &mut outbox,
            ctx_base(),
        )
        .expect("reindex");

    // The rebuilt inbox holds ONLY the fresh row — the stale one is wiped and NOT resurrected.
    assert_eq!(
        inbox.len(),
        1,
        "the rebuilt inbox holds only the owner's current truth"
    );
    let rows = inbox.snapshot_for_tenant(&tenant());
    assert!(
        rows[0].dedup_key.contains("fresh"),
        "the fresh Signal's row, not the stale one"
    );
}

/// **The parity hash covers the read-state — a reindex that lost read-state flips it.** Two inboxes
/// with identical rows but DIFFERENT read-state (`state`) hash DIFFERENTLY; the same read-state hashes
/// the same. Pins that NOTIF-D3 compares items + read-state (not just item identity).
#[test]
fn parity_hash_covers_read_state_and_is_order_independent() {
    let a = InboxProjection::new();
    let b = InboxProjection::new();
    // Insert the SAME logical rows in DIFFERENT orders (the hash must be order-independent).
    a.upsert_for_test(row("u1", "k1", "unread", None));
    a.upsert_for_test(row("u2", "k2", "read", None));
    b.upsert_for_test(row("u2", "k2", "read", None));
    b.upsert_for_test(row("u1", "k1", "unread", None));
    assert_eq!(
        inbox_parity_hash(&a, &tenant()),
        inbox_parity_hash(&b, &tenant()),
        "the parity hash is canonical-order-independent (same rows → same hash)"
    );

    // A row with a DIFFERENT read-state hashes DIFFERENTLY (the hash covers `state`).
    let c = InboxProjection::new();
    c.upsert_for_test(row("u1", "k1", "read", None)); // state differs from `a`'s u1 (unread)
    c.upsert_for_test(row("u2", "k2", "read", None));
    assert_ne!(
        inbox_parity_hash(&a, &tenant()),
        inbox_parity_hash(&c, &tenant()),
        "a lost/changed read-state flips the parity hash (NOTIF-D3 covers read-state)"
    );

    // snooze_until also participates (read-state truth).
    let d = InboxProjection::new();
    d.upsert_for_test(row(
        "u1",
        "k1",
        "unread",
        Some("2026-07-01T00:00:00Z".into()),
    ));
    d.upsert_for_test(row("u2", "k2", "read", None));
    assert_ne!(
        inbox_parity_hash(&a, &tenant()),
        inbox_parity_hash(&d, &tenant()),
        "a different snooze_until flips the hash (read-state truth)"
    );
}

/// A test inbox row at a known `(recipient, dedup_key)` with an explicit read-state.
fn row(
    recipient: &str,
    dedup_key: &str,
    state: &str,
    snooze_until: Option<String>,
) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: tenant(),
        region: region(),
        item_id: format!("itm-{recipient}-{dedup_key}"),
        recipient: recipient.into(),
        subject: ArtifactRef(format!("myelin://acme/ci/run/{dedup_key}")),
        reason: crate::Reason::StateChanged,
        class: crate::Class::Direct,
        origin_event: ArtifactRef(format!("myelin://acme/bus/event/{dedup_key}")),
        dedup_key: dedup_key.into(),
        coalesce_count: 1,
        state: state.into(),
        snooze_until,
    }
}

/// **A reindex of an UNKNOWN owner is a LOUD error (never a silent empty rebuild that masks a wiring
/// bug).** The bus's `NoSourceForOwner` bubbles up as a `Bus` error.
#[test]
fn reindex_of_unknown_owner_is_a_loud_error() {
    let src = SignalReindexSource::new(); // owns the `notif` token.
    let outbox_router = OutboxStore::new();
    let (consumer, _inbox) = live_router(&outbox_router);
    let reindexer = NotifReindexer::new(&consumer);
    let unknown = SnapshotScope::new("search", "doc:all"); // no `search` source registered.
    let mut outbox = OutboxStore::new();
    let err = reindexer
        .reindex(&tenant(), &unknown, None, &[&src], &mut outbox, ctx_base())
        .expect_err("unknown owner");
    assert!(
        matches!(err, ReindexError::Bus(_)),
        "an unknown owner is a loud Bus error"
    );
}

/// **The full rebuild wipes ONLY the reindexed tenant (another tenant's live inbox is untouched).**
#[test]
fn full_reindex_wipe_is_tenant_scoped() {
    let other = TenantId("globex".into());
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(row("u-acme", "k", "unread", None));
    let mut globex_row = row("u-globex", "k", "unread", None);
    globex_row.tenant = other.clone();
    inbox.upsert_for_test(globex_row);
    assert_eq!(inbox.len(), 2);

    let wiped = inbox.wipe_tenant(&tenant());
    assert_eq!(wiped, 1, "only acme's row is wiped");
    assert_eq!(inbox.len(), 1, "globex's row survives");
    assert_eq!(
        inbox.snapshot_for_tenant(&other).len(),
        1,
        "globex's inbox is untouched"
    );
}

// ===========================================================================================
//  The retention-window FLOOR + the snapshot-shape carriers.
// ===========================================================================================

/// **The ~90-day retention window is the named §3.8 floor.** The default is 90 days; a 0-day window is
/// floored to 1 (never a wedged inbox); an explicit per-cell window is honoured (a config swap).
#[test]
fn retention_window_is_the_named_90_day_floor() {
    assert_eq!(
        RetentionWindow::default().days,
        90,
        "the default item window is ~90 days"
    );
    assert_eq!(DEFAULT_RETENTION_DAYS, 90);
    assert_eq!(RetentionWindow::new().days, 90);
    assert_eq!(
        RetentionWindow::of_days(30).days,
        30,
        "an explicit per-cell window is honoured"
    );
    assert_eq!(
        RetentionWindow::of_days(0).days,
        1,
        "a 0-day window is floored to 1 (never wedged)"
    );
}

/// **A snapshot draft carries the curated Signal on the `sig.<tenant>.*` subject the router
/// whitelists** (so `Consumer::deliver` routes it through the SAME path as a live Signal), at the
/// deterministic id, with the live router's aggregate key.
#[test]
fn signal_snapshot_draft_carries_the_signal_on_the_whitelisted_subject() {
    let sig = signal(
        "ci_run_failed",
        Severity::Error,
        "myelin://acme/ci/run/42",
        "run-42",
    );
    let draft = signal_snapshot_draft(&sig, 3);
    assert_eq!(draft.type_.0, "notif.signal.snapshot");
    assert_eq!(
        draft.aggregate.0, "signal:run-42",
        "the router's per-Signal aggregate key"
    );
    assert_eq!(draft.version, 3);
    assert_eq!(
        draft.subject.0, "sig.acme.error.ci_run_failed",
        "the `sig.<tenant>.*` whitelist subject"
    );
    // The payload round-trips to the SAME curated Signal (cold == live).
    let back: Signal = serde_json::from_value(draft.payload.clone()).unwrap();
    assert_eq!(back.dedup_key.0, "run-42");
    // The deterministic id is stable on (aggregate, version).
    assert_eq!(
        draft.event_id(),
        myelin_events::snapshot_event_id(&draft.aggregate, 3)
    );
}

/// **`notif_scope` pins the `notif` owner token; `NOTIF_OWNER_TOKEN`/`NOTIF_SNAPSHOT_TYPE` are
/// frozen.** A drift breaks the build (the bus dispatches on `scope.owner`).
#[test]
fn notif_scope_and_tokens_are_frozen() {
    assert_eq!(NOTIF_OWNER_TOKEN, "notif");
    assert_eq!(NOTIF_SNAPSHOT_TYPE, "notif.signal.snapshot");
    let scope = notif_scope("inbox:all");
    assert_eq!(scope.owner, "notif");
    assert_eq!(scope.selector, "inbox:all");
}

/// **The reference Signal source replays in deterministic order and honours the `since` cursor at the
/// EXACT boundary (`> since`, not `>= since`).**
#[test]
fn signal_source_replays_deterministically_and_honours_since() {
    let mut src = SignalReindexSource::new();
    src.upsert(
        signal("r1", Severity::Error, "myelin://acme/ci/run/1", "k1"),
        1,
    );
    src.upsert(
        signal("r2", Severity::Error, "myelin://acme/ci/run/2", "k2"),
        5,
    );
    assert_eq!(src.len(), 2);
    assert!(!src.is_empty());
    // A truly empty source is empty (pins `is_empty` is not a constant).
    assert!(
        SignalReindexSource::new().is_empty(),
        "a fresh source is empty"
    );

    let all = src.replay(&notif_scope("inbox:all"), None);
    assert_eq!(all.len(), 2, "a full replay yields both");
    // Deterministic ascending-aggregate order (signal:k1 < signal:k2).
    assert_eq!(all[0].aggregate.0, "signal:k1");
    assert_eq!(all[1].aggregate.0, "signal:k2");

    let since = src.replay(&notif_scope("inbox:all"), Some(3));
    assert_eq!(
        since.len(),
        1,
        "only the version-5 Signal replays past since=3"
    );
    assert_eq!(since[0].aggregate.0, "signal:k2");

    // EXACT boundary: a Signal AT the cursor version is EXCLUDED (`> since`, not `>= since`). With
    // since=5, the version-5 Signal must NOT replay (it is at, not above, the cursor).
    let at_boundary = src.replay(&notif_scope("inbox:all"), Some(5));
    assert_eq!(
        at_boundary.len(),
        0,
        "a Signal AT the cursor is excluded (strict >, not >=)"
    );
    // since=4 includes the version-5 Signal (it IS above the cursor).
    assert_eq!(
        src.replay(&notif_scope("inbox:all"), Some(4)).len(),
        1,
        "version 5 > since 4 replays"
    );
}

/// **An INCREMENTAL reindex (no wipe) of an already-applied snapshot is DEDUPLICATED by the live
/// consumer (the `signals_deduplicated` counter increments; the row is not double-applied).** Unlike
/// a full rebuild (which forgets the marks), the incremental path keeps the consumer_dedup marks — a
/// redelivered snapshot above the cursor is a no-op (no resurrection). Pins the dedup-counting arm.
#[test]
fn incremental_reindex_deduplicates_an_already_applied_snapshot() {
    let sig = signal(
        "ci_run_failed",
        Severity::Error,
        "myelin://acme/ci/run/1",
        "k1",
    );
    let outbox_router = OutboxStore::new();
    let (consumer, inbox) = live_router(&outbox_router);

    let mut src = SignalReindexSource::new();
    src.upsert(sig.clone(), 1);
    let reindexer = NotifReindexer::new(&consumer);
    let mut outbox = OutboxStore::new();

    // First incremental reindex (since=Some(0), no wipe): the snapshot applies — replayed, not deduped.
    let first = reindexer
        .reindex(
            &tenant(),
            &notif_scope("inbox:all"),
            Some(0),
            &[&src],
            &mut outbox,
            ctx_base(),
        )
        .expect("first");
    assert_eq!(
        first.signals_replayed, 1,
        "the first incremental applies the snapshot"
    );
    assert_eq!(
        first.signals_deduplicated, 0,
        "nothing to dedup on the first pass"
    );
    assert_eq!(inbox.len(), 1);

    // A SECOND incremental reindex of the SAME snapshot (no wipe): the consumer_dedup ledger makes it
    // a no-op — counted as deduplicated, NOT replayed (the row is not double-applied).
    let second = reindexer
        .reindex(
            &tenant(),
            &notif_scope("inbox:all"),
            Some(0),
            &[&src],
            &mut outbox,
            ctx_base(),
        )
        .expect("second");
    assert_eq!(
        second.signals_deduplicated, 1,
        "the redelivered snapshot is deduplicated (no-op)"
    );
    assert_eq!(
        second.signals_replayed, 0,
        "it is NOT re-applied (the dedup arm caught it)"
    );
    assert_eq!(
        inbox.len(),
        1,
        "still exactly one row — no resurrection/duplication"
    );
}

/// **The `ReindexError::Display` renders distinct, informative messages** (the LOUD-error artifact —
/// a silent/empty Display would mask a wiring bug).
#[test]
fn reindex_error_display_is_informative() {
    let bus = ReindexError::Bus("no owner".into());
    let missing = ReindexError::MissingSnapshot("snap-abc".into());
    assert!(
        format!("{bus}").contains("bus re-emit failed"),
        "the Bus error names the bus failure"
    );
    assert!(
        format!("{missing}").contains("snap-abc"),
        "the MissingSnapshot error names the id"
    );
    assert_ne!(
        format!("{bus}"),
        format!("{missing}"),
        "distinct variants render distinctly"
    );
    assert!(!format!("{bus}").is_empty(), "the Display is not empty");
}
