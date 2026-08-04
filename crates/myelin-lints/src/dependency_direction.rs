use std::collections::BTreeMap;
use std::path::Path;

const CORE_LAYERS: &[(&str, u8)] = &[
    ("myelin-tenancy", 0),
    ("myelin-identity", 1),
    ("myelin-events", 2),
    ("myelin-refs", 3),
    ("myelin-query", 4),
    ("myelin-content", 5),
    ("myelin-agent", 3),
    ("myelin-gdpr", 3),
    ("myelin-client", 3),
    ("myelin-storage", 4),
    ("myelin-substrate", 6),
];

pub fn scan_dependency_directions(root: &Path) -> Vec<String> {
    let layers = CORE_LAYERS.iter().copied().collect::<BTreeMap<_, _>>();
    let mut errors = Vec::new();
    for (owner, owner_layer) in &layers {
        let path = root.join("crates").join(owner).join("Cargo.toml");
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                errors.push(format!("L2: cannot read `{}`: {error}", path.display()));
                continue;
            }
        };
        let manifest: toml::Value = match toml::from_str(&source) {
            Ok(manifest) => manifest,
            Err(error) => {
                errors.push(format!("L2: cannot parse `{}`: {error}", path.display()));
                continue;
            }
        };
        let Some(dependencies) = manifest.get("dependencies").and_then(toml::Value::as_table)
        else {
            continue;
        };
        for (alias, specification) in dependencies {
            let dependency = specification
                .as_table()
                .and_then(|table| table.get("package"))
                .and_then(toml::Value::as_str)
                .unwrap_or(alias);
            let Some(dependency_layer) = layers.get(dependency) else {
                continue;
            };
            if dependency_layer >= owner_layer {
                errors.push(format!(
                    "L2: `{owner}` (layer {owner_layer}) depends on `{dependency}` (layer {dependency_layer}) via `{alias}`; core dependencies must point strictly downward"
                ));
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "myelin-dependency-direction-{name}-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&root).ok();
        for (name, _) in CORE_LAYERS {
            let directory = root.join("crates").join(name);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"0.0.0\"\n"),
            )
            .unwrap();
        }
        root
    }

    #[test]
    fn the_frozen_layers_admit_downward_edges_and_reject_upward_or_peer_edges() {
        let layers = CORE_LAYERS.iter().copied().collect::<BTreeMap<_, _>>();
        let allowed = |owner: &str, dependency: &str| layers[dependency] < layers[owner];
        assert!(allowed("myelin-storage", "myelin-events"));
        assert!(allowed("myelin-substrate", "myelin-storage"));
        assert!(!allowed("myelin-identity", "myelin-events"));
        assert!(!allowed("myelin-agent", "myelin-gdpr"));
    }

    #[test]
    fn workspace_scanner_is_green_then_red_for_a_real_manifest_edge() {
        let root = fixture_root("scanner");
        let manifest = root.join("crates/myelin-storage/Cargo.toml");
        std::fs::write(
            &manifest,
            "[package]\nname = \"myelin-storage\"\nversion = \"0.0.0\"\n\n[dependencies]\nmyelin-events = \"0\"\n",
        )
        .unwrap();
        assert!(scan_dependency_directions(&root).is_empty());

        std::fs::write(
            &manifest,
            "[package]\nname = \"myelin-storage\"\nversion = \"0.0.0\"\n\n[dependencies]\ncontent_alias = { package = \"myelin-content\", version = \"0\" }\n",
        )
        .unwrap();
        assert!(scan_dependency_directions(&root)
            .iter()
            .any(|error| error.contains("myelin-content")));
        std::fs::remove_dir_all(root).ok();
    }
}
