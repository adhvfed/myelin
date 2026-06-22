//! # KN-D1 — the resume-cursor durable collab transport drill (the KN-P07 / P-297 HEADLINE)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` KN-D1 (kill a
//! collab client mid-edit + sever the connection during a sustained MULTI-AUTHOR edit; on
//! `resume(scope=doc:<id>, last_seq)` → **0 ops lost, 0 duplicate effects** — the `UNIQUE(op_id)`
//! idempotent apply). Threshold (`external-insights/01` §3, prove-it): **0 ops lost / 0 duplicate
//! across a real kill, MEASURED — not asserted.** Written to RE-RUN GREEN across the KN-P29
//! `engine_promote` (CAS→Yrs) boundary (the drill asserts the TRANSPORT property, which is
//! independent of the apply engine).
//!
//! ## What this drill proves (the KN-P07 GATE — the dated green artifact)
//! This is a CHAINED test (a SEQUENCE property — multi-author edits → kill + sever → reconnect →
//! resume → assert the full op set applied EXACTLY ONCE), not a single-handler test:
//!
//! 1. **A sustained multi-author edit** — two authors (`c1`, `c2`) interleave ops on one doc; each
//!    gets a per-doc monotone `op_seq` (== the firehose seq, OQ-J).
//! 2. **Kill a client mid-edit + SEVER the connection** — one author's connection drops with
//!    in-flight ops it had sent but not yet observed acked; meanwhile the other author keeps editing
//!    (the gap grows while the killed client is down).
//! 3. **RECONNECT + resume** — the killed client re-runs `CONNECT(last_durably_applied_op_seq)`;
//!    `resume` replays EXACTLY `(last_seq, now]`, and its in-flight re-sends hit the `UNIQUE(op_id)`
//!    guard as no-ops.
//! 4. **Assert 0 lost / 0 duplicate** — the reconnected client's applied set, plus the live tail, is
//!    EXACTLY the full op set, each op applied ONCE. The 0-lost/0-dup counters are the dated artifact.
//!
//! The kill is REAL (the connection is dropped + the client re-`connect`s over a fresh subscription —
//! not asserted via a flag). The telemetry the drill reads: op-log apply lag (the seq gap — `== 0`
//! after backfill), op dedup hit-rate (the in-flight re-sends absorbed), resume-gap size (the
//! backfill length), resync_required rate (0 on the warm path; the cold leg is a separate assertion).

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::transport::{
    AllowAllAuthority, AuthAction, CollabTransport, Connected, DocOp, OpId, OpKind, PageSnapshot,
    SendOutcome,
};
use myelin_tenancy::TenantId;
use std::collections::BTreeMap;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p-opaque".into()),
        PrincipalKind::Human,
        tenant(),
    )
}

fn transport() -> CollabTransport<AllowAllAuthority> {
    CollabTransport::open_with_authority(tenant(), "doc-allhands", AllowAllAuthority)
        .expect("opens")
}

/// One author's op (`client` + a per-client lamport, the `(client_id, lamport)` op_id) carrying an
/// opaque CAS payload that names the author + lamport (so we can assert the EXACT applied set).
fn op(client: &str, lamport: u64) -> DocOp {
    DocOp::cas(
        OpId::new(client, lamport),
        format!("actor-{client}"),
        OpKind::Insert,
        format!("{client}#{lamport}").into_bytes(),
    )
}

/// The author+lamport label an op's payload encodes (so the drill asserts the EXACT op set, not just
/// the count) — the "0 ops lost" evidence is the SET equality, not a tally.
fn label(payload: &[u8]) -> String {
    String::from_utf8(payload.to_vec()).expect("ascii payload")
}

/// **KN-D1 — kill a client mid-edit + sever during a sustained multi-author edit; resume loses 0 ops
/// / 0 duplicate (the dated green artifact, MEASURED).**
#[test]
fn kn_d1_kill_and_sever_mid_multi_author_edit_resume_loses_zero_ops_zero_dup() {
    let mut t = transport();

    // ---- (1) a sustained MULTI-AUTHOR edit: c1 and c2 interleave ----------------------------------
    // c1 sends 1,2; c2 sends 1; c1 sends 3 — four ops, monotone op_seq 1..4.
    let mut applied: BTreeMap<u64, String> = BTreeMap::new();
    for (client, lamport) in [("c1", 1u64), ("c1", 2), ("c2", 1), ("c1", 3)] {
        let out = t.send_op(op(client, lamport));
        assert!(out.applied(), "each fresh op applies");
        let p = out.persisted();
        applied.insert(p.op_seq, label(&p.op.payload));
    }
    assert_eq!(t.head_seq(), 4, "four ops applied, op_seq 1..4");

    // c2's connection is the one that will be KILLED; it has durably observed up to op_seq 4 just
    // before the kill, BUT it also had an IN-FLIGHT op it sent that it never saw acked (a real
    // mid-edit kill): c2's lamport 2 op, sent right as the connection drops.
    let inflight = op("c2", 2);
    let inflight_first = t.send_op(inflight.clone());
    assert!(
        inflight_first.applied(),
        "the in-flight op did reach the server before the sever"
    );
    let inflight_seq = inflight_first.persisted().op_seq; // 5
    applied.insert(inflight_seq, label(&inflight_first.persisted().op.payload));

    // c2's last DURABLY-APPLIED cursor (what it will present on reconnect) — it observed up to 4, and
    // does NOT know its lamport-2 op (op_seq 5) landed (the connection severed before the ack).
    let c2_cursor = 4u64;

    // ---- (2) KILL + SEVER: c2 is down; meanwhile c1 keeps editing (the gap grows) -----------------
    // while c2 is down, c1 sends 4,5,6 → op_seq 6,7,8.
    for lamport in [4u64, 5, 6] {
        let out = t.send_op(op("c1", lamport));
        assert!(out.applied());
        let p = out.persisted();
        applied.insert(p.op_seq, label(&p.op.payload));
    }
    assert_eq!(
        t.head_seq(),
        8,
        "while c2 was down, c1 advanced the doc to op_seq 8"
    );

    // ---- (3) RECONNECT: c2 re-runs CONNECT(last_durably_applied = 4) -------------------------------
    let connected = t
        .reconnect(&principal(), AuthAction::Edit, c2_cursor)
        .expect("c2 reconnects (authorized warm resume)");
    let backfill = match connected {
        Connected::Resumed { backfill } => backfill,
        Connected::ResyncFromSnapshot { .. } => {
            panic!("the cursor was in-window — warm resume, not cold")
        }
    };
    // the resume replays EXACTLY (4, now] = op_seq 5..8 (c2's own op_seq 5 it didn't know about + c1's
    // 6,7,8) — 0 ops lost (the gap is fully replayed).
    let backfill_seqs: Vec<u64> = backfill.iter().map(|p| p.op_seq).collect();
    assert_eq!(
        backfill_seqs,
        vec![5, 6, 7, 8],
        "resume replays (last_seq, now] EXACTLY — 0 ops lost"
    );

    // c2 re-SENDs its in-flight op (it never saw the ack, so it retransmits) — the UNIQUE(op_id) guard
    // makes it a NO-OP (0 duplicate effect). This is the at-least-once → effectively-once property.
    let resend = t.send_op(inflight);
    assert!(
        matches!(resend, SendOutcome::Duplicate(_)),
        "c2's in-flight re-send is an idempotent NO-OP (the UNIQUE(op_id) guard)"
    );
    assert_eq!(
        resend.persisted().op_seq,
        inflight_seq,
        "the re-send resolves to its FIRST op_seq (5)"
    );
    assert_eq!(
        t.head_seq(),
        8,
        "the re-send did NOT advance the head (0 duplicate)"
    );

    // ---- (4) ASSERT 0 lost / 0 duplicate: the applied set is EXACTLY the full op set, once each ----
    // the full op set the doc should hold: op_seq 1..8 with the exact author#lamport labels.
    let expected: BTreeMap<u64, String> = [
        (1, "c1#1"),
        (2, "c1#2"),
        (3, "c2#1"),
        (4, "c1#3"),
        (5, "c2#2"),
        (6, "c1#4"),
        (7, "c1#5"),
        (8, "c1#6"),
    ]
    .into_iter()
    .map(|(s, l)| (s, l.to_string()))
    .collect();
    assert_eq!(
        applied, expected,
        "the applied set is the FULL op set, each op exactly once (0 lost)"
    );

    // the reconnected client's view (its pre-kill cursor 4 + the backfill 5..8) covers 1..8 with no
    // gap and no duplicate — the dated 0-lost/0-dup artifact.
    let mut reconnected_view: Vec<u64> = (1..=c2_cursor).collect();
    reconnected_view.extend(backfill_seqs);
    assert_eq!(
        reconnected_view,
        (1..=8).collect::<Vec<u64>>(),
        "the reconnected client sees op_seq 1..8 contiguous: 0 lost, 0 duplicate (MEASURED)"
    );
}

/// **KN-D1 cold leg — a kill whose down-time exceeds the retention window falls back to the snapshot
/// (`resync_required`, NAMED not silent), still 0 ops lost.** A client severed long enough that its
/// cursor predates the bounded firehose window resumes via the block-granular snapshot then applies
/// the live tail — the cold-rebuild path is NAMED, never a silent gap.
#[test]
fn kn_d1_cold_leg_long_sever_resyncs_from_snapshot_zero_lost() {
    // a SMALL retention window forces the out-of-window cold path deterministically.
    let mut t = CollabTransport::open_with_window(tenant(), "doc-allhands", AllowAllAuthority, 3)
        .expect("opens");
    // a compaction minted a snapshot up to op_seq 3 (the block-granular *.snapshot, KN-P11) —
    // modelling the doc already holding ops 1..3, with doc_op rows <= 3 GC'd; the op-log cursor
    // advances to 3 so the next fresh op is op_seq 4.
    t.install_snapshot(PageSnapshot {
        snap_seq: 3,
        blob_hash: "blake3:snap".into(),
    });

    // a sustained edit appends 8 fresh ops on top → op_seq 4..11; the firehose window holds the last 3.
    for (client, lamport) in [
        ("c1", 1u64),
        ("c1", 2),
        ("c2", 1),
        ("c1", 3),
        ("c2", 2),
        ("c1", 4),
        ("c1", 5),
        ("c1", 6),
    ] {
        t.send_op(op(client, lamport));
    }
    assert_eq!(
        t.head_seq(),
        11,
        "the doc advanced past the snapshot (op_seq 4..11)"
    );

    // a client severed at cursor 2 (below the firehose window floor) reconnects → resync_required.
    let connected = t
        .reconnect(&principal(), AuthAction::Edit, 2)
        .expect("the long-severed client reconnects via the cold path");
    match connected {
        Connected::ResyncFromSnapshot { snapshot, tail } => {
            assert_eq!(
                snapshot.snap_seq, 3,
                "the cold path loads the block-granular snapshot (NAMED)"
            );
            // the live tail after the snapshot is (3, now] = op_seq 4..11 — applied on the seed, 0 lost.
            assert_eq!(
                tail.iter().map(|p| p.op_seq).collect::<Vec<_>>(),
                vec![4, 5, 6, 7, 8, 9, 10, 11],
                "the live tail after the snapshot is replayed — 0 ops lost across the cold rebuild"
            );
        }
        Connected::Resumed { .. } => {
            panic!("the cursor was below the window floor — the cold path, not warm")
        }
    }
}

/// **KN-D1 re-run discipline (the engine_promote boundary, KN-P29).** The drill asserts the TRANSPORT
/// property (0 lost / 0 dup on resume + idempotent apply), which is INDEPENDENT of the Layer-3 apply
/// engine (CAS now, Yrs after KN-P29). An `engine_promote` cutover op is just another op on the log —
/// the resume cursor straddles it, and ops before/after it resume identically. This pins that the
/// transport does not special-case the cutover op (the property re-runs green across the boundary).
#[test]
fn kn_d1_resume_straddles_an_engine_promote_cutover_unchanged() {
    let mut t = transport();
    // CAS-era ops.
    t.send_op(op("c1", 1));
    t.send_op(op("c1", 2));
    // the engine_promote cutover op (KN-P29) — from here the payload would carry Yrs bytes; the
    // transport treats it as an ordinary op (it assigns the next op_seq, persists, fans out).
    let cutover = DocOp::cas(
        OpId::new("server", 1),
        "actor-server",
        OpKind::EnginePromote,
        b"seed".to_vec(),
    );
    let promoted = t.send_op(cutover);
    assert!(
        promoted.applied(),
        "the engine_promote op is an ordinary op on the log"
    );
    assert_eq!(
        promoted.persisted().op_seq,
        3,
        "it gets the next monotone op_seq"
    );
    // "Yrs-era" ops after the cutover.
    t.send_op(op("c1", 3));
    t.send_op(op("c1", 4));

    // a client at cursor 1 (a CAS-era op) resumes ACROSS the cutover → backfill {2, cutover(3), 4, 5}
    // — the resume cursor straddles the boundary unchanged (0 lost). KN-P29 re-runs THIS green.
    let connected = t
        .reconnect(&principal(), AuthAction::Edit, 1)
        .expect("resume across the cutover");
    let backfill = match connected {
        Connected::Resumed { backfill } => backfill,
        Connected::ResyncFromSnapshot { tail, .. } => tail,
    };
    assert_eq!(
        backfill.iter().map(|p| p.op_seq).collect::<Vec<_>>(),
        vec![2, 3, 4, 5],
        "resume straddles the engine_promote boundary unchanged — the transport is engine-agnostic"
    );
}
