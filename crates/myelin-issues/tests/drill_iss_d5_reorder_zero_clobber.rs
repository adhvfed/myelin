//! **ISS-D5 / ISS-P09 / P-375 (M4) — the server-arbitrated `order_key` CAS reorder drill (the
//! silent-clobber floor) + the `issue.reordered` provider/consumer CDC pair.**
//!
//! This is the prompt's GATE artifact (drill catalogue row ISS-D5): N humans + an agent re-ranking
//! the SAME region of a board → **0 silent clobber**, bounded re-base churn, converges with the
//! 2-char jitter, and the 48-char rebalance NEVER reorders the displayed order.
//!
//! - The reorder is a **server-arbitrated CAS** on the moved issue's last-seen `(order_key, version)`
//!   ([`myelin_issues::reorder`]). A stale version LOSES the CAS, is returned the authoritative order
//!   and version, and re-bases honestly — it writes NO rank and emits NO event (0 silent clobber).
//!   One winner per CAS contention; the losers converge by re-basing against fresh state.
//! - Humans and agents drive the SAME mechanism (server-arbitrated, not client-trust — arch §5 agent
//!   parity): the drill's "agent" writer is an ordinary [`myelin_issues::ReorderRequest`], arbitrated
//!   identically to the human writers. An agent that loses re-plans against the authoritative order.
//! - The 48-char rebalance ([`myelin_issues::rebalance`]) re-spaces precision-exhausted keys onto
//!   short keys WITHOUT reordering the displayed order (the displayed SEQUENCE is identical
//!   issue-for-issue before and after).
//! - The reorder write co-commits its `issue.reordered` event through the ONE shared outbox
//!   (contract 2.2; emit-iff-committed — a lost CAS commits neither the rank nor the event).
//!
//! **Reconciliation (EI-01 §7).** The `OrderKey`/LexoRank codec is the SHARED, byte-identical crate
//! (`myelin_query`, co-owned with Knowledge) — this drill does NOT re-define the encoding; it drives
//! the Issues CAS ranking engine THROUGH it. The outbox + relay + dedup ledger are the shared
//! substrate's (`myelin-events`), driven here, never re-implemented.
//!
//! **FLOOR named (VISION §3).** ranking = `order_key` + server-arbitrated CAS; the **move-CRDT (Yrs
//! list / Fugue)** reusing the byte-identical `order_key` is the measured M5 follow-on (ISS-P32) —
//! the promotion swaps the conflict engine, not the data model.

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

// ---------------------------------------------------------------------------
// scaffolding
// ---------------------------------------------------------------------------

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

/// Seed a board of `n` evenly-spaced issues `I0 < I1 < … < I(n-1)`, each at version 0.
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

/// Assert the displayed order is a STRICT total order (every adjacent pair strictly increases by the
/// contract-13.3 `tiebreak`) — no two issues collide unbroken.
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

// ===========================================================================
// THE DRILL (ISS-D5): N humans + an agent re-rank the SAME region → 0 silent clobber, converges
// ===========================================================================

/// **ISS-D5: N writers (humans + an agent) all drag DIFFERENT issues into the SAME region; the CAS
/// arbitrates each, 0 silent clobber, and the board converges to a strict total order.**
///
/// Eight issues `I0..I7`. Five writers (four humans + one agent) each move a distinct issue into the
/// narrow `(I3, I4)` region. Each move is a server-arbitrated CAS; because the writers move DISTINCT
/// issues each holding its own fresh version, every CAS WINS in turn (the contention is the gap, not
/// the row), and the 2-char jitter makes the five same-region bisections produce DISTINCT keys.
/// After the storm: the board is a strict total order, all five movers sit in the target region in a
/// stable sequence, and exactly five `issue.reordered` events co-committed (one per accepted move) —
/// **0 silent clobber** (no move overwrote another's key; no committed rank lacks its event).
#[test]
fn iss_d5_n_writers_same_region_zero_clobber_converges() {
    let mut board = seed(8);
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    // Five writers move I7, I6, I5, I0, I1 into the (I3, I4) gap. The last writer is "the agent"
    // (server-arbitrated identically — arch §5 parity). Distinct jitters so same-gap bisections
    // produce distinct keys.
    let movers = [
        ("I7", jit(1, 1)),
        ("I6", jit(2, 2)),
        ("I5", jit(3, 3)),
        ("I0", jit(4, 4)),
        ("I1", jit(5, 5)), // the agent
    ];

    let mut applied = 0usize;
    for (issue, jitter) in movers {
        // Each writer holds the issue's CURRENT (fresh) version — read from the authoritative board.
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
    // 0 silent clobber: exactly one event per accepted move co-committed (no committed rank without
    // its event; no move silently overwrote another).
    assert_eq!(
        store.outbox_depth(),
        5,
        "one issue.reordered per accepted move"
    );
    assert_eq!(store.committed_count(), 5);

    // the board converged to a strict total order (the 2-char jitter kept the five same-region keys
    // distinct — no collision).
    let order = board.displayed_order();
    assert_total_order(&order);
    assert_eq!(order.len(), 8, "no issue was lost");

    // all five movers landed strictly between I3 and I4 in the displayed order.
    let pos = |id: &str| ids(&order).iter().position(|x| x == id).unwrap();
    for mover in ["I7", "I6", "I5", "I0", "I1"] {
        assert!(
            pos("I3") < pos(mover) && pos(mover) < pos("I4"),
            "{mover} converged into the (I3, I4) region"
        );
    }

    // every emitted type is the registered issue.reordered token (the names anchor X-5).
    for row in store.committed_rows() {
        assert_eq!(row.envelope.type_.0, events::ISSUE_REORDERED);
        assert!(events::ISSUE_EVENT_TOKENS.contains(&row.envelope.type_.0.as_str()));
    }
}

/// **ISS-D5 contention leg: many writers contend the SAME issue+version → exactly ONE wins, the rest
/// LOSE and re-base; 0 silent clobber, bounded re-base churn.** Six writers all hold version 0 for
/// the SAME issue I5 and all drag it to the front. The first wins (version → 1); the other five lose
/// the CAS (stale version 0), each returned the authoritative order to re-base against. The losers
/// wrote NO rank and emitted NO event — only ONE `issue.reordered` committed.
#[test]
fn iss_d5_same_issue_contention_one_winner_losers_rebase() {
    let mut board = seed(8);
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    let order_before = ids(&board.displayed_order());

    let mut winners = 0usize;
    let mut losers = 0usize;
    // six writers, all holding the STALE version 0 (the simultaneous-snapshot scenario).
    for w in 0..6 {
        let req = ReorderRequest {
            issue_id: "I5".into(),
            before_id: None,
            after_id: Some("I0".into()),
            expected_version: 0, // every writer snapshotted version 0
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
                // the loser is told the REAL version (1, after the winner) and the REAL order.
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
    // 0 silent clobber: only the ONE winner co-committed an event; the five losers wrote nothing.
    assert_eq!(
        store.outbox_depth(),
        1,
        "only the winner's issue.reordered committed"
    );
    assert_eq!(store.committed_count(), 1);

    // the board reflects EXACTLY the winner's move (I5 to the front) — no loser corrupted it.
    let order_after = ids(&board.displayed_order());
    assert_eq!(order_after[0], "I5", "the winner's move stands");
    assert_eq!(
        order_after.len(),
        order_before.len(),
        "no issue lost in the storm"
    );
    assert_total_order(&board.displayed_order());

    // bounded re-base churn: a loser that re-bases against the authoritative version now WINS (one
    // bounded retry, not an unbounded livelock).
    let req = ReorderRequest {
        issue_id: "I5".into(),
        before_id: Some("I0".into()),
        after_id: Some("I1".into()),
        expected_version: 1, // the re-based version
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

/// **ISS-D5 rebalance leg: the 48-char rebalance re-spaces precision-exhausted keys but NEVER
/// reorders the displayed order.** Build a region whose repeated same-gap inserts grew the keys past
/// the 48-char trigger, rebalance, and assert: the displayed SEQUENCE is identical issue-for-issue,
/// every new key is short (well under 48), and a subsequent reorder against the rebalanced state
/// still arbitrates correctly.
#[test]
fn iss_d5_rebalance_never_reorders_displayed_order() {
    // Force precision exhaustion: repeatedly bisect into the SAME adjacent gap so the keys grow.
    let mut board = BoardRanking::new();
    // two anchor issues with low keys.
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
    // grow a chain of long keys: each step bisects between `lo` and its IMMEDIATE successor (last
    // digit + 1) — an adjacent gap with no digit between, so the midpoint descends one level and the
    // key grows one char. Advancing `lo` to that midpoint keeps the gap adjacent, so the chain grows
    // monotonically to the 48-char trigger (the precision-exhaustion pathology, mirrors field.rs).
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

    // REBALANCE — re-space all keys onto short, evenly-gapped ranks.
    let after = rebalance(&mut board, &[]);

    // (1) the displayed SEQUENCE is unchanged issue-for-issue (the load-bearing invariant).
    assert!(
        same_displayed_sequence(&before, &after),
        "the 48-char rebalance must NOT reorder the displayed order"
    );
    // (2) every new key is short — the precision exhaustion is fixed.
    for r in &after {
        assert!(
            !r.order_key.needs_rebalance(),
            "{} is re-spaced to a short key",
            r.issue_id
        );
    }
    // (3) the re-spaced board is still a strict total order in the same sequence.
    assert_total_order(&board.displayed_order());
    assert_eq!(
        ids(&before),
        ids(&board.displayed_order()),
        "same sequence post-rebalance"
    );

    // (4) a reorder against the rebalanced state still arbitrates (the engine works post-rebalance).
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

// ===========================================================================
// THE CDC PAIR: the issue.reordered provider rows (2.2/2.3) + the consumer dedup (2.5)
// ===========================================================================

/// **Provider half (2.2/2.3): the reorder write emits `issue.reordered` co-committed on the issue's
/// aggregate, per-issue ordered.** Two accepted reorders on the SAME issue co-commit `issue.reordered`
/// at seq 0 then seq 1 — monotonic, gap-free, in commit order (the per-aggregate ordering the relay
/// drains); the payload carries the rank delta (from_rank/to_rank) by reference, never an inline body.
#[test]
fn cdc_provider_issue_reordered_is_per_aggregate_ordered() {
    let mut board = seed(4);
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    // first reorder of I0 (version 0 -> 1).
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
    // second reorder of the SAME issue I0 (version 1 -> 2) — same aggregate.
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
    // references-not-payloads: the rank delta is carried, never an inline body; no PII flag.
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

/// **Consumer half (2.5): a redelivery of the SAME `issue.reordered` event is DEDUP-SUPPRESSED.** The
/// consumer marks the event handled by its stable `event_id`; a replay is recognised already-handled
/// and skipped (0 double-handle) — so a re-delivered reorder never double-applies the rank.
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
        ledger.mark_handled(&consumer, &eid),
        "first delivery is newly handled"
    );
    assert!(
        !ledger.mark_handled(&consumer, &eid),
        "a replay of the same stable event_id is dedup-suppressed (0 double-apply of the rank)"
    );
}
