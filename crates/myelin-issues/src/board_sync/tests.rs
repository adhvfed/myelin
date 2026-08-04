use super::*;
use myelin_substrate::firehose_selector::ScopeWindow;

fn win() -> ScopeWindow {
    ScopeWindow::new(0, 200, 50)
}

fn open(stream: &str, scope: &str) -> BoardSync {
    BoardSync::open(stream, scope, win()).expect("a bounded board scope")
}

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

#[test]
fn board_stream_is_fan_tenant_project() {
    assert_eq!(board_stream("acme", "web"), "fan.acme.web");
}

#[test]
fn optimistic_move_applies_immediately_and_confirm_keeps_it() {
    let mut bs = open("fan.acme.web", "board:p1");
    bs.cache
        .apply(&BoardOp::Upsert(BoardCard::new("I-1", "todo", "m")));

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

    assert!(bs.confirm_local("mut-1"), "the confirm clears the pending");
    assert_eq!(bs.pending_count(), 0);
    assert_eq!(bs.cache.card("I-1").unwrap().state_category, "in_progress");

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

#[test]
fn optimistic_reject_rolls_back_to_exact_prior_state() {
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
    assert_eq!(bs.cache.card("I-1").unwrap().state_category, "done");
    assert!(bs.reject_local("mut-1"), "the reject rolls back");
    assert_eq!(
        bs.cache.card("I-1").unwrap().state_category,
        "todo",
        "the card reverted to its EXACT prior lane on reject"
    );
    assert_eq!(bs.pending_count(), 0);

    bs.apply_local("mut-2", BoardOp::Upsert(BoardCard::new("I-9", "todo", "z")))
        .unwrap();
    assert!(bs.cache.card("I-9").is_some(), "optimistic create shows");
    assert!(bs.reject_local("mut-2"));
    assert!(
        bs.cache.card("I-9").is_none(),
        "a rejected optimistic create is removed (its prior was absent)"
    );
}

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
    assert!(!bs.confirm_local("nope"));
    assert!(!bs.reject_local("nope"));
}

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

#[test]
fn accessors_read_back_the_sync_state() {
    let bs = open("fan.acme.web", "board:p1");
    assert_eq!(bs.scope().selector(), "board:p1");
    assert_eq!(bs.stream(), "fan.acme.web");
    assert_eq!(bs.window().delivered_span(), 250);
    assert!(!bs.is_connected(), "not connected before subscribe");
    assert_eq!(bs.last_seq(), 0);
    assert_eq!(bs.pending_count(), 0);
    assert_eq!(bs.resync_required_count(), 0);
}

#[test]
fn board_sync_floors_are_named() {
    assert_eq!(BoardSyncFloors::OFFLINE_LOCAL_FIRST, "R-8");
    assert_eq!(BoardSyncFloors::CONNECTION_TIER, "P-403");
}
