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

#[test]
fn iss_p32_move_crdt_promotes_only_on_measured_reorder_pain() {
    let mut board = seed_cas(8);
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let pressure = ReorderPressure::new();

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
    for _ in 0..2 {
        let req = ReorderRequest {
            issue_id: "I5".into(),
            before_id: None,
            after_id: Some("I0".into()),
            expected_version: 0,
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

#[test]
fn iss_d5_re_green_across_the_move_crdt_engine_promote_boundary() {
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

    let a = MoveCrdtBoard::seed_from_order(&seed_order);
    let b =
        MoveCrdtBoard::from_state(&a.encode_state()).expect("replica b loads the cutover state");

    assert_eq!(
        a.order(),
        seed_order,
        "the move-CRDT preserves the displayed order across the cutover (unchanged data model)"
    );

    a.move_issue("I7", 4).expect("A moves I7 into the region");
    b.move_issue("I6", 4).expect("B moves I6 into the region");

    a.merge_peer(&b).expect("A merges B");
    b.merge_peer(&a).expect("B merges A");

    assert_eq!(
        a.order(),
        b.order(),
        "the two replicas converge (0 divergence)"
    );
    let order = a.order();
    assert_eq!(order.len(), 8, "no issue was lost across the boundary");
    assert!(
        order.contains(&"I7".to_string()) && order.contains(&"I6".to_string()),
        "BOTH concurrent moves survive (0 clobber - stronger than the CAS floor's serial accept)"
    );

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

#[test]
fn cross_cell_portfolio_rollup_crosses_only_the_pii_free_projection() {
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

    let local = vec![RollupAggregate {
        total: 8,
        done: 3,
        estimate_sum: 24,
        input_hash: 0x1234,
    }];

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

    let projection = rollup.resolve_cell_local(&ptr, "pm", &HomeCellB);
    assert!(
        projection.is_rolled(),
        "the authorised viewer gets the aggregate"
    );
    assert_eq!(projection.aggregate().unwrap().total, 12);

    let portfolio = CrossCellPortfolioRollup::combine(&local, &[projection]);
    assert_eq!(
        portfolio.total, 20,
        "8 (cell A) + 12 (cell B) summed across cells"
    );
    assert_eq!(portfolio.done, 8);
    assert_eq!(portfolio.estimate_sum, 72);

    let tombstone = rollup.resolve_cell_local(&ptr, "stranger", &HomeCellB);
    assert!(!tombstone.is_rolled());
    let portfolio_for_stranger = CrossCellPortfolioRollup::combine(&local, &[tombstone]);
    assert_eq!(
        portfolio_for_stranger.total, 8,
        "the tombstoned remote epic contributes nothing (only the local epic)"
    );

    assert_eq!(
        rollup.pii_crossed(),
        0,
        "0 PII crosses the cross-cell rollup bridge"
    );
}
