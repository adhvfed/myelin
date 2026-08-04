use std::path::PathBuf;
use std::process::Command;

use myelin_git::backup::{
    restore_repo, restore_repo_from_file, GitBackupError, GitRepoBackup, VerifiedGitRepoBackupFile,
};
use myelin_git::core::{Oid, RepoLoc};
use myelin_git::durable::{DurableGitRepo, DurableGitStore};

fn temp_root(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    p.push(format!("myelin-gt002-{tag}-{nanos}"));
    p
}

fn seed_history(repo: &DurableGitRepo, tenant: &str) -> Vec<(String, Oid)> {
    let psn = format!("psn-7@{tenant}.noreply");
    let b1 = repo.write_blob(b"line one\n").unwrap();
    let t1 = repo.write_tree(&[("file.txt", &b1)]).unwrap();
    let c1 = repo.write_commit(&t1, &[], "c1: seed", &psn, &psn).unwrap();
    let b2 = repo.write_blob(b"line one\nline two\n").unwrap();
    let t2 = repo.write_tree(&[("file.txt", &b2)]).unwrap();
    let c2 = repo
        .write_commit(&t2, &[&c1], "c2: extend", &psn, &psn)
        .unwrap();

    repo.update_ref_cas("refs/heads/main", None, Some(&c2), "create main", &psn)
        .unwrap();
    repo.update_ref_cas(
        "refs/heads/feature",
        None,
        Some(&c1),
        "create feature",
        &psn,
    )
    .unwrap();

    let git = git2::Repository::open(repo.path()).unwrap();
    let sig = git2::Signature::now(&psn, &psn).unwrap();
    let target = git
        .find_object(git2::Oid::from_str(c1.as_str()).unwrap(), None)
        .unwrap();
    let tag_oid = git
        .tag("v1.0", &target, &sig, "release v1.0", false)
        .unwrap();

    let mut want = vec![
        ("refs/heads/feature".to_string(), c1),
        ("refs/heads/main".to_string(), c2),
        ("refs/tags/v1.0".to_string(), Oid::new(tag_oid.to_string())),
    ];
    want.sort();
    want
}

fn assert_git_fsck_clean(repo_path: &std::path::Path) {
    let out = Command::new("git")
        .arg("--git-dir")
        .arg(repo_path)
        .arg("fsck")
        .arg("--full")
        .arg("--strict")
        .output()
        .expect("run git fsck");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "git fsck must exit clean on the RESTORED repo. stdout={stdout} stderr={stderr}"
    );
    assert!(
        !stderr.contains("error") && !stderr.contains("missing") && !stderr.contains("broken"),
        "git fsck reported problems on the restored repo: {stderr}"
    );
}

#[test]
fn destructive_round_trip_into_a_clean_target_is_git_fsck_clean() {
    let loc = RepoLoc::new("acme", "fr-par", "core");

    let src_root = temp_root("src");
    let src_store = DurableGitStore::rooted(&src_root);
    let src_repo = src_store.create_repo(&loc).expect("create source");
    let want = seed_history(&src_repo, "acme");

    let mut want_obj_bytes: Vec<(Oid, Vec<u8>)> = Vec::new();
    {
        let git = git2::Repository::open(src_repo.path()).unwrap();
        let odb = git.odb().unwrap();
        odb.foreach(|oid| {
            let o = Oid::new(oid.to_string());
            want_obj_bytes.push((
                o.clone(),
                src_repo.read_object_bounded(&o, 64 * 1024 * 1024).unwrap(),
            ));
            true
        })
        .unwrap();
    }
    assert!(
        want_obj_bytes.len() >= 6,
        "expected ≥6 objects (2 commits, 2 trees, 2 blobs, 1 tag), got {}",
        want_obj_bytes.len()
    );

    let artifact_dir = temp_root("artifact");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    let artifact_file = artifact_dir.join("acme-core.gitbackup");
    let backup = GitRepoBackup::create_to_file(&src_repo, &artifact_file).expect("backup to file");
    assert_eq!(backup.refs(), want.as_slice());
    assert!(
        backup.pack_len() > 0,
        "a REAL packfile (repo bytes), not a modeled offset"
    );
    drop(backup);

    std::fs::remove_dir_all(&src_root).expect("delete the source store");
    assert!(
        !src_root.exists(),
        "the original repo is unavailable for the restore"
    );

    let dst_root = temp_root("dst");
    let dst_store = DurableGitStore::rooted(&dst_root);
    assert!(
        !dst_store.repo_exists(&loc),
        "the target starts clean/empty"
    );

    let mut reloaded =
        VerifiedGitRepoBackupFile::open(&artifact_file).expect("verify artifact alone");
    let restored = restore_repo_from_file(&dst_store, &loc, &mut reloaded)
        .expect("stream restore onto clean target");

    assert_eq!(
        restored
            .list_refs_bounded(1_000_000)
            .expect("list restored refs"),
        want,
        "every ref restored identical (name → oid)"
    );
    for (oid, bytes) in &want_obj_bytes {
        assert!(
            restored.has_object(oid),
            "object {} present in the restored odb",
            oid.as_str()
        );
        assert_eq!(
            &restored
                .read_object_bounded(oid, 64 * 1024 * 1024)
                .expect("read restored object"),
            bytes,
            "object {} bytes identical after restore",
            oid.as_str()
        );
    }

    restored.fsck().expect("in-process fsck clean");
    assert_git_fsck_clean(restored.path());

    std::fs::remove_dir_all(&dst_root).ok();
    std::fs::remove_dir_all(&artifact_dir).ok();
}

#[test]
fn restore_refuses_to_clobber_a_live_repo() {
    let loc = RepoLoc::new("acme", "fr-par", "core");
    let root = temp_root("noclobber");
    let store = DurableGitStore::rooted(&root);
    let repo = store.create_repo(&loc).unwrap();
    seed_history(&repo, "acme");
    let backup = GitRepoBackup::create(&repo).unwrap();

    let err = restore_repo(&store, &loc, &backup)
        .expect_err("restoring over a live repo must be refused");
    assert!(
        matches!(err, GitBackupError::TargetNotClean(_)),
        "got {err:?}"
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_failed_restore_does_not_poison_the_target() {
    let loc = RepoLoc::new("acme", "fr-par", "core");
    let src_root = temp_root("retry-src");
    let src_store = DurableGitStore::rooted(&src_root);
    let src_repo = src_store.create_repo(&loc).unwrap();
    let want = seed_history(&src_repo, "acme");
    let artifact_root = temp_root("retry-artifacts");
    std::fs::create_dir_all(&artifact_root).unwrap();
    let good_file = artifact_root.join("good.gitbackup");
    GitRepoBackup::create_to_file(&src_repo, &good_file).unwrap();

    let mut bytes = std::fs::read(&good_file).unwrap();
    let frame_len = bytes.len() - blake3::OUT_LEN;
    bytes[frame_len - 1] ^= 0xFF;
    let checksum = blake3::hash(&bytes[..frame_len]);
    bytes[frame_len..].copy_from_slice(checksum.as_bytes());
    let corrupt_file = artifact_root.join("corrupt.gitbackup");
    std::fs::write(&corrupt_file, bytes).unwrap();
    let mut corrupt =
        VerifiedGitRepoBackupFile::open(&corrupt_file).expect("outer frame valid; pack corrupt");

    let dst_root = temp_root("retry-dst");
    let dst_store = DurableGitStore::rooted(&dst_root);
    let final_path = dst_store.repo_path(&loc).unwrap();
    assert!(!dst_store.repo_exists(&loc), "target starts clean");

    let err = restore_repo_from_file(&dst_store, &loc, &mut corrupt)
        .expect_err("a corrupt pack must fail the restore");
    assert!(
        !matches!(err, GitBackupError::TargetNotClean(_)),
        "should fail at ingest, got {err:?}"
    );

    assert!(
        !final_path.exists(),
        "a failed restore left a repo at the target {} - it poisoned the location",
        final_path.display()
    );
    assert!(
        !dst_store.repo_exists(&loc),
        "a failed restore must leave the target CLEAN for an immediate retry"
    );
    let tenant_region_dir = dst_root.join("acme/fr-par");
    if tenant_region_dir.exists() {
        let leftovers: Vec<String> = std::fs::read_dir(&tenant_region_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            leftovers.is_empty(),
            "the failed restore left orphan dirs: {leftovers:?}"
        );
    }

    let mut good = VerifiedGitRepoBackupFile::open(&good_file).unwrap();
    let restored = restore_repo_from_file(&dst_store, &loc, &mut good)
        .expect("retry with a good artifact must succeed on the un-poisoned target");
    assert_eq!(
        restored.list_refs_bounded(1_000_000).unwrap(),
        want,
        "the retry restored every ref"
    );
    assert_git_fsck_clean(restored.path());

    std::fs::remove_dir_all(&src_root).ok();
    std::fs::remove_dir_all(&dst_root).ok();
    std::fs::remove_dir_all(&artifact_root).ok();
}

#[test]
fn path_replacement_after_verified_open_cannot_swap_the_artifact() {
    let loc = RepoLoc::new("acme", "fr-par", "core");
    let src_root = temp_root("held-handle-src");
    let src_store = DurableGitStore::rooted(&src_root);
    let src_repo = src_store.create_repo(&loc).unwrap();
    let want = seed_history(&src_repo, "acme");

    let artifact_root = temp_root("held-handle-artifacts");
    std::fs::create_dir_all(&artifact_root).unwrap();
    let artifact_file = artifact_root.join("backup.gitbackup");
    let mut verified = GitRepoBackup::create_to_file(&src_repo, &artifact_file).unwrap();
    let replacement = artifact_root.join("replacement");
    std::fs::write(&replacement, b"not a backup").unwrap();
    std::fs::rename(&replacement, &artifact_file).unwrap();

    let dst_root = temp_root("held-handle-dst");
    let dst_store = DurableGitStore::rooted(&dst_root);
    let restored = restore_repo_from_file(&dst_store, &loc, &mut verified)
        .expect("restore reads the verified open handle, not the replaced path");
    assert_eq!(restored.list_refs_bounded(1_000_000).unwrap(), want);
    assert_git_fsck_clean(restored.path());

    std::fs::remove_dir_all(&src_root).ok();
    std::fs::remove_dir_all(&dst_root).ok();
    std::fs::remove_dir_all(&artifact_root).ok();
}

#[test]
fn multiple_repos_restore_independently_and_tenant_scoped() {
    let a = RepoLoc::new("tenant-a", "fr-par", "secret");
    let b = RepoLoc::new("tenant-b", "eu-west", "secret");

    let src_root = temp_root("multi-src");
    let src_store = DurableGitStore::rooted(&src_root);
    let repo_a = src_store.create_repo(&a).unwrap();
    let repo_b = src_store.create_repo(&b).unwrap();
    let want_a = seed_history(&repo_a, "tenant-a");
    let want_b = seed_history(&repo_b, "tenant-b");
    let a_main_tip = want_a
        .iter()
        .find(|(n, _)| n == "refs/heads/main")
        .map(|(_, o)| o.clone())
        .unwrap();

    let backup_a = GitRepoBackup::create(&repo_a).unwrap();
    let backup_b = GitRepoBackup::create(&repo_b).unwrap();

    let dst_root = temp_root("multi-dst");
    let dst_store = DurableGitStore::rooted(&dst_root);
    let restored_a = restore_repo(&dst_store, &a, &backup_a).expect("restore a");
    let restored_b = restore_repo(&dst_store, &b, &backup_b).expect("restore b");

    assert_eq!(restored_a.list_refs_bounded(1_000_000).unwrap(), want_a);
    assert_eq!(restored_b.list_refs_bounded(1_000_000).unwrap(), want_b);

    assert_eq!(
        dst_store.repo_path(&a).unwrap(),
        dst_root.join("tenant-a/fr-par/secret.git")
    );
    assert_eq!(
        dst_store.repo_path(&b).unwrap(),
        dst_root.join("tenant-b/eu-west/secret.git")
    );

    assert!(
        restored_a.has_object(&a_main_tip),
        "A's object is in A's restored repo"
    );
    assert!(
        !restored_b.has_object(&a_main_tip),
        "tenant A's object must NOT leak into tenant B's restored repo"
    );

    assert_git_fsck_clean(restored_a.path());
    assert_git_fsck_clean(restored_b.path());

    std::fs::remove_dir_all(&src_root).ok();
    std::fs::remove_dir_all(&dst_root).ok();
}

#[test]
fn restore_through_a_traversing_locator_is_refused() {
    let root = temp_root("traversal");
    let store = DurableGitStore::rooted(&root);
    let legit = RepoLoc::new("tenant-a", "fr-par", "ok");
    let repo = store.create_repo(&legit).unwrap();
    seed_history(&repo, "tenant-a");
    let backup = GitRepoBackup::create(&repo).unwrap();

    let attack = RepoLoc::new("tenant-b", "fr-par", "../../tenant-a/fr-par/ok");
    let err = restore_repo(&store, &attack, &backup)
        .expect_err("a traversing restore locator must be refused");
    assert!(matches!(err, GitBackupError::Durable(_)), "got {err:?}");

    std::fs::remove_dir_all(&root).ok();
}
