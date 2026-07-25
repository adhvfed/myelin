//! # CDC pair — contracts 2.2/2.3: the `git.ref.updated` one-tx outbox emit (provider) ↔ a
//! consumer reading the wire envelope (consumer) — P-270 / GIT-P9
//!
//! **Contract:** 2.2 (`OutboxTx::emit` — the same-transaction co-commit) + 2.3 (the `outbox` row's
//! per-aggregate ordering, `UNIQUE(aggregate, seq)`) + 2.9 (`git.ref.updated`). The PROVIDER half is
//! [`myelin_git::receive_pack::RefStore::receive`] — the receive-pack → ref-CAS → outbox emit in one
//! transaction. The CONSUMER half is a reader that decodes the serialized [`EventEnvelope`] off the
//! outbox row to its NAMED `git.ref.updated` fields (the wire shape CI/Search/Refs/Agents consume).
//!
//! This is the provider/consumer contract-test pair the prompt requires for rows 2.2/2.3: the
//! provider produces the wire envelope; the consumer reconstructs the fields from JSON — proving the
//! `git.ref.updated` payload is the frozen, round-tripping shape (no drift between emit + consume).

use myelin_events::{
    Actor, CausedBy, EmitContextBase, EventEnvelope, IdMinter, MonotonicMinter, OutboxStore,
    Region, TenantId, Timestamp,
};
use myelin_git::events::GIT_REF_UPDATED;
use myelin_git::receive_pack::{
    CrashPoint, InMemoryObjectDb, Oid, ProposedRefUpdate, PushOutcome, PushSession, Pusher,
    QuarantineObject, RefName, RefStore,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use std::sync::{Arc, Barrier};

/// The CONSUMER-side view of a `git.ref.updated` event (the fields CI/Search/Refs key on) decoded
/// from the wire envelope's JSON payload — the consumer half of the CDC pair.
#[derive(Debug, PartialEq, Eq)]
struct RefUpdatedView {
    repo: String,
    ref_name: String,
    old_oid: String,
    new_oid: String,
    forced: bool,
    commit_oids: Vec<String>,
    pusher_pseudonym: String,
    update_seq: u64,
}

impl RefUpdatedView {
    /// Decode the wire envelope to the named `git.ref.updated` fields (the consumer contract). A
    /// missing/wrong-shaped field is a LOUD decode failure (never a silent wrong value).
    fn decode(env: &EventEnvelope) -> Result<RefUpdatedView, String> {
        if env.type_.0 != GIT_REF_UPDATED {
            return Err(format!("not a git.ref.updated event: {}", env.type_.0));
        }
        let p = &env.payload;
        let s = |k: &str| {
            p.get(k)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| format!("missing string field {k}"))
        };
        Ok(RefUpdatedView {
            repo: s("repo")?,
            ref_name: s("ref")?,
            old_oid: s("old_oid")?,
            new_oid: s("new_oid")?,
            forced: p
                .get("forced")
                .and_then(|v| v.as_bool())
                .ok_or("missing forced")?,
            commit_oids: p
                .get("commit_oids")
                .and_then(|v| v.as_array())
                .ok_or("missing commit_oids")?
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect(),
            pusher_pseudonym: s("pusher_pseudonym")?,
            update_seq: p
                .get("update_seq")
                .and_then(|v| v.as_u64())
                .ok_or("missing update_seq")?,
        })
    }
}

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
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:cdc".into())),
    }
}

/// **The CDC pair: provider emits the wire envelope → consumer decodes the named fields, lossless.**
/// The provider (the receive-pack one-tx path) produces a `git.ref.updated` row; the consumer reads
/// the SERIALIZED envelope (round-tripped through JSON, as the relay+broker carry it) and
/// reconstructs the named fields — proving the 2.2/2.3/2.9 wire shape is the contract.
#[test]
fn git_ref_updated_provider_consumer_wire_shape_round_trips() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let store = RefStore::open("core", ctx_base(), outbox.clone(), minter);
    let db = InMemoryObjectDb::new();

    let push = PushSession {
        updates: vec![ProposedRefUpdate {
            ref_name: RefName::new("refs/heads/main"),
            expected_old: Oid::zero(),
            new_oid: Oid::new("abc123"),
            forced: false,
            commit_oids: vec![Oid::new("abc123"), Oid::new("def456")],
        }],
        quarantine: vec![QuarantineObject {
            oid: Oid::new("abc123"),
            bytes: b"commit".to_vec(),
        }],
        pusher: Pusher {
            pseudonym: "anon-9@acme.noreply".into(),
            is_agent: false,
        },
    };

    let id = match store.receive(&push, &db, CrashPoint::None).unwrap() {
        PushOutcome::Accepted { emitted, .. } => emitted[0].clone(),
        o => panic!("expected Accepted, got {o:?}"),
    };

    // PROVIDER: the committed outbox row carries the canonical envelope.
    let row = outbox.row(&id).expect("the committed git.ref.updated row");
    // Serialize → deserialize, exactly as the relay+broker carry it over the wire (no drift).
    let wire = serde_json::to_string(&row.envelope).expect("the envelope serializes");
    let env: EventEnvelope = serde_json::from_str(&wire).expect("the envelope round-trips");

    // CONSUMER: decode the named git.ref.updated fields off the wire envelope.
    let view = RefUpdatedView::decode(&env).expect("the consumer decodes the named fields");
    assert_eq!(
        view,
        RefUpdatedView {
            repo: "core".into(),
            ref_name: "refs/heads/main".into(),
            old_oid: Oid::zero().0,
            new_oid: "abc123".into(),
            forced: false,
            commit_oids: vec!["abc123".into(), "def456".into()],
            pusher_pseudonym: "anon-9@acme.noreply".into(),
            update_seq: 1,
        }
    );

    // The envelope's own frozen fields are the 2.1/2.3 contract: type, per-ref aggregate, and the
    // pseudonymous payload carries NO inline PII (references-not-payloads).
    assert_eq!(env.type_.0, GIT_REF_UPDATED);
    assert_eq!(
        env.aggregate.0, "ref:core:refs%2Fheads%2Fmain",
        "the per-ref aggregate (2.3) — `ref:` prefix + percent-encoded ref name, matching \
         `GitRefEventKey::aggregate` (receive_pack.rs) and the format every other aggregate-key \
         test in this crate already expects (gt003_reconcile.rs, code_projection.rs)"
    );
    assert!(
        !env.contains_personal_data,
        "the pusher pseudonym is not inline PII (4.8)"
    );
    assert_eq!(
        row.seq, 0,
        "first event on the per-ref aggregate is outbox seq 0 (2.3)"
    );
}

/// **2.3 per-aggregate ordering across the CDC boundary: successive pushes to one ref carry
/// contiguous outbox seqs the consumer reads in order.** The provider commits three pushes; the
/// consumer reads update_seq 1,2,3 in the per-aggregate outbox seq order 0,1,2.
#[test]
fn git_ref_updated_per_ref_ordering_is_consumed_in_order() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let store = RefStore::open("core", ctx_base(), outbox.clone(), minter);
    let db = InMemoryObjectDb::new();

    let mut ids = Vec::new();
    for (old, new) in [
        (Oid::zero(), Oid::new("s1")),
        (Oid::new("s1"), Oid::new("s2")),
        (Oid::new("s2"), Oid::new("s3")),
    ] {
        let p = PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name: RefName::new("refs/heads/feature"),
                expected_old: old,
                new_oid: new.clone(),
                forced: false,
                commit_oids: vec![new],
            }],
            quarantine: vec![],
            pusher: Pusher {
                pseudonym: "anon-1@acme.noreply".into(),
                is_agent: false,
            },
        };
        match store.receive(&p, &db, CrashPoint::None).unwrap() {
            PushOutcome::Accepted { emitted, .. } => ids.push(emitted[0].clone()),
            o => panic!("{o:?}"),
        }
    }

    // The consumer reads the per-aggregate rows ordered by outbox seq → update_seq is monotonic.
    let mut rows: Vec<_> = ids.iter().map(|id| outbox.row(id).unwrap()).collect();
    rows.sort_by_key(|r| r.seq);
    let seqs: Vec<u64> = rows.iter().map(|r| r.seq).collect();
    assert_eq!(
        seqs,
        vec![0, 1, 2],
        "contiguous per-aggregate outbox seqs (2.3, gap-free)"
    );
    let update_seqs: Vec<u64> = rows
        .iter()
        .map(|r| RefUpdatedView::decode(&r.envelope).unwrap().update_seq)
        .collect();
    assert_eq!(
        update_seqs,
        vec![1, 2, 3],
        "the consumer reads update_seq in per-ref order"
    );
}

/// **2.3 per-ref ordering UNDER A HOT-REF BURST (GIT-P10 / GIT-D1): a concurrent burst on one ref
/// still yields a contiguous, per-aggregate-ordered consumer stream.** N racers chain-advance one
/// hot ref (one wins each generation, the rest are CAS-rejected); the consumer, reading the
/// committed rows in per-aggregate `seq` order, sees `update_seq` 1,2,…,k with NO gap and NO reorder
/// — proving the per-ref aggregate ordering (2.3) holds at push QPS, not just for serial pushes.
#[test]
fn git_ref_updated_per_ref_ordering_survives_a_concurrent_burst() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let store = Arc::new(RefStore::open("core", ctx_base(), outbox.clone(), minter));

    let k = 20u64;
    let multiplier = 12usize; // 12 racers per generation, all hammering the one hot ref.
    let mut tip = Oid::zero();
    let mut committed_ids = Vec::new();

    for round in 1..=k {
        let barrier = Arc::new(Barrier::new(multiplier));
        let mut handles = Vec::new();
        for racer in 0..multiplier {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            let old = tip.clone();
            let new = Oid::new(format!("g{round:02}c{racer:02}"));
            handles.push(std::thread::spawn(move || {
                let db = InMemoryObjectDb::new();
                let p = PushSession {
                    updates: vec![ProposedRefUpdate {
                        ref_name: RefName::new("refs/heads/hot"),
                        expected_old: old,
                        new_oid: new.clone(),
                        forced: false,
                        commit_oids: vec![new],
                    }],
                    quarantine: vec![],
                    pusher: Pusher {
                        pseudonym: "anon-1@acme.noreply".into(),
                        is_agent: false,
                    },
                };
                barrier.wait();
                store.receive(&p, &db, CrashPoint::None).unwrap()
            }));
        }
        let mut winner: Option<(Oid, myelin_events::EventId)> = None;
        for h in handles {
            if let PushOutcome::Accepted { moved, emitted } = h.join().unwrap() {
                assert!(
                    winner.is_none(),
                    "round {round}: two winners — a lost update!"
                );
                winner = Some((moved[0].1.clone(), emitted[0].clone()));
            }
        }
        let (winner_tip, id) = winner.expect("exactly one winner per generation");
        committed_ids.push(id);
        tip = winner_tip;
    }

    // The consumer reads the per-aggregate rows in `seq` order → contiguous `update_seq` 1..=k.
    let mut rows: Vec<_> = committed_ids
        .iter()
        .map(|id| outbox.row(id).unwrap())
        .collect();
    rows.sort_by_key(|r| r.seq);
    let seqs: Vec<u64> = rows.iter().map(|r| r.seq).collect();
    assert_eq!(
        seqs,
        (0..k).collect::<Vec<_>>(),
        "per-aggregate outbox seq is gap-free under burst (2.3)"
    );
    let update_seqs: Vec<u64> = rows
        .iter()
        .map(|r| RefUpdatedView::decode(&r.envelope).unwrap().update_seq)
        .collect();
    assert_eq!(
        update_seqs,
        (1..=k).collect::<Vec<_>>(),
        "the consumer reads update_seq in per-ref push order despite the concurrent burst"
    );
}
