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

const FORBIDDEN_PRODUCTION_EDGES: &[(&str, &[&str])] = &[(
    "myelin-gdpr-service",
    &[
        "myelin-storage",
        "myelin-search",
        "myelin-refs",
        "myelin-refs-service",
        "myelin-notif",
    ],
)];

fn production_dependencies(root: &Path, owner: &str) -> Result<Vec<(String, String)>, String> {
    let path = root.join("crates").join(owner).join("Cargo.toml");
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("L2: cannot read `{}`: {error}", path.display()))?;
    let manifest: toml::Value = toml::from_str(&source)
        .map_err(|error| format!("L2: cannot parse `{}`: {error}", path.display()))?;
    let Some(dependencies) = manifest.get("dependencies").and_then(toml::Value::as_table) else {
        return Ok(Vec::new());
    };
    Ok(dependencies
        .iter()
        .map(|(alias, specification)| {
            let dependency = specification
                .as_table()
                .and_then(|table| table.get("package"))
                .and_then(toml::Value::as_str)
                .unwrap_or(alias);
            (alias.clone(), dependency.to_owned())
        })
        .collect())
}

pub fn scan_dependency_directions(root: &Path) -> Vec<String> {
    let layers = CORE_LAYERS.iter().copied().collect::<BTreeMap<_, _>>();
    let mut errors = Vec::new();
    for (owner, owner_layer) in &layers {
        let dependencies = match production_dependencies(root, owner) {
            Ok(dependencies) => dependencies,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        for (alias, specification) in dependencies {
            let Some(dependency_layer) = layers.get(specification.as_str()) else {
                continue;
            };
            if dependency_layer >= owner_layer {
                errors.push(format!(
                    "L2: `{owner}` (layer {owner_layer}) depends on `{specification}` (layer {dependency_layer}) via `{alias}`; core dependencies must point strictly downward"
                ));
            }
        }
    }
    for (owner, forbidden) in FORBIDDEN_PRODUCTION_EDGES {
        match production_dependencies(root, owner) {
            Ok(dependencies) => {
                for (alias, dependency) in dependencies {
                    if forbidden.contains(&dependency.as_str()) {
                        errors.push(format!(
                            "L2: `{owner}` depends on forbidden production crate `{dependency}` via `{alias}`; cross-store reads must use the personal-data-holder seam"
                        ));
                    }
                }
            }
            Err(error) => errors.push(error),
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
        let service = root.join("crates/myelin-gdpr-service");
        std::fs::create_dir_all(&service).unwrap();
        std::fs::write(
            service.join("Cargo.toml"),
            "[package]\nname = \"myelin-gdpr-service\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
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

    #[test]
    fn gdpr_service_can_test_store_crates_but_cannot_read_them_in_production() {
        let root = fixture_root("gdpr-service");
        let manifest = root.join("crates/myelin-gdpr-service/Cargo.toml");
        std::fs::write(
            &manifest,
            "[package]\nname = \"myelin-gdpr-service\"\nversion = \"0.0.0\"\n\n[dev-dependencies]\nmyelin-search = \"0\"\n",
        )
        .unwrap();
        assert!(scan_dependency_directions(&root).is_empty());

        std::fs::write(
            &manifest,
            "[package]\nname = \"myelin-gdpr-service\"\nversion = \"0.0.0\"\n\n[dependencies]\nsearch_alias = { package = \"myelin-search\", version = \"0\" }\n",
        )
        .unwrap();
        assert!(scan_dependency_directions(&root)
            .iter()
            .any(|error| error.contains("forbidden production crate `myelin-search`")));
        std::fs::remove_dir_all(root).ok();
    }
}
