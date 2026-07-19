//! # The chained e2e for GIT-P25 / P-287 — **push code → the code-projection emits per changed
//! blob, incrementally** (the §9 TE-27 code projection).
//!
//! "Actually try it — chain the mutations end-to-end" (EI-01 §4). This drives a REAL push through
//! the receive-pack write path ([`myelin_git::receive_pack::RefStore`], GIT-P9) — the ref moves +
//! `git.ref.updated` co-commits — and then runs the **code-projection emitter** (GIT-P25) for that
//! same push against the post-commit tree, asserting:
//!
//! 1. the FIRST index of `main` projects the whole tree (one `git.blob.snapshot` per file);
//! 2. a SECOND push that touches a SUBSET of files emits exactly that subset (incremental —
//!    emit-count == changed-blob-count, NOT the whole tree; 0 missed / 0 stale);
//! 3. the `code_projection_cursor` advances to each new tip (so the next diff is against it);
//! 4. a delete emits a tombstone; an unchanged blob emits nothing.
//!
//! The receive-pack `git.ref.updated` and the code-projection `git.blob.snapshot` both ride the ONE
//! outbox (the same co-commit substrate) — so the projection is ordered behind the ref move that
//! produced it (per-ref aggregate). The GATE is the incremental emit count.

use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp,
};
use myelin_git::code_projection::{
    Blob, CodeProjectionCursor, CodeProjectionEmitter, NoRestrictions, Tree,
};
use myelin_git::events::{GIT_BLOB_REMOVED, GIT_BLOB_SNAPSHOT, GIT_REF_UPDATED};
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

/// Drive a real receive-pack push to `main` (the ref moves + git.ref.updated co-commits), returning
/// the new tip oid the projection cursor advances to.
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
            // a normal commit blob — passes the secret/size policy.
            bytes: b"a normal commit".to_vec(),
        }],
        pusher: Pusher {
            pseudonym: "anon-7@acme.noreply".into(),
            is_agent: false,
        },
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
    // The ref store (the receive-pack write path) shares the outbox with the projection emitter.
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

    // ── Push 1: the first commit lands main with three files. ──
    let tip1 = push_to_main(&store, &db, Oid::zero(), Oid::new("commit-1"));
    assert_eq!(
        store.tip(&RefName::new("refs/heads/main")),
        Some(Oid::new("commit-1"))
    );
    // The receive-pack git.ref.updated is durable.
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
    // The first index projects the WHOLE tree: 3 files → 3 blob snapshots.
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
    // The outbox now carries the ref event + the 3 blob snapshots.
    assert_eq!(outbox.committed_count(), 1 + 3);
    // Every projection emit is the NAMED git.blob.snapshot token on the per-ref aggregate.
    for id in &emit1.emitted {
        let row = outbox.row(id).unwrap();
        assert_eq!(row.envelope.type_.0, GIT_BLOB_SNAPSHOT);
        assert_eq!(row.aggregate.0, "ref:core:refs%2Fheads%2Fmain");
    }

    // ── Push 2: modify ONE file, add ONE, delete ONE; one file unchanged. ──
    let tip2 = push_to_main(&store, &db, Oid::new("commit-1"), Oid::new("commit-2"));
    let tree2 = Tree::empty()
        // src/main.rs modified (new oid)
        .with(
            "src/main.rs",
            Blob::new("o-main-2", b"fn runServer() { listen() }".to_vec()),
        )
        // src/lib.rs UNCHANGED (same oid)
        .with(
            "src/lib.rs",
            Blob::new("o-lib-1", b"pub fn parse_config() {}".to_vec()),
        )
        // README.md deleted (absent)
        // src/net.rs added
        .with(
            "src/net.rs",
            Blob::new("o-net-1", b"fn connect() {}".to_vec()),
        );

    let emit2 = emitter
        .emit_for_push("refs/heads/main", &tip2, &tree1, &tree2, "second commit")
        .unwrap()
        .unwrap();
    // 3 changes: main.rs modified, net.rs added, README.md deleted. lib.rs UNCHANGED → no emit.
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

    // Classify the three by EVENT TYPE — which is the only signal Search dispatches on. A delete
    // is a `git.blob.removed` removal verb; an upsert is a `git.blob.snapshot`. (A snapshot carrying
    // a payload `op = "delete"` is NOT a tombstone: Search never reads that field, so such an event
    // falls through to the upsert path and the stale doc survives.)
    let mut deletes = 0;
    let mut upserts = 0;
    for id in &emit2.emitted {
        let env = outbox.row(id).unwrap().envelope;
        match env.type_.0.as_str() {
            GIT_BLOB_REMOVED => {
                deletes += 1;
                // The subject is the CANONICAL percent-encoded ArtifactRef (`.` → `%2E`, `/` → `%2F`
                // inside a component), not the legacy raw slash-delimited id.
                assert!(
                    env.subject.0.ends_with("README%2Emd"),
                    "the deleted file is the tombstone: {}",
                    env.subject.0
                );
                assert_eq!(env.payload["reason"], serde_json::json!("deleted"));
                assert_eq!(
                    env.payload["ref"], env.subject.0,
                    "the tombstone names the doc id Search removes"
                );
            }
            GIT_BLOB_SNAPSHOT => {
                upserts += 1;
                assert_eq!(env.payload["op"], serde_json::json!("upsert"));
            }
            other => panic!("unexpected projection event type {other}"),
        }
    }
    assert_eq!(
        deletes, 1,
        "the deleted blob is a REAL removal tombstone (Gone is never silently dropped)"
    );
    assert_eq!(upserts, 2, "the modified + added blobs are upserts");

    // ── Push 3: a no-op-on-content push (same tree) emits nothing. ──
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
    // The cursor still advances to the new tip (the ref moved; the index is up to date).
    assert_eq!(
        cursor.last_indexed("core", "refs/heads/main").as_deref(),
        Some("commit-3")
    );

    // The total outbox: 3 git.ref.updated (one per push) + (3 + 3 + 0) projection events = 9.
    assert_eq!(
        outbox.committed_count(),
        9,
        "3 ref events + 6 projection events"
    );
    // Every projection emit over pushes 1+2 is one of the two NAMED projection tokens: 5 upsert
    // snapshots (3 first-index + 2 changed) and 1 removal tombstone (the deleted README).
    let all: Vec<String> = emit1
        .emitted
        .iter()
        .chain(emit2.emitted.iter())
        .map(|id| outbox.row(id).unwrap().envelope.type_.0)
        .collect();
    assert_eq!(
        all.iter().filter(|t| *t == GIT_BLOB_SNAPSHOT).count(),
        5,
        "5 upsert snapshots over pushes 1+2"
    );
    assert_eq!(
        all.iter().filter(|t| *t == GIT_BLOB_REMOVED).count(),
        1,
        "1 removal tombstone (the deleted README)"
    );
    // The receive-pack ref-event token is the NAMED constant (the receive-pack suite asserts its emit).
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

    // Push to a feature branch (NOT the indexed ref).
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
        pusher: Pusher {
            pseudonym: "anon-1@acme.noreply".into(),
            is_agent: false,
        },
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
    // The ref event committed, but no blob snapshot.
    assert_eq!(
        outbox.committed_count(),
        1,
        "only the git.ref.updated, no projection"
    );
}
