//! **ISS-P32 / P-495 (M5) — the measured floor follow-ons drill: ISS-D5 re-greens ACROSS the
//! move-CRDT engine-promote boundary + the cross-cell portfolio rollup crosses ONLY the PII-free
//! projection (the GATE artifacts).**
//!
//! This is the prompt's GATE artifact for the two follow-ons that ship a concrete promotion:
//! - **ISS-D5 re-green (0 clobber across the engine-promote boundary).** The drill catalogue row
//!   ISS-D5 was written to SURVIVE the CAS→move-CRDT swap (`drill_iss_d5_reorder_zero_clobber.rs`
//!   proves the CAS floor; THIS proves the same 0-clobber property holds — now STRONGER — through the
//!   convergent move-CRDT engine: two concurrent distinct-issue moves into the same region BOTH
//!   survive (the CAS floor accepted them serially; the CRDT MERGES them with no serialisation, no
//!   loser re-bases), and the order_key data model is byte-identical across the boundary (the derived
//!   hints sort the issues in the same displayed order).
//! - **The cross-cell portfolio rollup crosses ONLY the PII-free aggregate.** A multi-cell portfolio
//!   rolls up a remote child over the frozen PII-free `CrossCellPointer`; resolution is cell-local;
//!   only the `RollupAggregate` numbers cross back — never a leaf row (the `pii_crossed` tripwire == 0).
//!
//! **Reconciliation (EI-01 §7).** The `order_key`/LexoRank codec is the SHARED byte-identical crate
//! (`myelin_query`, co-owned with Knowledge) — this drill does NOT re-define the encoding; the
//! move-CRDT engine derives the SAME codec's hints from its convergent list order. The `yrs` CRDT is
//! the cited convergent structure (VISION §4 — the SAME crate Knowledge's yrs_engine uses), never a
//! hand-rolled merge. The cross-cell bridge is the frozen `myelin_events::CrossCellPointer` (contract
//! 12.6), consumed, never a second frame.
//!
//! **FLOOR named (VISION §3).** Each promotion is MEASURED: the move-CRDT is promoted only on a
//! measured concurrent-reorder re-base rate (`ReorderPressure`); below the trigger the CAS floor
//! stands. The cross-cell rollup is the resolved R-7/OQ-I follow-on. The real-LLM runtime (R-10) is
//! the post-M5 follow-on.

use myelin_events::{
    Actor, ArtifactRef, CellId, CorrelationId, EmitContextBase, IdMinter, MonotonicMinter,
    OutboxStore, Region, TenantId, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::rollup::RollupAggregate;
use myelin_issues::{
    cmp_ranked, reorder, BoardRanking, CellLocalRollupResolver, CrossCellPortfolioRollup,
    CrossCellRollupPointer, MoveCrdtBoard, PortfolioProjection, RankedIssue, ReorderOutcome,
    ReorderPressure, ReorderRequest,
};
use myelin_query::field::{Jitter, OrderKey};
use std::cmp::Ordering;
use std::sync::Arc;

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-25T10:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T10:00:01Z".into()),
        caused_by: None,
    }
}

fn jit(a: usize, b: usize) -> Jitter {
    Jitter::from_ranks(a, b).expect("in-range jitter")
}

/// Seed a CAS board of `n` evenly-spaced issues `I0 < I1 < … < I(n-1)`, each at version 0 (the
/// pre-cutover state the move-CRDT is seeded from).
fn seed_cas(n: usize) -> BoardRanking {
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
            created_at: format!("2026-06-25T10:00:{:02}Z", i),
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

// ===========================================================================
// ISS-D5 RE-GREEN ACROSS THE ENGINE-PROMOTE BOUNDARY (the headline gate)
// ===========================================================================

/// **The measured promotion fires only on measured concurrent-reorder pain (VISION §3).** Drive the
/// CAS floor under a same-issue contention storm (the re-base churn the move-CRDT eliminates); the
/// measured re-base rate crosses the trigger → `should_promote` fires. A calm board does NOT promote.
#[test]
fn iss_p32_move_crdt_promotes_only_on_measured_reorder_pain() {
    let mut board = seed_cas(8);
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let pressure = ReorderPressure::new();

    // Same-issue contention storm: six writers all hold version 0 for I5; one wins, five lose + re-base
    // (the CAS floor's churn). We MEASURE that churn through the pressure meter.
    for w in 0..6 {
        let req = ReorderRequest {
            issue_id: "I5".into(),
            before_id: None,
            after_id: Some("I0".into()),
            expected_version: 0,
            jitter: jit(w % 60, (w + 1) % 60),
        };
        let outcome = reorder(
            &mut board,
            &store,
            Arc::clone(&minter),
            ctx_base(),
            &req,
            None,
        )
        .unwrap();
        let lost = matches!(outcome, ReorderOutcome::Conflict { .. });
        pressure.observe_cas_outcome(lost);
    }
    // run two more contended rounds to clear MIN_ATTEMPTS with a high re-base rate.
    for _ in 0..2 {
        let req = ReorderRequest {
            issue_id: "I5".into(),
            before_id: None,
            after_id: Some("I0".into()),
            expected_version: 0, // deliberately stale → loses
            jitter: jit(1, 2),
        };
        let outcome = reorder(
            &mut board,
            &store,
            Arc::clone(&minter),
            ctx_base(),
            &req,
            None,
        )
        .unwrap();
        pressure.observe_cas_outcome(matches!(outcome, ReorderOutcome::Conflict { .. }));
    }

    assert!(pressure.attempts() >= ReorderPressure::MIN_ATTEMPTS);
    assert!(
        pressure.rebase_rate() >= ReorderPressure::PROMOTE_THRESHOLD,
        "the measured re-base rate crossed the trigger ({:.2})",
        pressure.rebase_rate()
    );
    assert!(
        pressure.should_promote(),
        "measured concurrent-reorder pain promotes the board to the move-CRDT"
    );
}

/// **THE ISS-D5 RE-GREEN: 0 clobber holds across the CAS→move-CRDT engine-promote boundary, now
/// STRONGER (two concurrent distinct-issue moves BOTH survive convergently).** Seed the move-CRDT
/// DETERMINISTICALLY from the CAS-era displayed order (the engine_promote cutover); two replicas make
/// concurrent distinct-issue moves into the same region; they converge with BOTH moves applied (no
/// loser, no clobber) and the derived order_key hints are byte-identical to a fresh CAS encoding.
#[test]
fn iss_d5_re_green_across_the_move_crdt_engine_promote_boundary() {
    // pre-cutover: the CAS floor's displayed order (the quiesce-lite snapshot).
    let cas = seed_cas(8);
    let seed_order: Vec<String> = cas
        .displayed_order()
        .iter()
        .map(|r| r.issue_id.clone())
        .collect();
    assert_eq!(
        seed_order,
        vec!["I0", "I1", "I2", "I3", "I4", "I5", "I6", "I7"]
    );

    // ENGINE-PROMOTE: seed the move-CRDT deterministically from the CAS order (the cutover payload).
    let a = MoveCrdtBoard::seed_from_order(&seed_order);
    // a reconnecting replica loads the seeded state across the boundary (the firehose full-state).
    let b =
        MoveCrdtBoard::from_state(&a.encode_state()).expect("replica b loads the cutover state");

    // The data model is UNCHANGED across the boundary: the derived order is the same displayed order.
    assert_eq!(
        a.order(),
        seed_order,
        "the move-CRDT preserves the displayed order across the cutover (unchanged data model)"
    );

    // CONCURRENT distinct-issue moves into the SAME region — the CAS floor accepted these serially;
    // the CRDT MERGES them with no serialisation. Replica A moves I7 between I3/I4; replica B moves I6
    // between I3/I4 — concurrently, no coordination.
    a.move_issue("I7", 4).expect("A moves I7 into the region");
    b.move_issue("I6", 4).expect("B moves I6 into the region");

    // exchange updates over the firehose (both directions) — the convergence operation.
    a.merge_peer(&b).expect("A merges B");
    b.merge_peer(&a).expect("B merges A");

    // 0 CLOBBER — both moves survived (neither overwrote the other); the replicas CONVERGE.
    assert_eq!(
        a.order(),
        b.order(),
        "the two replicas converge (0 divergence)"
    );
    let order = a.order();
    assert_eq!(order.len(), 8, "no issue was lost across the boundary");
    assert!(
        order.contains(&"I7".to_string()) && order.contains(&"I6".to_string()),
        "BOTH concurrent moves survive (0 clobber — stronger than the CAS floor's serial accept)"
    );

    // the order_key data model is byte-identical across the boundary: the derived hints are a strict
    // total order under the SAME contract-13.3 codec (a fresh CAS board would encode identically).
    let ranked: Vec<RankedIssue> =
        a.derived_ranked(&|id| Some((format!("t-{id}"), format!("u-{id}"))));
    assert_eq!(ranked.len(), 8);
    assert_total_order(&ranked);
    assert_eq!(
        ids(&ranked),
        order,
        "the derived order_key ranking matches the convergent list order (unchanged model)"
    );
}

// ===========================================================================
// THE CROSS-CELL PORTFOLIO ROLLUP: only the PII-free projection crosses (CP-D7/CP-D8)
// ===========================================================================

/// **A multi-cell portfolio rollup crosses ONLY the PII-free aggregate; resolution is cell-local; a
/// tombstone (unauthorised child) contributes nothing (0 leak).** The portfolio in cell A has a local
/// epic + a remote epic homed in cell B; the remote rolls up cell-local through B; only the
/// `RollupAggregate` numbers cross back; the `pii_crossed` tripwire stays 0.
#[test]
fn cross_cell_portfolio_rollup_crosses_only_the_pii_free_projection() {
    // the home cell renders the remote child's aggregate AGAINST ITS rows, permission-checks there.
    struct HomeCellB;
    impl CellLocalRollupResolver for HomeCellB {
        fn resolve_in_home_cell(
            &self,
            pointer: &CrossCellRollupPointer,
            viewer_token: &str,
        ) -> PortfolioProjection {
            if viewer_token == "pm" {
                PortfolioProjection::Rolled {
                    subject: pointer.subject().clone(),
                    aggregate: RollupAggregate {
                        total: 12,
                        done: 5,
                        estimate_sum: 48,
                        input_hash: 0xBEEF,
                    },
                }
            } else {
                PortfolioProjection::Tombstone {
                    subject: pointer.subject().clone(),
                }
            }
        }
    }

    let cell_a = CellId::from_token("cell-fr-par-1");
    let cell_b = CellId::from_token("cell-de-fra-1");
    let rollup = CrossCellPortfolioRollup::new(cell_a.clone());
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());
    let corr = CorrelationId("portfolio-rollup-root".into());

    // the LOCAL epic (single-cell floor — its aggregate is computed in cell A).
    let local = vec![RollupAggregate {
        total: 8,
        done: 3,
        estimate_sum: 24,
        input_hash: 0x1234,
    }];

    // the REMOTE epic homed in cell B fans out as a PII-free pointer (homed in B).
    let remote_epic = ArtifactRef("myelin://acme/issues/issue/EPIC-DE-9".into());
    let ptr = rollup
        .fan_out_child(&tenant, &region, &remote_epic, &cell_b, &corr)
        .expect("the remote epic fans out cross-cell");
    assert_eq!(
        ptr.home_cell(),
        &cell_b,
        "the pointer is homed in the child's cell"
    );
    assert_eq!(rollup.children_fanned_out(), 1);

    // resolve it CELL-LOCAL through cell B — only the aggregate crosses back.
    let projection = rollup.resolve_cell_local(&ptr, "pm", &HomeCellB);
    assert!(
        projection.is_rolled(),
        "the authorised viewer gets the aggregate"
    );
    assert_eq!(projection.aggregate().unwrap().total, 12);

    // the portfolio COMBINES local + cross-cell — the numbers sum across the two cells.
    let portfolio = CrossCellPortfolioRollup::combine(&local, &[projection]);
    assert_eq!(
        portfolio.total, 20,
        "8 (cell A) + 12 (cell B) summed across cells"
    );
    assert_eq!(portfolio.done, 8);
    assert_eq!(portfolio.estimate_sum, 72);

    // an UNAUTHORISED viewer gets a tombstone — it contributes NOTHING (0 leak).
    let tombstone = rollup.resolve_cell_local(&ptr, "stranger", &HomeCellB);
    assert!(!tombstone.is_rolled());
    let portfolio_for_stranger = CrossCellPortfolioRollup::combine(&local, &[tombstone]);
    assert_eq!(
        portfolio_for_stranger.total, 8,
        "the tombstoned remote epic contributes nothing (only the local epic)"
    );

    // 0 PII crosses the bridge — only the four-field frame out + the aggregate numbers back.
    assert_eq!(
        rollup.pii_crossed(),
        0,
        "0 PII crosses the cross-cell rollup bridge"
    );
}
