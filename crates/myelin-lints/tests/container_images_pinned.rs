use std::{fs, path::Path};

#[test]
fn production_container_stages_use_immutable_base_images() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("myelin-lints lives under <workspace>/crates");
    let dockerfile = workspace.join("frontend/apps/web/Dockerfile");
    let source = fs::read_to_string(&dockerfile).expect("read production web Dockerfile");

    let mut stages = 0;
    for (index, line) in source.lines().enumerate() {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("FROM") {
            continue;
        }
        stages += 1;
        let image = fields.next().expect("FROM has an image");
        let digest = image
            .split_once("@sha256:")
            .map(|(_, digest)| digest)
            .unwrap_or_default();
        assert!(
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "{}:{} base image `{image}` must use a full sha256 digest",
            dockerfile.display(),
            index + 1,
        );
    }
    assert!(
        stages > 0,
        "production web Dockerfile must contain a FROM stage"
    );
}
