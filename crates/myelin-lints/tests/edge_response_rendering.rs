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
fn edge_response_render_failures_never_become_false_successes() {
    let server = fs::read_to_string(workspace().join("crates/myelin-edge/src/server.rs"))
        .expect("edge server");
    assert!(
        server.contains("fn response_render_failure() -> Response<EdgeBody>")
            && server.contains("hyper::StatusCode::INTERNAL_SERVER_ERROR")
            && server.matches(".unwrap_or_else(|_| response_render_failure())").count() == 3,
        "every response-builder failure must converge on the canonical fail-closed 500"
    );
    assert!(
        !server.contains("Response::new(full_body(b\"{}\".to_vec()))"),
        "a render failure must never fall back to a bare 200"
    );
    for required in [
        "handler_response_headers_are_safe(&headers)",
        "\"content-length\"",
        "\"transfer-encoding\"",
        "\"connection\"",
        "\"x-request-id\"",
    ] {
        assert!(server.contains(required), "response framing guard drift: {required}");
    }
}
