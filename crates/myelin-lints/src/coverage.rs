use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

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
    CdcFileMissing {
        row: RowId,
        title: String,
        file: String,
    },
    CdcFileHasNoTests {
        row: RowId,
        title: String,
        file: String,
    },
    CoveredWithNoCdc {
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
                "row {row}: in contract-coverage.toml but NO LONGER in contract-index.md - a stale \
                 claim. Remove or re-point the manifest entry."
            ),
            CoverageError::CdcFileMissing { row, title, file } => write!(
                f,
                "row {row} ({title}): claims coverage via `{file}` which does NOT exist on disk - a \
                 falsely-claimed CDC pair. Ship the file or mark the row deferred with its landing \
                 prompt."
            ),
            CoverageError::CdcFileHasNoTests { row, title, file } => write!(
                f,
                "row {row} ({title}): CDC file `{file}` contains no test functions - it cannot \
                 prove the contract. Ship a real test or mark the row deferred."
            ),
            CoverageError::CoveredWithNoCdc { row, title } => write!(
                f,
                "row {row} ({title}): status=covered but declares NO cdc files - an empty claim of \
                 coverage. Name the CDC test file(s), or mark the row deferred."
            ),
            CoverageError::DeferredWithNoLandingPrompt { row, title } => write!(
                f,
                "row {row} ({title}): status=deferred but names NO landing prompt - an un-named \
                 floor. Name the prompt (e.g. landing = \"P-067\") that will ship its CDC pair."
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

pub fn parse_manifest(toml: &str) -> Result<Vec<ManifestEntry>, String> {
    let mut entries = Vec::new();
    let mut cur: Option<RawEntry> = None;

    let raw_lines: Vec<&str> = toml.lines().collect();
    let mut logical: Vec<(usize, String)> = Vec::new();
    let mut i = 0;
    while i < raw_lines.len() {
        let mut acc = strip_toml_comment(raw_lines[i]).trim().to_string();
        let opens = acc.matches('[').count();
        let closes = acc.matches(']').count();
        if acc != "[[contract]]" && opens > closes {
            let mut j = i + 1;
            let mut o = opens;
            let mut c = closes;
            while j < raw_lines.len() && o > c {
                let next = strip_toml_comment(raw_lines[j]).trim();
                acc.push(' ');
                acc.push_str(next);
                o += next.matches('[').count();
                c += next.matches(']').count();
                j += 1;
            }
            logical.push((i, acc));
            i = j;
        } else {
            logical.push((i, acc));
            i += 1;
        }
    }

    let mut skipping_frontend = false;
    for (lineno, line) in logical {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "[[contract]]" {
            if let Some(e) = cur.take() {
                entries.push(e.finish(lineno)?);
            }
            cur = Some(RawEntry::default());
            skipping_frontend = false;
            continue;
        }
        if line == "[[frontend]]" {
            if let Some(e) = cur.take() {
                entries.push(e.finish(lineno)?);
            }
            skipping_frontend = true;
            continue;
        }
        if skipping_frontend {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "line {}: not a key = value pair: {line}",
                lineno + 1
            ));
        };
        let key = key.trim();
        let value = value.trim();
        let Some(e) = cur.as_mut() else {
            return Err(format!(
                "line {}: key `{key}` before any [[contract]] table",
                lineno + 1
            ));
        };
        match key {
            "row" => e.row = Some(unquote(value, lineno)?),
            "title" => e.title = Some(unquote(value, lineno)?),
            "status" => e.status = Some(unquote(value, lineno)?),
            "landing" => e.landing = Some(unquote(value, lineno)?),
            "cdc" => e.cdc = Some(parse_string_array(value, lineno)?),
            other => return Err(format!("line {}: unknown key `{other}`", lineno + 1)),
        }
    }
    if let Some(e) = cur.take() {
        entries.push(e.finish(usize::MAX)?);
    }
    Ok(entries)
}

pub fn parse_frontend_contracts(toml: &str) -> Result<Vec<FrontendContract>, String> {
    let mut entries = Vec::new();
    let mut cur: Option<RawFrontendContract> = None;
    let raw_lines: Vec<&str> = toml.lines().collect();
    let mut i = 0;
    while i < raw_lines.len() {
        let lineno = i;
        let mut line = strip_toml_comment(raw_lines[i]).trim().to_string();
        let mut opens = line.matches('[').count();
        let mut closes = line.matches(']').count();
        i += 1;
        while !line.starts_with("[[") && opens > closes && i < raw_lines.len() {
            let next = strip_toml_comment(raw_lines[i]).trim();
            line.push(' ');
            line.push_str(next);
            opens += next.matches('[').count();
            closes += next.matches(']').count();
            i += 1;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "[[frontend]]" {
            if let Some(entry) = cur.take() {
                entries.push(entry.finish(lineno)?);
            }
            cur = Some(RawFrontendContract::default());
            continue;
        }
        if line == "[[contract]]" {
            if let Some(entry) = cur.take() {
                entries.push(entry.finish(lineno)?);
            }
            continue;
        }
        let Some(entry) = cur.as_mut() else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "line {}: not a key = value pair: {line}",
                lineno + 1
            ));
        };
        match key.trim() {
            "id" => entry.id = Some(unquote(value.trim(), lineno)?),
            "golden" => entry.golden = Some(unquote(value.trim(), lineno)?),
            "rust_tests" => entry.rust_tests = Some(parse_string_array(value.trim(), lineno)?),
            "frontend_tests" => {
                entry.frontend_tests = Some(parse_string_array(value.trim(), lineno)?)
            }
            "e2e_tests" => entry.e2e_tests = Some(parse_string_array(value.trim(), lineno)?),
            other => {
                return Err(format!(
                    "line {}: unknown frontend key `{other}`",
                    lineno + 1
                ))
            }
        }
    }
    if let Some(entry) = cur.take() {
        entries.push(entry.finish(usize::MAX)?);
    }
    Ok(entries)
}

#[derive(Default)]
struct RawFrontendContract {
    id: Option<String>,
    golden: Option<String>,
    rust_tests: Option<Vec<String>>,
    frontend_tests: Option<Vec<String>>,
    e2e_tests: Option<Vec<String>>,
}

impl RawFrontendContract {
    fn finish(self, lineno: usize) -> Result<FrontendContract, String> {
        let near = lineno.saturating_add(1);
        Ok(FrontendContract {
            id: self
                .id
                .ok_or_else(|| format!("frontend entry near line {near} has no `id`"))?,
            golden: self
                .golden
                .ok_or_else(|| format!("frontend entry near line {near} has no `golden`"))?,
            rust_tests: self.rust_tests.unwrap_or_default(),
            frontend_tests: self.frontend_tests.unwrap_or_default(),
            e2e_tests: self.e2e_tests.unwrap_or_default(),
        })
    }
}

#[derive(Default)]
struct RawEntry {
    row: Option<String>,
    title: Option<String>,
    status: Option<String>,
    cdc: Option<Vec<String>>,
    landing: Option<String>,
}

impl RawEntry {
    fn finish(self, lineno: usize) -> Result<ManifestEntry, String> {
        let row_s = self
            .row
            .ok_or_else(|| format!("contract entry near line {} has no `row`", lineno + 1))?;
        let row = RowId::parse(&row_s)
            .ok_or_else(|| format!("bad row id `{row_s}` near line {}", lineno + 1))?;
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

fn strip_toml_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) if !before_is_in_string(&line[..i]) => &line[..i],
        _ => line,
    }
}

fn before_is_in_string(prefix: &str) -> bool {
    prefix.chars().filter(|&c| c == '"').count() % 2 == 1
}

fn unquote(value: &str, lineno: usize) -> Result<String, String> {
    let v = value.trim();
    v.strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .map(|s| s.to_string())
        .ok_or_else(|| {
            format!(
                "line {}: expected a quoted string, got `{value}`",
                lineno + 1
            )
        })
}

fn parse_string_array(value: &str, lineno: usize) -> Result<Vec<String>, String> {
    let v = value.trim();
    let inner = v
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .ok_or_else(|| {
            format!(
                "line {}: expected an array `[...]`, got `{value}`",
                lineno + 1
            )
        })?;
    let mut out = Vec::new();
    for part in inner.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        out.push(unquote(p, lineno)?);
    }
    Ok(out)
}

pub fn has_test_fn(src: &str) -> bool {
    src.contains("#[test]") || src.contains("#[tokio::test") || src.contains("#[sqlx::test")
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
                 0 falsely-claimed.",
                self.rows_checked, self.covered, self.deferred
            )
        } else {
            format!(
                "{date} contract-coverage: RED - {} falsely-claimed/dropped row(s) (CLAIMED, NOT \
                 PROVEN; fix the code, never weaken the gate).",
                self.errors.len()
            )
        }
    }
}

pub trait CdcSource {
    fn read(&self, file: &str) -> Option<String>;
}

pub fn scan_frontend_contracts(
    contracts: &[FrontendContract],
    source: &dyn CdcSource,
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
        let marker = format!("FRONTEND-CONTRACT: {id}");
        match source.read(&contract.golden) {
            None => errors.push(format!(
                "frontend contract `{id}` golden `{}` does not exist",
                contract.golden
            )),
            Some(golden) => {
                if !golden.contains(id) || !golden.contains("\"vectors\"") {
                    errors.push(format!(
                        "frontend contract `{id}` golden `{}` lacks its id or vectors",
                        contract.golden
                    ));
                }
            }
        }
        for (kind, files, must_name_golden) in [
            ("Rust provider", &contract.rust_tests, true),
            ("frontend consumer", &contract.frontend_tests, true),
            ("browser proof", &contract.e2e_tests, false),
        ] {
            if files.is_empty() {
                errors.push(format!("frontend contract `{id}` has no {kind} files"));
            }
            for file in files {
                match source.read(file) {
                    None => errors.push(format!(
                        "frontend contract `{id}` {kind} file `{file}` does not exist"
                    )),
                    Some(body) => {
                        if !body.contains(&marker) {
                            errors.push(format!(
                                "frontend contract `{id}` {kind} file `{file}` lacks `{marker}`"
                            ));
                        }
                        if must_name_golden && !body.contains(&contract.golden) {
                            errors.push(format!(
                                "frontend contract `{id}` {kind} file `{file}` does not consume `{}`",
                                contract.golden
                            ));
                        }
                    }
                }
            }
        }
    }
    errors
}

pub struct FsCdc {
    pub workspace_root: PathBuf,
}

impl CdcSource for FsCdc {
    fn read(&self, file: &str) -> Option<String> {
        std::fs::read_to_string(self.workspace_root.join(file)).ok()
    }
}

pub fn scan(rows: &[RowId], manifest: &[ManifestEntry], cdc: &dyn CdcSource) -> ScanReport {
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
                    report.errors.push(CoverageError::CoveredWithNoCdc {
                        row: *row,
                        title: entry.title.clone(),
                    });
                    continue;
                }
                for file in files {
                    match cdc.read(file) {
                        None => report.errors.push(CoverageError::CdcFileMissing {
                            row: *row,
                            title: entry.title.clone(),
                            file: file.clone(),
                        }),
                        Some(src) => {
                            if !has_test_fn(&src) {
                                report.errors.push(CoverageError::CdcFileHasNoTests {
                                    row: *row,
                                    title: entry.title.clone(),
                                    file: file.clone(),
                                });
                            }
                        }
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

    struct FakeCdc(std::collections::HashMap<&'static str, &'static str>);
    impl CdcSource for FakeCdc {
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
    fn green_when_covered_rows_have_a_real_pair_and_deferred_rows_name_a_landing() {
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
        let cdc = FakeCdc(
            [("cdc_1_1.rs", "#[test]\nfn proves_the_contract() {}")]
                .into_iter()
                .collect(),
        );
        let report = scan(&rows(&["1.1", "4.3"]), &manifest, &cdc);
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
        let cdc = FakeCdc(Default::default());
        let report = scan(&rows(&["1.1"]), &manifest, &cdc);
        assert!(!report.is_green());
        assert!(matches!(
            report.errors[0],
            CoverageError::CdcFileMissing { .. }
        ));
        assert!(report.artifact_row("2026-06-19").contains("RED"));
    }

    #[test]
    fn red_when_a_covered_file_has_no_test_functions() {
        let manifest = vec![ManifestEntry {
            row: RowId::parse("1.1").unwrap(),
            title: "serve".into(),
            coverage: Coverage::Covered {
                cdc: vec!["empty.rs".into()],
            },
        }];
        let cdc = FakeCdc([("empty.rs", "fn helper() {}")].into_iter().collect());
        let report = scan(&rows(&["1.1"]), &manifest, &cdc);
        assert!(!report.is_green());
        assert!(matches!(
            report.errors[0],
            CoverageError::CdcFileHasNoTests { .. }
        ));
    }

    #[test]
    fn red_when_a_contract_row_is_absent_from_the_manifest() {
        let report = scan(&rows(&["1.1", "9.9"]), &[], &FakeCdc(Default::default()));
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
        let report = scan(&rows(&["4.3"]), &manifest, &FakeCdc(Default::default()));
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
        let report = scan(&rows(&["1.1"]), &manifest, &FakeCdc(Default::default()));
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
        let report = scan(&rows(&["1.1"]), &manifest, &FakeCdc(Default::default()));
        assert!(matches!(
            report.errors[0],
            CoverageError::CoveredWithNoCdc { .. }
        ));
    }
}
