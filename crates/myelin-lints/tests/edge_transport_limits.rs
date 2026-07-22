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
        "const MAX_CONCURRENT_GIT_WIRE_OPERATIONS: usize",
        "const MAX_CONCURRENT_GATEWAY_DISPATCHES: usize",
        "const MAX_REQUEST_HEADERS: usize",
        "const MAX_HTTP_BUFFER_BYTES: usize",
        "const HEADER_READ_TIMEOUT: Duration",
        "const API_BODY_READ_TIMEOUT: Duration",
        "const GIT_PUSH_BODY_READ_TIMEOUT: Duration",
        "Semaphore::new(MAX_CONNECTIONS)",
        "Semaphore::new(MAX_CONCURRENT_GIT_WIRE_OPERATIONS)",
        "Semaphore::new(MAX_CONCURRENT_GATEWAY_DISPATCHES)",
        ".header_read_timeout(HEADER_READ_TIMEOUT)",
        ".max_headers(MAX_REQUEST_HEADERS)",
        ".max_buf_size(MAX_HTTP_BUFFER_BYTES)",
        "path.ends_with(\"/git-receive-pack\")",
        "MAX_JSON_REQUEST_BODY_BYTES",
        "tokio::time::timeout(deadline",
        "BoundedCollectError::TimedOut",
        "Err(BoundedCollectError::Read) => return request_body_read_error()",
        "fn request_body_read_error()",
        "let gateway_permit = match gateway_dispatch_slots.try_acquire_owned()",
        "tokio::task::spawn_blocking(move || {",
        "let _gateway_permit = gateway_permit;",
        "let _git_wire_permit = git_wire_permit;",
        "handle_gateway_safely(&gw, edge_req)",
        "path.ends_with(\"/git-upload-pack\")",
        "path.ends_with(\"/info/refs\")",
    ] {
        assert!(
            server.contains(required),
            "missing edge transport bound: {required}"
        );
    }
}
