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
fn edge_keeps_non_cacheable_nosniff_response_defaults() {
    let server = fs::read_to_string(workspace().join("crates/myelin-edge/src/server.rs"))
        .expect("edge server");
    for required in [
        "harden_response_headers(&mut response)",
        "hyper::header::CACHE_CONTROL",
        "HeaderValue::from_static(\"no-store\")",
        "HeaderName::from_static(\"x-content-type-options\")",
        "HeaderValue::from_static(\"nosniff\")",
    ] {
        assert!(
            server.contains(required),
            "edge response hardening drift: {required}"
        );
    }
}
