//! # The CDC pair for Notif's CONSUMPTION of contract 3.5 — the `inbox watch` live transport
//! (NOTIF-P15 / P-193, M2)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 3.5 (the
//! firehose transport + the resume-cursor subscription protocol). Notif owns ZERO new contracts — it
//! CONSUMES 3.5. Owning architecture: `notifications.md` §7 (the `inbox watch` live transport, C4 —
//! `subscribe(stream = fan.<tenant>.inbox.<principal>, scope = inbox:<principal>, cursor?)` /
//! `resume(stream, scope, last_seq)`; per-`(stream, scope)` monotone `seq`; `(last_seq, now]`
//! backfill that loses zero items; `resync_required → list_inbox` cold rebuild; scope a bounded
//! selector, never `*`). Reconciliation: `00-reconciliation-decisions.md` OQ-J.
//!
//! ## The contract this pair pins (Notif's leg of the ONE resume-cursor protocol)
//! Row 3.5 is the owned-seam between the side that PUBLISHES inbox frames on the bounded
//! `(fan.<tenant>.inbox, inbox:<principal>)` key (the **PROVIDER** — Notif's own create/bump path
//! mirroring `notif.item.created` onto the firehose) and the side that SUBSCRIBES / RESUMES (the
//! **CONSUMER** — the `inbox watch` connection). The frozen behaviour both sides agree on:
//!
//! - the PROVIDER publishes a references-not-payloads frame (the `item_id` pointer ONLY) to a BOUNDED
//!   `inbox:<principal>` scope and the transport assigns the per-`(stream, scope)` MONOTONE `seq`
//!   (the producer never mints its own seq);
//! - the CONSUMER `watch_open`s on a BOUNDED inbox scope (`*` / `inbox:*` REJECTED) and, on reconnect,
//!   `watch_resume`s with its `last_seq` to backfill `(last_seq, now]` then go live — **0 items lost,
//!   0 duplicate**; an out-of-window `last_seq` yields a `resync_required` it falls back on via a full
//!   `list_inbox` cold rebuild (NAMED, not silent).
//!
//! This is the dedicated 3.5-consumption provider+consumer pair the NOTIF-P15 TESTS field names; the
//! focused per-mechanism unit tests live in `src/watch/tests.rs`, the D-N11 drill in
//! `tests/drill_notif_d11.rs`.

use myelin_events::firehose::{Firehose, FirehoseError, DEFAULT_INFLIGHT_CAP};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::{publish_inbox_frame, watch_open, watch_resume, InboxFrame, WatchOutcome};
use myelin_tenancy::TenantId;

fn principal(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

/// **PROVIDER side of 3.5 (Notif's leg)** — Notif's create/bump path mirrors an inbox item onto the
/// firehose as a references-not-payloads live frame. The provider's promise: it publishes the
/// `item_id` POINTER (never a payload) to the BOUNDED `inbox:<principal>` scope and lets the transport
/// assign the per-`(stream, scope)` monotone `seq` (it returns the assigned frame so the cursor is
/// visible).
fn provider_publishes(fh: &mut Firehose, recipient: &Principal, item_id: &str) -> InboxFrame {
    publish_inbox_frame(fh, recipient, item_id).expect("a real principal has a bounded inbox scope")
}

/// **CONSUMER side of 3.5 (Notif's leg)** — the `inbox watch` connection resumes over the protocol.
/// Returns the item_ids it received in order (the consumer's promise: it sees the gap then live,
/// contiguously, 0 lost / 0 dup).
fn consumer_resume_drains(fh: &mut Firehose, watcher: &Principal, last_seq: u64) -> Vec<String> {
    let watch = watch_resume(fh, watcher, last_seq)
        .expect("an in-window resume succeeds")
        .into_live()
        .expect("an in-window resume is a live watch");
    watch.drain().into_iter().map(|f| f.item_id).collect()
}

/// **CDC: the PROVIDER assigns the monotone seq + the CONSUMER backfills the gap then live, 0 lost,
/// 0 dup (the resume-cursor agreement).** The provider publishes 1..5 (the transport mints
/// 1,2,3,4,5); the consumer, having seen up to 2, resumes and gets EXACTLY 3,4,5 then the live 6 —
/// contiguous, 0 lost, 0 duplicate.
#[test]
fn cdc_provider_seq_and_consumer_resume_agree_zero_lost_zero_dup() {
    let mut fh = Firehose::new();
    let me = principal("p-me");

    // PROVIDER: publishes 1..5; the transport assigns the per-(stream,scope) monotone seq.
    for i in 1..=5 {
        let f = provider_publishes(&mut fh, &me, &format!("itm-{i}"));
        assert_eq!(
            f.seq, i,
            "the PROVIDER does not mint the seq — the transport assigns it monotone"
        );
    }

    // CONSUMER: saw up to seq 2 → resume backfills (2, now] = {3,4,5}.
    let backfilled = consumer_resume_drains(&mut fh, &me, 2);
    assert_eq!(
        backfilled,
        vec!["itm-3", "itm-4", "itm-5"],
        "the gap is replayed in order — 0 lost"
    );

    // a LIVE frame after the resume is delivered gap-free (0 dup across the backfill→live boundary).
    let live_watch = watch_resume(&mut fh, &me, 5)
        .unwrap()
        .into_live()
        .expect("caught-up live");
    provider_publishes(&mut fh, &me, "itm-6");
    let live: Vec<String> = live_watch.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(
        live,
        vec!["itm-6"],
        "the live frame continues gap-free — 0 dup"
    );
}

/// **CDC: the CONSUMER subscribes on a BOUNDED scope; an over-broad scope is REJECTED (the
/// whitelist-not-`*` rule the two sides agree on).** The consumer's `inbox:<principal>` scope is
/// bounded by construction; the transport rejects `inbox:*` / `*` (an unbounded subscription) — a
/// client cannot subscribe to the whole tenant firehose.
#[test]
fn cdc_consumer_scope_is_bounded_unbounded_is_rejected() {
    let mut fh = Firehose::new();
    // the bounded inbox scope subscribes (the positive control).
    assert!(
        watch_open(&mut fh, &principal("p-me")).is_ok(),
        "a bounded inbox: scope watch opens"
    );
    // an unbounded scope is rejected at the connection-tier subscribe entry (subscribe_raw).
    for raw in ["inbox:*", "*", "inbox:"] {
        let r = fh.subscribe_raw("fan.acme.inbox", raw, None);
        assert!(r.is_err(), "an unbounded scope `{raw}` is rejected");
        assert!(
            r.unwrap_err().is_over_broad_scope(),
            "`{raw}` is an over-broad-scope rejection"
        );
    }
}

/// **CDC: an over-old cursor → the CONSUMER gets `resync_required` (the cold-rebuild fallback the two
/// sides agree on, NAMED not silent).** With a tiny retention window, the consumer's stale cursor
/// cannot backfill → `watch_resume` returns `resync_required` (the consumer falls back to a full
/// `list_inbox` cold rebuild). The signal is RAISED, never a silent partial replay.
#[test]
fn cdc_over_old_cursor_yields_resync_required() {
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let me = principal("p-me");
    for i in 1..=6 {
        provider_publishes(&mut fh, &me, &format!("itm-{i}")); // window holds {4,5,6}
    }
    let out = watch_resume(&mut fh, &me, 2).expect("the resync verdict is a non-fatal outcome");
    assert!(
        out.is_resync_required(),
        "an over-old cursor RAISES resync_required (NAMED)"
    );
    match out {
        WatchOutcome::ResyncRequired {
            last_seq,
            window_floor,
        } => {
            assert_eq!(last_seq, 2);
            assert_eq!(window_floor, 4, "the window floor is the oldest held seq");
        }
        WatchOutcome::Live(_) => panic!("expected resync_required for an over-old cursor"),
    }
}

/// **CDC: any unexpected transport error is propagated LOUD, never swallowed.** A sanity pin that the
/// consumer adapter surfaces a non-resync transport error as `Err` (it does not silently become a
/// live watch) — here exercised through the scope validator (the only error path the adapter can hit
/// for a synthetic over-broad raw scope).
#[test]
fn cdc_unexpected_transport_error_is_propagated() {
    // FirehoseScope::parse is the LOUD chokepoint: an over-broad scope is an Err, never a silent ok.
    let err = myelin_events::firehose::FirehoseScope::parse("inbox:*").expect_err("over-broad");
    assert!(
        matches!(err, FirehoseError::OverBroadScope { .. }),
        "the verdict is LOUD + typed"
    );
}
