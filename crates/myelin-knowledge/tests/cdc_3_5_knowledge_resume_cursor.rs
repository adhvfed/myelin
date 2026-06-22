//! # The CDC pair for contract 3.5 — Knowledge's resume-cursor half (KN-P07 / P-297)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 3.5 (the
//! firehose resume-cursor transport — the **owned-seam**: the Bus provides the transport seam, but
//! **the resume-cursor + idempotent-apply discipline is Knowledge's deliverable**). Owning
//! architecture: `knowledge-platform/architecture/02-internals-and-algorithms.md` §2 (the full
//! `CONNECT`/`SEND_OP`/`RECONNECT` protocol). Reconciliation: `00-reconciliation-decisions.md` OQ-J.
//!
//! ## The contract this pair pins (Knowledge's half of the owned seam)
//! Where the Bus CDC (`myelin-events/tests/cdc_3_5_firehose_resume_cursor.rs`) pins the TRANSPORT
//! seam (publish/subscribe/resume + the monotone seq), THIS pair pins **Knowledge's resume-cursor +
//! idempotent-apply discipline over it**:
//!
//! - the **PROVIDER** side = a collaborator client `SEND_OP`s an op; Knowledge assigns the per-doc
//!   monotone `op_seq` (== the firehose seq), PERSISTs it idempotently (`UNIQUE(op_id)`), and fans
//!   the frame out — and a re-delivered op is a NO-OP (the idempotent-apply property Knowledge owns);
//! - the **CONSUMER** side = a reconnecting client `CONNECT`s at its `last_seq`; the transport
//!   backfills `(last_seq, now]` from the `doc_op` op-log then goes live — **0 ops lost, 0 duplicate**
//!   — and an out-of-window cursor falls back to the snapshot (`resync_required`, NAMED).
//!
//! This is the dedicated 3.5 Knowledge provider+consumer pair the KN-P07 TESTS field names; the
//! focused per-mechanism unit tests live in `transport.rs::tests`, the KN-D1 chained drill in
//! `tests/drill_kn_d1_resume_cursor.rs`.

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::transport::{
    AllowAllAuthority, AuthAction, CollabTransport, Connected, DocOp, OpId, OpKind, SendOutcome,
};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn principal() -> Principal {
    Principal::stub(PrincipalId("p-opaque".into()), PrincipalKind::Human, tenant())
}

fn transport() -> CollabTransport<AllowAllAuthority> {
    CollabTransport::open_with_authority(tenant(), "doc-design", AllowAllAuthority).expect("opens")
}

fn op(client: &str, lamport: u64) -> DocOp {
    DocOp::cas(OpId::new(client, lamport), "actor", OpKind::Insert, format!("cas:{lamport}").into_bytes())
}

/// **PROVIDER side of 3.5 (Knowledge's half)** — a collaborator client `SEND_OP`s an op. Knowledge's
/// promise: it assigns the per-doc monotone `op_seq` (the client never mints it), PERSISTs idempotent
/// on `UNIQUE(op_id)`, and fans the frame out. Returns the [`SendOutcome`] (the assigned op_seq +
/// whether it was a fresh apply or an idempotent no-op).
fn provider_send_op(t: &mut CollabTransport<AllowAllAuthority>, op: DocOp) -> SendOutcome {
    t.send_op(op)
}

/// **CONSUMER side of 3.5 (Knowledge's half)** — a reconnecting client `CONNECT`s at its `last_seq`.
/// The consumer's promise: it authorizes (Layer 2), then backfills `(last_seq, now]` from the op-log
/// then goes live — contiguous, 0 lost, 0 duplicate. Returns the backfilled op_seqs it received.
fn consumer_connect_backfill(
    t: &mut CollabTransport<AllowAllAuthority>,
    last_seq: u64,
) -> Vec<u64> {
    match t
        .connect(&principal(), AuthAction::Edit, Some(last_seq))
        .expect("an authorized in-window connect resumes")
    {
        Connected::Resumed { backfill } => backfill.iter().map(|p| p.op_seq).collect(),
        Connected::ResyncFromSnapshot { tail, .. } => tail.iter().map(|p| p.op_seq).collect(),
    }
}

/// The 3.5 pair, end-to-end (Knowledge's half): a PROVIDER `SEND_OP`s ops with a transport-assigned
/// monotone `op_seq`; the connection drops; the provider keeps sending; the CONSUMER `RECONNECT`s and
/// backfills the gap then goes live — **0 lost, 0 duplicate** — the resume-cursor + idempotent-apply
/// discipline Knowledge OWNS over the Bus transport seam.
#[test]
fn cdc_3_5_knowledge_provider_sends_consumer_reconnects_loses_zero_ops() {
    let mut t = transport();

    // PROVIDER sends 1,2,3 — each gets a transport-assigned per-doc monotone op_seq.
    for (i, lamport) in [1, 2, 3].iter().enumerate() {
        let out = provider_send_op(&mut t, op("c1", *lamport));
        assert!(out.applied(), "a fresh op applies");
        assert_eq!(out.persisted().op_seq, (i + 1) as u64, "op_seq is monotone (== firehose seq)");
    }

    // the consumer saw up to op_seq 2, then the connection dropped; meanwhile 3 (already sent) +
    // 4,5 are sent.
    provider_send_op(&mut t, op("c1", 4));
    provider_send_op(&mut t, op("c1", 5));

    // CONSUMER RECONNECTs with last_seq = 2 → backfill (2, now] = {3,4,5}, 0 lost.
    let backfilled = consumer_connect_backfill(&mut t, 2);
    assert_eq!(backfilled, vec![3, 4, 5], "the gap (last_seq, now] is replayed — 0 ops lost");
}

/// **The idempotent-apply half (Knowledge's owned discipline): a re-delivered op is a NO-OP.** The
/// PROVIDER re-sends an in-flight op (same `op_id` = `(client_id, lamport)`); the `UNIQUE(op_id)`
/// guard makes it a no-op — NOT a second apply, NOT a new op_seq, NOT a duplicate frame — so a flaky
/// network's at-least-once retransmit cannot double-apply (the 0-duplicate property).
#[test]
fn cdc_3_5_knowledge_redelivered_op_is_idempotent_no_op() {
    let mut t = transport();
    let first = provider_send_op(&mut t, op("c1", 9));
    assert!(first.applied());
    let seq = first.persisted().op_seq;

    // a re-delivery of the SAME op (at-least-once) — a no-op.
    let again = provider_send_op(&mut t, op("c1", 9));
    assert!(!again.applied(), "a re-delivered op did NOT freshly apply");
    assert!(matches!(again, SendOutcome::Duplicate(_)), "it is an idempotent Duplicate no-op");
    assert_eq!(again.persisted().op_seq, seq, "the duplicate resolves to the FIRST op_seq");
    assert_eq!(t.head_seq(), seq, "the head did NOT advance (0 duplicate effect)");
    assert_eq!(t.op_count(), 1, "exactly one op persisted (the duplicate was absorbed)");
}
