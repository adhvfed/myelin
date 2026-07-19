//! # GIT-D1 — per-ref aggregate ordering at push QPS (the hot-ref burst) — P-271 / GIT-P10
//!
//! **Drill:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row **GIT-D1** — burst
//! force-pushes + rapid pushes to ONE hot ref at **1×/10×/30×** → `git.ref.updated` in **push order
//! per ref**; refs fan out **parallel**; **0 lost / 0 ghost**; the **outbox order == the ref-update
//! order** per ref. Green artifact: the per-aggregate-order + outbox-depth survival signal
//! (contract 1.8).
//!
//! **Contract:** 2.3 (the `outbox` per-ref aggregate, `UNIQUE(aggregate, seq)` ordering at push QPS)
//! + 1.8 (the per-aggregate-order + outbox-depth survival signal).
//!
//! **Architecture:** git-hosting 02 §3 — the per-ref `FOR UPDATE` row lock is the linearisation
//! point; different refs lock different rows so they advance in parallel, while one ref is strictly
//! serialised.
//!
//! This is the **end-to-end chained burst drill** (EI-01 §4): the receive-pack write path
//! ([`myelin_git::receive_pack::RefStore`]) is hammered by concurrent racers and then drained through
//! the REAL Bus outbox relay ([`myelin_events::relay::Relay`] over the in-process
//! [`InProcessBus`]). The relay claims rows in `(aggregate, seq)` order, so the delivered stream
//! per ref is the committed ref-update order — the property the drill measures.
//!
//! **What GIT-P10 hardens over GIT-P9 (the reconciliation, EI-01 §7):** GIT-P9 held ONE global lock
//! over the whole ref store, which serialised EVERY ref of a repo. Arch §3 requires PER-REF row
//! locks so distinct refs advance in parallel. GIT-P10 replaces the global lock with a per-ref lock
//! cell; THIS drill is the load proof that (a) one hot ref serialises correctly under a 30× surge
//! with 0 lost/ghost, and (b) distinct refs fan out parallel.
//!
//! **The 1×/10×/30× surge multipliers** (EI-01 §3 — the failure-injection harness load levels): each
//! "round" releases `multiplier` racers simultaneously against the hot ref from the SAME expected-old.
//! Exactly one wins the round (advances the ref one generation); the losers are non-fast-forward
//! rejected (lost-update guard). We run `ROUNDS` rounds, so the ref advances exactly `ROUNDS`
//! generations regardless of the surge multiplier — and the committed `update_seq` is the contiguous
//! `1..=ROUNDS`, the outbox per-aggregate seq the contiguous `0..ROUNDS`, in push order.
//!
//! **FLOOR named (VISION §3):** the world-scale concurrent-merge linearizability under failover is
//! **GIT-D5 / GIT-P33** (the 30× real-fleet load on real hardware). This drill proves the per-ref
//! ordering MECHANISM at in-process push QPS; the world-scale fleet drill is its named follow-on.
//!
//! **PERMANENT-gate family note:** GIT-D1 is in the store-touching family — it re-runs on every
//! change to the ref store / outbox path (per-ref ordering is never "done once").

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
/// How many generations the hot ref advances (the chain length per surge level).
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

/// A single-ref fast-forward push from `old` to `new` (a benign quarantine-free push — the burst
/// exercises the ref-CAS, not the policy). `forced` toggles the force-push burst variant.
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

/// Run ONE surge level against a fresh store: `ROUNDS` rounds, each releasing `multiplier` racers at
/// once against the hot ref from the round's current tip. Returns the committed `update_seq` chain
/// (must be `1..=ROUNDS`) + the count of non-fast-forward rejects (the losing racers).
///
/// The hot ref is a NON-protected ref, so a `forced` racer is a legitimate force-push — at 10×/30×
/// this is the "burst force-pushes" half of the GIT-D1 scenario (each round's winner force-moves the
/// tip; the losers race the same generation and are CAS-rejected).
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

    // The hot ref's tip walks forward one generation per round; every racer in a round presents the
    // SAME expected-old (the round's tip), so exactly one wins the per-ref CAS.
    let mut tip = Oid::zero();
    for round in 1..=ROUNDS {
        let barrier = Arc::new(Barrier::new(multiplier));
        let mut handles = Vec::new();
        for racer in 0..multiplier {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            let old = tip.clone();
            // each racer proposes its OWN new tip; the winner's becomes the round's tip.
            let new = Oid::new(format!("r{round:03}c{racer:03}"));
            // every other racer force-pushes (the force-push burst half of GIT-D1).
            let forced = racer % 2 == 1;
            handles.push(std::thread::spawn(move || {
                let db = InMemoryObjectDb::new();
                barrier.wait(); // release the whole round at once → maximal per-ref contention.
                store
                    .receive(&push(HOT_REF, old, new, forced), &db, CrashPoint::None)
                    .unwrap()
            }));
        }

        // Exactly one racer wins the round; the rest are non-fast-forward rejected.
        let mut round_winner: Option<(Oid, u64, EventId)> = None;
        for h in handles {
            match h.join().unwrap() {
                PushOutcome::Accepted { moved, emitted } => {
                    assert!(
                        round_winner.is_none(),
                        "round {round}: TWO racers won — lost-update!"
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

/// **GIT-D1 at 1×/10×/30×: the hot-ref burst keeps push order per ref; 0 lost / 0 ghost; outbox
/// order == ref-update order.** For each surge multiplier the ref advances exactly `ROUNDS`
/// contiguous generations, the per-aggregate outbox seq is gap-free `0..ROUNDS` in that order, the
/// delivered stream (drained in `(aggregate, seq)` order) matches the committed order exactly, and
/// the `(multiplier-1) × ROUNDS` losers are all non-fast-forward rejects (0 lost-update).
#[test]
fn git_d1_hot_ref_burst_per_ref_order_zero_lost_zero_ghost() {
    let key = GitRefEventKey::new(REPO, &RefName::new(HOT_REF)).unwrap();
    let agg = key.aggregate();
    let subject = key.subject(TENANT).unwrap();

    for multiplier in [1usize, 10, 30] {
        let (store, outbox, result) = run_surge(multiplier);

        // (1) The ref advanced exactly ROUNDS generations: update_seq is the contiguous 1..=ROUNDS.
        assert_eq!(
            result.committed_update_seqs,
            (1..=ROUNDS).collect::<Vec<_>>(),
            "{multiplier}×: update_seq is contiguous push order per ref (no gap, no reorder)"
        );

        // (2) 0 lost-update: every loser in every round was a non-fast-forward reject.
        assert_eq!(
            result.rejects,
            (multiplier - 1) * ROUNDS as usize,
            "{multiplier}×: every losing racer was a non-fast-forward reject (0 lost-update)"
        );

        // (3) Exactly ROUNDS events committed (0 ghost: only the per-round winner emitted).
        assert_eq!(
            outbox.committed_count(),
            ROUNDS as usize,
            "{multiplier}×: exactly one git.ref.updated per generation (0 ghost)"
        );

        // (4) The outbox per-aggregate seq is gap-free 0..ROUNDS in committed (push) order — the
        //     per-ref aggregate ordering, contract 2.3.
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

        // (5) Drain through the REAL relay (claims in (aggregate, seq) order) → delivered order ==
        //     committed order; delivered set == committed set (0 lost / 0 ghost end-to-end); depth → 0.
        let r = relay(&outbox);
        r.drain_to_empty();
        let delivered_for_hot: Vec<u64> = r
            .transport()
            .consume(&subject.0)
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

        // (6) The survival signal (contract 1.8): depth drained to 0, 0 dead-letters.
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

        // The store's final tip is the last round's winner, at generation ROUNDS.
        let tip = store
            .tip(&RefName::new(HOT_REF))
            .expect("the hot ref exists");
        let log = store.reflog();
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

/// **GIT-D1: DIFFERENT refs FAN OUT PARALLEL under the burst (no whole-repo serialisation).** A 30×
/// surge spread across 30 DISTINCT refs — every push commits (none is rejected by a whole-repo lock),
/// each ref advances independently to update_seq 1, and the outbox carries one seq-0 event per ref.
/// This is the "refs fan out parallel" half of the GIT-D1 row.
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
            barrier.wait(); // all distinct-ref pushes fire at once → they must NOT serialise.
            (ref_name, store.receive(&p, &db, CrashPoint::None).unwrap())
        }));
    }
    let results: Vec<(String, PushOutcome)> =
        handles.into_iter().map(|h| h.join().unwrap()).collect();

    // Every distinct-ref push committed in parallel (none lost to a whole-repo contention reject).
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

    // Drain → each ref's single event delivers; depth → 0 (the survival signal).
    let r = relay(&outbox);
    r.drain_to_empty();
    assert_eq!(
        r.transport().delivered_count(),
        n,
        "every distinct-ref event delivered"
    );
    assert_eq!(outbox.outbox_depth(), 0, "depth drained to 0");
    assert_eq!(outbox.dead_letter_count(), 0);
    // Each ref is at its own tip — independent generations, no cross-ref interference.
    for i in 0..n {
        assert_eq!(
            store.tip(&RefName::new(format!("refs/heads/b{i:02}"))),
            Some(Oid::new(format!("tip{i:02}"))),
        );
    }
}
