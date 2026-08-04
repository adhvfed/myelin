#![cfg(feature = "integration")]

use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    resolve_bare_repo_path, verified_gvisor_git_rootfs, GitWireSpec, IdemToken, MeterTarget,
    ReserveHandle, ResourceLimits, RunTokenCredential, RunnerHooks, SandboxBackend,
};
use std::path::Path;
use std::process::Command;

fn runsc_bin() -> Option<String> {
    let bin = std::env::var("MYELIN_RUNSC_BIN").unwrap_or_else(|_| "runsc".to_string());
    if bin.contains('/') {
        return Path::new(&bin).exists().then_some(bin);
    }
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(':') {
        if Path::new(dir).join(&bin).exists() {
            return Some(bin);
        }
    }
    None
}

fn require_or_skip(test: &str) -> Option<String> {
    let bin = match runsc_bin() {
        Some(bin) => bin,
        None if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") => {
            panic!(
                "[{test}] MYELIN_REQUIRE_RUNSC=1 but runsc is absent - refusing a vacuous green."
            );
        }
        None => {
            eprintln!("[{test}] SKIPPED: runsc is absent.");
            return None;
        }
    };

    match verified_gvisor_git_rootfs() {
        Ok(_) => Some(bin),
        Err(error) if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") => {
            panic!(
                "[{test}] MYELIN_REQUIRE_RUNSC=1 but the pinned production git rootfs is \
                 unavailable: {error} - refusing a vacuous green."
            );
        }
        Err(error) => {
            eprintln!("[{test}] SKIPPED: pinned production git rootfs unavailable: {error}");
            None
        }
    }
}

fn run_git(args: &[&str], cwd: Option<&Path>) -> std::process::Output {
    let mut c = Command::new("git");
    c.args(args);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let out = c.output().expect("run host git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn parse_batch(mut buf: &[u8]) -> Vec<(String, String, Vec<u8>)> {
    let mut out = Vec::new();
    while !buf.is_empty() {
        let nl = match buf.iter().position(|&b| b == b'\n') {
            Some(i) => i,
            None => break,
        };
        let header = String::from_utf8_lossy(&buf[..nl]).to_string();
        buf = &buf[nl + 1..];
        let parts: Vec<&str> = header.split(' ').collect();
        if parts.len() < 3 {
            break;
        }
        let size: usize = parts[2].parse().unwrap_or(0);
        let payload = buf[..size].to_vec();
        buf = &buf[size..];
        if !buf.is_empty() && buf[0] == b'\n' {
            buf = &buf[1..];
        }
        out.push((parts[0].to_string(), parts[1].to_string(), payload));
    }
    out
}

fn ok_hooks() -> RunnerHooks {
    RunnerHooks::new(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(|_t| Ok(())),
        Box::new(|_s| Ok(())),
    )
}

#[test]
fn sandboxed_receive_pack_ingests_a_thin_pack_and_streams_a_validated_pack() {
    let Some(_bin) = require_or_skip("ct006d receive-pack ingest") else {
        return;
    };

    let root = std::env::temp_dir().join(format!("myelin-ct006d-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bare = resolve_bare_repo_path(&root, "acme", "fr-par", "widgets").unwrap();
    std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
    run_git(
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            &bare.to_string_lossy(),
        ],
        None,
    );

    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    run_git(&["init", "-q", "-b", "main"], Some(&work));
    run_git(&["config", "user.email", "t@t.t"], Some(&work));
    run_git(&["config", "user.name", "t"], Some(&work));
    std::fs::write(work.join("a.txt"), b"base content for the delta base\n").unwrap();
    run_git(&["add", "-A"], Some(&work));
    run_git(
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "c1"],
        Some(&work),
    );
    run_git(
        &["push", "-q", &bare.to_string_lossy(), "main"],
        Some(&work),
    );
    let base_oid = String::from_utf8_lossy(&run_git(&["rev-parse", "HEAD"], Some(&work)).stdout)
        .trim()
        .to_string();

    std::fs::write(
        work.join("a.txt"),
        b"base content for the delta base\nplus a second line\n",
    )
    .unwrap();
    run_git(&["add", "-A"], Some(&work));
    run_git(
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "c2"],
        Some(&work),
    );
    let new_oid = String::from_utf8_lossy(&run_git(&["rev-parse", "HEAD"], Some(&work)).stdout)
        .trim()
        .to_string();

    let pack = {
        let out = Command::new("git")
            .args([
                "-C",
                &work.to_string_lossy(),
                "pack-objects",
                "--stdout",
                "--thin",
                "--revs",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map(|mut ch| {
                use std::io::Write;
                let revs = format!("{new_oid}\n^{base_oid}\n");
                ch.stdin.take().unwrap().write_all(revs.as_bytes()).unwrap();
                ch.wait_with_output().unwrap()
            })
            .unwrap();
        assert!(
            out.status.success(),
            "pack-objects: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    };
    println!(
        "=== thin pack size = {} bytes (base={base_oid}, new={new_oid}) ===",
        pack.len()
    );
    assert!(
        pack.starts_with(b"PACK"),
        "the generated pack must start with PACK"
    );

    let spec = GitWireSpec::for_repo(
        &root,
        "acme",
        "fr-par",
        "widgets",
        Vec::new(),
        pack,
        Vec::new(),
        None,
        ResourceLimits {
            cpu_millis: 2000,
            mem_bytes: 512 << 20,
            disk_bytes: 512 << 20,
            tmpfs_bytes: 512 << 20,
            pids_max: 256,
            timeout_secs: 120,
        },
        RunTokenCredential::new("test-bearer", "rp-jti", 300).unwrap(),
        MeterTarget {
            reserve_id: "rp-res".into(),
        },
        IdemToken(format!("rp-{}", std::process::id())),
    )
    .expect("locator resolves");

    let backend = GvisorBackend::git_wire_only();
    let launch = backend
        .launch_git_receive_pack(&spec, &ok_hooks())
        .expect("ingest launch runs");
    let result = &launch.result;
    println!(
        "=== sandboxed index-pack: exit={:?} timed_out={} stdout={}B ===\nstderr={}",
        result.exit_code,
        result.timed_out,
        result.stdout.len(),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.exit_code, Some(0), "ingest must exit 0");

    let objs = parse_batch(&result.stdout);
    println!("=== streamed {} fully-resolved objects ===", objs.len());
    for (oid, ty, bytes) in &objs {
        println!("  {oid} {ty} {}B", bytes.len());
    }
    let commit = objs
        .iter()
        .find(|(oid, ty, _)| oid == &new_oid && ty == "commit");
    assert!(
        commit.is_some(),
        "the new commit {new_oid} must be in the streamed object set, fully resolved"
    );
    assert!(
        commit.unwrap().2.starts_with(b"tree "),
        "the streamed commit must be a fully-materialised object"
    );

    backend.kill(&launch.handle).expect("teardown");
    println!("=== CT-006d sandbox ingest PROVEN: thin pack → validated self-contained pack via /tmp quarantine ===");
    let _ = std::fs::remove_dir_all(&root);
}
