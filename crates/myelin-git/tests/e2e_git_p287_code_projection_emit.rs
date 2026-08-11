use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp,
};
use myelin_git::code_projection::{
    Blob, CodeProjectionCursor, CodeProjectionEmitter, NoRestrictions, Tree,
};
use myelin_git::events::{GIT_BLOB_SNAPSHOT, GIT_REF_UPDATED};
use myelin_git::receive_pack::{
    CrashPoint, InMemoryObjectDb, Oid, ProposedRefUpdate, PushOutcome, PushSession, Pusher,
    QuarantineObject, RefName, RefStore,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
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
        occurred_at: Timestamp("2026-06-22T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-22T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:push-e2e".into())),
    }
}

fn push_to_main(store: &RefStore, db: &InMemoryObjectDb, old: Oid, new: Oid) -> String {
    let push = PushSession {
        updates: vec![ProposedRefUpdate {
            ref_name: RefName::new("refs/heads/main"),
            expected_old: old,
            new_oid: new.clone(),
            forced: false,
            commit_oids: vec![new.clone()],
        }],
        quarantine: vec![QuarantineObject {
            oid: new.clone(),
            bytes: b"a normal commit".to_vec(),
        }],
        pusher: Pusher::direct("anon-7@acme.noreply", false),
    };
    match store.receive(&push, db, CrashPoint::None).unwrap() {
        PushOutcome::Accepted { moved, .. } => moved[0].1 .0.clone(),
        other => panic!("expected the push to be accepted, got {other:?}"),
    }
}

#[test]
fn push_code_then_projection_emits_per_changed_blob_incrementally() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let store = RefStore::open("core", ctx_base(), outbox.clone(), Arc::clone(&minter));
    let db = InMemoryObjectDb::new();

    let cursor = CodeProjectionCursor::new();
    let restriction = NoRestrictions;
    let emitter = CodeProjectionEmitter::new(
        "core",
        "main",
        ctx_base(),
        &outbox,
        Arc::clone(&minter),
        &cursor,
        &restriction,
    );

    let tip1 = push_to_main(&store, &db, Oid::zero(), Oid::new("commit-1"));
    assert_eq!(
        store.tip(&RefName::new("refs/heads/main")),
        Some(Oid::new("commit-1"))
    );
    let depth_after_push1 = outbox.outbox_depth();
    assert_eq!(depth_after_push1, 1, "one git.ref.updated committed");

    let tree1 = Tree::empty()
        .with(
            "src/main.rs",
            Blob::new("o-main-1", b"fn runServer() {}".to_vec()),
        )
        .with(
            "src/lib.rs",
            Blob::new("o-lib-1", b"pub fn parse_config() {}".to_vec()),
        )
        .with(
            "README.md",
            Blob::new("o-readme-1", b"# the project".to_vec()),
        );

    let emit1 = emitter
        .emit_for_push(
            "refs/heads/main",
            &tip1,
            &Tree::empty(),
            &tree1,
            "initial commit",
        )
        .unwrap()
        .expect("an indexed-ref push emits a projection");
    assert_eq!(emit1.changed_blob_count, 3);
    assert_eq!(
        emit1.emitted.len(),
        3,
        "first index = one blob snapshot per file"
    );
    assert_eq!(
        cursor.last_indexed("core", "refs/heads/main").as_deref(),
        Some("commit-1")
    );
    assert_eq!(outbox.committed_count(), 1 + 3);
    for id in &emit1.emitted {
        let row = outbox.row(id).unwrap();
        assert_eq!(row.envelope.type_.0, GIT_BLOB_SNAPSHOT);
        assert_eq!(row.aggregate.0, "ref:core:refs%2Fheads%2Fmain");
    }

    let tip2 = push_to_main(&store, &db, Oid::new("commit-1"), Oid::new("commit-2"));
    let tree2 = Tree::empty()
        .with(
            "src/main.rs",
            Blob::new("o-main-2", b"fn runServer() { listen() }".to_vec()),
        )
        .with(
            "src/lib.rs",
            Blob::new("o-lib-1", b"pub fn parse_config() {}".to_vec()),
        )
        .with(
            "src/net.rs",
            Blob::new("o-net-1", b"fn connect() {}".to_vec()),
        );

    let emit2 = emitter
        .emit_for_push("refs/heads/main", &tip2, &tree1, &tree2, "second commit")
        .unwrap()
        .unwrap();
    assert_eq!(
        emit2.changed_blob_count, 3,
        "modified + added + deleted; the unchanged file is skipped"
    );
    assert_eq!(
        emit2.emitted.len(),
        3,
        "incremental: NOT the whole 3-file tree re-emitted"
    );
    assert_eq!(
        cursor.last_indexed("core", "refs/heads/main").as_deref(),
        Some("commit-2")
    );

    let mut deletes = 0;
    let mut upserts = 0;
    for id in &emit2.emitted {
        let pl = outbox.row(id).unwrap().envelope.payload;
        match pl["op"].as_str().unwrap() {
            "delete" => {
                deletes += 1;
                assert_eq!(
                    pl["path"],
                    serde_json::json!("README.md"),
                    "the deleted file is the tombstone"
                );
            }
            "upsert" => upserts += 1,
            other => panic!("unexpected op {other}"),
        }
    }
    assert_eq!(
        deletes, 1,
        "the deleted blob is a tombstone (Gone is never silently dropped)"
    );
    assert_eq!(upserts, 2, "the modified + added blobs are upserts");

    let tip3 = push_to_main(&store, &db, Oid::new("commit-2"), Oid::new("commit-3"));
    let emit3 = emitter
        .emit_for_push("refs/heads/main", &tip3, &tree2, &tree2, "empty-ish")
        .unwrap()
        .unwrap();
    assert_eq!(emit3.changed_blob_count, 0);
    assert_eq!(
        emit3.emitted.len(),
        0,
        "a push that changed no blobs emits no projection (incremental)"
    );
    assert_eq!(
        cursor.last_indexed("core", "refs/heads/main").as_deref(),
        Some("commit-3")
    );

    assert_eq!(
        outbox.committed_count(),
        9,
        "3 ref events + 6 blob snapshots"
    );
    let blob_count = emit1
        .emitted
        .iter()
        .chain(emit2.emitted.iter())
        .filter(|id| outbox.row(id).unwrap().envelope.type_.0 == GIT_BLOB_SNAPSHOT)
        .count();
    assert_eq!(blob_count, 6, "6 blob snapshots over pushes 1+2");
    assert_eq!(GIT_REF_UPDATED, "git.ref.updated");
}

#[test]
fn a_feature_branch_push_indexes_no_code() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let store = RefStore::open("core", ctx_base(), outbox.clone(), Arc::clone(&minter));
    let db = InMemoryObjectDb::new();
    let cursor = CodeProjectionCursor::new();
    let restriction = NoRestrictions;
    let emitter = CodeProjectionEmitter::new(
        "core",
        "main",
        ctx_base(),
        &outbox,
        Arc::clone(&minter),
        &cursor,
        &restriction,
    );

    let push = PushSession {
        updates: vec![ProposedRefUpdate {
            ref_name: RefName::new("refs/heads/feature"),
            expected_old: Oid::zero(),
            new_oid: Oid::new("f1"),
            forced: false,
            commit_oids: vec![Oid::new("f1")],
        }],
        quarantine: vec![QuarantineObject {
            oid: Oid::new("f1"),
            bytes: b"x".to_vec(),
        }],
        pusher: Pusher::direct("anon-1@acme.noreply", false),
    };
    store.receive(&push, &db, CrashPoint::None).unwrap();

    let tree = Tree::empty().with("a.rs", Blob::new("o", b"fn a() {}".to_vec()));
    let out = emitter
        .emit_for_push("refs/heads/feature", "f1", &Tree::empty(), &tree, "wip")
        .unwrap();
    assert!(
        out.is_none(),
        "a feature-branch push does not index code (only indexed refs)"
    );
    assert_eq!(
        outbox.committed_count(),
        1,
        "only the git.ref.updated, no projection"
    );
}
