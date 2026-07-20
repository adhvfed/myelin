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
fn edge_overload_and_readiness_failures_keep_retry_guidance() {
    let server = fs::read_to_string(workspace().join("crates/myelin-edge/src/server.rs"))
        .expect("edge server");
    for required in [
        "const GIT_WIRE_RETRY_AFTER_SECONDS: &str",
        "const READINESS_RETRY_AFTER_SECONDS: &str",
        "hyper::header::RETRY_AFTER",
        "GIT_WIRE_RETRY_AFTER_SECONDS",
        "READINESS_RETRY_AFTER_SECONDS",
    ] {
        assert!(server.contains(required), "retry guidance drift: {required}");
    }
}
