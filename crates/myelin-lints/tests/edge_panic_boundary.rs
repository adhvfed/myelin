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
fn edge_panics_are_contained_without_logging_payloads() {
    let root = workspace();
    let server =
        fs::read_to_string(root.join("crates/myelin-edge/src/server.rs")).expect("edge server");
    let consumer = fs::read_to_string(root.join("crates/myelin-events/src/consumer.rs"))
        .expect("shared event consumer");

    assert!(
        consumer.contains("std::panic::set_hook") && consumer.contains("payload suppressed"),
        "the shared production panic hook must suppress its payload"
    );
    assert!(
        !consumer.contains("downcast_ref::<&str>") && !consumer.contains("downcast_ref::<String>"),
        "the event consumer must never inspect or format panic payloads"
    );
    for required in [
        "catch_unwind(AssertUnwindSafe(|| gw.handle(request)))",
        "EdgeError::Internal(\"gateway handler panicked\".into())",
        "handle_gateway_safely(&gw, edge_req)",
    ] {
        assert!(
            server.contains(required),
            "panic containment drift: {required}"
        );
    }

    for (path, service) in [
        (
            "crates/myelin-ci-controlplane/src/main.rs",
            "ci-controlplane",
        ),
        ("crates/myelin-ci-dispatch/src/main.rs", "ci-dispatch"),
        ("crates/myelin-edge/src/main.rs", "edge"),
        ("crates/myelin-flow/src/main.rs", "flow"),
        ("crates/myelin-identity-service/src/main.rs", "identity"),
        ("crates/myelin-issues/src/main.rs", "issues"),
        ("crates/myelin-knowledge/src/main.rs", "knowledge"),
        ("crates/myelin-mcp/src/main.rs", "mcp"),
        ("crates/myelin-notif/src/main.rs", "notif"),
        ("crates/myelin-search/src/main.rs", "search"),
    ] {
        let source = fs::read_to_string(root.join(path)).unwrap_or_else(|error| {
            panic!("could not read production service root {path}: {error}")
        });
        let install = format!("install_payload_free_panic_hook(\"{service}\")");
        assert!(
            source.contains(&install),
            "production service {path} must install {install} before starting workers"
        );
    }
}
