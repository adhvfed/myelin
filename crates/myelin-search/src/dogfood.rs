//! # `dogfood` — Search over Myelin's OWN work + the self-hosting CI graph (SRCH-P33 / P-515, M6)
//!
//! **The Search M6 dogfood prompt.** S-M6 promotes NOTHING and freezes NO new contract — the engine is
//! fixed at M2 and hardened through M5. This prompt *exercises* the production-hardened Search engine on
//! **real (self-)tenant data**: the platform's own development. The cheapest, most honest load generator
//! is the team's own work (search-and-indexing §3 — production-hardened before real self-tenant data;
//! EI-01 §5 — the ratchet runs on the builders' own work), and the moat thesis (refined arch §1) is only
//! real once Search runs over Myelin's own commits: code search on the Myelin monorepo, search over its
//! own Knowledge space (the roadmap/gap-report/scorecard docs), its own issues, its own chat.
//!
//! ## What this module IS (the dogfood DRIVER over the EXISTING engine — EI-01 §7)
//! This is a **caller that drives the already-shipped Search surface over the Myelin self-tenant** —
//! never a second query / reindex / erase / E2E. It REUSES:
//! - [`crate::run_e2e_1_pr_pane`] — the PR context pane (the SAME [`crate::pipeline::query`] /
//!   [`crate::pipeline::semantic`] permission-aware pre-filter, SRCH-P08/P09/P11), reframed as **code +
//!   issue search on the Myelin monorepo**: a hit on a confidential issue NEVER enters the candidate set
//!   for a denied viewer (0 doc/count/IDF/RAG leak), and the in-context unfurl tombstones (0 title leak).
//! - [`crate::run_e2e_3_spec_to_ship`] — the spec-to-ship reindex-from-source parity (the SAME
//!   [`crate::reindex::SearchReindexer`], SRCH-P16/P28), reframed onto Myelin's **roadmap / gap-report /
//!   scorecard living as a Myelin Knowledge space** (the searchable corpus rebuilds byte-for-byte).
//! - [`crate::run_e2e_4_dsar_fanout`] — the DSAR fan-out structural erase (the SAME
//!   [`crate::hyok_scale::BackupScaleEraseGate`] over [`crate::erase::SearchEraseHolder`], SRCH-P15/P29),
//!   reframed onto a Myelin team member's own data: Search's docs + EMBEDDINGS return 0 recoverable PII
//!   incl. vectors incl. backups, and the holder-coverage receipt INCLUDES Search (H7).
//!
//! ## What this module wires (the dogfood loop is live)
//! - **The Search drills run as Myelin CI jobs on Myelin's own commits** — wired into the frozen
//!   `myelin_harness::self_hosting_ci::self_hosting_jobs` graph (the SRCH-P33 band; see the harness
//!   module). The dogfood loop is live: the Search E2E wedge + the truth-up pass run on every Myelin
//!   commit.
//! - **The truth-up pass** ([`SearchTruthUpPass`] over [`proven_search_rows`]) — every PROVEN Search row
//!   (SRCH-D1..SRCH-D10 + the E2E legs E2E-1/E2E-3/E2E-4) rests on a DATED green artifact whose proof
//!   SOURCE exists on disk; no earlier-band Search gate is red. A row that names a vanished artifact is
//!   surfaced LOUDLY, never trusted on faith (EI-01 §1, code-wins-over-docs).
//! - **The every-incident-adds-a-drill loop** ([`SearchIncident`]) — a Search incident files a PII-free
//!   Myelin issue draft AND a reproducing-drill ticket; the integration drill registers the repro into
//!   the harness `DrillRegistry` (the T-3 `register_drill` hook) so it re-runs forever.
//!
//! ## The switch test (split into [`crate::switch_test`])
//! The Search contribution to the per-subsystem SWITCH TESTS (code-by-symbol / doc-by-content /
//! issue-by-facet found within the latency budget, driven over the real surface — EI-01 §4) lives in the
//! sibling [`crate::switch_test`] module. The dogfood band wires it as a Myelin CI job too.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **None new** — M6 promotes nothing; it exercises the production-hardened Search on real self-tenant
//!   data. The ONE legitimate remaining floor is the world-scale 30× fleet-hardware load drill (the CI
//!   variant runs a MODERATE corpus, not the world-scale fleet corpus — already named SRCH-P25/P29).
//! - **The real EU-hostable embedding-model adapter swap** remains the post-M5/runtime follow-on: M6 runs
//!   on the [`crate::indexer::MockEmbeddingAdapter`] (the strategy-pattern mock, VISION §3), not a real
//!   embedding service. Recorded honestly here ([`EMBEDDING_ADAPTER_POSTURE`]) — the swap is a config/
//!   implementation change, not a rewrite.
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/search-and-indexing.md`
//! §3 (the honest progression — production-hardened before real self-tenant data), §7 (the nine drills
//! run as Myelin CI jobs). **Roadmap:** `planning/06-roadmaps/shared/search-and-indexing.md` §2 S-M6.
//! **Doctrine:** `external-insights/01-process-and-quality-doctrine.md` §1 (code-wins-over-docs — the
//! truth-up pass), §3/§4 (prove it / drive the whole thing), §5 (the ratchet runs on the builders' own
//! work). **VISION §3** (the switch test) / **§5** (dogfooding).

use crate::e2e_wedge::E2eArtifact;
#[cfg(any(test, feature = "test-support"))]
use crate::{run_e2e_1_pr_pane, run_e2e_3_spec_to_ship, run_e2e_4_dsar_fanout};

/// The Myelin self-tenant id (the platform self-hosts as exactly one cell — P-508 / CP-M6). Opaque,
/// PII-free — the dogfood Search runs over the platform's OWN work under this tenant.
pub const MYELIN_SELF_TENANT: &str = "myelin";

/// The region the Myelin self-tenant is pinned to (fr-par — the dev/prod residency pin; a config swap,
/// never a code change). The dogfood Search resolves cell-local in this region.
pub const MYELIN_SELF_REGION: &str = "fr-par";

/// **The embedding-adapter posture, recorded HONESTLY (the prompt's required note, EI-01 §1).** M6 runs
/// on the [`crate::indexer::MockEmbeddingAdapter`] — a deterministic strategy-pattern mock (VISION §3:
/// mock implementations during development; the real EU-hostable embedding-model adapter is a config
/// swap, never a rewrite). The real adapter swap is the named post-M5/runtime follow-on. We RECORD this
/// rather than claim a real-embedding green we did not earn.
pub const EMBEDDING_ADAPTER_POSTURE: &str =
    "mock (MockEmbeddingAdapter; real EU-hostable embedding \
                                             adapter is the named post-M5/runtime config swap)";

// ───────────────────────────── (1) Search over Myelin's own work ─────────────────────────────

/// **The named green artifact the Search dogfood run emits.** The production-hardened Search engine
/// driven over Myelin's OWN work, across the three production faces:
/// - **code + issue search** on the Myelin monorepo (the PR context pane — per-viewer leak-free hits,
///   the confidential issue tombstones, 0 doc/count/IDF/RAG leak) — REUSES [`run_e2e_1_pr_pane`];
/// - **search over Myelin's own Knowledge space** (the roadmap/gap-report/scorecard docs living as a
///   Knowledge space — the searchable corpus reindexes byte-for-byte from source) — REUSES
///   [`run_e2e_3_spec_to_ship`];
/// - **the DSAR fan-out** over a Myelin team member's own data (Search's docs + EMBEDDINGS return 0
///   recoverable PII incl. vectors incl. backups; the holder-coverage receipt includes Search H7) —
///   REUSES [`run_e2e_4_dsar_fanout`].
///
/// Search is GREEN on the platform's own work iff every face is green AND 0 leak across the three. A
/// face that did not reach green fails LOUDLY ([`DogfoodArtifact::is_green`] is false) — never a
/// claimed-but-unearned green.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the dogfood artifact must be checked — an unread RED face silently claims a green Search \
              did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct DogfoodArtifact {
    /// The date the dogfood run was asserted (every face is dated at this run).
    pub date: String,
    /// The code + issue search face (the Myelin monorepo's code + the per-viewer leak-free issue hit).
    pub code_and_issue: E2eArtifact,
    /// The Knowledge-space search face (Myelin's roadmap/scorecard as a Knowledge space, reindex-parity).
    pub knowledge_space: E2eArtifact,
    /// The DSAR fan-out face (a Myelin team member's own data, 0 recoverable PII incl. embeddings/backups).
    pub dsar_fanout: E2eArtifact,
}

impl DogfoodArtifact {
    /// `true` iff Search is GREEN on Myelin's own work — every face green AND 0 leak. The ONLY way to
    /// read the dogfood run (a RED face is never silently a pass).
    pub fn is_green(&self) -> bool {
        self.code_and_issue.is_green()
            && self.knowledge_space.is_green()
            && self.dsar_fanout.is_green()
    }

    /// The total leak counter across the three faces (the F1 leak spine — must be 0).
    pub fn total_leaks(&self) -> u64 {
        self.code_and_issue.leaks + self.knowledge_space.leaks + self.dsar_fanout.leaks
    }

    /// The dated one-line summary (the artifact body the dogfood CI run prints).
    pub fn summary(&self) -> String {
        format!(
            "P-515 SEARCH DOGFOOD {} — tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             code+issue={} knowledge-space={} dsar-fanout={} total-leaks={} embedding-adapter={} \
             verdict={}",
            self.date,
            self.code_and_issue.is_green(),
            self.knowledge_space.is_green(),
            self.dsar_fanout.is_green(),
            self.total_leaks(),
            EMBEDDING_ADAPTER_POSTURE,
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

/// **Run Search over Myelin's OWN work (SRCH-P33).** Drives the production Search surface across the
/// three faces (code + issue search, Knowledge-space reindex-parity, the DSAR fan-out) on the Myelin
/// self-tenant, REUSING the existing E2E-wedge runners (the SAME permission-aware query pre-filter /
/// reindex-from-source / structural-erase engine — EI-01 §7, never a second implementation). `date` is
/// the run stamp.
/// **MR-009b Wave 5 — `test-support`-gated:** this in-process drill constructs the in-memory
/// `KmsEngine` test double (the production engine is the durable `kms_durable::load_or_generate`);
/// its consumers (the tests-dir wedge/dogfood drills) reach it via the `test-support` feature.
#[cfg(any(test, feature = "test-support"))]
pub fn run_search_over_myelins_own_work(date: &str) -> DogfoodArtifact {
    DogfoodArtifact {
        date: date.to_string(),
        // Code + issue search on the Myelin monorepo (per-viewer leak-free hits; the confidential
        // issue tombstones, 0 doc/count/IDF/RAG leak).
        code_and_issue: run_e2e_1_pr_pane(),
        // Search over Myelin's own Knowledge space (the roadmap/gap-report/scorecard as a Knowledge
        // space — the searchable corpus reindexes byte-for-byte from source).
        knowledge_space: run_e2e_3_spec_to_ship(),
        // The DSAR fan-out over a Myelin team member's own data (0 recoverable PII incl. embeddings
        // incl. backups; the holder-coverage receipt includes Search H7).
        dsar_fanout: run_e2e_4_dsar_fanout(),
    }
}

// ───────────────────────────── (2) the truth-up pass over the PROVEN Search rows ─────────────────────────────

/// One PROVEN Search row the truth-up pass enumerates. A gate/drill the ledger claims PROVEN, with the
/// proof command that emits its dated green artifact AND the repo-relative path to that proof source. The
/// truth-up pass asserts EACH row rests on a DATED green artifact whose source EXISTS on disk — a row
/// that names a vanished artifact is surfaced, never trusted on faith (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenSearchRow {
    /// The stable gate/drill id (e.g. `"SRCH-D1"`, `"E2E-1"`).
    pub id: &'static str,
    /// The contract SECTION the row's gate belongs to (the §x.y / drill face of the search-and-indexing
    /// doc — e.g. `"4.2"` the pre-filter, `"4.8"` erase, `"4.9"` reindex). The scorecard groups by
    /// section so the coverage is visible at a glance.
    pub section: &'static str,
    /// A one-line human title (what the row proves).
    pub title: &'static str,
    /// The proof command that emits this row's dated green artifact (the `cargo test` target that lives
    /// with the feature prompt — named so the artifact is reproducible).
    pub proof_command: &'static str,
    /// The repo-RELATIVE path to the proof source (the test file the `proof_command` runs). The truth-up
    /// pass asserts this file EXISTS on disk — a row that names a vanished artifact is surfaced as
    /// CLAIMED-NOT-PROVEN, never swallowed (EI-01 §1).
    pub artifact_path: &'static str,
    /// The DATE the row's green artifact was last emitted, if any. `Some(date)` ⇒ dated + proven;
    /// `None` ⇒ CLAIMED-NOT-PROVEN (recorded honestly with a date, surfaced as a loud red).
    pub artifact_date: Option<String>,
}

impl ProvenSearchRow {
    /// `true` iff this row rests on a dated green artifact (the per-row truth-up invariant).
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    /// Resolve this row's [`artifact_path`](Self::artifact_path) to an absolute path under `repo_root` so
    /// a caller can assert the proof source exists on disk.
    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

/// **The FROZEN set of PROVEN Search rows the truth-up pass enumerates (SRCH-P33).** Every Search gate
/// the ledger claims PROVEN: the ten engine/M-spanning drills **SRCH-D1..SRCH-D10** (the leak/stale/
/// cross-tenant invariants + the M5 world-scale family — surge / freshness / filtered-ANN / restore-
/// re-erase / HYOK-backup-erasure) **plus** the whole-system E2E legs **E2E-1 / E2E-3 / E2E-4**
/// (SRCH-P32). The truth-up pass asserts EVERY id here rests on a dated green artifact whose proof source
/// exists on disk; a row without one is a loud failure. `date` is supplied by the runner (the dogfood
/// run's `today_iso()`) so a claim never outlives its verification (EI-01 §1).
pub fn proven_search_rows(date: &str) -> Vec<ProvenSearchRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenSearchRow {
        ProvenSearchRow {
            id,
            section,
            title,
            proof_command: cmd,
            artifact_path,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        // ── The engine drills (SRCH-D1..SRCH-D3) — the leak-free read invariants. ──
        row(
            "SRCH-D1",
            "4.2",
            "the zero-escape leak — a hidden doc NEVER enters the candidate set (0 doc/count/IDF/RAG leak; the §4.2 pre-filter, not a post-filter)",
            "cargo test -p myelin-search --test drill_srch_d1_zero_escape_leak",
            "crates/myelin-search/tests/drill_srch_d1_zero_escape_leak.rs",
            date,
        ),
        row(
            "SRCH-D2",
            "4.10",
            "the no-stale-grant invariant — a revoked grant never serves a stale hit (the zookie/consistency floor admits at-or-after the baseline snapshot)",
            "cargo test -p myelin-search --test drill_srch_d2_no_stale_grant",
            "crates/myelin-search/tests/drill_srch_d2_no_stale_grant.rs",
            date,
        ),
        row(
            "SRCH-D3",
            "1.1",
            "the cross-tenant isolation — a query is (tenant, region)-keyed; no doc from another tenant ever enters a candidate set",
            "cargo test -p myelin-search --test drill_srch_d3_cross_tenant",
            "crates/myelin-search/tests/drill_srch_d3_cross_tenant.rs",
            date,
        ),
        // ── The erasure + reindex family (SRCH-D4..SRCH-D5). ──
        row(
            "SRCH-D4",
            "4.8",
            "the erasure — erase = purge+reindex, not hide; the docs are GONE from FT + k-NN (0 recoverable incl. vectors)",
            "cargo test -p myelin-search --test drill_srch_d4_erasure",
            "crates/myelin-search/tests/drill_srch_d4_erasure.rs",
            date,
        ),
        row(
            "SRCH-D5",
            "4.9",
            "reindex-from-source — the only rebuild path; the wiped index reindexes to byte-match live (the reindex-parity hash)",
            "cargo test -p myelin-search --test cdc_6_4_reindex",
            "crates/myelin-search/tests/cdc_6_4_reindex.rs",
            date,
        ),
        // ── The M5 world-scale family (SRCH-D6..SRCH-D10). ──
        row(
            "SRCH-D6",
            "4.11",
            "the 30× search surge — DRR-fair shed within the tuned budget, the human lane holds, the filtered-ANN follow-on named",
            "cargo test -p myelin-search --test drill_srch_d6_surge",
            "crates/myelin-search/tests/drill_srch_d6_surge.rs",
            date,
        ),
        row(
            "SRCH-D7",
            "4.6",
            "freshness at scale — the event→searchable p99 within budget (the projection feeder keeps the index fresh under load)",
            "cargo test -p myelin-search --test drill_srch_d7_freshness_at_scale",
            "crates/myelin-search/tests/drill_srch_d7_freshness_at_scale.rs",
            date,
        ),
        row(
            "SRCH-D8",
            "4.5",
            "filtered-ANN recall — the permission-aware filter-during-traversal returns k VISIBLE neighbours within the recall floor",
            "cargo test -p myelin-search --test drill_srch_d8_filtered_ann_recall",
            "crates/myelin-search/tests/drill_srch_d8_filtered_ann_recall.rs",
            date,
        ),
        row(
            "SRCH-D9",
            "4.8",
            "restore + re-erase — a restore-from-backup re-applies every erasure (an erased subject stays erased after a restore, 0 recoverable)",
            "cargo test -p myelin-search --test drill_srch_d9_restore_reerase",
            "crates/myelin-search/tests/drill_srch_d9_restore_reerase.rs",
            date,
        ),
        row(
            "SRCH-D10",
            "7.5",
            "HYOK + backup-scale erasure — the tenant-decommission crypto-shred destroys the per-tenant index DEK; every sealed backup segment is plaintext-unrecoverable (0 recoverable incl. vectors incl. backups)",
            "cargo test -p myelin-search --test drill_srch_d10_hyok_and_backup_erasure",
            "crates/myelin-search/tests/drill_srch_d10_hyok_and_backup_erasure.rs",
            date,
        ),
        // ── The whole-system E2E wedge legs (E2E-1 / E2E-3 / E2E-4 — SRCH-P32). ──
        row(
            "E2E-1",
            "4.2",
            "the PR context pane — a denied viewer's hit on a confidential issue NEVER enters the candidate set (0 leak); mid-flight ci.check.updated live-update; the unfurl tombstones (0 title leak)",
            "cargo test -p myelin-search --test e2e_wedge_srch_p32",
            "crates/myelin-search/tests/e2e_wedge_srch_p32.rs",
            date,
        ),
        row(
            "E2E-3",
            "4.9",
            "spec-to-ship reindex-parity — wipe → reindex-from-source → byte-match live; the restore-verify gate confirms cold==live",
            "cargo test -p myelin-search --test e2e_wedge_srch_p32",
            "crates/myelin-search/tests/e2e_wedge_srch_p32.rs",
            date,
        ),
        row(
            "E2E-4",
            "4.8",
            "the DSAR fan-out — Search's docs + EMBEDDINGS return 0 recoverable PII incl. backups; the holder-coverage receipt includes Search H7",
            "cargo test -p myelin-search --test e2e_wedge_srch_p32",
            "crates/myelin-search/tests/e2e_wedge_srch_p32.rs",
            date,
        ),
    ]
}

/// The verdict of the Search truth-up pass — Green (every PROVEN row dated) or Red (the undated rows
/// named). Never a swallowed bool — a RED points at exactly which Search claim outran its verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchTruthUpVerdict {
    /// Every enumerated PROVEN Search row rests on a dated green artifact (no earlier-band Search gate red).
    Green {
        /// How many PROVEN rows were confirmed dated + green.
        rows_confirmed: usize,
        /// The date the truth-up pass ran.
        date: String,
    },
    /// One or more PROVEN rows are CLAIMED-NOT-PROVEN. Names them so the failure is specific.
    Red {
        /// The ids of the rows lacking a dated green artifact (the loud failure list).
        undated_rows: Vec<&'static str>,
    },
}

impl SearchTruthUpVerdict {
    /// `true` iff the truth-up pass is green (every PROVEN row dated). The ONLY way to read a pass.
    pub fn is_green(&self) -> bool {
        matches!(self, SearchTruthUpVerdict::Green { .. })
    }

    /// The ids of any CLAIMED-NOT-PROVEN rows (empty on a green pass).
    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            SearchTruthUpVerdict::Green { .. } => &[],
            SearchTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

/// **The Search truth-up pass (SRCH-P33 / EI-01 §1).** Enumerates every PROVEN Search row and confirms
/// each rests on a DATED green artifact. A row WITHOUT one is a LOUD failure
/// ([`SearchTruthUpVerdict::Red`]), never a silent pass — the code-wins-over-docs discipline made
/// mechanical. A zero-sized orchestrator.
#[derive(Clone, Copy, Debug, Default)]
pub struct SearchTruthUpPass;

impl SearchTruthUpPass {
    /// A new truth-up pass (stateless).
    pub fn new() -> SearchTruthUpPass {
        SearchTruthUpPass
    }

    /// **Run the truth-up pass over `rows`.** Returns [`SearchTruthUpVerdict::Green`] (every row dated) or
    /// [`SearchTruthUpVerdict::Red`] (the undated rows named). `date` stamps the green verdict.
    pub fn run(&self, rows: &[ProvenSearchRow], date: &str) -> SearchTruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            SearchTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            SearchTruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    /// **The loud-never-swallowed truth-up CI entrypoint (EI-01 §5).** Run the pass and turn a RED verdict
    /// into a process-failing `Err` — so a CI invocation `pass.run_or_fail_ci(&rows, date)?` FAILS the
    /// dogfood truth-up job if ANY PROVEN Search row lacks a dated green artifact. On GREEN it returns the
    /// number of confirmed rows.
    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenSearchRow],
        date: &str,
    ) -> Result<usize, SearchTruthUpRed> {
        match self.run(rows, date) {
            SearchTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            SearchTruthUpVerdict::Red { undated_rows } => Err(SearchTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

/// A RED truth-up pass surfaced as an `Err` — the CLAIMED-NOT-PROVEN Search rows, loud + specific (the
/// process exits non-zero, never a silent docs drift).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchTruthUpRed {
    /// The ids of the rows lacking a dated green artifact.
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for SearchTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL — {} Search row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} — a claim \
             that outlives its verification misleads the next agent (EI-01 §1); fix the doc or re-run \
             the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for SearchTruthUpRed {}

// ───────────────────────────── the enumerated truth-up scorecard (the green artifact) ─────────────────────────────

/// How a PROVEN Search row's proof stands at truth-up time: a dated green artifact, or an
/// honestly-recorded CLAIMED-NOT-PROVEN note. Either way the status carries a DATE (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchRowStatus {
    /// The row rests on a dated green artifact whose proof source exists on disk.
    DatedGreen {
        /// The date the green artifact was last emitted.
        date: String,
    },
    /// The row is CLAIMED but NOT PROVEN — no dated green artifact, or its proof source is gone.
    ClaimedNotProven {
        /// The date the truth-up pass recorded the gap.
        date: String,
        /// Why the row is not proven (no artifact date, or the proof source is missing on disk).
        reason: String,
    },
}

impl SearchRowStatus {
    /// `true` iff this is a dated green artifact (the per-row truth-up invariant).
    pub fn is_dated_green(&self) -> bool {
        matches!(self, SearchRowStatus::DatedGreen { .. })
    }
}

/// One scorecard line: a PROVEN Search row resolved to its [`SearchRowStatus`] at truth-up time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchScorecardEntry {
    /// The row this line scores.
    pub row: ProvenSearchRow,
    /// Its resolved status (dated-green or claimed-not-proven, both dated).
    pub status: SearchRowStatus,
}

/// **The enumerated Search truth-up scorecard (the GATE/DRILLS green artifact, SRCH-P33).** Every PROVEN
/// Search row → its dated green artifact (or a dated CLAIMED-NOT-PROVEN note). The scorecard itself is the
/// closing-honesty-pass artifact: rendering it produces the section-grouped table the prompt's GATE
/// demands, and [`Self::is_green`] is true iff NO earlier-band Search gate is red.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked — an unread CLAIMED-NOT-PROVEN row silently drifts \
              the docs from the code (EI-01 §1)"]
pub struct SearchTruthUpScorecard {
    /// The run date the scorecard is stamped with.
    pub date: String,
    /// One entry per PROVEN Search row, in section order.
    pub entries: Vec<SearchScorecardEntry>,
}

impl SearchTruthUpScorecard {
    /// `true` iff every row rests on a dated green artifact (the gate invariant: no Search gate red).
    pub fn is_green(&self) -> bool {
        self.entries.iter().all(|e| e.status.is_dated_green())
    }

    /// How many rows the scorecard enumerates.
    pub fn rows_total(&self) -> usize {
        self.entries.len()
    }

    /// How many rows rest on a dated green artifact.
    pub fn rows_dated_green(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status.is_dated_green())
            .count()
    }

    /// The ids of any CLAIMED-NOT-PROVEN rows (empty on a green pass) — the loud failure list.
    pub fn claimed_not_proven(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| !e.status.is_dated_green())
            .map(|e| e.row.id)
            .collect()
    }

    /// **Render the enumerated scorecard as the dated green artifact** (the section-grouped table a
    /// truth-up CI run prints). CLAIMED-NOT-PROVEN rows are rendered LOUD, never elided.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let verdict = if self.is_green() {
            "GREEN (no earlier-band Search gate red)"
        } else {
            "RED (a Search claim outran its verification)"
        };
        out.push_str(&format!(
            "P-515 SEARCH TRUTH-UP SCORECARD {} — {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                SearchRowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                SearchRowStatus::ClaimedNotProven { date, reason } => {
                    format!("CLAIMED-NOT-PROVEN({date}: {reason})")
                }
            };
            out.push_str(&format!(
                "  [§{}] {:<10} {:<28} — {}  ⟨{}⟩\n",
                e.row.section, e.row.id, status, e.row.title, e.row.proof_command,
            ));
        }
        out
    }
}

/// **Run the Search truth-up pass and produce the enumerated [`SearchTruthUpScorecard`] (SRCH-P33).** For
/// each PROVEN Search row this resolves a dated [`SearchRowStatus`]: a row is DATED-GREEN iff it carries
/// an `artifact_date` AND its proof source exists on disk under `repo_root`; otherwise it is recorded
/// CLAIMED-NOT-PROVEN with the run `date` and the honest reason. The scorecard surfaces — never swallows —
/// any gap (EI-01 §1). `repo_root` is the workspace root the `artifact_path`s are relative to.
pub fn run_search_truth_up_scorecard(
    date: &str,
    repo_root: &std::path::Path,
) -> SearchTruthUpScorecard {
    let entries = proven_search_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => SearchRowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    SearchRowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => SearchRowStatus::DatedGreen { date: d.clone() },
            };
            SearchScorecardEntry { row, status }
        })
        .collect();
    SearchTruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

// ───────────────────────────── (3) the every-incident-adds-a-drill loop ─────────────────────────────

/// **A Search incident on Myelin's own development (the every-incident-adds-a-drill loop, EI-01 §3/§5).**
/// A real incident ends by filing a PII-free Myelin issue draft AND a reproducing-drill ticket — both
/// reference-linked (the moat thesis: the issue points at the drill that reproduces it). The integration
/// drill registers the repro into the harness `DrillRegistry` so it re-runs forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchIncident {
    /// The incident id (PII-free, e.g. `"INC-SEARCH-DOGFOOD-1"`).
    pub incident_id: String,
    /// The Search gate the incident regressed (e.g. `"SRCH-D1"`).
    pub gate_id: String,
    /// A PII-free one-line description of what broke.
    pub description: String,
    /// The name of the reproducing drill the incident files (the test that re-fires the failure).
    pub repro_drill_name: String,
}

impl SearchIncident {
    /// A new Search incident (every field PII-free).
    pub fn new(
        incident_id: &str,
        gate_id: &str,
        description: &str,
        repro_drill_name: &str,
    ) -> SearchIncident {
        SearchIncident {
            incident_id: incident_id.into(),
            gate_id: gate_id.into(),
            description: description.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    /// The PII-free Myelin issue draft the incident files (names the gate + the repro drill — the issue is
    /// reference-linked to the drift that reproduces it).
    pub fn issue_draft(&self) -> SearchIncidentIssueDraft {
        SearchIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!(
                "[{}] Search gate {} regressed",
                self.incident_id, self.gate_id
            ),
            body: format!(
                "Search incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked — every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    /// The reproducing-drill ticket the incident files (the test that joins the permanent suite).
    pub fn drill_ticket(&self) -> SearchIncidentDrillTicket {
        SearchIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

/// The PII-free Myelin issue draft a [`SearchIncident`] files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchIncidentIssueDraft {
    /// The Search gate the issue is about.
    pub gate_id: String,
    /// The issue title (PII-free).
    pub title: String,
    /// The issue body (PII-free; names the repro drill).
    pub body: String,
}

/// The reproducing-drill ticket a [`SearchIncident`] files (the drill that joins the permanent suite).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchIncidentDrillTicket {
    /// The drill name (the test that re-fires the failure).
    pub drill_name: String,
    /// The gate the drill guards.
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    /// **THE HEADLINE: Search is GREEN on Myelin's OWN work.** Code + issue search, the Knowledge-space
    /// reindex-parity, and the DSAR fan-out all green over the Myelin self-tenant, 0 leak.
    #[test]
    fn search_greens_on_myelins_own_work() {
        let artifact = run_search_over_myelins_own_work(RUN_DATE);

        assert!(
            artifact.is_green(),
            "Search must be green on Myelin's own work: {}",
            artifact.summary()
        );
        assert_eq!(
            artifact.total_leaks(),
            0,
            "0 leak across the three faces: {}",
            artifact.summary()
        );
        assert!(
            artifact.code_and_issue.is_green(),
            "code + issue search is green"
        );
        assert!(
            artifact.knowledge_space.is_green(),
            "the Knowledge-space reindex-parity is green"
        );
        assert!(artifact.dsar_fanout.is_green(), "the DSAR fan-out is green");

        let s = artifact.summary();
        assert!(s.contains("P-515 SEARCH DOGFOOD 2026-06-26"), "dated: {s}");
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
        // The embedding-adapter posture is recorded honestly (mock, named follow-on).
        assert!(
            s.contains("embedding-adapter=mock"),
            "the embedding-adapter posture is recorded honestly (mock): {s}"
        );
    }

    /// The truth-up pass is GREEN — every PROVEN Search row rests on a dated green artifact.
    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_search_rows(RUN_DATE);
        assert!(
            rows.len() >= 13,
            "the PROVEN set covers SRCH-D1..SRCH-D10 + the E2E legs"
        );
        let confirmed = SearchTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red earlier-band Search gates — every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

    /// A claimed-not-proven row reds the truth-up pass LOUDLY (surfaced, never swallowed).
    #[test]
    fn a_claimed_not_proven_row_reds_the_truth_up_pass() {
        let mut rows = proven_search_rows(RUN_DATE);
        rows[0].artifact_date = None; // simulate a doc claim with no dated green artifact
        let verdict = SearchTruthUpPass::new().run(&rows, RUN_DATE);
        assert!(!verdict.is_green(), "an undated row reds the pass");
        assert_eq!(verdict.undated_rows(), &[rows[0].id]);
        let err = SearchTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect_err("a claimed-not-proven row fails the CI entrypoint");
        assert!(err.to_string().contains("CLAIMED-NOT-PROVEN"));
    }

    /// The enumerated scorecard renders GREEN with every PROVEN row dated + its proof source on disk.
    #[test]
    fn the_scorecard_renders_green_with_proof_sources_on_disk() {
        // The workspace root (two levels up from this crate's manifest dir).
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let scorecard = run_search_truth_up_scorecard(RUN_DATE, &repo_root);
        assert!(
            scorecard.is_green(),
            "the scorecard must be green — every PROVEN Search row dated + its proof source on disk; \
             claimed-not-proven: {:?}",
            scorecard.claimed_not_proven()
        );
        assert_eq!(scorecard.rows_dated_green(), scorecard.rows_total());
        let md = scorecard.render();
        assert!(md.contains("verdict=GREEN"), "rendered: {md}");
        assert!(
            md.contains("SRCH-D1") && md.contains("E2E-4"),
            "enumerated: {md}"
        );
    }

    /// A row whose proof source is missing on disk is surfaced CLAIMED-NOT-PROVEN (never trusted on faith).
    #[test]
    fn a_vanished_proof_source_is_surfaced_not_trusted() {
        // Point the scorecard at a bogus root so no proof source exists — every dated row flips to
        // CLAIMED-NOT-PROVEN with the "proof source missing on disk" reason (the artifact-exists check).
        let bogus_root = std::path::Path::new("/nonexistent-search-truth-up-root");
        let scorecard = run_search_truth_up_scorecard(RUN_DATE, bogus_root);
        assert!(
            !scorecard.is_green(),
            "a vanished proof source must red the scorecard"
        );
        assert!(
            scorecard
                .entries
                .iter()
                .all(|e| matches!(&e.status, SearchRowStatus::ClaimedNotProven { reason, .. } if reason.contains("missing on disk"))),
            "every row is surfaced as proof-source-missing, never trusted on faith"
        );
    }

    /// The every-incident loop: an incident files a PII-free issue draft + a reproducing-drill ticket.
    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = SearchIncident::new(
            "INC-SEARCH-DOGFOOD-1",
            "SRCH-D1",
            "a pre-filter regression let a confidential issue enter the candidate set on the Myelin self-tenant",
            "repro_srch_d1_dogfood_candidate_leak",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "SRCH-D1");
        assert!(draft.title.contains("INC-SEARCH-DOGFOOD-1"));
        assert!(
            draft.body.contains("repro_srch_d1_dogfood_candidate_leak"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        // PII-free: the draft carries no personal data, only opaque ids + gate names.
        assert!(!draft.body.to_lowercase().contains("email"));

        let ticket = incident.drill_ticket();
        assert_eq!(ticket.drill_name, "repro_srch_d1_dogfood_candidate_leak");
        assert_eq!(ticket.gate_id, "SRCH-D1");
    }

    /// The proof-source paths the truth-up rows name all exist on disk (the rows are not stale).
    #[test]
    fn every_proven_row_proof_source_exists_on_disk() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        for row in proven_search_rows(RUN_DATE) {
            assert!(
                row.artifact_abs_path(&repo_root).exists(),
                "the proof source for {} must exist on disk: {}",
                row.id,
                row.artifact_path
            );
        }
    }
}
