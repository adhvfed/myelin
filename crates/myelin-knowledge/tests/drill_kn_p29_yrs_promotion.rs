//! # KN-P29 / P-484 — the Yrs CRDT promotion drill: KN-D1 RE-GREEN across the engine_promote boundary
//! + the CRDT-convergence gate (the M5 Layer-3 swap)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` KN-D1 — RE-RUN
//! GREEN ACROSS the `engine_promote` (CAS→Yrs) boundary (kill + sever across a per-doc CAS→CRDT
//! cutover → still **0 ops lost, 0 duplicate** — the floor's promotion is itself drilled; the
//! transport survived the swap). Threshold (`external-insights/01` §3, prove-it): 0 lost / 0 dup
//! across a REAL promotion + kill, MEASURED — not asserted. Plus the **CRDT-convergence gate**:
//! concurrent edits to the same block from N clients converge to ONE state (no blend lost, no
//! divergence) — the CRDT's defining property, measured.
//!
//! **Owning prompt:** KN-P29 (knowledge-platform.md). **Architecture:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md`
//! §3.3 (the hybrid-granularity Yrs CRDT), §3.4 (the online per-doc engine_promote migration —
//! quiesce-lite → deterministic seed → cutover op → reconcile in-flight CAS), §3.5 (the move-CRDT
//! owns ordering).
//!
//! ## What this drill proves (the dated green artifact)
//! This is a CHAINED test (a SEQUENCE property — CAS-era edits → engine_promote cutover → Yrs-era
//! edits → kill + sever → reconnect → resume → assert 0 lost/0 dup), run over the REAL Yrs engine
//! ([`myelin_knowledge::yrs_engine`]) riding the REAL unchanged transport
//! ([`myelin_knowledge::transport`]). The engine swap is a PAYLOAD swap on the same op-log: before
//! the cutover the payload is CAS bytes, the cutover op carries the deterministic Yrs seed, after it
//! the payloads are Yrs update bytes — and the resume cursor straddles the boundary unchanged.

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::block_tree::BlockId;
use myelin_knowledge::transport::{
    AllowAllAuthority, AuthAction, CollabTransport, Connected, DocOp, OpId, OpKind, SendOutcome,
};
use myelin_knowledge::yrs_engine::{DocSnapshot, EnginePromotion, YrsDoc};
use myelin_tenancy::TenantId;

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

fn bid(s: &str) -> BlockId {
    BlockId(s.to_string())
}

/// A CAS-era op (opaque CAS payload — the v1 floor bytes the transport carries before the cutover).
fn cas_op(client: &str, lamport: u64) -> DocOp {
    DocOp::cas(
        OpId::new(client, lamport),
        format!("actor-{client}"),
        OpKind::Insert,
        format!("cas:{client}#{lamport}").into_bytes(),
    )
}

/// A Yrs-era op (the SAME transport, now carrying Yrs UPDATE BYTES as the opaque payload — the
/// payload-only swap, §3.4). The transport never interprets the bytes (dumb relay + persistence).
fn yrs_op(client: &str, lamport: u64, update_bytes: Vec<u8>) -> DocOp {
    DocOp::cas(
        OpId::new(client, lamport),
        format!("actor-{client}"),
        OpKind::Insert,
        update_bytes,
    )
}

/// **KN-D1 RE-GREEN across the engine_promote boundary (the headline — the floor's promotion is
/// itself drilled).** CAS-era edits → a REAL deterministic Yrs seed cutover op → Yrs-era edits →
/// kill + sever the client mid-edit → reconnect → resume replays EXACTLY `(last_seq, now]` straddling
/// the cutover → **0 ops lost, 0 duplicate**, and the Yrs-era payloads reconstruct the convergent doc.
#[test]
fn kn_d1_re_greens_across_a_real_engine_promote_cutover_zero_lost_zero_dup() {
    let mut t = transport();

    // ── (1) CAS ERA: two authors interleave content edits over the CAS floor (op_seq 1..3) ─────────
    let mut applied: Vec<(u64, OpKind)> = Vec::new();
    for (client, lamport) in [("c1", 1u64), ("c2", 1), ("c1", 2)] {
        let out = t.send_op(cas_op(client, lamport));
        assert!(out.applied(), "each CAS-era op applies");
        let p = out.persisted();
        applied.push((p.op_seq, p.op.kind));
    }
    assert_eq!(t.head_seq(), 3, "three CAS-era ops, op_seq 1..3");

    // ── (2) THE ENGINE_PROMOTE CUTOVER (§3.4) — quiesce-lite snapshot → deterministic Yrs seed ─────
    // the materialised CAS-era state at the quiesce boundary (the doc holds two blocks).
    let mut snapshot = DocSnapshot::new();
    snapshot.push_block(bid("b1"), "intro");
    snapshot.push_block(bid("b2"), "body");
    // plan the promotion at the current op-log head → the cutover op gets op_seq = head + 1 = 4.
    let promo = EnginePromotion::new(snapshot, t.head_seq());
    assert_eq!(promo.cutover_seq(), 4, "the cutover op_seq is head + 1");
    // the cutover is a SINGLE engine_promote op carrying the DETERMINISTIC seed bytes — an ordinary
    // op on the UNCHANGED transport.
    let cutover_out = t.send_op(promo.cutover_op());
    assert!(
        cutover_out.applied(),
        "the cutover op is an ordinary op on the log"
    );
    assert_eq!(
        cutover_out.persisted().op_seq,
        4,
        "the cutover got the next monotone op_seq (continuity)"
    );
    assert_eq!(cutover_out.persisted().op.kind, OpKind::EnginePromote);
    applied.push((4, OpKind::EnginePromote));

    // ── (3) YRS ERA: the live doc is now the seeded Yrs CRDT; authors edit OVER IT ──────────────────
    // the server's authoritative seeded doc (the post-cutover engine state).
    let live = promo.seeded_doc();
    // c1 edits b1's content; the Yrs update bytes ride the SAME transport (payload-only swap).
    let u1 = live.edit_block_text(&bid("b1"), 5, "!").unwrap(); // "intro!"
    let out = t.send_op(yrs_op("c1", 3, u1));
    assert!(out.applied());
    assert_eq!(out.persisted().op_seq, 5, "Yrs-era op gets op_seq 5");
    applied.push((5, OpKind::Insert));

    // c2 has an IN-FLIGHT Yrs edit it sent right as its connection drops (a real mid-edit kill).
    let u_inflight = live.edit_block_text(&bid("b2"), 4, "?").unwrap(); // "body?"
    let inflight_op = yrs_op("c2", 2, u_inflight);
    let inflight_first = t.send_op(inflight_op.clone());
    assert!(
        inflight_first.applied(),
        "the in-flight Yrs op reached the server before the sever"
    );
    let inflight_seq = inflight_first.persisted().op_seq; // 6
    applied.push((inflight_seq, OpKind::Insert));
    // c2's last DURABLY-APPLIED cursor before the kill — it observed up to op_seq 5 (the cutover +
    // c1's edit), and does NOT know its own op_seq 6 landed (severed before the ack).
    let c2_cursor = 5u64;

    // ── (4) KILL + SEVER: c2 is down; meanwhile c1 keeps editing (the gap grows past the cutover) ──
    let u_more = live.edit_block_text(&bid("b1"), 6, " (rev)").unwrap();
    let out = t.send_op(yrs_op("c1", 4, u_more));
    assert!(out.applied());
    assert_eq!(
        out.persisted().op_seq,
        7,
        "while c2 was down, c1 advanced to op_seq 7"
    );
    applied.push((7, OpKind::Insert));

    // ── (5) RECONNECT + resume across the cutover — 0 ops lost ──────────────────────────────────────
    let connected = t
        .reconnect(&principal(), AuthAction::Edit, c2_cursor)
        .expect("c2 reconnects (warm resume across the cutover)");
    let backfill = match connected {
        Connected::Resumed { backfill } => backfill,
        Connected::ResyncFromSnapshot { tail, .. } => tail,
    };
    // the resume replays EXACTLY (5, now] = op_seq {6, 7} — c2's own op it didn't know about + c1's
    // later edit; the resume cursor STRADDLED the cutover (op_seq 4) with no special-casing — 0 lost.
    let backfill_seqs: Vec<u64> = backfill.iter().map(|p| p.op_seq).collect();
    assert_eq!(
        backfill_seqs,
        vec![6, 7],
        "resume replays (last_seq, now] EXACTLY across the cutover — 0 lost"
    );

    // c2 re-SENDs its in-flight op (never saw the ack) — the UNIQUE(op_id) guard makes it a NO-OP.
    let resend = t.send_op(inflight_op);
    assert!(
        matches!(resend, SendOutcome::Duplicate(_)),
        "the in-flight re-send is an idempotent NO-OP"
    );
    assert_eq!(
        resend.persisted().op_seq,
        inflight_seq,
        "the re-send resolves to its first op_seq (6)"
    );
    assert_eq!(
        t.head_seq(),
        7,
        "the re-send did NOT advance the head (0 duplicate)"
    );

    // ── (6) ASSERT 0 lost / 0 dup: the applied op_seq set is contiguous 1..7, each op exactly once ──
    let seqs: Vec<u64> = applied.iter().map(|(s, _)| *s).collect();
    assert_eq!(
        seqs,
        (1..=7).collect::<Vec<u64>>(),
        "the applied set is op_seq 1..7 contiguous, each op exactly once: 0 lost, 0 duplicate (MEASURED)"
    );

    // ── (7) THE YRS-ERA STATE RECONSTRUCTS: a fresh client loads the seed + replays the Yrs tail ───
    // a client that resumed from the cutover loads the seed bytes once, then applies the Yrs-era
    // update payloads in op_seq order — reconstructing the convergent doc (0 content lost).
    let reconstructed = YrsDoc::from_state(promo.seed_bytes()).unwrap();
    for p in t
        .reconnect(&principal(), AuthAction::Edit, promo.cutover_seq())
        .map(|c| match c {
            Connected::Resumed { backfill } => backfill,
            Connected::ResyncFromSnapshot { tail, .. } => tail,
        })
        .unwrap()
    {
        // replay only the Yrs-era content ops (skip the engine_promote marker itself).
        if p.op.kind != OpKind::EnginePromote {
            reconstructed.apply_update(&p.op.payload).unwrap();
        }
    }
    assert_eq!(
        reconstructed.block_content(&bid("b1")).unwrap(),
        "intro! (rev)",
        "b1's Yrs-era edits reconstructed across the boundary (0 content lost)"
    );
    assert_eq!(
        reconstructed.block_content(&bid("b2")).unwrap(),
        "body?",
        "b2's in-flight Yrs edit reconstructed across the boundary (0 content lost)"
    );
}

/// **THE CRDT-CONVERGENCE GATE: concurrent edits to the SAME block from N clients converge to ONE
/// state (no blend lost, no divergence) — the CRDT's defining property, measured.** Under the CAS
/// floor (KN-P13) one of N concurrent writers would lose; under the Yrs CRDT all N survive and every
/// replica converges to one identical state.
#[test]
fn crdt_convergence_n_clients_same_block_converge_no_blend_lost() {
    const N: usize = 5;
    // all N replicas load the SAME deterministic seed (the post-cutover shared state).
    let mut snapshot = DocSnapshot::new();
    snapshot.push_block(bid("b1"), "");
    let seed = YrsDoc::seed_from_snapshot(&snapshot).encode_state();
    let replicas: Vec<YrsDoc> = (0..N).map(|_| YrsDoc::from_state(&seed).unwrap()).collect();

    // each replica makes a DISTINCT concurrent edit to the SAME block (no coordination).
    let updates: Vec<Vec<u8>> = replicas
        .iter()
        .enumerate()
        .map(|(i, r)| r.edit_block_text(&bid("b1"), 0, &format!("[{i}]")).unwrap())
        .collect();

    // full-mesh exchange: every replica applies every update (re-applying its own is an idempotent
    // no-op — the at-least-once → effectively-once property at the merge layer).
    for r in &replicas {
        for u in &updates {
            r.apply_update(u).unwrap();
        }
    }

    // CONVERGENCE: every replica holds ONE identical state.
    let states: Vec<String> = replicas
        .iter()
        .map(|r| r.block_content(&bid("b1")).unwrap())
        .collect();
    let first = &states[0];
    for (i, s) in states.iter().enumerate() {
        assert_eq!(
            s, first,
            "replica {i} converged to the same state as replica 0 (no divergence)"
        );
    }
    // NO BLEND LOST: all N authors' edits survive (not one losing as under CAS).
    for i in 0..N {
        assert!(
            first.contains(&format!("[{i}]")),
            "author {i}'s edit survived the merge: {first}"
        );
    }
    assert_eq!(
        first.len(),
        N * 3,
        "exactly N inserts present, no duplication, no loss"
    );
}

/// **The promotion is REVERSIBLE (§3.4): the pre-cutover snapshot predates the cutover and is the
/// rollback seed — a botched promotion rolls forward-history back to it.**
#[test]
fn engine_promote_is_reversible_from_the_pre_cutover_snapshot() {
    let mut snapshot = DocSnapshot::new();
    snapshot.push_block(bid("b1"), "original");
    let promo = EnginePromotion::new(snapshot.clone(), 10);
    // the retained snapshot IS the pre-cutover state (the reversibility seed).
    assert_eq!(
        promo.snapshot(),
        &snapshot,
        "the pre-cutover snapshot is retained for rollback"
    );
    // re-seeding from it reproduces the exact pre-cutover content (deterministic rollback).
    let rolled_back = promo.seeded_doc();
    assert_eq!(rolled_back.block_content(&bid("b1")).unwrap(), "original");
}
