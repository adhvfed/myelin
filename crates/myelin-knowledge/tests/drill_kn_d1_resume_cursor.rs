use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::transport::{
    AllowAllAuthority, AuthAction, CollabTransport, DocOp, OpId, OpKind, PageSnapshot, Recovery,
    SendOutcome,
};
use myelin_storage::blob::ContentHash;
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
    CollabTransport::open(tenant(), "doc-allhands", AllowAllAuthority).expect("opens")
}

fn op(client: &str, lamport: u64) -> DocOp {
    DocOp::cas(
        OpId::new(client, lamport),
        format!("actor-{client}"),
        OpKind::Insert,
        format!("{client}#{lamport}").into_bytes(),
    )
}

fn send(t: &mut CollabTransport<AllowAllAuthority>, op: DocOp) -> SendOutcome {
    let actor = Principal::stub(
        PrincipalId(op.actor.clone()),
        PrincipalKind::Human,
        tenant(),
    );
    t.send_op(&actor, op)
        .expect("the actor is authorized to edit")
}

fn label(payload: &[u8]) -> String {
    String::from_utf8(payload.to_vec()).expect("ascii payload")
}

#[test]
fn kn_d1_kill_and_sever_mid_multi_author_edit_resume_loses_zero_ops_zero_dup() {
    let mut t = transport();

    let mut applied: BTreeMap<u64, String> = BTreeMap::new();
    for (client, lamport) in [("c1", 1u64), ("c1", 2), ("c2", 1), ("c1", 3)] {
        let out = send(&mut t, op(client, lamport));
        assert!(out.applied(), "each fresh op applies");
        let p = out.persisted();
        applied.insert(p.op_seq, label(&p.op.payload));
    }
    assert_eq!(t.head_seq(), 4, "four ops applied, op_seq 1..4");

    let inflight = op("c2", 2);
    let inflight_first = send(&mut t, inflight.clone());
    assert!(
        inflight_first.applied(),
        "the in-flight op did reach the server before the sever"
    );
    let inflight_seq = inflight_first.persisted().op_seq;
    applied.insert(inflight_seq, label(&inflight_first.persisted().op.payload));

    let c2_cursor = 4u64;

    for lamport in [4u64, 5, 6] {
        let out = send(&mut t, op("c1", lamport));
        assert!(out.applied());
        let p = out.persisted();
        applied.insert(p.op_seq, label(&p.op.payload));
    }
    assert_eq!(
        t.head_seq(),
        8,
        "while c2 was down, c1 advanced the doc to op_seq 8"
    );

    let connected = t
        .recover(&principal(), AuthAction::Edit, Some(c2_cursor))
        .expect("c2 reconnects (authorized warm resume)");
    let backfill = match connected {
        Recovery::Resumed { backfill, .. } => backfill,
        Recovery::RebuiltFromLog { .. } | Recovery::ResyncFromSnapshot { .. } => {
            panic!("the cursor was in-window - warm resume, not cold")
        }
    };
    let backfill_seqs: Vec<u64> = backfill.iter().map(|p| p.op_seq).collect();
    assert_eq!(
        backfill_seqs,
        vec![5, 6, 7, 8],
        "resume replays (last_seq, now] EXACTLY - 0 ops lost"
    );

    let resend = send(&mut t, inflight);
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

    let mut reconnected_view: Vec<u64> = (1..=c2_cursor).collect();
    reconnected_view.extend(backfill_seqs);
    assert_eq!(
        reconnected_view,
        (1..=8).collect::<Vec<u64>>(),
        "the reconnected client sees op_seq 1..8 contiguous: 0 lost, 0 duplicate (MEASURED)"
    );
}

#[test]
fn kn_d1_cold_leg_long_sever_resyncs_from_snapshot_zero_lost() {
    let mut t = CollabTransport::open_with_window(tenant(), "doc-allhands", AllowAllAuthority, 3)
        .expect("opens");
    t.install_snapshot(PageSnapshot {
        snap_seq: 3,
        blob_hash: ContentHash::blake3(b"snapshot at 3"),
    })
    .expect("the snapshot seeds an empty live stream");

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
        send(&mut t, op(client, lamport));
    }
    assert_eq!(
        t.head_seq(),
        11,
        "the doc advanced past the snapshot (op_seq 4..11)"
    );

    let connected = t
        .recover(&principal(), AuthAction::Edit, Some(2))
        .expect("the long-severed client reconnects via the cold path");
    match connected {
        Recovery::ResyncFromSnapshot { snapshot, tail, .. } => {
            assert_eq!(
                snapshot.snap_seq, 3,
                "the cold path loads the block-granular snapshot (NAMED)"
            );
            assert_eq!(
                tail.iter().map(|p| p.op_seq).collect::<Vec<_>>(),
                vec![4, 5, 6, 7, 8, 9, 10, 11],
                "the live tail after the snapshot is replayed - 0 ops lost across the cold rebuild"
            );
        }
        Recovery::Resumed { .. } | Recovery::RebuiltFromLog { .. } => {
            panic!("the cursor was below the window floor - the cold path, not warm")
        }
    }
}

#[test]
fn kn_d1_resume_straddles_an_engine_promote_cutover_unchanged() {
    let mut t = transport();
    send(&mut t, op("c1", 1));
    send(&mut t, op("c1", 2));
    let cutover = DocOp::cas(
        OpId::new("server", 1),
        "actor-server",
        OpKind::EnginePromote,
        b"seed".to_vec(),
    );
    let promoted = send(&mut t, cutover);
    assert!(
        promoted.applied(),
        "the engine_promote op is an ordinary op on the log"
    );
    assert_eq!(
        promoted.persisted().op_seq,
        3,
        "it gets the next monotone op_seq"
    );
    send(&mut t, op("c1", 3));
    send(&mut t, op("c1", 4));

    let connected = t
        .recover(&principal(), AuthAction::Edit, Some(1))
        .expect("resume across the cutover");
    let backfill = match connected {
        Recovery::Resumed { backfill, .. } | Recovery::RebuiltFromLog { backfill, .. } => backfill,
        Recovery::ResyncFromSnapshot { tail, .. } => tail,
    };
    assert_eq!(
        backfill.iter().map(|p| p.op_seq).collect::<Vec<_>>(),
        vec![2, 3, 4, 5],
        "resume straddles the engine_promote boundary unchanged - the transport is engine-agnostic"
    );
}
