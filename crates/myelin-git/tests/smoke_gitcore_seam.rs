use myelin_git::core::{
    Backend, GitCore, GitCoreError, GitOp, ReadOp, RepoLoc, RoutedGitCore, Service, WireExecutor,
    WireInvocation, WireOutput,
};
use myelin_git::gix_backend::{GixCore, RepoPathResolver};
use std::path::PathBuf;
use std::process::Command;

struct LocalGitExecutor {
    cwd: PathBuf,
}

impl WireExecutor for LocalGitExecutor {
    fn run(&self, inv: &WireInvocation) -> Result<WireOutput, GitCoreError> {
        let mut argv = inv.argv.clone();
        argv.push(".".to_string());
        let out = Command::new("git")
            .args(&argv)
            .current_dir(&self.cwd)
            .output()
            .map_err(|e| GitCoreError::Wire(format!("spawn git: {e}")))?;
        Ok(WireOutput {
            stdout: out.stdout,
            status: out.status.code().unwrap_or(-1),
        })
    }
}

struct FixedResolver(PathBuf);
impl RepoPathResolver for FixedResolver {
    fn repo_path(&self, _repo: &RepoLoc) -> Result<PathBuf, GitCoreError> {
        Ok(self.0.clone())
    }
}

fn git(cwd: &std::path::Path, args: &[&str]) {
    let st = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        st.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&st.stderr)
    );
}

fn git_stdout(cwd: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn gitcore_seam_clones_via_canonical_wire_then_diffs_and_blames_via_gix() {
    let base = std::env::temp_dir().join(format!(
        "myelin-gitcore-smoke-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).unwrap();

    let work = base.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    std::fs::write(work.join("file.txt"), "alpha\nbeta\ngamma\n").unwrap();
    git(&work, &["add", "file.txt"]);
    git(&work, &["commit", "-q", "-m", "c1"]);
    let c1 = git_stdout(&work, &["rev-parse", "HEAD"]);
    std::fs::write(work.join("file.txt"), "alpha\nBETA-changed\ngamma\ndelta\n").unwrap();
    git(&work, &["add", "file.txt"]);
    git(&work, &["commit", "-q", "-m", "c2"]);
    let c2 = git_stdout(&work, &["rev-parse", "HEAD"]);

    let blob1 = git_stdout(&work, &["rev-parse", &format!("{c1}:file.txt")]);
    let blob2 = git_stdout(&work, &["rev-parse", &format!("{c2}:file.txt")]);

    let bare = base.join("origin.git");
    git(
        &base,
        &[
            "clone",
            "-q",
            "--bare",
            work.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );

    let wire_exec = LocalGitExecutor { cwd: bare.clone() };
    let read = GixCore::new(FixedResolver(bare.clone()));
    let core = RoutedGitCore::new(wire_exec, read);

    let repo = RepoLoc::new("acme", "fr-par", "widgets");

    assert_eq!(
        core.route(GitOp::AdvertiseRefs(Service::UploadPack)),
        Backend::Shell,
        "wire op routes to the canonical-git backend"
    );
    let adv = core
        .advertise_refs(&repo, Service::UploadPack)
        .expect("ref advertisement runs through the wire executor");
    assert_eq!(adv.status, 0, "canonical git advertise-refs succeeded");
    let adv_text = String::from_utf8_lossy(&adv.stdout);
    assert!(
        adv_text.contains(&c2),
        "the wire advertisement carries the tip commit {c2}; got: {adv_text}"
    );

    let clone_dst = base.join("clone");
    git(
        &base,
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            clone_dst.to_str().unwrap(),
        ],
    );
    assert!(
        clone_dst.join("file.txt").exists(),
        "clone delivered the tree"
    );

    assert_eq!(
        core.route(GitOp::Read(ReadOp::Diff)),
        Backend::Gix,
        "read op routes to the in-process backend"
    );

    let diff = core
        .diff_blobs_bounded(
            &repo,
            &myelin_git::core::Oid::new(&blob1),
            &myelin_git::core::Oid::new(&blob2),
            1024,
            100,
            8192,
        )
        .expect("in-process diff succeeds");
    let added: Vec<_> = diff.iter().filter(|l| l.origin == '+').collect();
    let removed: Vec<_> = diff.iter().filter(|l| l.origin == '-').collect();
    assert!(
        added.iter().any(|l| l.content == "BETA-changed")
            && added.iter().any(|l| l.content == "delta"),
        "diff shows the added lines; got {diff:?}"
    );
    assert!(
        removed.iter().any(|l| l.content == "beta"),
        "diff shows the removed line; got {diff:?}"
    );

    let blame = core
        .blame_bounded(
            &repo,
            "file.txt",
            &myelin_git::core::Oid::new(&c2),
            128,
            1024,
            100,
        )
        .expect("in-process blame succeeds");
    assert!(!blame.is_empty(), "blame produced hunks");
    let total_lines: usize = blame.iter().map(|h| h.lines).sum();
    assert_eq!(total_lines, 4, "blame covers all 4 lines of file.txt at c2");
    let attributed: std::collections::BTreeSet<_> = blame
        .iter()
        .map(|h| h.commit.as_str().to_string())
        .collect();
    assert!(
        attributed.contains(&c1) && attributed.contains(&c2),
        "blame attributes lines to BOTH commits (c1 unchanged, c2 changed); got {attributed:?}"
    );
    let c2_oid = myelin_git::core::Oid::new(&c2);
    assert!(
        core.blame_bounded(&repo, "file.txt", &c2_oid, 7, 1024, 100)
            .is_err(),
        "path cap plus one is rejected"
    );
    assert!(
        core.blame_bounded(&repo, "file.txt", &c2_oid, 128, 1, 100)
            .is_err(),
        "blob cap plus one is rejected from the target tree"
    );
    assert!(
        core.blame_bounded(&repo, "file.txt", &c2_oid, 128, 1024, 0)
            .is_err(),
        "hunk cap plus one is rejected"
    );

    let bytes = core
        .read_blob_bounded(&repo, &myelin_git::core::Oid::new(&blob2), 1024)
        .expect("in-process read_blob");
    assert_eq!(bytes, b"alpha\nBETA-changed\ngamma\ndelta\n");

    let _ = std::fs::remove_dir_all(&base);
}
