use std::fs;
use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("lint crate must live under workspace/crates")
        .to_path_buf()
}

#[test]
fn edge_shutdown_reaches_active_git_wire_containers() {
    let root = workspace();
    let main = fs::read_to_string(root.join("crates/myelin-edge/src/main.rs")).expect("edge main");
    let durable = fs::read_to_string(root.join("crates/myelin-edge/src/git_durable.rs"))
        .expect("durable Git backend");
    let executor = fs::read_to_string(root.join("crates/myelin-edge/src/git_wire_exec.rs"))
        .expect("Git wire executor");
    let gvisor =
        fs::read_to_string(root.join("crates/myelin-ci-sandbox/src/gvisor/output_capture.rs"))
            .expect("gVisor output capture");

    for required in [
        "git_shutdown_for_signal.store(true, Ordering::Release)",
        ".with_git_shutdown_signal(git_shutdown.clone())",
    ] {
        assert!(
            main.contains(required),
            "edge does not publish shutdown: {required}"
        );
    }
    assert!(
        durable.contains("production_git_core_with_shutdown_and_issuer"),
        "durable Git serving must propagate the shared shutdown flag and credential issuer"
    );
    for required in [
        "launch_git_wire_until_cancelled",
        "launch_git_receive_pack_until_cancelled",
        "self.shutdown.load(Ordering::Acquire)",
    ] {
        assert!(
            executor.contains(required),
            "executor cancellation drift: {required}"
        );
    }
    for required in [
        "let cancelled = cancellation.load(Ordering::Acquire)",
        "if cancelled || executed_at.elapsed() >= timeout",
        "timed_out = !cancelled",
    ] {
        assert!(
            gvisor.contains(required),
            "runsc cancellation drift: {required}"
        );
    }
}
