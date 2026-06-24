//! Pure-logic unit tests for the real-time board sync (ISS-P30) — the half that does NOT publish to
//! the firehose (so the `no-raw-publish` lint stays fully live on this source file). The
//! firehose-PRODUCING transport-protocol tests (live-frame patch, reconnect backfill, resync→snapshot)
//! live in `tests/board_sync_protocol.rs` (a separate `tests/` file the lint-gate does not scan — the
//! firehose `publish` method name collides with the durable-bus `.publish(` fingerprint; §4.3 / OQ-J
//! two-transport split). Covered HERE: the bounded `board:<id>` scope (never `*`); optimistic apply →
//! confirm / roll-back; the normalised cache lane render; and the op encode/decode round-trip.

use super::*;
use myelin_substrate::firehose_selector::ScopeWindow;

fn win() -> ScopeWindow {
    // A 200-row visible window + 50-row margin — a paginated slice of a huge board, never all rows.
    ScopeWindow::new(0, 200, 50)
}

fn open(stream: &str, scope: &str) -> BoardSync {
    BoardSync::open(stream, scope, win()).expect("a bounded board scope")
}

// ---- bounded scope (never *) ------------------------------------------------------------------

/// **The board scope is BOUNDED, never `*` (§7 / contract 3.5).** `board:<id>` parses; `*`,
/// `board:*`, an un-prefixed bare id, and the empty string are REJECTED at `open` through the ONE
/// `*`-rejecting `FirehoseScope::parse` chokepoint — an unbounded board subscription is unrepresentable.
#[test]
fn board_scope_is_bounded_never_star() {
    assert!(BoardSync::open("fan.acme.web", "board:proj-42", win()).is_ok());

    for raw in ["*", "board:*", "board:", "42", "", "team:eng"] {
        let r = BoardSync::open("fan.acme.web", raw, win());
        assert!(
            r.is_err(),
            "over-broad board scope `{raw}` must be rejected at open"
        );
        assert!(
            r.err().unwrap().is_over_broad_scope(),
            "`{raw}` is an over-broad-scope rejection"
        );
    }
}

/// The board firehose stream is `fan.<tenant>.<project>` (§7) — a PII-free identifier.
#[test]
fn board_stream_is_fan_tenant_project() {
    assert_eq!(board_stream("acme", "web"), "fan.acme.web");
}

// ---- optimistic apply → confirm / roll back ---------------------------------------------------

/// **Optimistic local mutation: the card moves IMMEDIATELY; a server confirm keeps it (§7).** The
/// card is in the new lane before the server confirms; the pending set clears on confirm; the
/// authoritative re-apply of the SAME op is idempotent (a no-op — already in the cache).
#[test]
fn optimistic_move_applies_immediately_and_confirm_keeps_it() {
    let mut bs = open("fan.acme.web", "board:p1");
    bs.cache
        .apply(&BoardOp::Upsert(BoardCard::new("I-1", "todo", "m")));

    // optimistic move to in_progress — applied immediately, pending a confirm.
    bs.apply_local(
        "mut-1",
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "in_progress".into(),
        },
    )
    .expect("first optimistic mutation applies");
    assert_eq!(
        bs.cache.card("I-1").unwrap().state_category,
        "in_progress",
        "the card moved optimistically before the server confirmed"
    );
    assert_eq!(bs.pending_count(), 1, "the mutation is pending a confirm");

    // server confirms → the optimistic apply stands; the pending record clears.
    assert!(bs.confirm_local("mut-1"), "the confirm clears the pending");
    assert_eq!(bs.pending_count(), 0);
    assert_eq!(bs.cache.card("I-1").unwrap().state_category, "in_progress");

    // the authoritative re-apply of the same op is idempotent (a no-op).
    let mut after = bs.cache.clone();
    after.apply(&BoardOp::Move {
        issue_id: "I-1".into(),
        state_category: "in_progress".into(),
    });
    assert_eq!(
        after.card("I-1").unwrap().state_category,
        "in_progress",
        "the authoritative re-apply is idempotent"
    );
}

/// **A server REJECT rolls the optimistic mutation back to its EXACT prior state (§7 — never a silent
/// loss).** The card reverts to its pre-mutation lane; a rejected optimistic CREATE is removed (its
/// prior was absent).
#[test]
fn optimistic_reject_rolls_back_to_exact_prior_state() {
    let mut bs = open("fan.acme.web", "board:p1");
    bs.cache
        .apply(&BoardOp::Upsert(BoardCard::new("I-1", "todo", "m")));

    // optimistic move, then a server reject → roll back to the prior lane (`todo`).
    bs.apply_local(
        "mut-1",
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "done".into(),
        },
    )
    .unwrap();
    assert_eq!(bs.cache.card("I-1").unwrap().state_category, "done");
    assert!(bs.reject_local("mut-1"), "the reject rolls back");
    assert_eq!(
        bs.cache.card("I-1").unwrap().state_category,
        "todo",
        "the card reverted to its EXACT prior lane on reject"
    );
    assert_eq!(bs.pending_count(), 0);

    // a rejected optimistic CREATE (no prior) REMOVES the card.
    bs.apply_local("mut-2", BoardOp::Upsert(BoardCard::new("I-9", "todo", "z")))
        .unwrap();
    assert!(bs.cache.card("I-9").is_some(), "optimistic create shows");
    assert!(bs.reject_local("mut-2"));
    assert!(
        bs.cache.card("I-9").is_none(),
        "a rejected optimistic create is removed (its prior was absent)"
    );
}

/// A second optimistic edit with an in-flight id is rejected (`AlreadyPending`) — v1 serialises
/// optimistic edits per mutation id; a tolerated late confirm/reject of an unknown id is a no-op.
#[test]
fn second_pending_mutation_is_rejected_and_unknown_confirm_is_noop() {
    let mut bs = open("fan.acme.web", "board:p1");
    bs.cache
        .apply(&BoardOp::Upsert(BoardCard::new("I-1", "todo", "m")));
    bs.apply_local(
        "mut-1",
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "done".into(),
        },
    )
    .unwrap();
    let again = bs.apply_local(
        "mut-1",
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "todo".into(),
        },
    );
    assert!(matches!(
        again,
        Err(LocalMutationError::AlreadyPending { .. })
    ));
    // an unknown id confirm/reject is a tolerated no-op (a late signal after a resync clear).
    assert!(!bs.confirm_local("nope"));
    assert!(!bs.reject_local("nope"));
}

// ---- the normalised cache reads (lane render) -------------------------------------------------

/// The cache renders a lane in `order_key` order (the column render — bounded, deterministic).
#[test]
fn cache_renders_a_lane_in_order_key_order() {
    let mut cache = BoardCache::new();
    cache.apply(&BoardOp::Upsert(BoardCard::new("I-1", "todo", "c")));
    cache.apply(&BoardOp::Upsert(BoardCard::new("I-2", "todo", "a")));
    cache.apply(&BoardOp::Upsert(BoardCard::new("I-3", "done", "b")));
    cache.apply(&BoardOp::Upsert(BoardCard::new("I-4", "todo", "b")));

    let todo = cache.lane("todo");
    assert_eq!(
        todo.iter().map(|c| c.issue_id.as_str()).collect::<Vec<_>>(),
        vec!["I-2", "I-4", "I-1"],
        "the todo lane is in order_key order (a, b, c)"
    );
    assert_eq!(cache.lane("done").len(), 1);
    assert_eq!(cache.lane("missing").len(), 0, "an empty lane is empty");
}

/// **Cache apply is idempotent on the card id (the zero-dup half of zero-loss).** Applying the same
/// upsert + move twice lands the same final state and creates no duplicate card; a move/reorder of an
/// unknown id is a no-op (the card is not on this board's window).
#[test]
fn cache_apply_is_idempotent_and_unknown_id_is_noop() {
    let mut cache = BoardCache::new();
    let ops = [
        BoardOp::Upsert(BoardCard::new("I-1", "todo", "a")),
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "done".into(),
        },
    ];
    for _ in 0..2 {
        for op in &ops {
            cache.apply(op);
        }
    }
    assert_eq!(cache.len(), 1, "no duplicate card (idempotent on the id)");
    assert_eq!(cache.card("I-1").unwrap().state_category, "done");

    // a move/reorder/remove of an unknown id is a tolerated no-op.
    cache.apply(&BoardOp::Move {
        issue_id: "ghost".into(),
        state_category: "x".into(),
    });
    cache.apply(&BoardOp::Reorder {
        issue_id: "ghost".into(),
        order_key: "z".into(),
    });
    cache.apply(&BoardOp::Remove {
        issue_id: "ghost".into(),
    });
    assert_eq!(cache.len(), 1, "an unknown-id op patches nothing");
    assert!(cache.card("ghost").is_none());
}

// ---- op encode/decode round-trip (the in-process frame payload) -------------------------------

/// Every board op round-trips through the firehose frame payload encoding (the in-process floor's
/// pointer); an unrecognised payload decodes to `None` (a frame the board layer skips).
#[test]
fn board_ops_round_trip_through_the_frame_payload() {
    let ops = vec![
        BoardOp::Upsert(BoardCard::new("I-1", "todo", "m")),
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "done".into(),
        },
        BoardOp::Reorder {
            issue_id: "I-1".into(),
            order_key: "z".into(),
        },
        BoardOp::Remove {
            issue_id: "I-1".into(),
        },
    ];
    for op in ops {
        let payload = op.encode();
        assert_eq!(
            BoardOp::decode(&payload),
            Some(op.clone()),
            "round-trip: {payload}"
        );
        assert_eq!(op.issue_id(), "I-1", "every op targets exactly one card");
    }
    assert_eq!(
        BoardOp::decode("presence|x|typing"),
        None,
        "a non-board payload decodes to None"
    );
    assert_eq!(
        BoardOp::decode(""),
        None,
        "an empty payload decodes to None"
    );
}

/// The board sync reports its bounded scope/stream key + the paginated window accessors exactly (the
/// thin reads the connection tier + the survival-signal scrape rely on); it is not connected before a
/// subscribe.
#[test]
fn accessors_read_back_the_sync_state() {
    let bs = open("fan.acme.web", "board:p1");
    assert_eq!(bs.scope().selector(), "board:p1");
    assert_eq!(bs.stream(), "fan.acme.web");
    // start=0 saturates the lower margin to 0; upper = 0+200+50 = 250 → span 250 (bounded, not the
    // whole board — the §7.7 "paginates its scope" guarantee).
    assert_eq!(bs.window().delivered_span(), 250);
    assert!(!bs.is_connected(), "not connected before subscribe");
    assert_eq!(bs.last_seq(), 0);
    assert_eq!(bs.pending_count(), 0);
    assert_eq!(bs.resync_required_count(), 0);
}

/// The named v1 sync floors are greppable (R-8 offline/local-first; the connection-tier follow-on).
#[test]
fn board_sync_floors_are_named() {
    assert_eq!(BoardSyncFloors::OFFLINE_LOCAL_FIRST, "R-8");
    assert_eq!(BoardSyncFloors::CONNECTION_TIER, "P-403");
}
