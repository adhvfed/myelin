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
fn edge_requests_keep_bounded_pii_free_completion_signals() {
    let server = fs::read_to_string(workspace().join("crates/myelin-edge/src/server.rs"))
        .expect("edge server");
    for required in [
        "static REQUEST_SEQUENCE: AtomicU64",
        "static CONNECTIONS_SHED: AtomicU64",
        "HeaderName::from_static(\"x-request-id\")",
        "\"event\": \"edge.http.request\"",
        "\"route_class\": route_class",
        "\"duration_us\"",
        "if count.is_power_of_two()",
        "\"event\": \"edge.connection.shed\"",
    ] {
        assert!(
            server.contains(required),
            "request observability drift: {required}"
        );
    }
    assert!(
        !server.contains("\"path\": path"),
        "raw request paths may carry tenant, repository, or user-controlled PII"
    );
}
