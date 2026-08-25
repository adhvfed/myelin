use myelin_events::{
    Actor, CausedBy, ConsumerName, DedupLedger, EmitContextBase, IdMinter, MonotonicMinter,
    OutboxStore, Region, TenantId, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::{
    cmp_ranked, events, rebalance, reorder, same_displayed_sequence, BoardRanking, RankedIssue,
    ReorderOutcome, ReorderRequest,
};
use myelin_query::field::{Jitter, OrderKey};
use std::cmp::Ordering;
use std::sync::Arc;

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("eu-west".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T10:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T10:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn jit(a: usize, b: usize) -> Jitter {
    Jitter::from_ranks(a, b).expect("in-range jitter")
}

fn seed(n: usize) -> BoardRanking {
    let mut board = BoardRanking::new();
    let mut prev: Option<OrderKey> = None;
    for i in 0..n {
        let key = match &prev {
            None => OrderKey::rank_first(jit(0, 0)),
            Some(p) => OrderKey::rank_last(Some(p), jit(0, 0)),
        };
        board.upsert(RankedIssue {
            issue_id: format!("I{i}"),
            order_key: key.clone(),
            version: 0,
            created_at: format!("2026-06-21T10:00:{:02}Z", i),
            ulid: format!("01{i:03}"),
        });
        prev = Some(key);
    }
    board
}

fn ids(order: &[RankedIssue]) -> Vec<String> {
    order.iter().map(|r| r.issue_id.clone()).collect()
}

fn assert_total_order(order: &[RankedIssue]) {
    for w in order.windows(2) {
        assert_eq!(
            cmp_ranked(&w[0], &w[1]),
            Ordering::Less,
            "displayed order is a strict total order: {} < {}",
            w[0].issue_id,
            w[1].issue_id
        );
    }
}

#[test]
fn iss_d5_n_writers_same_region_zero_clobber_converges() {
    let mut board = seed(8);
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    let movers = [
        ("I7", jit(1, 1)),
        ("I6", jit(2, 2)),
        ("I5", jit(3, 3)),
        ("I0", jit(4, 4)),
        ("I1", jit(5, 5)),
    ];

    let mut applied = 0usize;
    for (issue, jitter) in movers {
        let expected_version = board.get(issue).unwrap().version;
        let req = ReorderRequest {
            issue_id: issue.into(),
            before_id: Some("I3".into()),
            after_id: Some("I4".into()),
            expected_version,
            jitter,
        };
        match reorder(
            &mut board,
            &store,
            Arc::clone(&minter),
            ctx_base(),
            &req,
            None,
        )
        .unwrap()
        {
            ReorderOutcome::Applied { .. } => applied += 1,
            ReorderOutcome::Conflict { .. } => {
                panic!("a distinct-issue move with a fresh version must WIN")
            }
        }
    }

    assert_eq!(applied, 5, "every distinct-issue move was accepted");
    assert_eq!(
        store.outbox_depth(),
        5,
        "one issue.reordered per accepted move"
    );
    assert_eq!(store.committed_count(), 5);

    let order = board.displayed_order();
    assert_total_order(&order);
    assert_eq!(order.len(), 8, "no issue was lost");

    let pos = |id: &str| ids(&order).iter().position(|x| x == id).unwrap();
    for mover in ["I7", "I6", "I5", "I0", "I1"] {
        assert!(
            pos("I3") < pos(mover) && pos(mover) < pos("I4"),
            "{mover} converged into the (I3, I4) region"
        );
    }

    for row in store.committed_rows() {
        assert_eq!(row.envelope.type_.0, events::ISSUE_REORDERED);
        assert!(events::ISSUE_EVENT_TOKENS.contains(&row.envelope.type_.0.as_str()));
    }
}

#[test]
fn iss_d5_same_issue_contention_one_winner_losers_rebase() {
    let mut board = seed(8);
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    let order_before = ids(&board.displayed_order());

    let mut winners = 0usize;
    let mut losers = 0usize;
    for w in 0..6 {
        let req = ReorderRequest {
            issue_id: "I5".into(),
            before_id: None,
            after_id: Some("I0".into()),
            expected_version: 0,
            jitter: jit(w % 60, (w + 1) % 60),
        };
        match reorder(
            &mut board,
            &store,
            Arc::clone(&minter),
            ctx_base(),
            &req,
            None,
        )
        .unwrap()
        {
            ReorderOutcome::Applied { ranked, .. } => {
                winners += 1;
                assert_eq!(ranked.version, 1, "the first winner bumps version 0 -> 1");
            }
            ReorderOutcome::Conflict {
                authoritative_version,
                authoritative_order,
            } => {
                losers += 1;
                assert_eq!(
                    authoritative_version, 1,
                    "the loser sees the winner's version"
                );
                assert!(
                    !authoritative_order.is_empty(),
                    "the loser gets the authoritative order to re-base"
                );
            }
        }
    }

    assert_eq!(
        winners, 1,
        "exactly ONE writer wins the CAS on a contended issue+version"
    );
    assert_eq!(
        losers, 5,
        "the other five LOSE and re-base (0 silent clobber)"
    );
    assert_eq!(
        store.outbox_depth(),
        1,
        "only the winner's issue.reordered committed"
    );
    assert_eq!(store.committed_count(), 1);

    let order_after = ids(&board.displayed_order());
    assert_eq!(order_after[0], "I5", "the winner's move stands");
    assert_eq!(
        order_after.len(),
        order_before.len(),
        "no issue lost in the storm"
    );
    assert_total_order(&board.displayed_order());

    let req = ReorderRequest {
        issue_id: "I5".into(),
        before_id: Some("I0".into()),
        after_id: Some("I1".into()),
        expected_version: 1,
        jitter: jit(7, 7),
    };
    let rebased = reorder(
        &mut board,
        &store,
        Arc::clone(&minter),
        ctx_base(),
        &req,
        None,
    )
    .unwrap();
    assert!(
        matches!(rebased, ReorderOutcome::Applied { .. }),
        "a re-based loser wins on one retry"
    );
}

#[test]
fn iss_d5_rebalance_never_reorders_displayed_order() {
    let mut board = BoardRanking::new();
    board.upsert(RankedIssue {
        issue_id: "LO".into(),
        order_key: OrderKey::parse("V0").unwrap(),
        version: 0,
        created_at: "t0".into(),
        ulid: "010".into(),
    });
    board.upsert(RankedIssue {
        issue_id: "HI".into(),
        order_key: OrderKey::parse("z").unwrap(),
        version: 0,
        created_at: "t9".into(),
        ulid: "019".into(),
    });
    let succ = |k: &OrderKey| -> OrderKey {
        let s = k.as_str();
        let (head, last) = s.split_at(s.len() - 1);
        let alphabet = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let pos = alphabet
            .iter()
            .position(|&b| b == last.as_bytes()[0])
            .unwrap();
        let bumped = alphabet[pos + 1] as char;
        OrderKey::parse(format!("{head}{bumped}")).unwrap()
    };
    let mut lo = OrderKey::parse("V0").unwrap();
    let mut tripped = false;
    for i in 0..400 {
        let hi = succ(&lo);
        let mid = OrderKey::bisect(Some(&lo), Some(&hi));
        let needs = mid.needs_rebalance();
        board.upsert(RankedIssue {
            issue_id: format!("M{i}"),
            order_key: mid.clone(),
            version: 0,
            created_at: format!("t{i:03}"),
            ulid: format!("02{i:03}"),
        });
        if needs {
            tripped = true;
            break;
        }
        lo = mid;
    }
    assert!(
        tripped,
        "the same-gap chain grew a key past the 48-char rebalance trigger"
    );

    let before = board.displayed_order();
    assert!(
        before.iter().any(|r| r.order_key.needs_rebalance()),
        "at least one key tripped the 48-char trigger before the rebalance"
    );

    let after = rebalance(&mut board, &[]);

    assert!(
        same_displayed_sequence(&before, &after),
        "the 48-char rebalance must NOT reorder the displayed order"
    );
    for r in &after {
        assert!(
            !r.order_key.needs_rebalance(),
            "{} is re-spaced to a short key",
            r.issue_id
        );
    }
    assert_total_order(&board.displayed_order());
    assert_eq!(
        ids(&before),
        ids(&board.displayed_order()),
        "same sequence post-rebalance"
    );

    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let target = ids(&board.displayed_order())[2].clone();
    let v = board.get(&target).unwrap().version;
    let req = ReorderRequest {
        issue_id: target.clone(),
        before_id: None,
        after_id: Some("LO".into()),
        expected_version: v,
        jitter: jit(1, 1),
    };
    let r = reorder(&mut board, &store, minter, ctx_base(), &req, None).unwrap();
    assert!(
        matches!(r, ReorderOutcome::Applied { .. }),
        "the engine arbitrates post-rebalance"
    );
}

#[test]
fn cdc_provider_issue_reordered_is_per_aggregate_ordered() {
    let mut board = seed(4);
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    let r0 = match reorder(
        &mut board,
        &store,
        Arc::clone(&minter),
        ctx_base(),
        &ReorderRequest {
            issue_id: "I0".into(),
            before_id: Some("I2".into()),
            after_id: Some("I3".into()),
            expected_version: 0,
            jitter: jit(1, 1),
        },
        None,
    )
    .unwrap()
    {
        ReorderOutcome::Applied { event_id, .. } => event_id,
        _ => panic!("the only writer wins"),
    };
    let r1 = match reorder(
        &mut board,
        &store,
        Arc::clone(&minter),
        ctx_base(),
        &ReorderRequest {
            issue_id: "I0".into(),
            before_id: None,
            after_id: Some("I1".into()),
            expected_version: 1,
            jitter: jit(2, 2),
        },
        None,
    )
    .unwrap()
    {
        ReorderOutcome::Applied { event_id, .. } => event_id,
        _ => panic!("the re-based writer wins"),
    };

    let row0 = store.row(&r0).unwrap();
    let row1 = store.row(&r1).unwrap();
    assert_eq!(
        row0.aggregate, row1.aggregate,
        "same issue → same aggregate"
    );
    assert_eq!(row0.seq, 0, "first issue.reordered is seq 0");
    assert_eq!(row1.seq, 1, "second is seq 1 (monotonic, gap-free)");
    assert_eq!(row0.envelope.type_.0, events::ISSUE_REORDERED);
    assert_eq!(row1.envelope.payload["issue_local_id"], "I0");
    assert!(
        row1.envelope.payload.get("to_rank").is_some(),
        "the rank delta is carried by reference"
    );
    assert!(
        !row1.envelope.contains_personal_data,
        "a rank delta carries no free-text PII"
    );
}

#[test]
fn cdc_consumer_dedup_suppresses_a_replayed_reorder() {
    let mut board = seed(4);
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    let eid = match reorder(
        &mut board,
        &store,
        minter,
        ctx_base(),
        &ReorderRequest {
            issue_id: "I0".into(),
            before_id: Some("I2".into()),
            after_id: Some("I3".into()),
            expected_version: 0,
            jitter: jit(1, 1),
        },
        None,
    )
    .unwrap()
    {
        ReorderOutcome::Applied { event_id, .. } => event_id,
        _ => panic!("the only writer wins"),
    };

    let ledger = DedupLedger::new();
    let consumer = ConsumerName("issues-board-projection".into());
    assert!(
        ledger
            .mark_handled(&consumer, &eid)
            .expect("in-memory dedup storage is available"),
        "first delivery is newly handled"
    );
    assert!(
        !ledger
            .mark_handled(&consumer, &eid)
            .expect("in-memory dedup storage is available"),
        "a replay of the same stable event_id is dedup-suppressed (0 double-apply of the rank)"
    );
}
