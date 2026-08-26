use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RowId {
    pub cluster: u32,
    pub item: u32,
}

impl RowId {
    pub fn parse(s: &str) -> Option<RowId> {
        let (a, b) = s.trim().split_once('.')?;
        Some(RowId {
            cluster: a.trim().parse().ok()?,
            item: b.trim().parse().ok()?,
        })
    }
}

impl fmt::Display for RowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.cluster, self.item)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Coverage {
    Covered { cdc: Vec<String> },
    Deferred { landing: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestEntry {
    pub row: RowId,
    pub title: String,
    pub coverage: Coverage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontendContract {
    pub id: String,
    pub golden: String,
    pub rust_tests: Vec<String>,
    pub frontend_tests: Vec<String>,
    pub e2e_tests: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoverageError {
    RowMissingFromManifest {
        row: RowId,
    },
    StaleManifestEntry {
        row: RowId,
    },
    CoverageArtifactMissing {
        row: RowId,
        title: String,
        file: String,
    },
    CoveredWithNoArtifacts {
        row: RowId,
        title: String,
    },
    DeferredWithNoLandingPrompt {
        row: RowId,
        title: String,
    },
}

impl fmt::Display for CoverageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoverageError::RowMissingFromManifest { row } => write!(
                f,
                "row {row}: present in contract-index but ABSENT from contract-coverage.toml - a \
                 silently-dropped contract. Add a `[[contract]]` entry (covered or deferred)."
            ),
            CoverageError::StaleManifestEntry { row } => write!(
                f,
                "row {row}: in contract-coverage.toml but no longer in contract-index.md. Remove or \
                 re-point the stale registry entry."
            ),
            CoverageError::CoverageArtifactMissing { row, title, file } => write!(
                f,
                "row {row} ({title}): registered coverage artifact `{file}` does not exist. \
                 Restore the artifact, update the registry, or defer the row with a landing prompt."
            ),
            CoverageError::CoveredWithNoArtifacts { row, title } => write!(
                f,
                "row {row} ({title}): status=covered but registers no coverage artifacts. Name the \
                 relevant test file(s), or mark the row deferred."
            ),
            CoverageError::DeferredWithNoLandingPrompt { row, title } => write!(
                f,
                "row {row} ({title}): status=deferred but names NO landing prompt - an un-named \
                 floor. Name the prompt (e.g. landing = \"P-067\") that will add its coverage \
                 artifacts."
            ),
        }
    }
}

pub fn parse_contract_index_rows(markdown: &str) -> Vec<RowId> {
    let mut rows = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("| ") else {
            continue;
        };
        let Some((first, _)) = rest.split_once('|') else {
            continue;
        };
        if let Some(id) = RowId::parse(first) {
            rows.push(id);
        }
    }
    rows.sort();
    rows.dedup();
    rows
}

pub fn parse_manifest(source: &str) -> Result<Vec<ManifestEntry>, String> {
    let document = parse_registry(source)?;
    document
        .contract
        .into_iter()
        .enumerate()
        .map(|(index, entry)| entry.finish(index))
        .collect()
}

pub fn parse_frontend_contracts(source: &str) -> Result<Vec<FrontendContract>, String> {
    let document = parse_registry(source)?;
    document
        .frontend
        .into_iter()
        .enumerate()
        .map(|(index, entry)| entry.finish(index))
        .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistry {
    #[serde(default)]
    contract: Vec<RawEntry>,
    #[serde(default)]
    frontend: Vec<RawFrontendContract>,
}

fn parse_registry(source: &str) -> Result<RawRegistry, String> {
    toml::from_str(source).map_err(|error| format!("invalid contract registry TOML: {error}"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFrontendContract {
    id: Option<String>,
    golden: Option<String>,
    rust_tests: Option<Vec<String>>,
    frontend_tests: Option<Vec<String>>,
    e2e_tests: Option<Vec<String>>,
}

impl RawFrontendContract {
    fn finish(self, index: usize) -> Result<FrontendContract, String> {
        let entry = index.saturating_add(1);
        Ok(FrontendContract {
            id: self
                .id
                .ok_or_else(|| format!("frontend entry {entry} has no `id`"))?,
            golden: self
                .golden
                .ok_or_else(|| format!("frontend entry {entry} has no `golden`"))?,
            rust_tests: self.rust_tests.unwrap_or_default(),
            frontend_tests: self.frontend_tests.unwrap_or_default(),
            e2e_tests: self.e2e_tests.unwrap_or_default(),
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    row: Option<String>,
    title: Option<String>,
    status: Option<String>,
    cdc: Option<Vec<String>>,
    landing: Option<String>,
}

impl RawEntry {
    fn finish(self, index: usize) -> Result<ManifestEntry, String> {
        let entry = index.saturating_add(1);
        let row_s = self
            .row
            .ok_or_else(|| format!("contract entry {entry} has no `row`"))?;
        let row = RowId::parse(&row_s)
            .ok_or_else(|| format!("contract entry {entry} has bad row id `{row_s}`"))?;
        let title = self.title.unwrap_or_default();
        let status = self
            .status
            .ok_or_else(|| format!("row {row} has no `status`"))?;
        let coverage = match status.as_str() {
            "covered" => Coverage::Covered {
                cdc: self.cdc.unwrap_or_default(),
            },
            "deferred" => Coverage::Deferred {
                landing: self.landing.unwrap_or_default(),
            },
            other => return Err(format!("row {row} has unknown status `{other}`")),
        };
        Ok(ManifestEntry {
            row,
            title,
            coverage,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScanReport {
    pub rows_checked: usize,
    pub covered: usize,
    pub deferred: usize,
    pub errors: Vec<CoverageError>,
}

impl ScanReport {
    pub fn is_green(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn artifact_row(&self, date: &str) -> String {
        if self.is_green() {
            format!(
                "{date} contract-coverage: GREEN - {} rows reconciled ({} covered, {} deferred), \
                 0 registry errors.",
                self.rows_checked, self.covered, self.deferred
            )
        } else {
            format!(
                "{date} contract-coverage: RED - {} missing, stale, or malformed registry \
                 entry/entries.",
                self.errors.len()
            )
        }
    }
}

pub trait ArtifactSource {
    fn read(&self, file: &str) -> Option<String>;
}

pub fn scan_frontend_contracts(
    contracts: &[FrontendContract],
    source: &dyn ArtifactSource,
) -> Vec<String> {
    if contracts.is_empty() {
        return Vec::new();
    }
    let mut errors = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for contract in contracts {
        let id = contract.id.trim();
        if id.is_empty() {
            errors.push("frontend contract has an empty id".into());
            continue;
        }
        if !ids.insert(id.to_string()) {
            errors.push(format!(
                "frontend contract `{id}` is registered more than once"
            ));
        }
        match source.read(&contract.golden) {
            None => errors.push(format!(
                "frontend contract `{id}` golden `{}` does not exist",
                contract.golden
            )),
            Some(golden) => match serde_json::from_str::<serde_json::Value>(&golden) {
                Err(error) => errors.push(format!(
                    "frontend contract `{id}` golden `{}` is not valid JSON: {error}",
                    contract.golden
                )),
                Ok(document) => {
                    if document
                        .get("schema_version")
                        .and_then(|value| value.as_u64())
                        != Some(1)
                    {
                        errors.push(format!(
                            "frontend contract `{id}` golden `{}` must declare schema_version 1",
                            contract.golden
                        ));
                    }
                    if document.get("contract_id").and_then(|value| value.as_str()) != Some(id) {
                        errors.push(format!(
                            "frontend contract `{id}` golden `{}` has a different contract_id",
                            contract.golden
                        ));
                    }
                    if !matches!(
                        document.get("vectors").and_then(|value| value.as_array()),
                        Some(vectors) if !vectors.is_empty()
                    ) {
                        errors.push(format!(
                            "frontend contract `{id}` golden `{}` must contain at least one vector",
                            contract.golden
                        ));
                    }
                }
            },
        }
        for (kind, files) in [
            ("Rust provider", &contract.rust_tests),
            ("frontend consumer", &contract.frontend_tests),
            ("browser journey", &contract.e2e_tests),
        ] {
            if files.is_empty() {
                errors.push(format!("frontend contract `{id}` has no {kind} files"));
            }
            for file in files {
                if source.read(file).is_none() {
                    errors.push(format!(
                        "frontend contract `{id}` {kind} file `{file}` does not exist"
                    ));
                }
            }
        }
    }
    errors
}

pub struct FsArtifacts {
    pub workspace_root: PathBuf,
}

impl ArtifactSource for FsArtifacts {
    fn read(&self, file: &str) -> Option<String> {
        std::fs::read_to_string(self.workspace_root.join(file)).ok()
    }
}

pub fn scan(
    rows: &[RowId],
    manifest: &[ManifestEntry],
    artifacts: &dyn ArtifactSource,
) -> ScanReport {
    let mut report = ScanReport::default();

    let mut by_row: BTreeMap<RowId, &ManifestEntry> = BTreeMap::new();
    for e in manifest {
        by_row.insert(e.row, e);
    }

    let row_set: std::collections::BTreeSet<RowId> = rows.iter().copied().collect();
    report.rows_checked = row_set.len();

    for row in &row_set {
        let Some(entry) = by_row.get(row) else {
            report
                .errors
                .push(CoverageError::RowMissingFromManifest { row: *row });
            continue;
        };
        match &entry.coverage {
            Coverage::Covered { cdc: files } => {
                report.covered += 1;
                if files.is_empty() {
                    report.errors.push(CoverageError::CoveredWithNoArtifacts {
                        row: *row,
                        title: entry.title.clone(),
                    });
                    continue;
                }
                for file in files {
                    if artifacts.read(file).is_none() {
                        report.errors.push(CoverageError::CoverageArtifactMissing {
                            row: *row,
                            title: entry.title.clone(),
                            file: file.clone(),
                        });
                    }
                }
            }
            Coverage::Deferred { landing } => {
                report.deferred += 1;
                if landing.trim().is_empty() {
                    report
                        .errors
                        .push(CoverageError::DeferredWithNoLandingPrompt {
                            row: *row,
                            title: entry.title.clone(),
                        });
                }
            }
        }
    }

    for e in manifest {
        if !row_set.contains(&e.row) {
            report
                .errors
                .push(CoverageError::StaleManifestEntry { row: e.row });
        }
    }

    report
}

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeArtifacts(std::collections::HashMap<&'static str, &'static str>);
    impl ArtifactSource for FakeArtifacts {
        fn read(&self, file: &str) -> Option<String> {
            self.0.get(file).map(|s| s.to_string())
        }
    }

    fn rows(ids: &[&str]) -> Vec<RowId> {
        ids.iter().map(|s| RowId::parse(s).unwrap()).collect()
    }

    #[test]
    fn row_id_parses_and_sorts_numerically_not_lexically() {
        let mut r = rows(&["1.10", "1.2", "1.1"]);
        r.sort();
        assert_eq!(r, rows(&["1.1", "1.2", "1.10"]));
        assert_eq!(RowId::parse("11.5").unwrap().to_string(), "11.5");
        assert!(RowId::parse("not-a-row").is_none());
    }

    #[test]
    fn parses_the_authoritative_row_set_out_of_a_contract_index_table() {
        let md = "\
## 1. Bootstrap\n\
| # | Contract | Owner |\n\
|---|---|---|\n\
| 1.1 | **serve** | substrate |\n\
| 1.10 | **shed** | harness |\n\
## 2. Bus\n\
| 2.1 | **EventEnvelope** | Bus |\n\
prose line | not a row\n";
        let got = parse_contract_index_rows(md);
        assert_eq!(got, rows(&["1.1", "1.10", "2.1"]));
    }

    #[test]
    fn parses_a_manifest_with_covered_and_deferred_entries_incl_multiline_array() {
        let toml = "\
# a comment\n\
[[contract]]\n\
row = \"1.1\"\n\
title = \"serve\"\n\
status = \"covered\"\n\
cdc = [\n  \"a.rs\",\n  \"b.rs\",\n]\n\
\n\
[[contract]]\n\
row = \"4.3\"\n\
title = \"list_objects\"\n\
status = \"deferred\"\n\
landing = \"P-070\"\n";
        let m = parse_manifest(toml).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].row, RowId::parse("1.1").unwrap());
        assert_eq!(
            m[0].coverage,
            Coverage::Covered {
                cdc: vec!["a.rs".into(), "b.rs".into()]
            }
        );
        assert_eq!(
            m[1].coverage,
            Coverage::Deferred {
                landing: "P-070".into()
            }
        );
    }

    #[test]
    fn green_when_covered_rows_register_artifacts_and_deferred_rows_name_a_landing() {
        let manifest = vec![
            ManifestEntry {
                row: RowId::parse("1.1").unwrap(),
                title: "serve".into(),
                coverage: Coverage::Covered {
                    cdc: vec!["cdc_1_1.rs".into()],
                },
            },
            ManifestEntry {
                row: RowId::parse("4.3").unwrap(),
                title: "list_objects".into(),
                coverage: Coverage::Deferred {
                    landing: "P-070".into(),
                },
            },
        ];
        let artifacts = FakeArtifacts([("cdc_1_1.rs", "")].into_iter().collect());
        let report = scan(&rows(&["1.1", "4.3"]), &manifest, &artifacts);
        assert!(report.is_green(), "errors: {:?}", report.errors);
        assert_eq!(report.covered, 1);
        assert_eq!(report.deferred, 1);
        assert!(report.artifact_row("2026-06-19").contains("GREEN"));
    }

    #[test]
    fn red_when_a_covered_row_names_a_missing_cdc_file() {
        let manifest = vec![ManifestEntry {
            row: RowId::parse("1.1").unwrap(),
            title: "serve".into(),
            coverage: Coverage::Covered {
                cdc: vec!["does_not_exist.rs".into()],
            },
        }];
        let artifacts = FakeArtifacts(Default::default());
        let report = scan(&rows(&["1.1"]), &manifest, &artifacts);
        assert!(!report.is_green());
        assert!(matches!(
            report.errors[0],
            CoverageError::CoverageArtifactMissing { .. }
        ));
        assert!(report.artifact_row("2026-06-19").contains("RED"));
    }

    #[test]
    fn coverage_registry_does_not_guess_test_semantics_from_source_text() {
        let manifest = vec![ManifestEntry {
            row: RowId::parse("1.1").unwrap(),
            title: "serve".into(),
            coverage: Coverage::Covered {
                cdc: vec!["empty.rs".into()],
            },
        }];
        let artifacts = FakeArtifacts([("empty.rs", "")].into_iter().collect());
        let report = scan(&rows(&["1.1"]), &manifest, &artifacts);
        assert!(report.is_green());
    }

    #[test]
    fn frontend_registry_validates_shared_vectors_and_artifact_membership() {
        let contract = FrontendContract {
            id: "git-parity".into(),
            golden: "git.json".into(),
            rust_tests: vec!["provider.rs".into()],
            frontend_tests: vec!["consumer.ts".into()],
            e2e_tests: vec!["journey.ts".into()],
        };
        let artifacts = FakeArtifacts(
            [
                (
                    "git.json",
                    r#"{"schema_version":1,"contract_id":"git-parity","vectors":[{}]}"#,
                ),
                ("provider.rs", "source syntax is deliberately irrelevant"),
                ("consumer.ts", ""),
                ("journey.ts", ""),
            ]
            .into_iter()
            .collect(),
        );

        assert!(scan_frontend_contracts(&[contract], &artifacts).is_empty());
    }

    #[test]
    fn frontend_registry_rejects_malformed_vectors_and_missing_artifacts() {
        let contract = FrontendContract {
            id: "ci-parity".into(),
            golden: "ci.json".into(),
            rust_tests: vec!["missing-provider.rs".into()],
            frontend_tests: vec![],
            e2e_tests: vec!["journey.ts".into()],
        };
        let artifacts = FakeArtifacts(
            [
                (
                    "ci.json",
                    r#"{"schema_version":2,"contract_id":"another-contract","vectors":[]}"#,
                ),
                ("journey.ts", ""),
            ]
            .into_iter()
            .collect(),
        );

        let errors = scan_frontend_contracts(&[contract], &artifacts);
        assert_eq!(errors.len(), 5, "errors: {errors:#?}");
        assert!(errors
            .iter()
            .any(|error| error.contains("schema_version 1")));
        assert!(errors
            .iter()
            .any(|error| error.contains("different contract_id")));
        assert!(errors
            .iter()
            .any(|error| error.contains("at least one vector")));
        assert!(errors
            .iter()
            .any(|error| error.contains("missing-provider.rs")));
        assert!(errors
            .iter()
            .any(|error| error.contains("no frontend consumer")));
    }

    #[test]
    fn red_when_a_contract_row_is_absent_from_the_manifest() {
        let report = scan(
            &rows(&["1.1", "9.9"]),
            &[],
            &FakeArtifacts(Default::default()),
        );
        assert_eq!(report.errors.len(), 2);
        assert!(report
            .errors
            .iter()
            .all(|e| matches!(e, CoverageError::RowMissingFromManifest { .. })));
    }

    #[test]
    fn red_when_a_deferred_row_has_no_landing_prompt() {
        let manifest = vec![ManifestEntry {
            row: RowId::parse("4.3").unwrap(),
            title: "list_objects".into(),
            coverage: Coverage::Deferred { landing: "".into() },
        }];
        let report = scan(
            &rows(&["4.3"]),
            &manifest,
            &FakeArtifacts(Default::default()),
        );
        assert!(matches!(
            report.errors[0],
            CoverageError::DeferredWithNoLandingPrompt { .. }
        ));
    }

    #[test]
    fn red_when_a_manifest_entry_is_stale() {
        let manifest = vec![ManifestEntry {
            row: RowId::parse("99.1").unwrap(),
            title: "gone".into(),
            coverage: Coverage::Deferred {
                landing: "P-001".into(),
            },
        }];
        let report = scan(
            &rows(&["1.1"]),
            &manifest,
            &FakeArtifacts(Default::default()),
        );
        assert!(report.errors.iter().any(
            |e| matches!(e, CoverageError::StaleManifestEntry { row } if row.to_string() == "99.1")
        ));
        assert!(report
            .errors
            .iter()
            .any(|e| matches!(e, CoverageError::RowMissingFromManifest { .. })));
    }

    #[test]
    fn red_when_covered_declares_no_cdc_files() {
        let manifest = vec![ManifestEntry {
            row: RowId::parse("1.1").unwrap(),
            title: "serve".into(),
            coverage: Coverage::Covered { cdc: vec![] },
        }];
        let report = scan(
            &rows(&["1.1"]),
            &manifest,
            &FakeArtifacts(Default::default()),
        );
        assert!(matches!(
            report.errors[0],
            CoverageError::CoveredWithNoArtifacts { .. }
        ));
    }
}
