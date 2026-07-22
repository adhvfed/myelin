//! # GT-001 durable on-disk git storage — the cross-cutting proofs (builder ≠ verifier oracle).
//!
//! These tests exercise the WHOLE durable path end to end: the durable store
//! ([`myelin_git::durable::DurableGitStore`]) + the reconciled durable ref store
//! ([`myelin_git::receive_pack::RefStore::open_durable`]), and prove the four properties the prompt's
//! VERIFY section pins:
//!   1. **Durability across restart (the core proof)** — a ref + object written through the durable
//!      `RefStore` are read back by a FRESH `RefStore` over the SAME on-disk root (a simulated
//!      process restart). A test that passed on the in-memory store would NOT survive the fresh
//!      instance — these hit the on-disk repo.
//!   2. **`git fsck` clean (the EXTERNAL oracle)** — the created bare repo is verified by the REAL
//!      canonical `git fsck --full` binary (not just an in-process check) → clean.
//!   3. **Ref CAS** — a stale expected-old push is rejected through the durable `RefStore` and the
//!      on-disk ref does not move.
//!   4. **Tenant isolation** — a repo under tenant A's path is not reachable via tenant B's resolver.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp,
};
use myelin_git::core::RepoLoc;
use myelin_git::durable::{DurableGitRepo, DurableGitStore};
use myelin_git::receive_pack::{
    CrashPoint, InMemoryObjectDb, Oid, ProposedRefUpdate, Pusher, PushOutcome, PushSession, RefName,
    RefStore,
    RejectReason,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn temp_root(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    p.push(format!("myelin-gt001-{tag}-{nanos}"));
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
        caused_by: Some(CausedBy("session:gt001".into())),
    }
}

/// Build a real, `fsck`-clean commit (blob → tree → commit) authored to `tenant`'s pseudonym.
fn seed_commit(repo: &DurableGitRepo, tenant: &str, content: &[u8]) -> Oid {
    let psn = format!("psn-7@{tenant}.noreply");
    let blob = repo.write_blob(content).expect("blob");
    let tree = repo.write_tree(&[("file.txt", &blob)]).expect("tree");
    let core_oid = repo
        .write_commit(&tree, &[], "feat: seed", &psn, &psn)
        .expect("commit");
    // The receive-pack `Oid` is this module's type; the durable backend speaks `core::Oid`. Convert.
    Oid::new(core_oid.0)
}

fn push_create(ref_name: &str, new: &Oid, tenant: &str) -> PushSession {
    PushSession {
        updates: vec![ProposedRefUpdate {
            ref_name: RefName::new(ref_name),
            expected_old: Oid::zero(),
            new_oid: new.clone(),
            forced: false,
            commit_oids: vec![new.clone()],
        }],
        // Empty quarantine: the objects are already in the on-disk odb (pre-written); the durable
        // ref simply points at them. (The object-byte durability is proven in the durable.rs unit
        // tests; here the focus is the durable REF surviving restart.)
        quarantine: vec![],
        pusher: Pusher {
            pseudonym: format!("psn-7@{tenant}.noreply"),
            is_agent: false,
        },
    }
}

/// **THE CORE PROOF — a ref + object written through the durable `RefStore` survive a FRESH
/// `RefStore` over the same on-disk root (a simulated restart).**
#[test]
fn ref_written_through_refstore_survives_a_fresh_refstore_over_the_same_root() {
    let root = temp_root("restart");
    let loc = RepoLoc::new("acme", "fr-par", "core");
    let commit;

    {
        // First "process": create the repo, open a durable RefStore, push a ref.
        let store = DurableGitStore::rooted(&root);
        let repo = Arc::new(store.create_repo(&loc).expect("create repo"));
        commit = seed_commit(&repo, "acme", b"durable through refstore\n");

        let outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let refstore = RefStore::open_durable(
            Arc::clone(&repo),
            "core",
            ctx_base("acme"),
            outbox.clone(),
            minter,
        );
        let db = InMemoryObjectDb::new();
        let outcome = refstore
            .receive(&push_create("refs/heads/main", &commit, "acme"), &db, CrashPoint::None)
            .expect("receive");
        assert!(
            matches!(outcome, PushOutcome::Accepted { .. }),
            "the durable push was accepted, got {outcome:?}"
        );
        assert_eq!(
            refstore.tip(&RefName::new("refs/heads/main")),
            Some(commit.clone())
        );
        // The git.ref.updated event committed durably to the outbox (the co-commit is unchanged).
        assert_eq!(outbox.committed_count(), 1);
    } // EVERYTHING drops — no in-memory state carries to the next "process".

    // Second "process" (the RESTART): a brand-new store + repo handle + RefStore over the same root.
    let store2 = DurableGitStore::rooted(&root);
    let repo2 = Arc::new(store2.open_repo(&loc).expect("open after restart"));
    let refstore2 = RefStore::open_durable(
        Arc::clone(&repo2),
        "core",
        ctx_base("acme"),
        OutboxStore::new(),
        Arc::new(MonotonicMinter::new()),
    );

    // The ref survived (SI-012 fixed: `open` loads the entry point from disk, not an empty map).
    assert_eq!(
        refstore2.tip(&RefName::new("refs/heads/main")),
        Some(commit.clone()),
        "the ref written by the first RefStore is read by a FRESH RefStore after restart"
    );
    // The object survived (F-git-2: the oid→object lookup is the real on-disk odb).
    assert!(
        repo2.has_object(&myelin_git::core::Oid::new(commit.0.clone())),
        "the commit object survived the restart in the on-disk odb"
    );
    // The reflog survived too (durable on-disk git reflog).
    let reflog = refstore2.reflog().expect("read durable reflog");
    assert_eq!(reflog.len(), 1, "the reflog entry survived the restart");
    assert_eq!(reflog[0].new_oid, commit);
    assert!(reflog[0].pusher_pseudonym.contains("acme.noreply"));

    std::fs::remove_dir_all(&root).ok();
}

/// **`git fsck` clean — the EXTERNAL oracle.** After the durable writes, the real canonical `git
/// fsck --full` binary verifies the on-disk bare repo is a valid git repository.
#[test]
fn created_bare_repo_is_git_fsck_clean_external_oracle() {
    let root = temp_root("fsck");
    let loc = RepoLoc::new("acme", "fr-par", "core");
    let store = DurableGitStore::rooted(&root);
    let repo = Arc::new(store.create_repo(&loc).expect("create"));
    let commit = seed_commit(&repo, "acme", b"fsck me through the oracle\n");

    let refstore = RefStore::open_durable(
        Arc::clone(&repo),
        "core",
        ctx_base("acme"),
        OutboxStore::new(),
        Arc::new(MonotonicMinter::new()),
    );
    refstore
        .receive(&push_create("refs/heads/main", &commit, "acme"), &InMemoryObjectDb::new(), CrashPoint::None)
        .expect("receive");

    // In-process integrity (the src slice) is clean.
    repo.fsck().expect("in-process fsck clean");

    // The EXTERNAL oracle: real `git fsck --full --strict` against the bare repo.
    let out = Command::new("git")
        .arg("--git-dir")
        .arg(repo.path())
        .arg("fsck")
        .arg("--full")
        .arg("--strict")
        .output()
        .expect("run git fsck");
    assert!(
        out.status.success(),
        "git fsck must exit clean. stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // fsck reports dangling/broken objects on stderr; a clean repo has no error lines.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("error") && !stderr.contains("missing") && !stderr.contains("broken"),
        "git fsck reported problems: {stderr}"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// **Ref CAS through the durable `RefStore`: a stale expected-old is rejected; the on-disk ref does
/// not move.**
#[test]
fn durable_refstore_rejects_stale_cas_and_ref_does_not_move() {
    let root = temp_root("cas");
    let loc = RepoLoc::new("acme", "fr-par", "core");
    let store = DurableGitStore::rooted(&root);
    let repo = Arc::new(store.create_repo(&loc).expect("create"));
    let c1 = seed_commit(&repo, "acme", b"v1\n");
    let c2 = seed_commit(&repo, "acme", b"v2 - a different object\n");

    let refstore = RefStore::open_durable(
        Arc::clone(&repo),
        "core",
        ctx_base("acme"),
        OutboxStore::new(),
        Arc::new(MonotonicMinter::new()),
    );
    let db = InMemoryObjectDb::new();

    // Create refs/heads/feature at c1.
    assert!(matches!(
        refstore
            .receive(&push_create("refs/heads/feature", &c1, "acme"), &db, CrashPoint::None)
            .unwrap(),
        PushOutcome::Accepted { .. }
    ));

    // A second push believes feature is still absent (stale expected-old = zero) → non-fast-forward.
    let stale = refstore
        .receive(&push_create("refs/heads/feature", &c2, "acme"), &db, CrashPoint::None)
        .unwrap();
    assert!(
        matches!(stale, PushOutcome::Rejected(RejectReason::NonFastForward { .. })),
        "a stale CAS is rejected, got {stale:?}"
    );
    // The on-disk ref still points at c1 (the rejected push moved nothing).
    assert_eq!(
        refstore.tip(&RefName::new("refs/heads/feature")),
        Some(c1.clone())
    );
    // And a fresh handle over the same root confirms it on disk (durable).
    let repo2 = store.open_repo(&loc).unwrap();
    assert_eq!(
        repo2.read_ref("refs/heads/feature").unwrap(),
        Some(myelin_git::core::Oid::new(c1.0))
    );

    std::fs::remove_dir_all(&root).ok();
}

/// **Tenant isolation: a repo under tenant A's path is not reachable via tenant B's resolver path.**
#[test]
fn tenant_isolation_through_the_durable_store() {
    let root = temp_root("isolation");
    let store = DurableGitStore::rooted(&root);
    let a = RepoLoc::new("tenant-a", "fr-par", "secret");
    let b = RepoLoc::new("tenant-b", "fr-par", "secret");

    let repo_a = Arc::new(store.create_repo(&a).expect("create a"));
    let commit = seed_commit(&repo_a, "tenant-a", b"tenant a private\n");
    let refstore_a = RefStore::open_durable(
        Arc::clone(&repo_a),
        "secret",
        ctx_base("tenant-a"),
        OutboxStore::new(),
        Arc::new(MonotonicMinter::new()),
    );
    refstore_a
        .receive(&push_create("refs/heads/main", &commit, "tenant-a"), &InMemoryObjectDb::new(), CrashPoint::None)
        .expect("receive a");

    // Different tenants resolve to different on-disk paths; B's repo does not even exist.
    assert_ne!(store.repo_path(&a).unwrap(), store.repo_path(&b).unwrap());
    assert!(store.repo_exists(&a));
    assert!(!store.repo_exists(&b), "tenant B cannot reach A's repo by path");

    // B opens its OWN repo + RefStore — A's ref/object are not visible (path isolation).
    let repo_b = Arc::new(store.create_repo(&b).expect("create b"));
    let refstore_b = RefStore::open_durable(
        Arc::clone(&repo_b),
        "secret",
        ctx_base("tenant-b"),
        OutboxStore::new(),
        Arc::new(MonotonicMinter::new()),
    );
    assert_eq!(
        refstore_b.tip(&RefName::new("refs/heads/main")),
        None,
        "tenant B's main is empty — A's ref did not bleed across the tenant path"
    );
    assert!(
        !repo_b.has_object(&myelin_git::core::Oid::new(commit.0)),
        "tenant A's object is NOT in tenant B's on-disk odb"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// **SECURITY REGRESSION (REJECT-level hole): a crafted `..` repo slug must NOT break out of the
/// tenant boundary — on BOTH the WRITE path (`DurableGitStore`) and the READ path (`GixCore`).**
///
/// Vector: tenant A has a real repo with a secret commit. Tenant B (a legit authenticated tenant with
/// its own `root/tenant-b/fr-par/` dir) crafts `repo = "../../tenant-a/fr-par/secret"`, which a raw
/// `join` would collapse onto A's repo. The resolver must REFUSE it fail-closed — B creates nothing,
/// reads nothing of A's, and A's repo is untouched.
#[test]
fn path_traversal_cross_tenant_breakout_is_rejected_on_read_and_write() {
    use myelin_git::core::{Oid as CoreOid, ReadBackend};
    use myelin_git::gix_backend::{GixCore, RootedResolver};

    let root = temp_root("traversal");
    let store = DurableGitStore::rooted(&root);

    // Tenant A: a real repo + a secret commit, ref'd.
    let a = RepoLoc::new("tenant-a", "fr-par", "secret");
    let repo_a = Arc::new(store.create_repo(&a).expect("create a"));
    let secret = seed_commit(&repo_a, "tenant-a", b"TOP SECRET tenant-a payload\n");
    let refstore_a = RefStore::open_durable(
        Arc::clone(&repo_a),
        "secret",
        ctx_base("tenant-a"),
        OutboxStore::new(),
        Arc::new(MonotonicMinter::new()),
    );
    refstore_a
        .receive(&push_create("refs/heads/main", &secret, "tenant-a"), &InMemoryObjectDb::new(), CrashPoint::None)
        .expect("receive a");

    // Tenant B is a legit tenant with its own dir (so the `..` target is reachable on disk).
    let b_legit = RepoLoc::new("tenant-b", "fr-par", "mine");
    store.create_repo(&b_legit).expect("create b's own repo");

    // The ATTACK locator: tenant B, crafted traversing slug aiming at A's repo.
    let attack = RepoLoc::new("tenant-b", "fr-par", "../../tenant-a/fr-par/secret");

    // ---- WRITE path: create_repo / open_repo / repo_exists / repo_path all fail-closed. ----
    assert!(
        store.create_repo(&attack).is_err(),
        "create_repo must REFUSE a traversing slug (no cross-tenant write)"
    );
    assert!(store.open_repo(&attack).is_err(), "open_repo must REFUSE a traversing slug");
    assert!(
        !store.repo_exists(&attack),
        "repo_exists must be false (fail-closed) for a traversing slug"
    );
    assert!(
        store.repo_path(&attack).is_err(),
        "repo_path must refuse to resolve a traversing slug"
    );

    // A's ref is untouched by the breakout attempt (the attacker never obtained a handle).
    assert_eq!(
        repo_a.read_ref("refs/heads/main").unwrap(),
        Some(CoreOid::new(secret.0.clone())),
        "tenant A's ref is untouched by the breakout attempt"
    );

    // ---- READ path: GixCore (the read backend) shares the SAME resolver → also refuses. ----
    let reader = GixCore::new(RootedResolver::new(&root));
    let read_attempt =
        reader.read_blob_bounded(&attack, &CoreOid::new(secret.0.clone()), 1024);
    assert!(
        read_attempt.is_err(),
        "GixCore read through a traversing slug must be refused (read path closed), got {read_attempt:?}"
    );

    // Nothing was smuggled outside the per-tenant tree: only the two legit repos exist.
    assert!(root.join("tenant-a/fr-par/secret.git").is_dir());
    assert!(root.join("tenant-b/fr-par/mine.git").is_dir());
    let b_dir = root.join("tenant-b/fr-par");
    let entries: Vec<String> = std::fs::read_dir(&b_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries, vec!["mine.git".to_string()], "no smuggled dir under tenant-b");

    std::fs::remove_dir_all(&root).ok();
}

/// **SECURITY REGRESSION: an absolute-component locator must not write outside the store root.**
#[test]
fn absolute_component_locator_is_rejected_on_write() {
    let root = temp_root("abs");
    let store = DurableGitStore::rooted(&root);
    // An absolute tenant would (with a raw join) discard the root and materialise host-wide.
    let attack = RepoLoc::new("/tmp/evil-myelin-escape", "fr-par", "core");
    assert!(store.create_repo(&attack).is_err(), "absolute tenant refused");
    assert!(store.repo_path(&attack).is_err(), "absolute tenant not resolved");
    assert!(
        !std::path::Path::new("/tmp/evil-myelin-escape").exists(),
        "nothing was created outside the store root"
    );
    std::fs::remove_dir_all(&root).ok();
}
