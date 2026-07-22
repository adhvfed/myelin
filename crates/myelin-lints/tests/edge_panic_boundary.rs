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
    let main = fs::read_to_string(root.join("crates/myelin-edge/src/main.rs")).expect("edge main");
    let server =
        fs::read_to_string(root.join("crates/myelin-edge/src/server.rs")).expect("edge server");
    let consumer = fs::read_to_string(root.join("crates/myelin-events/src/consumer.rs"))
        .expect("shared event consumer");

    assert!(
        main.contains("install_payload_free_panic_hook(\"edge\")")
            && consumer.contains("std::panic::set_hook")
            && consumer.contains("payload suppressed"),
        "production must install the shared payload-free panic hook"
    );
    assert!(
        !consumer.contains("downcast_ref::<&str>")
            && !consumer.contains("downcast_ref::<String>"),
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
}
