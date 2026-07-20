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
fn edge_transport_keeps_slowloris_connection_and_body_memory_bounds() {
    let server = fs::read_to_string(workspace().join("crates/myelin-edge/src/server.rs"))
        .expect("read edge server source");

    for required in [
        "const MAX_CONNECTIONS: usize",
        "const MAX_CONCURRENT_GIT_PUSHES: usize",
        "const MAX_REQUEST_HEADERS: usize",
        "const MAX_HTTP_BUFFER_BYTES: usize",
        "const HEADER_READ_TIMEOUT: Duration",
        "const API_BODY_READ_TIMEOUT: Duration",
        "const GIT_PUSH_BODY_READ_TIMEOUT: Duration",
        "Semaphore::new(MAX_CONNECTIONS)",
        "Semaphore::new(MAX_CONCURRENT_GIT_PUSHES)",
        ".header_read_timeout(HEADER_READ_TIMEOUT)",
        ".max_headers(MAX_REQUEST_HEADERS)",
        ".max_buf_size(MAX_HTTP_BUFFER_BYTES)",
        "path.ends_with(\"/git-receive-pack\")",
        "MAX_JSON_REQUEST_BODY_BYTES",
        "tokio::time::timeout(deadline",
        "BoundedCollectError::TimedOut",
    ] {
        assert!(
            server.contains(required),
            "missing edge transport bound: {required}"
        );
    }
}
