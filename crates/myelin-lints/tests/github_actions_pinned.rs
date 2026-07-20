use std::{fs, path::Path};

#[test]
fn every_github_action_is_pinned_to_an_immutable_commit() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("myelin-lints lives under <workspace>/crates");
    let workflows = workspace.join(".github/workflows");

    for entry in fs::read_dir(&workflows).expect("read .github/workflows") {
        let path = entry.expect("workflow directory entry").path();
        if !matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("yml" | "yaml")
        ) {
            continue;
        }
        let source = fs::read_to_string(&path).expect("read workflow");
        for (index, line) in source.lines().enumerate() {
            let trimmed = line.trim_start().trim_start_matches("- ");
            let Some(value) = trimmed.strip_prefix("uses:") else {
                continue;
            };
            let action = value
                .trim()
                .split_once(" #")
                .map_or_else(|| value.trim(), |(reference, _)| reference.trim());
            if action.starts_with("./") {
                continue;
            }
            let revision = action.rsplit_once('@').map(|(_, revision)| revision);
            assert!(
                revision.is_some_and(|revision| {
                    revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
                }),
                "{}:{} action `{action}` must be pinned to a full 40-character commit SHA",
                path.display(),
                index + 1,
            );
        }
    }
}
