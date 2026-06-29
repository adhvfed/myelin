//! # GT-003 — the cross-system reconciler: crash-window recovery proof (builder ≠ verifier oracle).
//!
//! Drives the REAL durable push path ([`myelin_git::receive_pack::RefStore::open_durable`]) into the
//! apply-after-outbox-commit crash window ([`CrashPoint::AfterCommitBeforeApply`]) and proves the
//! reconciler ([`myelin_git::reconcile`]) recovers the on-disk ref to the committed `update_seq`,
//! idempotently, with `git fsck` clean afterward (the external oracle).

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp,
};
use myelin_git::core::RepoLoc;
use myelin_git::durable::{DurableGitRepo, DurableGitStore};
use myelin_git::reconcile::{reconcile_refs, refs_from_outbox};
use myelin_git::receive_pack::{
    CrashPoint, InMemoryObjectDb, Oid, ProposedRefUpdate, PushOutcome, PushSession, Pusher, RefName,
    RefStore,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn temp_root(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    p.push(format!("myelin-gt003-{tag}-{nanos}"));
    p
}

fn ctx_base(tenant: &str) -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId(tenant.into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-29T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-29T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:gt003".into())),
    }
}

fn seed_commit(repo: &DurableGitRepo, content: &[u8]) -> Oid {
    let blob = repo.write_blob(content).expect("blob");
    let tree = repo.write_tree(&[("file.txt", &blob)]).expect("tree");
    Oid::new(
        repo.write_commit(&tree, &[], "feat: seed", "psn@acme.noreply", "psn@acme.noreply")
            .expect("commit")
            .0,
    )
}

fn push_create(ref_name: &str, new: &Oid) -> PushSession {
    PushSession {
        updates: vec![ProposedRefUpdate {
            ref_name: RefName::new(ref_name),
            expected_old: Oid::zero(),
            new_oid: new.clone(),
            forced: false,
            commit_oids: vec![new.clone()],
        }],
        quarantine: vec![],
        pusher: Pusher {
            pseudonym: "psn@acme.noreply".into(),
            is_agent: false,
        },
    }
}

fn git_fsck(repo_path: &std::path::Path) -> (bool, String) {
    match Command::new("git")
        .arg("--git-dir")
        .arg(repo_path)
        .args(["fsck", "--full", "--strict"])
        .output()
    {
        Ok(out) => (
            out.status.success(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        ),
        Err(e) => (false, format!("git not runnable: {e}")),
    }
}

/// **THE CRASH-WINDOW PROOF.** A push is crashed AFTER the outbox committed but BEFORE the on-disk ref
/// CAS applied. The committed `git.ref.updated` is durable; the on-disk ref is BEHIND. The reconciler
/// replays the committed event and recovers the ref onto disk — then is idempotent on a re-run, and the
/// repo is `git fsck` clean.
#[test]
fn reconciler_recovers_the_apply_after_outbox_commit_window_idempotently_fsck_clean() {
    let root = temp_root("window");
    let loc = RepoLoc::new("acme", "fr-par", "core");

    let store = DurableGitStore::rooted(&root);
    let repo = Arc::new(store.create_repo(&loc).expect("create"));
    let c1 = seed_commit(&repo, b"hello reconciler\n");

    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let rs = RefStore::open_durable(repo.clone(), "core", ctx_base("acme"), outbox.clone(), minter);

    // Crash in the apply-after-outbox-commit window.
    let db = InMemoryObjectDb::new();
    let outcome = rs
        .receive(&push_create("refs/heads/main", &c1), &db, CrashPoint::AfterCommitBeforeApply)
        .expect("receive");
    assert!(
        matches!(outcome, PushOutcome::Crashed(c) if c.at == CrashPoint::AfterCommitBeforeApply),
        "the push crashed in the reconciler window: {outcome:?}"
    );
    // The event is the durable witness (committed); the on-disk ref is BEHIND (not yet applied).
    assert_eq!(outbox.committed_count(), 1, "git.ref.updated committed (durable witness)");
    assert_eq!(
        repo.read_ref("refs/heads/main").expect("read"),
        None,
        "the on-disk ref is behind its committed update_seq (the crash window)"
    );

    // Simulate restart: a FRESH durable store + repo handle over the same root, replay the committed
    // events through the reconciler.
    let store2 = DurableGitStore::rooted(&root);
    let repo2 = store2.open_repo(&loc).expect("open after restart");
    let records = refs_from_outbox(&outbox, Some("core"));
    assert_eq!(records.len(), 1);
    let report = reconcile_refs(&repo2, &records).expect("reconcile");
    assert_eq!(
        report.reapplied,
        vec![("refs/heads/main".to_string(), 1)],
        "the window was recovered"
    );
    assert_eq!(
        repo2.read_ref("refs/heads/main").expect("read").map(|o| o.0),
        Some(c1.0.clone()),
        "the committed ref move is now on disk (recovered to the committed update_seq)"
    );

    // Idempotent: a second reconcile re-applies nothing.
    let again = reconcile_refs(&repo2, &records).expect("reconcile again");
    assert!(!again.recovered_any(), "idempotent on update_seq");
    assert_eq!(again.already_current, 1);

    // External oracle: git fsck clean after the recovery.
    let (ok, err) = git_fsck(repo2.path());
    assert!(ok, "git fsck clean after reconcile: {err}");

    std::fs::remove_dir_all(&root).ok();
}
