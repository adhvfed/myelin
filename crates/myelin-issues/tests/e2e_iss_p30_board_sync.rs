use myelin_events::Firehose;
use myelin_issues::{BoardCard, BoardOp, BoardSync};
use myelin_substrate::firehose_selector::ScopeWindow;

fn board(fh: &mut Firehose) -> BoardSync {
    let mut bs = BoardSync::open("fan.acme.web", "board:proj-7", ScopeWindow::new(0, 200, 50))
        .expect("a bounded board:<id> scope");
    bs.subscribe(fh, None).expect("a bounded scope subscribes");
    bs
}

#[test]
fn board_sync_chained_mutation_loses_zero_ops() {
    let mut fh = Firehose::new();
    let mut bs = board(&mut fh);

    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-1", "todo", "a")).to_draft(),
    );
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-2", "todo", "b")).to_draft(),
    );
    bs.pump();
    assert_eq!(bs.cache().len(), 2);

    bs.apply_local(
        "mut-move-1",
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "in_progress".into(),
        },
    )
    .expect("optimistic move applies");
    assert_eq!(
        bs.cache().card("I-1").unwrap().state_category,
        "in_progress",
        "optimistic move shows immediately"
    );
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "in_progress".into(),
        }
        .to_draft(),
    );
    assert!(bs.confirm_local("mut-move-1"));
    bs.pump();
    assert_eq!(
        bs.cache().card("I-1").unwrap().state_category,
        "in_progress"
    );

    let storm = [
        BoardOp::Upsert(BoardCard::new("I-3", "todo", "c")),
        BoardOp::Move {
            issue_id: "I-2".into(),
            state_category: "in_progress".into(),
        },
        BoardOp::Upsert(BoardCard::new("I-4", "todo", "d")),
        BoardOp::Reorder {
            issue_id: "I-1".into(),
            order_key: "z".into(),
        },
        BoardOp::Remove {
            issue_id: "I-3".into(),
        },
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "done".into(),
        },
        BoardOp::Upsert(BoardCard::new("I-5", "todo", "e")),
    ];
    for op in &storm {
        fh.publish(bs.stream(), bs.scope(), op.to_draft());
    }

    let gap = bs
        .reconnect(&mut fh)
        .expect("in-window resume backfills the gap");
    assert_eq!(
        gap,
        storm.len(),
        "every storm op was replayed on reconnect - 0 ops lost"
    );

    assert_eq!(bs.cache().card("I-1").unwrap().state_category, "done");
    assert_eq!(bs.cache().card("I-1").unwrap().order_key, "z");
    assert_eq!(
        bs.cache().card("I-2").unwrap().state_category,
        "in_progress"
    );
    assert!(
        bs.cache().card("I-3").is_none(),
        "I-3's create+remove both replayed"
    );
    assert!(bs.cache().card("I-4").is_some());
    assert!(bs.cache().card("I-5").is_some());

    let prior_lane = bs.cache().card("I-2").unwrap().state_category.clone();
    bs.apply_local(
        "mut-bad",
        BoardOp::Move {
            issue_id: "I-2".into(),
            state_category: "done".into(),
        },
    )
    .expect("optimistic edit applies");
    assert_eq!(
        bs.cache().card("I-2").unwrap().state_category,
        "done",
        "optimistic edit shows"
    );
    assert!(bs.reject_local("mut-bad"), "the server rejects → roll back");
    assert_eq!(
        bs.cache().card("I-2").unwrap().state_category,
        prior_lane,
        "the rejected edit rolled back to the EXACT prior state (never a silent loss)"
    );

    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-6", "todo", "f")).to_draft(),
    );
    assert_eq!(bs.pump(), 1, "live continues gap-free");
    assert!(bs.cache().card("I-6").is_some());
}
