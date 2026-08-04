use crate::events;
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventId, EventType, IdMinter,
    OutboxStore, OutboxTx, Visibility,
};
use myelin_query::field::{Jitter, OrderKey};
use myelin_query::order_key::tiebreak;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use crate::write_path::{issue_aggregate_key, issue_ref};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankedIssue {
    pub issue_id: String,
    pub order_key: OrderKey,
    pub version: u64,
    pub created_at: String,
    pub ulid: String,
}

#[derive(Clone, Debug, Default)]
pub struct BoardRanking {
    rows: HashMap<String, RankedIssue>,
}

impl BoardRanking {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, row: RankedIssue) {
        self.rows.insert(row.issue_id.clone(), row);
    }

    pub fn get(&self, issue_id: &str) -> Option<&RankedIssue> {
        self.rows.get(issue_id)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn displayed_order(&self) -> Vec<RankedIssue> {
        let mut all: Vec<RankedIssue> = self.rows.values().cloned().collect();
        all.sort_by(|a, b| {
            tiebreak(
                &a.order_key,
                &a.created_at,
                &a.ulid,
                &b.order_key,
                &b.created_at,
                &b.ulid,
            )
        });
        all
    }

    fn gap_for(
        &self,
        before_id: Option<&str>,
        after_id: Option<&str>,
    ) -> (Option<OrderKey>, Option<OrderKey>) {
        let lo = before_id
            .and_then(|id| self.rows.get(id))
            .map(|r| r.order_key.clone());
        let hi = after_id
            .and_then(|id| self.rows.get(id))
            .map(|r| r.order_key.clone());
        (lo, hi)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReorderRequest {
    pub issue_id: String,
    pub before_id: Option<String>,
    pub after_id: Option<String>,
    pub expected_version: u64,
    pub jitter: Jitter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReorderOutcome {
    Applied {
        ranked: RankedIssue,
        event_id: EventId,
        needs_rebalance: bool,
    },
    Conflict {
        authoritative_version: u64,
        authoritative_order: Vec<RankedIssue>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReorderError {
    UnknownIssue(String),
    RankInvariant(String),
    Outbox(String),
}

impl std::fmt::Display for ReorderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReorderError::UnknownIssue(id) => write!(f, "reorder: unknown issue `{id}`"),
            ReorderError::RankInvariant(why) => {
                write!(f, "reorder: rank invariant violated: {why}")
            }
            ReorderError::Outbox(why) => write!(f, "reorder: outbox co-commit failed: {why}"),
        }
    }
}

impl std::error::Error for ReorderError {}

pub fn reorder(
    board: &mut BoardRanking,
    store: &OutboxStore,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    req: &ReorderRequest,
    cause: Option<&myelin_events::EventEnvelope>,
) -> Result<ReorderOutcome, ReorderError> {
    let current = board
        .get(&req.issue_id)
        .cloned()
        .ok_or_else(|| ReorderError::UnknownIssue(req.issue_id.clone()))?;

    if current.version != req.expected_version {
        return Ok(ReorderOutcome::Conflict {
            authoritative_version: current.version,
            authoritative_order: board.displayed_order(),
        });
    }

    let (lo, hi) = board.gap_for(req.before_id.as_deref(), req.after_id.as_deref());
    let new_rank = OrderKey::rank_between(lo.as_ref(), hi.as_ref(), req.jitter);

    if let Some(ref lo_k) = lo {
        if lo_k >= &new_rank {
            return Err(ReorderError::RankInvariant(format!(
                "new rank {new_rank} did not sort after its lower neighbour {lo_k}"
            )));
        }
    }
    if let Some(ref hi_k) = hi {
        if &new_rank >= hi_k {
            return Err(ReorderError::RankInvariant(format!(
                "new rank {new_rank} did not sort before its upper neighbour {hi_k}"
            )));
        }
    }

    let needs_rebalance = new_rank.needs_rebalance();

    let tenant = ctx_base.tenant.0.clone();
    let object_ref = issue_ref(&tenant, &req.issue_id);
    let aggregate = issue_aggregate_key(0, &req.issue_id);

    let mut tx = store.begin(minter, ctx_base);
    tx.stage_state_change(format!(
        "issue {} reordered: rank {} -> {} (version {} -> {})",
        req.issue_id,
        current.order_key,
        new_rank,
        current.version,
        current.version + 1
    ));

    let draft = reorder_event_draft(
        &object_ref,
        &aggregate,
        &req.issue_id,
        &current.order_key,
        &new_rank,
        current.version + 1,
    );
    let event_id = tx
        .emit(draft, cause)
        .map_err(|e| ReorderError::Outbox(format!("{e:?}")))?;

    tx.commit()
        .map_err(|e| ReorderError::Outbox(format!("{e:?}")))?;

    let ranked = RankedIssue {
        issue_id: current.issue_id.clone(),
        order_key: new_rank,
        version: current.version + 1,
        created_at: current.created_at.clone(),
        ulid: current.ulid.clone(),
    };
    board.upsert(ranked.clone());

    Ok(ReorderOutcome::Applied {
        ranked,
        event_id,
        needs_rebalance,
    })
}

fn reorder_event_draft(
    object: &ArtifactRef,
    aggregate: &AggregateKey,
    issue_id: &str,
    from_rank: &OrderKey,
    to_rank: &OrderKey,
    new_version: u64,
) -> EventDraft {
    EventDraft {
        type_: EventType(events::ISSUE_REORDERED.into()),
        subject: object.clone(),
        aggregate: aggregate.clone(),
        payload: serde_json::json!({
            "issue": object.0,
            "issue_local_id": issue_id,
            "from_rank": from_rank.as_str(),
            "to_rank": to_rank.as_str(),
            "version": new_version,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

pub fn rebalance(board: &mut BoardRanking, jitters: &[Jitter]) -> Vec<RankedIssue> {
    let order = board.displayed_order();
    let mut prev: Option<OrderKey> = None;
    let mut out = Vec::with_capacity(order.len());
    for (i, issue) in order.iter().enumerate() {
        let jitter = jitters.get(i).copied().unwrap_or(Jitter::ZERO);
        let new_key = match &prev {
            None => OrderKey::rank_first(jitter),
            Some(p) => OrderKey::rank_last(Some(p), jitter),
        };
        let ranked = RankedIssue {
            issue_id: issue.issue_id.clone(),
            order_key: new_key.clone(),
            version: issue.version + 1,
            created_at: issue.created_at.clone(),
            ulid: issue.ulid.clone(),
        };
        board.upsert(ranked.clone());
        out.push(ranked);
        prev = Some(new_key);
    }
    out
}

pub fn same_displayed_sequence(a: &[RankedIssue], b: &[RankedIssue]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.issue_id == y.issue_id)
}

pub fn cmp_ranked(a: &RankedIssue, b: &RankedIssue) -> Ordering {
    tiebreak(
        &a.order_key,
        &a.created_at,
        &a.ulid,
        &b.order_key,
        &b.created_at,
        &b.ulid,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{Actor, CausedBy, MonotonicMinter, Region, TenantId, Timestamp};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

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

    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }

    fn shared_minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }

    fn jit(a: usize, b: usize) -> Jitter {
        Jitter::from_ranks(a, b).expect("in-range jitter")
    }

    fn seed_three() -> BoardRanking {
        let mut board = BoardRanking::new();
        let a = OrderKey::rank_first(jit(0, 0));
        let b = OrderKey::rank_last(Some(&a), jit(0, 0));
        let c = OrderKey::rank_last(Some(&b), jit(0, 0));
        board.upsert(RankedIssue {
            issue_id: "ENG-1".into(),
            order_key: a,
            version: 0,
            created_at: "2026-06-21T10:00:00Z".into(),
            ulid: "01A".into(),
        });
        board.upsert(RankedIssue {
            issue_id: "ENG-2".into(),
            order_key: b,
            version: 0,
            created_at: "2026-06-21T10:00:01Z".into(),
            ulid: "01B".into(),
        });
        board.upsert(RankedIssue {
            issue_id: "ENG-3".into(),
            order_key: c,
            version: 0,
            created_at: "2026-06-21T10:00:02Z".into(),
            ulid: "01C".into(),
        });
        board
    }

    fn ids(order: &[RankedIssue]) -> Vec<String> {
        order.iter().map(|r| r.issue_id.clone()).collect()
    }

    #[test]
    fn reorder_wins_cas_and_co_commits_event() {
        let mut board = seed_three();
        let store = OutboxStore::new();
        let req = ReorderRequest {
            issue_id: "ENG-3".into(),
            before_id: None,
            after_id: Some("ENG-1".into()),
            expected_version: 0,
            jitter: jit(1, 1),
        };
        let out = reorder(&mut board, &store, minter(), ctx_base(), &req, None)
            .expect("the reorder is attempted");
        let (ranked, event_id) = match out {
            ReorderOutcome::Applied {
                ranked,
                event_id,
                needs_rebalance,
            } => {
                assert!(
                    !needs_rebalance,
                    "a short key does not trip the 48-char rebalance"
                );
                (ranked, event_id)
            }
            ReorderOutcome::Conflict { .. } => panic!("the only writer must WIN the CAS"),
        };
        assert_eq!(ranked.version, 1, "an accepted reorder bumps the version");
        assert_eq!(ids(&board.displayed_order()), ["ENG-3", "ENG-1", "ENG-2"]);
        assert_eq!(store.outbox_depth(), 1, "one issue.reordered co-committed");
        let row = store.row(&event_id).expect("the committed row is present");
        assert_eq!(row.envelope.type_.0, events::ISSUE_REORDERED);
        assert_eq!(row.seq, 0, "first event for the aggregate is seq 0");
    }

    #[test]
    fn stale_reorder_loses_cas_zero_clobber() {
        let mut board = seed_three();
        let store = OutboxStore::new();

        let w1 = ReorderRequest {
            issue_id: "ENG-2".into(),
            before_id: Some("ENG-3".into()),
            after_id: None,
            expected_version: 0,
            jitter: jit(1, 0),
        };
        let r1 = reorder(&mut board, &store, minter(), ctx_base(), &w1, None).unwrap();
        assert!(
            matches!(r1, ReorderOutcome::Applied { .. }),
            "writer 1 wins"
        );
        let order_after_w1 = ids(&board.displayed_order());
        let depth_after_w1 = store.outbox_depth();

        let w2 = ReorderRequest {
            issue_id: "ENG-2".into(),
            before_id: None,
            after_id: Some("ENG-1".into()),
            expected_version: 0,
            jitter: jit(2, 2),
        };
        let r2 = reorder(&mut board, &store, minter(), ctx_base(), &w2, None).unwrap();
        match r2 {
            ReorderOutcome::Conflict {
                authoritative_version,
                authoritative_order,
            } => {
                assert_eq!(
                    authoritative_version, 1,
                    "the loser is told the real version"
                );
                assert_eq!(
                    ids(&authoritative_order),
                    order_after_w1,
                    "the loser gets the real order"
                );
            }
            ReorderOutcome::Applied { .. } => panic!("the STALE writer must LOSE the CAS"),
        }
        assert_eq!(
            ids(&board.displayed_order()),
            order_after_w1,
            "the loser wrote no rank"
        );
        assert_eq!(
            store.outbox_depth(),
            depth_after_w1,
            "the loser emitted no event"
        );
    }

    #[test]
    fn loser_rebases_against_fresh_state_and_wins() {
        let mut board = seed_three();
        let store = OutboxStore::new();
        let m = shared_minter();
        let w1 = ReorderRequest {
            issue_id: "ENG-2".into(),
            before_id: Some("ENG-3".into()),
            after_id: None,
            expected_version: 0,
            jitter: jit(1, 0),
        };
        reorder(&mut board, &store, Arc::clone(&m), ctx_base(), &w1, None).unwrap();

        let mut w2 = ReorderRequest {
            issue_id: "ENG-2".into(),
            before_id: None,
            after_id: Some("ENG-1".into()),
            expected_version: 0,
            jitter: jit(2, 2),
        };
        let authoritative_version =
            match reorder(&mut board, &store, Arc::clone(&m), ctx_base(), &w2, None).unwrap() {
                ReorderOutcome::Conflict {
                    authoritative_version,
                    ..
                } => authoritative_version,
                _ => panic!("stale writer loses"),
            };
        w2.expected_version = authoritative_version;
        let r = reorder(&mut board, &store, Arc::clone(&m), ctx_base(), &w2, None).unwrap();
        assert!(
            matches!(r, ReorderOutcome::Applied { .. }),
            "the re-based writer wins"
        );
        assert_eq!(
            ids(&board.displayed_order())[0],
            "ENG-2",
            "ENG-2 is now at the front"
        );
    }

    #[test]
    fn concurrent_same_gap_moves_produce_distinct_ranks() {
        let mut board = seed_three();
        let store = OutboxStore::new();
        let m = shared_minter();
        let a = ReorderRequest {
            issue_id: "ENG-1".into(),
            before_id: Some("ENG-2".into()),
            after_id: Some("ENG-3".into()),
            expected_version: 0,
            jitter: jit(5, 5),
        };
        let ra = match reorder(&mut board, &store, Arc::clone(&m), ctx_base(), &a, None).unwrap() {
            ReorderOutcome::Applied { ranked, .. } => ranked,
            _ => panic!(),
        };
        let b = ReorderRequest {
            issue_id: "ENG-3".into(),
            before_id: Some("ENG-2".into()),
            after_id: Some("ENG-1".into()),
            expected_version: 0,
            jitter: jit(6, 6),
        };
        let rb = match reorder(&mut board, &store, Arc::clone(&m), ctx_base(), &b, None).unwrap() {
            ReorderOutcome::Applied { ranked, .. } => ranked,
            _ => panic!(),
        };
        assert_ne!(ra.order_key, rb.order_key, "distinct ranks via the jitter");
        let order = board.displayed_order();
        for w in order.windows(2) {
            assert!(
                cmp_ranked(&w[0], &w[1]) == Ordering::Less,
                "strictly increasing total order"
            );
        }
    }

    #[test]
    fn unknown_issue_is_a_loud_error() {
        let mut board = seed_three();
        let store = OutboxStore::new();
        let req = ReorderRequest {
            issue_id: "ENG-404".into(),
            before_id: None,
            after_id: Some("ENG-1".into()),
            expected_version: 0,
            jitter: jit(0, 0),
        };
        let err = reorder(&mut board, &store, minter(), ctx_base(), &req, None).unwrap_err();
        assert!(matches!(err, ReorderError::UnknownIssue(_)));
        assert_eq!(store.outbox_depth(), 0, "no event for an unknown issue");
    }

    #[test]
    fn rebalance_preserves_displayed_order_with_short_keys() {
        let mut board = BoardRanking::new();
        let long_a = OrderKey::parse(format!("{}1", "V".repeat(40))).unwrap();
        let long_b = OrderKey::parse(format!("{}2", "V".repeat(40))).unwrap();
        let long_c = OrderKey::parse(format!("{}3", "V".repeat(40))).unwrap();
        board.upsert(RankedIssue {
            issue_id: "A".into(),
            order_key: long_a,
            version: 7,
            created_at: "t1".into(),
            ulid: "01A".into(),
        });
        board.upsert(RankedIssue {
            issue_id: "B".into(),
            order_key: long_b,
            version: 3,
            created_at: "t2".into(),
            ulid: "01B".into(),
        });
        board.upsert(RankedIssue {
            issue_id: "C".into(),
            order_key: long_c,
            version: 9,
            created_at: "t3".into(),
            ulid: "01C".into(),
        });
        let before = board.displayed_order();
        assert_eq!(ids(&before), ["A", "B", "C"]);

        let after = rebalance(&mut board, &[jit(0, 0), jit(0, 0), jit(0, 0)]);

        assert!(
            same_displayed_sequence(&before, &after),
            "rebalance must not reorder displayed order"
        );
        assert_eq!(ids(&board.displayed_order()), ["A", "B", "C"]);
        for r in &after {
            assert!(
                !r.order_key.needs_rebalance(),
                "{} rebalanced to a short key",
                r.issue_id
            );
            assert!(r.order_key.as_str().len() < 8, "a re-spaced key is short");
        }
        for w in after.windows(2) {
            assert!(
                w[0].order_key < w[1].order_key,
                "re-spaced keys strictly increase"
            );
        }
        assert_eq!(board.get("A").unwrap().version, 8);
        assert_eq!(board.get("C").unwrap().version, 10);
    }

    #[test]
    fn rebalance_walks_displayed_order_not_insertion_order() {
        let mut board = BoardRanking::new();
        board.upsert(RankedIssue {
            issue_id: "C".into(),
            order_key: OrderKey::parse("z").unwrap(),
            version: 0,
            created_at: "t3".into(),
            ulid: "3".into(),
        });
        board.upsert(RankedIssue {
            issue_id: "A".into(),
            order_key: OrderKey::parse("1").unwrap(),
            version: 0,
            created_at: "t1".into(),
            ulid: "1".into(),
        });
        board.upsert(RankedIssue {
            issue_id: "B".into(),
            order_key: OrderKey::parse("M").unwrap(),
            version: 0,
            created_at: "t2".into(),
            ulid: "2".into(),
        });
        let before = board.displayed_order();
        assert_eq!(ids(&before), ["A", "B", "C"]);
        let after = rebalance(&mut board, &[]);
        assert!(same_displayed_sequence(&before, &after));
        assert_eq!(ids(&after), ["A", "B", "C"]);
    }
}
