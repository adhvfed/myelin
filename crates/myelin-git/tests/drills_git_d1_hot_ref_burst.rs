use myelin_events::relay::{BusTransport, InProcessBus, Relay};
use myelin_events::{
    Actor, CausedBy, EmitContextBase, EventId, IdMinter, MonotonicMinter, OutboxStore, Region,
    TenantId, Timestamp,
};
use myelin_git::events::GIT_REF_UPDATED;
use myelin_git::receive_pack::{
    CrashPoint, GitRefEventKey, InMemoryObjectDb, Oid, ProposedRefUpdate, PushOutcome, PushSession,
    Pusher, RefName, RefStore, RejectReason,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use std::collections::BTreeMap;
use std::sync::{Arc, Barrier};

const TENANT: &str = "acme";
const REGION: &str = "fr-par";
const REPO: &str = "core";
const HOT_REF: &str = "refs/heads/hot";
const ROUNDS: u64 = 30;

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId(TENANT.into()),
        region: Region(REGION.into()),
        actor: Actor(Principal::stub(
            PrincipalId("dev-1".into()),
            PrincipalKind::Human,
            TenantId(TENANT.into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:burst".into())),
    }
}

fn open_store() -> (Arc<RefStore>, OutboxStore) {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let store = RefStore::open(REPO, ctx_base(), outbox.clone(), minter);
    (Arc::new(store), outbox)
}

fn push(ref_name: &str, old: Oid, new: Oid, forced: bool) -> PushSession {
    PushSession {
        updates: vec![ProposedRefUpdate {
            ref_name: RefName::new(ref_name),
            expected_old: old,
            new_oid: new.clone(),
            forced,
            commit_oids: vec![new],
        }],
        quarantine: vec![],
        pusher: Pusher {
            pseudonym: "anon-3@acme.noreply".into(),
            is_agent: false,
        },
    }
}

fn relay(outbox: &OutboxStore) -> Relay<InProcessBus> {
    Relay::new(outbox.clone(), InProcessBus::new(), || {
        Timestamp("2026-06-21T00:05:00Z".into())
    })
}

struct SurgeResult {
    committed_update_seqs: Vec<u64>,
    rejects: usize,
    committed_ids: Vec<EventId>,
}

fn run_surge(multiplier: usize) -> (Arc<RefStore>, OutboxStore, SurgeResult) {
    let (store, outbox) = open_store();
    let mut committed_update_seqs = Vec::new();
    let mut committed_ids = Vec::new();
    let mut rejects = 0usize;

    let mut tip = Oid::zero();
    for round in 1..=ROUNDS {
        let barrier = Arc::new(Barrier::new(multiplier));
        let mut handles = Vec::new();
        for racer in 0..multiplier {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            let old = tip.clone();
            let new = Oid::new(format!("r{round:03}c{racer:03}"));
            let forced = racer % 2 == 1;
            handles.push(std::thread::spawn(move || {
                let db = InMemoryObjectDb::new();
                barrier.wait();
                store
                    .receive(&push(HOT_REF, old, new, forced), &db, CrashPoint::None)
                    .unwrap()
            }));
        }

        let mut round_winner: Option<(Oid, u64, EventId)> = None;
        for h in handles {
            match h.join().unwrap() {
                PushOutcome::Accepted { moved, emitted } => {
                    assert!(
                        round_winner.is_none(),
                        "round {round}: TWO racers won - lost-update!"
                    );
                    round_winner = Some((moved[0].1.clone(), moved[0].2, emitted[0].clone()));
                }
                PushOutcome::Rejected(RejectReason::NonFastForward { .. }) => rejects += 1,
                o => panic!("round {round}: unexpected outcome {o:?}"),
            }
        }
        let (winner_tip, seq, id) = round_winner.expect("every round has exactly one winner");
        committed_update_seqs.push(seq);
        committed_ids.push(id);
        tip = winner_tip;
    }

    (
        store,
        outbox,
        SurgeResult {
            committed_update_seqs,
            rejects,
            committed_ids,
        },
    )
}

#[test]
fn git_d1_hot_ref_burst_per_ref_order_zero_lost_zero_ghost() {
    let agg = GitRefEventKey::new(REPO, &RefName::new(HOT_REF))
        .expect("valid canonical ref key")
        .aggregate();

    for multiplier in [1usize, 10, 30] {
        let (store, outbox, result) = run_surge(multiplier);

        assert_eq!(
            result.committed_update_seqs,
            (1..=ROUNDS).collect::<Vec<_>>(),
            "{multiplier}×: update_seq is contiguous push order per ref (no gap, no reorder)"
        );

        assert_eq!(
            result.rejects,
            (multiplier - 1) * ROUNDS as usize,
            "{multiplier}×: every losing racer was a non-fast-forward reject (0 lost-update)"
        );

        assert_eq!(
            outbox.committed_count(),
            ROUNDS as usize,
            "{multiplier}×: exactly one git.ref.updated per generation (0 ghost)"
        );

        let outbox_seqs: Vec<u64> = result
            .committed_ids
            .iter()
            .map(|id| {
                let row = outbox.row(id).expect("the committed row");
                assert_eq!(
                    row.aggregate, agg,
                    "{multiplier}×: every burst event is on the hot-ref aggregate"
                );
                assert_eq!(row.envelope.type_.0, GIT_REF_UPDATED);
                row.seq
            })
            .collect();
        assert_eq!(
            outbox_seqs,
            (0..ROUNDS).collect::<Vec<_>>(),
            "{multiplier}×: outbox order == ref-update order per ref (gap-free per-aggregate seq)"
        );

        let r = relay(&outbox);
        r.drain_to_empty();
        let hot_subject = GitRefEventKey::new(REPO, &RefName::new(HOT_REF))
            .expect("valid canonical ref key")
            .subject(TENANT)
            .expect("canonical ref key forms a canonical ArtifactRef");
        let delivered_for_hot: Vec<u64> = r
            .transport()
            .consume(&hot_subject.0)
            .iter()
            .map(|e| {
                e.payload
                    .get("update_seq")
                    .and_then(|v| v.as_u64())
                    .expect("update_seq")
            })
            .collect();
        assert_eq!(
            delivered_for_hot,
            (1..=ROUNDS).collect::<Vec<_>>(),
            "{multiplier}×: the relay delivers the hot ref in push order per ref"
        );
        let delivered_ids = r.transport().delivered_ids();
        let committed_set: std::collections::HashSet<EventId> =
            result.committed_ids.iter().cloned().collect();
        assert_eq!(
            delivered_ids, committed_set,
            "{multiplier}×: delivered set == committed set (0 lost / 0 ghost end-to-end)"
        );

        assert_eq!(
            outbox.outbox_depth(),
            0,
            "{multiplier}×: outbox depth drained to 0 (survival signal)"
        );
        assert_eq!(
            outbox.dead_letter_count(),
            0,
            "{multiplier}×: 0 dead-letters"
        );

        let tip = store
            .tip(&RefName::new(HOT_REF))
            .expect("the hot ref exists");
        let log = store.reflog().expect("read reflog");
        let gens: Vec<u64> = log
            .iter()
            .filter(|e| e.ref_name == RefName::new(HOT_REF))
            .map(|e| e.update_seq)
            .collect();
        assert_eq!(
            gens,
            (1..=ROUNDS).collect::<Vec<_>>(),
            "{multiplier}×: the reflog is the contiguous chain"
        );
        assert!(
            tip.0.starts_with("r030"),
            "{multiplier}×: the tip is the final round's winner: {tip:?}"
        );
    }
}

#[test]
fn git_d1_distinct_refs_fan_out_parallel_under_burst() {
    let (store, outbox) = open_store();
    let n = 30usize;
    let barrier = Arc::new(Barrier::new(n));

    let mut handles = Vec::new();
    for i in 0..n {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let db = InMemoryObjectDb::new();
            let ref_name = format!("refs/heads/b{i:02}");
            let p = push(
                &ref_name,
                Oid::zero(),
                Oid::new(format!("tip{i:02}")),
                false,
            );
            barrier.wait();
            (ref_name, store.receive(&p, &db, CrashPoint::None).unwrap())
        }));
    }
    let results: Vec<(String, PushOutcome)> =
        handles.into_iter().map(|h| h.join().unwrap()).collect();

    let mut per_ref_seq: BTreeMap<String, u64> = BTreeMap::new();
    for (ref_name, outcome) in &results {
        match outcome {
            PushOutcome::Accepted { moved, .. } => {
                per_ref_seq.insert(ref_name.clone(), moved[0].2);
            }
            o => panic!("distinct ref {ref_name} must commit in parallel, got {o:?}"),
        }
    }
    assert_eq!(per_ref_seq.len(), n, "all N distinct refs advanced");
    assert!(
        per_ref_seq.values().all(|&s| s == 1),
        "each distinct ref is its own generation 1"
    );
    assert_eq!(
        outbox.committed_count(),
        n,
        "N distinct-ref events committed (refs fan out parallel)"
    );

    let r = relay(&outbox);
    r.drain_to_empty();
    assert_eq!(
        r.transport().delivered_count(),
        n,
        "every distinct-ref event delivered"
    );
    assert_eq!(outbox.outbox_depth(), 0, "depth drained to 0");
    assert_eq!(outbox.dead_letter_count(), 0);
    for i in 0..n {
        assert_eq!(
            store.tip(&RefName::new(format!("refs/heads/b{i:02}"))),
            Some(Oid::new(format!("tip{i:02}"))),
        );
    }
}
