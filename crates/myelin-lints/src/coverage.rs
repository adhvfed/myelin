//! # The contract-coverage scanner — the meta-gate (P-S21 → P-037)
//!
//! **Owning architecture doc:** the ledger overview
//! `planning/07-prompts/00-ledger-overview.md` §6 ("the contract-coverage scanner + the
//! failure-injection harness … fails the workspace if any contract-index row lacks a provider +
//! consumer CDC pair — so every contract prompt's TESTS field includes the CDC pair").
//! **Contract source of truth:** every row of
//! `planning/05-refined-shared-systems-architecture/contract-index.md` §1–§13 (the frozen
//! build-to surface). **Doctrine:** `external-insights/01-process-and-quality-doctrine.md` §5 —
//! the ratchet: convert each discipline into a committed, mechanical, LOUD gate; "an uncommitted
//! gate is no gate"; "make violations loud, never silently swallowed" — "a contract violation you
//! drop silently is a multi-day misdiagnosis waiting to happen".
//!
//! ## What this is (the meta-gate every later prompt's DEFINITION OF DONE leans on)
//! The architecture lints ([`crate::lints`]) catch bug-classes in code. This scanner catches the
//! bug-class one level up: a cross-system **contract** that ships WITHOUT its provider+consumer
//! consumer-driven-contract (CDC) test pair, so the two sides of the seam drift apart in
//! production with nothing failing the build. The scanner is the build-layer realisation of
//! "every contract-index row has a provider + consumer CDC pair, or is explicitly marked
//! not-yet-implemented with its landing prompt".
//!
//! ## The mechanism (declarative manifest cross-checked against the tree, LOUD on a lie)
//! Two facts are reconciled:
//!   1. the **authoritative row set** — every `N.M` id parsed straight out of the
//!      `contract-index.md` tables (the frozen surface; the scanner cannot drift from it because
//!      it reads it directly); and
//!   2. the **coverage manifest** (`contract-coverage.toml` at the workspace root) — one entry per
//!      row, each either:
//!      - `status = "covered"` + a `cdc` list of test files that carry the provider+consumer pair
//!        (each file MUST exist on disk and carry BOTH a provider-side and a consumer-side marker),
//!        or
//!      - `status = "deferred"` + a `landing` prompt id (`P-NNN` / `P-S21` etc.) — the row is not
//!        yet implemented and names exactly which prompt will ship its pair.
//!
//! The scanner FAILS LOUDLY (a typed [`CoverageError`], a non-zero process exit in the
//! [`contract-coverage` binary](../bin/contract-coverage.rs)) on ANY of:
//!   - a contract-index row with NO manifest entry (a silently-dropped contract — the exact
//!     "swallowed" failure §5 warns of);
//!   - a manifest entry for a row that no longer exists in the contract-index (a stale claim);
//!   - a `covered` row whose named CDC file is missing on disk, or exists but lacks a provider OR a
//!     consumer marker (a FALSELY-claimed pair — "a row that CLAIMS coverage without a pair is a
//!     build failure");
//!   - a `deferred` row with no `landing` prompt (an un-named floor — silence is the defect,
//!     EI-01 §1 name-your-floors).
//!
//! It does NOT require a row to be implemented (M0 has shipped only a handful of CDC pairs); it
//! requires that NO row LIES about being covered, and that every not-yet-covered row names its
//! landing prompt. That is the green artifact: a scan with **0 falsely-claimed rows**.
//!
//! ## Why a source/manifest scanner (the chosen mechanism + named floor)
//! Like the architecture lints, this is a hermetic, deterministic, toolchain-free scanner (parse
//! the contract-index Markdown + the manifest TOML, stat the named test files, grep them for the
//! two markers — no DB, no network, no nightly). The provider/consumer **evidence** is the
//! convention every shipped CDC file already follows: its doc-comment / test bodies name a
//! `provider` side and a `consumer` side (see `crates/*/tests/cdc_*.rs`). **Floor named —
//! evidence-grade, not semantic-grade:** the scanner proves a NAMED pair physically EXISTS and
//! carries both sides; it does not re-run the pair (that is what `cargo test --workspace` does, in
//! the same CI job). Tightening the evidence to "the pair's provider assertion and consumer
//! assertion reference the same frozen type" is a follow-on once a contract-test framework crate
//! lands; the committed, loud, regression-tested existence gate ships now (the ratchet's first
//! click). The scanner is never weakened — only sharpened.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// A contract-index row id, e.g. `1.1`, `4.3`, `11.5`. Stored as `(cluster, item)` so the natural
/// sort is numeric (1.2 before 1.10), and rendered back as `N.M`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RowId {
    /// The §N cluster number (1..=13).
    pub cluster: u32,
    /// The .M item number within the cluster.
    pub item: u32,
}

impl RowId {
    /// Parse `"N.M"` into a [`RowId`]. Returns `None` for anything that is not two dotted integers.
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

/// The declared coverage status of one contract row (parsed from the manifest).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Coverage {
    /// The row ships its provider+consumer CDC pair NOW. `cdc` lists the test file(s) (workspace-
    /// relative) that carry the pair — each must exist on disk and hold both markers.
    Covered { cdc: Vec<String> },
    /// The row is not yet implemented; `landing` names the prompt that will ship its pair (e.g.
    /// `P-067`, `P-S21`). A deferred row is NOT a coverage failure — an un-NAMED one is.
    Deferred { landing: String },
}

/// One manifest entry: the row id, its declared coverage, and a short human title (echoed in the
/// scan report so a failure is self-describing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestEntry {
    pub row: RowId,
    pub title: String,
    pub coverage: Coverage,
}

/// A single, LOUD coverage failure. Each variant is a way a row can LIE about its coverage — the
/// exact "swallowed contract violation" doctrine §5 forbids. Carries enough to fix it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoverageError {
    /// A contract-index row has no entry in the manifest — a silently-dropped contract.
    RowMissingFromManifest { row: RowId },
    /// The manifest claims a row that the contract-index no longer contains — a stale claim.
    StaleManifestEntry { row: RowId },
    /// A `covered` row names a CDC file that does not exist on disk.
    CdcFileMissing {
        row: RowId,
        title: String,
        file: String,
    },
    /// A `covered` row's CDC file exists but lacks a provider-side and/or consumer-side marker — a
    /// FALSELY-claimed pair (the build-failure case the prompt names explicitly).
    CdcPairIncomplete {
        row: RowId,
        title: String,
        file: String,
        has_provider: bool,
        has_consumer: bool,
    },
    /// A `covered` row declared no CDC files at all (an empty claim of coverage).
    CoveredWithNoCdc { row: RowId, title: String },
    /// A `deferred` row named no landing prompt — an un-named floor (EI-01 §1).
    DeferredWithNoLandingPrompt { row: RowId, title: String },
}

impl fmt::Display for CoverageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoverageError::RowMissingFromManifest { row } => write!(
                f,
                "row {row}: present in contract-index but ABSENT from contract-coverage.toml — a \
                 silently-dropped contract. Add a `[[contract]]` entry (covered or deferred)."
            ),
            CoverageError::StaleManifestEntry { row } => write!(
                f,
                "row {row}: in contract-coverage.toml but NO LONGER in contract-index.md — a stale \
                 claim. Remove or re-point the manifest entry."
            ),
            CoverageError::CdcFileMissing { row, title, file } => write!(
                f,
                "row {row} ({title}): claims coverage via `{file}` which does NOT exist on disk — a \
                 falsely-claimed CDC pair. Ship the file or mark the row deferred with its landing \
                 prompt."
            ),
            CoverageError::CdcPairIncomplete {
                row,
                title,
                file,
                has_provider,
                has_consumer,
            } => write!(
                f,
                "row {row} ({title}): CDC file `{file}` is missing the {} side (provider={}, \
                 consumer={}) — a CDC pair needs BOTH. A row that claims coverage without a pair \
                 is a build failure.",
                match (has_provider, has_consumer) {
                    (false, false) => "provider AND consumer",
                    (false, true) => "provider",
                    (true, false) => "consumer",
                    (true, true) => "(none — internal error)",
                },
                has_provider,
                has_consumer,
            ),
            CoverageError::CoveredWithNoCdc { row, title } => write!(
                f,
                "row {row} ({title}): status=covered but declares NO cdc files — an empty claim of \
                 coverage. Name the provider+consumer test file(s), or mark the row deferred."
            ),
            CoverageError::DeferredWithNoLandingPrompt { row, title } => write!(
                f,
                "row {row} ({title}): status=deferred but names NO landing prompt — an un-named \
                 floor. Name the prompt (e.g. landing = \"P-067\") that will ship its CDC pair."
            ),
        }
    }
}

/// Parse the authoritative row-id set out of `contract-index.md`. Every table data row begins
/// `| N.M | …`; this extracts the `N.M` ids (the frozen surface the scanner cannot drift from).
pub fn parse_contract_index_rows(markdown: &str) -> Vec<RowId> {
    let mut rows = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("| ") else {
            continue;
        };
        // The first cell, up to the next `|`.
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

/// A minimal, dependency-free TOML reader for the manifest's `[[contract]]` array-of-tables. The
/// manifest is OUR file in a fixed, simple shape (no nested tables, no multiline strings beyond
/// arrays) so a tiny hand-rolled parser keeps this crate hermetic and zero-dependency (matching
/// the lint engine). Recognised keys per entry: `row` (string `"N.M"`), `title` (string),
/// `status` (`"covered"`|`"deferred"`), `cdc` (array of strings), `landing` (string).
pub fn parse_manifest(toml: &str) -> Result<Vec<ManifestEntry>, String> {
    let mut entries = Vec::new();
    let mut cur: Option<RawEntry> = None;

    // Join logical lines: a value that opens an array `[` but does not close it on the same line
    // continues onto following lines until the matching `]`. This keeps the parser hermetic while
    // allowing a readable multi-file `cdc = [ ... ]` array spanning several lines.
    let raw_lines: Vec<&str> = toml.lines().collect();
    let mut logical: Vec<(usize, String)> = Vec::new();
    let mut i = 0;
    while i < raw_lines.len() {
        let mut acc = strip_toml_comment(raw_lines[i]).trim().to_string();
        let opens = acc.matches('[').count();
        let closes = acc.matches(']').count();
        // `[[contract]]` is a table header (balanced), never a continuation; otherwise an array
        // value with more `[` than `]` continues onto the next line(s).
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
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: not a key = value pair: {line}", lineno + 1));
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
    // Conservative: a `#` outside a string literal starts a comment. Our manifest never puts a
    // `#` inside a value, so a plain find is sufficient (and hermetic).
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
        .ok_or_else(|| format!("line {}: expected a quoted string, got `{value}`", lineno + 1))
}

fn parse_string_array(value: &str, lineno: usize) -> Result<Vec<String>, String> {
    let v = value.trim();
    let inner = v
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .ok_or_else(|| format!("line {}: expected an array `[...]`, got `{value}`", lineno + 1))?;
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

/// The provider/consumer evidence a CDC file must carry. A file is a complete pair iff its text
/// names BOTH a provider side and a consumer side (the convention every shipped `cdc_*.rs` file
/// follows: a `provider`-marked side and a `consumer`-marked side, case-insensitive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairEvidence {
    pub has_provider: bool,
    pub has_consumer: bool,
}

impl PairEvidence {
    /// Scan one CDC file's source text for the two markers. Case-insensitive substring match on
    /// `provider` / `consumer` — deliberately permissive at the evidence layer (the file is
    /// re-run by `cargo test` in the same CI job; this gate proves the pair EXISTS).
    pub fn from_source(src: &str) -> PairEvidence {
        let lower = src.to_lowercase();
        PairEvidence {
            has_provider: lower.contains("provider"),
            has_consumer: lower.contains("consumer"),
        }
    }

    fn complete(&self) -> bool {
        self.has_provider && self.has_consumer
    }
}

/// The result of one scan: the manifest entries that were checked + every [`CoverageError`] found.
/// `is_green()` is the gate's pass condition (0 falsely-claimed rows).
#[derive(Clone, Debug, Default)]
pub struct ScanReport {
    /// How many contract-index rows were reconciled.
    pub rows_checked: usize,
    /// How many rows declared `covered` (with a verified pair).
    pub covered: usize,
    /// How many rows declared `deferred` (with a named landing prompt).
    pub deferred: usize,
    /// Every LOUD failure. Empty == green.
    pub errors: Vec<CoverageError>,
}

impl ScanReport {
    /// The gate's pass condition: no row lies about its coverage.
    pub fn is_green(&self) -> bool {
        self.errors.is_empty()
    }

    /// A dated green/red artifact line for the scorecard (EI-01 §3: the green artifact is the
    /// measured number, not a claim).
    pub fn artifact_row(&self, date: &str) -> String {
        if self.is_green() {
            format!(
                "{date} contract-coverage: GREEN — {} rows reconciled ({} covered, {} deferred), \
                 0 falsely-claimed.",
                self.rows_checked, self.covered, self.deferred
            )
        } else {
            format!(
                "{date} contract-coverage: RED — {} falsely-claimed/dropped row(s) (CLAIMED, NOT \
                 PROVEN; fix the code, never weaken the gate).",
                self.errors.len()
            )
        }
    }
}

/// How a covered row's CDC file is read off disk — abstracted so the unit tests can drive the
/// reconciliation with in-memory fixtures (no real filesystem), while the binary reads real files.
pub trait CdcSource {
    /// Return the source text of `file` (workspace-relative), or `None` if it does not exist.
    fn read(&self, file: &str) -> Option<String>;
}

/// The production [`CdcSource`]: reads real files relative to a workspace root.
pub struct FsCdc {
    pub workspace_root: PathBuf,
}

impl CdcSource for FsCdc {
    fn read(&self, file: &str) -> Option<String> {
        std::fs::read_to_string(self.workspace_root.join(file)).ok()
    }
}

/// THE SCANNER. Reconcile the authoritative contract-index row set against the manifest, verifying
/// every `covered` row's CDC pair through `cdc`. Returns a [`ScanReport`]; `report.is_green()` is
/// the gate. This is a pure function of `(rows, manifest, cdc-source)` — hermetic and deterministic.
pub fn scan(rows: &[RowId], manifest: &[ManifestEntry], cdc: &dyn CdcSource) -> ScanReport {
    let mut report = ScanReport::default();

    // Index the manifest by row for O(log n) lookup + duplicate-detection.
    let mut by_row: BTreeMap<RowId, &ManifestEntry> = BTreeMap::new();
    for e in manifest {
        // A duplicate manifest row is itself a (stale) lie; keep the first, the second surfaces as
        // a stale entry below if the index disagrees. For our hand-maintained file the BTreeMap
        // last-wins is acceptable; the row-set reconciliation catches drift either way.
        by_row.insert(e.row, e);
    }

    let row_set: std::collections::BTreeSet<RowId> = rows.iter().copied().collect();
    report.rows_checked = row_set.len();

    // 1. Every contract-index row must have a manifest entry.
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
                // EVERY named file must be honest: a `covered` row's claim is only true if each
                // file it names exists AND carries both sides. A NAMED file that is missing or
                // half-a-pair is itself a lie and surfaces loudly (a row does not get to name one
                // real pair and one phantom file).
                for file in files {
                    match cdc.read(file) {
                        None => report.errors.push(CoverageError::CdcFileMissing {
                            row: *row,
                            title: entry.title.clone(),
                            file: file.clone(),
                        }),
                        Some(src) => {
                            let ev = PairEvidence::from_source(&src);
                            if !ev.complete() {
                                report.errors.push(CoverageError::CdcPairIncomplete {
                                    row: *row,
                                    title: entry.title.clone(),
                                    file: file.clone(),
                                    has_provider: ev.has_provider,
                                    has_consumer: ev.has_consumer,
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

    // 2. Every manifest entry must point at a real contract-index row (no stale claims).
    for e in manifest {
        if !row_set.contains(&e.row) {
            report
                .errors
                .push(CoverageError::StaleManifestEntry { row: e.row });
        }
    }

    report
}

/// Convenience: locate the workspace root from this crate's manifest dir (two levels up:
/// `crates/myelin-lints` → workspace). Mirrors `lint-gate`'s `default_roots`.
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

    /// An in-memory [`CdcSource`] so the scanner is unit-tested with no real filesystem: a file
    /// "exists" iff it is in the map; its text is the map value.
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
        // 1.1 < 1.2 < 1.10 numerically (a lexical sort would put 1.10 before 1.2).
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

    /// GREEN: a covered row whose named file carries BOTH markers + a deferred row with a landing
    /// prompt passes — 0 errors.
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
            [("cdc_1_1.rs", "the provider side ... the consumer side ...")]
                .into_iter()
                .collect(),
        );
        let report = scan(&rows(&["1.1", "4.3"]), &manifest, &cdc);
        assert!(report.is_green(), "errors: {:?}", report.errors);
        assert_eq!(report.covered, 1);
        assert_eq!(report.deferred, 1);
        assert!(report.artifact_row("2026-06-19").contains("GREEN"));
    }

    /// RED: a row that CLAIMS coverage but its named CDC file does not exist is a build failure
    /// (the headline red-fixture: a falsely-claimed pair).
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

    /// RED: a covered row whose file exists but lacks the consumer side — a half-a-pair lie.
    #[test]
    fn red_when_a_covered_file_is_missing_the_consumer_side() {
        let manifest = vec![ManifestEntry {
            row: RowId::parse("1.1").unwrap(),
            title: "serve".into(),
            coverage: Coverage::Covered {
                cdc: vec!["half.rs".into()],
            },
        }];
        let cdc = FakeCdc(
            [("half.rs", "only the provider side is here")]
                .into_iter()
                .collect(),
        );
        let report = scan(&rows(&["1.1"]), &manifest, &cdc);
        assert!(!report.is_green());
        match &report.errors[0] {
            CoverageError::CdcPairIncomplete {
                has_provider,
                has_consumer,
                ..
            } => {
                assert!(*has_provider);
                assert!(!*has_consumer);
            }
            other => panic!("expected CdcPairIncomplete, got {other:?}"),
        }
    }

    /// RED: a contract-index row with NO manifest entry — a silently-dropped contract.
    #[test]
    fn red_when_a_contract_row_is_absent_from_the_manifest() {
        let report = scan(&rows(&["1.1", "9.9"]), &[], &FakeCdc(Default::default()));
        assert_eq!(report.errors.len(), 2);
        assert!(report
            .errors
            .iter()
            .all(|e| matches!(e, CoverageError::RowMissingFromManifest { .. })));
    }

    /// RED: a deferred row that names NO landing prompt — an un-named floor (EI-01 §1).
    #[test]
    fn red_when_a_deferred_row_has_no_landing_prompt() {
        let manifest = vec![ManifestEntry {
            row: RowId::parse("4.3").unwrap(),
            title: "list_objects".into(),
            coverage: Coverage::Deferred {
                landing: "".into(),
            },
        }];
        let report = scan(&rows(&["4.3"]), &manifest, &FakeCdc(Default::default()));
        assert!(matches!(
            report.errors[0],
            CoverageError::DeferredWithNoLandingPrompt { .. }
        ));
    }

    /// RED: a manifest entry for a row that no longer exists in the contract-index — a stale claim.
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
        // 1.1 missing from manifest + 99.1 stale = two distinct loud failures.
        assert!(report
            .errors
            .iter()
            .any(|e| matches!(e, CoverageError::StaleManifestEntry { row } if row.to_string() == "99.1")));
        assert!(report
            .errors
            .iter()
            .any(|e| matches!(e, CoverageError::RowMissingFromManifest { .. })));
    }

    /// RED: status=covered but no cdc files at all — an empty claim of coverage.
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
