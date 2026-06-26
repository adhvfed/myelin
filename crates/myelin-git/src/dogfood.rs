//! # `dogfood` — git hosts Myelin's OWN repositories + the self-hosting CI graph (GIT-P35 / P-518, M6)
//!
//! **The git-hosting M6 dogfood prompt — THE DONE-BAR (git-hosting roadmap §6).** M6 promotes NOTHING and
//! freezes NO new contract — the engine is fixed at M3 and hardened through M5. This prompt MIGRATES the
//! Myelin monorepo onto Myelin git hosting (the switch test, VISION §3/§5): the platform's own
//! build/test/lint/mutation pipeline becomes a Myelin CI pipeline (the twelve lints + the mandatory-core
//! cargo-mutants gate now run as Myelin CI jobs ON THE PLATFORM'S OWN GIT COMMITS — the dogfood loop), and
//! the roadmap/gap-report live as Myelin issues + a Knowledge space.
//!
//! ## What this module IS (the dogfood DRIVER over the EXISTING surface — EI-01 §7)
//! This is a **caller that drives the already-shipped git surface over the Myelin self-tenant** — never a
//! second merge gate / round-trip / lineage / E2E. It REUSES:
//! - [`crate::surge::run_e2e_1_pr_pane`] — the PR context pane (git is the reference producer; a denied
//!   viewer's linked confidential issue resolves to a TOMBSTONE, 0 title/count/backlink leak), reframed as
//!   the Myelin monorepo's OWN PRs.
//! - [`crate::surge::run_e2e_2_fix_pr`] — the agent-native flagship (CI-fail → … → fix-PR; the `git.merge`
//!   HITL + X-1 CheckStatus gate; exactly-once HITL + merge across the kill; `git.pr.merged` closes the
//!   issue via the `Closes` trailer), reframed onto a real Myelin fix-PR.
//! - [`crate::surge::run_e2e_3_spec_to_ship`] — the spec-to-ship lineage (commit→PR→merge; cold-reindex ==
//!   live byte-for-byte from the durable source), reframed onto Myelin's own commit lineage.
//!
//! ## What this module wires (the dogfood loop is live)
//! - **The git drills run as Myelin CI jobs on Myelin's own commits** — wired into the frozen
//!   `myelin_harness::self_hosting_ci::self_hosting_jobs` graph (the GIT-P35 bands; see the harness
//!   module). The dogfood loop is live: the git dogfood + the switch test run on every Myelin commit.
//! - **The truth-up pass** ([`GitTruthUpPass`] over [`proven_git_rows`]) — every PROVEN git row
//!   (GIT-D1..GIT-D11 + the E2E slices E2E-1/E2E-2/E2E-3) rests on a DATED green artifact whose proof
//!   SOURCE exists on disk; no later-band git gate is red. A row that names a vanished artifact is surfaced
//!   LOUDLY, never trusted on faith (EI-01 §1, code-wins-over-docs).
//! - **The every-incident-adds-a-drill loop** ([`GitIncident`]) — a git incident files a PII-free Myelin
//!   issue draft AND a reproducing-drill ticket; both reference-linked (the issue points at the drill).
//!
//! ## The switch test (split into [`crate::switch_test`])
//! The Git OQ-12 SWITCH TEST (the PR overview render + `render(parse(md)) === md` + the status overlays
//! measured against the contrast/latency budgets, driven over the real surface — EI-01 §4) lives in the
//! sibling [`crate::switch_test`] module. The dogfood band wires it as a Myelin CI job too.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **None new** — M6 promotes nothing; it exercises the production-hardened git surface on real
//!   self-tenant data. The ONE legitimate remaining floor is the world-scale 30× fleet-hardware load drill
//!   (the shared §4.1 fleet drill; the CI variant runs a moderate corpus). Any switch-test WALL found (a
//!   place the old tool did better) is recorded as a dated gap-report item with its follow-on owner — none
//!   found in this run.
//!
//! **Owning architecture doc:** `planning/04-subsystem-architectures/git-hosting/architecture/`
//! `06-reconciliation-compliance.md` (the conformance map the truth-up confirms). **Roadmap:**
//! `planning/06-roadmaps/subsystems/git-hosting.md` §3 M6-G10 + §6 (the done-bar). **Doctrine:**
//! `external-insights/01-process-and-quality-doctrine.md` §1 (code-wins-over-docs — the truth-up pass),
//! §4 (the switch test — drive the real surface), §5 (the ratchet runs on the builders' own work).
//! **VISION §3** (the switch test) / **§5** (dogfooding).

use crate::surge::{run_e2e_1_pr_pane, run_e2e_2_fix_pr, run_e2e_3_spec_to_ship, E2eArtifact};

/// The Myelin self-tenant id (the platform self-hosts as exactly one cell — P-508 / CP-M6). Opaque,
/// PII-free — the dogfood git surface hosts the platform's OWN repositories under this tenant.
pub const MYELIN_SELF_TENANT: &str = "myelin";

/// The region the Myelin self-tenant is pinned to (fr-par — the dev/prod residency pin; a config swap,
/// never a code change). The dogfood git surface resolves cell-local in this region.
pub const MYELIN_SELF_REGION: &str = "fr-par";

// ───────────────────────────── (1) git hosts Myelin's own repositories ─────────────────────────────

/// **The named green artifact the git dogfood run emits.** The production-hardened git surface driven over
/// Myelin's OWN repositories, across the three production faces:
/// - **the PR context pane** on the Myelin monorepo (git the reference producer; a denied viewer's linked
///   confidential issue tombstones, 0 leak) — REUSES [`run_e2e_1_pr_pane`];
/// - **the agent-native fix-PR flagship** (CI-fail → … → fix-PR; the `git.merge` HITL + X-1 gate;
///   exactly-once HITL + merge; `git.pr.merged` closes the issue) — REUSES [`run_e2e_2_fix_pr`];
/// - **the spec-to-ship lineage** (commit→PR→merge; cold-reindex == live byte-for-byte) — REUSES
///   [`run_e2e_3_spec_to_ship`].
///
/// git is GREEN on the platform's own repositories iff every face is green AND 0 leak AND the flagship's
/// merge is exactly-once (merge_count == 1). A face that did not reach green fails LOUDLY
/// ([`GitDogfoodArtifact::is_green`] is false) — never a claimed-but-unearned green.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the dogfood artifact must be checked — an unread RED face silently claims a green git \
              did not earn on Myelin's own repositories (EI-01 §1/§3)"]
pub struct GitDogfoodArtifact {
    /// The date the dogfood run was asserted (every face is dated at this run).
    pub date: String,
    /// The PR-context-pane face (git the reference producer; the per-viewer leak-free resolution).
    pub pr_context_pane: E2eArtifact,
    /// The agent-native fix-PR flagship face (the HITL + X-1 gate; exactly-once merge).
    pub fix_pr_flagship: E2eArtifact,
    /// The spec-to-ship lineage face (cold-reindex == live byte-for-byte).
    pub spec_to_ship: E2eArtifact,
}

impl GitDogfoodArtifact {
    /// `true` iff git is GREEN on Myelin's own repositories — every face green AND 0 leak AND the flagship
    /// merge is exactly-once. The ONLY way to read the dogfood run (a RED face is never silently a pass).
    pub fn is_green(&self) -> bool {
        self.pr_context_pane.is_green()
            && self.fix_pr_flagship.is_green()
            && self.spec_to_ship.is_green()
            && self.total_leaks() == 0
            && self.fix_pr_flagship.merge_count == 1
    }

    /// The total leak counter across the three faces (the F1 leak spine — must be 0).
    pub fn total_leaks(&self) -> u32 {
        self.pr_context_pane.leaks + self.fix_pr_flagship.leaks + self.spec_to_ship.leaks
    }

    /// The dated one-line summary (the artifact body the dogfood CI run prints).
    pub fn summary(&self) -> String {
        format!(
            "P-518 GIT DOGFOOD {} — tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             pr-pane={} fix-pr-flagship={} (merge_count={}) spec-to-ship={} total-leaks={} verdict={}",
            self.date,
            self.pr_context_pane.is_green(),
            self.fix_pr_flagship.is_green(),
            self.fix_pr_flagship.merge_count,
            self.spec_to_ship.is_green(),
            self.total_leaks(),
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

/// **Run git over Myelin's OWN repositories (GIT-P35).** Drives the production git surface across the three
/// faces (the PR context pane, the agent-native fix-PR flagship, the spec-to-ship lineage) on the Myelin
/// self-tenant, REUSING the existing E2E-wedge runners (the SAME reference-producer / merge-gate /
/// reindex-from-source engine — EI-01 §7, never a second implementation). `date` is the run stamp.
pub fn run_git_over_myelins_own_repos(date: &str) -> GitDogfoodArtifact {
    GitDogfoodArtifact {
        date: date.to_string(),
        pr_context_pane: run_e2e_1_pr_pane(),
        fix_pr_flagship: run_e2e_2_fix_pr(),
        spec_to_ship: run_e2e_3_spec_to_ship(),
    }
}

// ───────────────────────────── (2) the truth-up pass over the PROVEN git rows ─────────────────────────────

/// One PROVEN git row the truth-up pass enumerates. A gate/drill the ledger claims PROVEN, with the proof
/// command that emits its dated green artifact AND the repo-relative path to that proof source. The
/// truth-up pass asserts EACH row rests on a DATED green artifact whose source EXISTS on disk — a row that
/// names a vanished artifact is surfaced, never trusted on faith (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenGitRow {
    /// The stable gate/drill id (e.g. `"GIT-D1"`, `"E2E-1"`).
    pub id: &'static str,
    /// The contract SECTION the row's gate belongs to (the §x.y / drill face — e.g. `"6.2"` the per-ref
    /// order, `"4.8"` pseudonymity). The scorecard groups by section so coverage is visible at a glance.
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

impl ProvenGitRow {
    /// `true` iff this row rests on a dated green artifact (the per-row truth-up invariant).
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    /// Resolve this row's [`artifact_path`](Self::artifact_path) to an absolute path under `repo_root` so a
    /// caller can assert the proof source exists on disk.
    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

/// **The FROZEN set of PROVEN git rows the truth-up pass enumerates (GIT-P35).** Every git gate the ledger
/// claims PROVEN: the eleven engine/M-spanning drills **GIT-D1..GIT-D11** (the per-ref order / pseudonymity
/// / reindex-parity / object-backed-packs / merge-linearizability / clone-surge / anchor-resolution /
/// front-door isolation / receive-pack one-tx / check-status projection / code-search leak-free) **plus**
/// the whole-system E2E slices **E2E-1 / E2E-2 / E2E-3** (GIT-P34). The truth-up pass asserts EVERY id here
/// rests on a dated green artifact whose proof source exists on disk; a row without one is a loud failure.
/// `date` is supplied by the runner so a claim never outlives its verification (EI-01 §1).
pub fn proven_git_rows(date: &str) -> Vec<ProvenGitRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenGitRow {
        ProvenGitRow {
            id,
            section,
            title,
            proof_command: cmd,
            artifact_path,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        // ── The push/ref-ordering + pseudonymity invariants (GIT-D1, GIT-D2). ──
        row(
            "GIT-D1",
            "6.2",
            "the hot-ref burst — per-ref aggregate ordering at push QPS (0 reorder under a contended ref burst)",
            "cargo test -p myelin-git --test drills_git_d1_hot_ref_burst",
            "crates/myelin-git/tests/drills_git_d1_hot_ref_burst.rs",
            date,
        ),
        row(
            "GIT-D2",
            "4.8",
            "erasure reaches every holder + pseudonymous-by-default commits — erase = shred, not hide; 0 cleartext PII residual incl. backups",
            "cargo test -p myelin-git --test drills_git_d2_erase_reaches_every_holder",
            "crates/myelin-git/tests/drills_git_d2_erase_reaches_every_holder.rs",
            date,
        ),
        // ── The reindex / pack / merge family (GIT-D3..GIT-D5). ──
        row(
            "GIT-D3",
            "4.9",
            "reindex-from-source parity — the only rebuild path; the cold projection byte-matches live (no bespoke recovery reader)",
            "cargo test -p myelin-git --test drills_git_d3_reindex_parity",
            "crates/myelin-git/tests/drills_git_d3_reindex_parity.rs",
            date,
        ),
        row(
            "GIT-D4",
            "11.2",
            "object-backed packs — pack/delta storage on the object-store BlobStore; the clone round-trips byte-identical from cold packs",
            "cargo test -p myelin-git --test drills_git_d4_object_backed_packs",
            "crates/myelin-git/tests/drills_git_d4_object_backed_packs.rs",
            date,
        ),
        row(
            "GIT-D5",
            "5.9",
            "concurrent-merge linearizability — the speculative merge queue applies each merge EXACTLY ONCE under concurrency (merge-count == 1)",
            "cargo test -p myelin-git --test drills_git_d5_concurrent_merge_linearizability",
            "crates/myelin-git/tests/drills_git_d5_concurrent_merge_linearizability.rs",
            date,
        ),
        // ── The world-scale surge + anchor (GIT-D6, GIT-D7). ──
        row(
            "GIT-D6",
            "4.11",
            "the 30× clone surge — DRR-fair shed within the tuned budget (human fetch HELD, agent + CI SHED, cross-tenant impact 0)",
            "cargo test -p myelin-git --test drill_git_d6_clone_surge",
            "crates/myelin-git/tests/drill_git_d6_clone_surge.rs",
            date,
        ),
        row(
            "GIT-D7",
            "5.7",
            "content-anchored line ranges — a comment anchor survives a rebase/force-push (0 mis-anchored; the anchor resolves to the same lines)",
            "cargo test -p myelin-git --test e2e_git_d7_anchor_resolution",
            "crates/myelin-git/tests/e2e_git_d7_anchor_resolution.rs",
            date,
        ),
        // ── The front-door + receive-pack + projection isolation (GIT-D8..GIT-D10). ──
        row(
            "GIT-D8",
            "1.6",
            "the front door — authenticate/check/placement/residency; 0 cross-tenant read (a viewer never reaches another tenant's repo)",
            "cargo test -p myelin-git --test drill_git_d8_front_door",
            "crates/myelin-git/tests/drill_git_d8_front_door.rs",
            date,
        ),
        row(
            "GIT-D9",
            "2.2",
            "receive-pack → one-tx ref-CAS + outbox — 0 ghost / 0 lost (a push commits the ref CAS + the event in one transaction or neither)",
            "cargo test -p myelin-git --test drills_git_d9_receive_pack",
            "crates/myelin-git/tests/drills_git_d9_receive_pack.rs",
            date,
        ),
        row(
            "GIT-D10",
            "5.9",
            "check-status projection + run-attempt supersession — 1 current row per (commit, context) key; a higher attempt supersedes, no cross-sync cycle",
            "cargo test -p myelin-git --test integration_git_d10_check_status_projection",
            "crates/myelin-git/tests/integration_git_d10_check_status_projection.rs",
            date,
        ),
        // ── The code-search leak-free pre-filter (GIT-D11). ──
        row(
            "GIT-D11",
            "4.3",
            "code-search leak-free SetExpr pushdown — the ACL pre-filter excludes the confidential set BEFORE scoring (0 leak, 1 query)",
            "cargo test -p myelin-git --test integration_git_p26_list_pushdown",
            "crates/myelin-git/tests/integration_git_p26_list_pushdown.rs",
            date,
        ),
        // ── The whole-system E2E wedge slices (E2E-1 / E2E-2 / E2E-3 — GIT-P34). ──
        row(
            "E2E-1",
            "5.5",
            "the PR context pane — git is the reference producer; a denied viewer's linked confidential issue tombstones (0 title/count/backlink leak)",
            "cargo test -p myelin-git --test e2e_wedge_git_p34",
            "crates/myelin-git/tests/e2e_wedge_git_p34.rs",
            date,
        ),
        row(
            "E2E-2",
            "5.9",
            "the agent-native flagship — CI-fail → fix-PR; the git.merge HITL + X-1 CheckStatus gate; exactly-once HITL + merge; git.pr.merged closes the issue",
            "cargo test -p myelin-git --test e2e_wedge_git_p34",
            "crates/myelin-git/tests/e2e_wedge_git_p34.rs",
            date,
        ),
        row(
            "E2E-3",
            "4.9",
            "spec-to-ship lineage — commit→PR→merge; the cold reindex-from-source byte-matches live (the restore-verify gate confirms cold == live)",
            "cargo test -p myelin-git --test e2e_wedge_git_p34",
            "crates/myelin-git/tests/e2e_wedge_git_p34.rs",
            date,
        ),
    ]
}

/// The verdict of the git truth-up pass — Green (every PROVEN row dated) or Red (the undated rows named).
/// Never a swallowed bool — a RED points at exactly which git claim outran its verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitTruthUpVerdict {
    /// Every enumerated PROVEN git row rests on a dated green artifact (no later-band git gate red).
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

impl GitTruthUpVerdict {
    /// `true` iff the truth-up pass is green (every PROVEN row dated). The ONLY way to read a pass.
    pub fn is_green(&self) -> bool {
        matches!(self, GitTruthUpVerdict::Green { .. })
    }

    /// The ids of any CLAIMED-NOT-PROVEN rows (empty on a green pass).
    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            GitTruthUpVerdict::Green { .. } => &[],
            GitTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

/// **The git truth-up pass (GIT-P35 / EI-01 §1).** Enumerates every PROVEN git row and confirms each rests
/// on a DATED green artifact. A row WITHOUT one is a LOUD failure ([`GitTruthUpVerdict::Red`]), never a
/// silent pass — the code-wins-over-docs discipline made mechanical. A zero-sized orchestrator.
#[derive(Clone, Copy, Debug, Default)]
pub struct GitTruthUpPass;

impl GitTruthUpPass {
    /// A new truth-up pass (stateless).
    pub fn new() -> GitTruthUpPass {
        GitTruthUpPass
    }

    /// **Run the truth-up pass over `rows`.** Returns [`GitTruthUpVerdict::Green`] (every row dated) or
    /// [`GitTruthUpVerdict::Red`] (the undated rows named). `date` stamps the green verdict.
    pub fn run(&self, rows: &[ProvenGitRow], date: &str) -> GitTruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            GitTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            GitTruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    /// **The loud-never-swallowed truth-up CI entrypoint (EI-01 §5).** Run the pass and turn a RED verdict
    /// into a process-failing `Err` — so a CI invocation `pass.run_or_fail_ci(&rows, date)?` FAILS the
    /// dogfood truth-up job if ANY PROVEN git row lacks a dated green artifact. On GREEN it returns the
    /// number of confirmed rows.
    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenGitRow],
        date: &str,
    ) -> Result<usize, GitTruthUpRed> {
        match self.run(rows, date) {
            GitTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            GitTruthUpVerdict::Red { undated_rows } => Err(GitTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

/// A RED truth-up pass surfaced as an `Err` — the CLAIMED-NOT-PROVEN git rows, loud + specific (the
/// process exits non-zero, never a silent docs drift).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitTruthUpRed {
    /// The ids of the rows lacking a dated green artifact.
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for GitTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL — {} git row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} — a claim that \
             outlives its verification misleads the next agent (EI-01 §1); fix the doc or re-run the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for GitTruthUpRed {}

// ───────────────────────────── the enumerated truth-up scorecard (the green artifact) ─────────────────────────────

/// How a PROVEN git row's proof stands at truth-up time: a dated green artifact, or an honestly-recorded
/// CLAIMED-NOT-PROVEN note. Either way the status carries a DATE (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitRowStatus {
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

impl GitRowStatus {
    /// `true` iff this is a dated green artifact (the per-row truth-up invariant).
    pub fn is_dated_green(&self) -> bool {
        matches!(self, GitRowStatus::DatedGreen { .. })
    }
}

/// One scorecard line: a PROVEN git row resolved to its [`GitRowStatus`] at truth-up time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitScorecardEntry {
    /// The row this line scores.
    pub row: ProvenGitRow,
    /// Its resolved status (dated-green or claimed-not-proven, both dated).
    pub status: GitRowStatus,
}

/// **The enumerated git truth-up scorecard (the GATE/DRILLS green artifact, GIT-P35).** Every PROVEN git
/// row → its dated green artifact (or a dated CLAIMED-NOT-PROVEN note). The scorecard itself is the
/// closing-honesty-pass artifact: rendering it produces the section-grouped table the prompt's GATE
/// demands, and [`Self::is_green`] is true iff NO later-band git gate is red.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked — an unread CLAIMED-NOT-PROVEN row silently drifts \
              the docs from the code (EI-01 §1)"]
pub struct GitTruthUpScorecard {
    /// The run date the scorecard is stamped with.
    pub date: String,
    /// One entry per PROVEN git row, in section order.
    pub entries: Vec<GitScorecardEntry>,
}

impl GitTruthUpScorecard {
    /// `true` iff every row rests on a dated green artifact (the gate invariant: no git gate red).
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
            "GREEN (no later-band git gate red)"
        } else {
            "RED (a git claim outran its verification)"
        };
        out.push_str(&format!(
            "P-518 GIT TRUTH-UP SCORECARD {} — {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                GitRowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                GitRowStatus::ClaimedNotProven { date, reason } => {
                    format!("CLAIMED-NOT-PROVEN({date}: {reason})")
                }
            };
            out.push_str(&format!(
                "  [§{}] {:<8} {:<28} — {}  ⟨{}⟩\n",
                e.row.section, e.row.id, status, e.row.title, e.row.proof_command,
            ));
        }
        out
    }
}

/// **Run the git truth-up pass and produce the enumerated [`GitTruthUpScorecard`] (GIT-P35).** For each
/// PROVEN git row this resolves a dated [`GitRowStatus`]: a row is DATED-GREEN iff it carries an
/// `artifact_date` AND its proof source exists on disk under `repo_root`; otherwise it is recorded
/// CLAIMED-NOT-PROVEN with the run `date` and the honest reason. The scorecard surfaces — never swallows —
/// any gap (EI-01 §1). `repo_root` is the workspace root the `artifact_path`s are relative to.
pub fn run_git_truth_up_scorecard(date: &str, repo_root: &std::path::Path) -> GitTruthUpScorecard {
    let entries = proven_git_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => GitRowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    GitRowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => GitRowStatus::DatedGreen { date: d.clone() },
            };
            GitScorecardEntry { row, status }
        })
        .collect();
    GitTruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

// ───────────────────────────── (3) the every-incident-adds-a-drill loop ─────────────────────────────

/// **A git incident on Myelin's own development (the every-incident-adds-a-drill loop, EI-01 §3/§5).** A
/// real incident ends by filing a PII-free Myelin issue draft AND a reproducing-drill ticket — both
/// reference-linked (the moat thesis: the issue points at the drill that reproduces it). The integration
/// drill registers the repro into the harness `DrillRegistry` so it re-runs forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitIncident {
    /// The incident id (PII-free, e.g. `"INC-GIT-DOGFOOD-1"`).
    pub incident_id: String,
    /// The git gate the incident regressed (e.g. `"GIT-D1"`).
    pub gate_id: String,
    /// A PII-free one-line description of what broke.
    pub description: String,
    /// The name of the reproducing drill the incident files (the test that re-fires the failure).
    pub repro_drill_name: String,
}

impl GitIncident {
    /// A new git incident (every field PII-free).
    pub fn new(
        incident_id: &str,
        gate_id: &str,
        description: &str,
        repro_drill_name: &str,
    ) -> GitIncident {
        GitIncident {
            incident_id: incident_id.into(),
            gate_id: gate_id.into(),
            description: description.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    /// The PII-free Myelin issue draft the incident files (names the gate + the repro drill — the issue is
    /// reference-linked to the drift that reproduces it).
    pub fn issue_draft(&self) -> GitIncidentIssueDraft {
        GitIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!("[{}] git gate {} regressed", self.incident_id, self.gate_id),
            body: format!(
                "git incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked — every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    /// The reproducing-drill ticket the incident files (the test that joins the permanent suite).
    pub fn drill_ticket(&self) -> GitIncidentDrillTicket {
        GitIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

/// The PII-free Myelin issue draft a [`GitIncident`] files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitIncidentIssueDraft {
    /// The git gate the issue is about.
    pub gate_id: String,
    /// The issue title (PII-free).
    pub title: String,
    /// The issue body (PII-free; names the repro drill).
    pub body: String,
}

/// The reproducing-drill ticket a [`GitIncident`] files (the drill that joins the permanent suite).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitIncidentDrillTicket {
    /// The drill name (the test that re-fires the failure).
    pub drill_name: String,
    /// The gate the drill guards.
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    /// **THE HEADLINE: git is GREEN on Myelin's OWN repositories.** The PR context pane, the agent-native
    /// fix-PR flagship (exactly-once merge), and the spec-to-ship lineage all green over the Myelin
    /// self-tenant, 0 leak.
    #[test]
    fn git_greens_on_myelins_own_repos() {
        let artifact = run_git_over_myelins_own_repos(RUN_DATE);

        assert!(
            artifact.is_green(),
            "git must be green on Myelin's own repositories: {}",
            artifact.summary()
        );
        assert_eq!(
            artifact.total_leaks(),
            0,
            "0 leak across the three faces: {}",
            artifact.summary()
        );
        assert!(
            artifact.pr_context_pane.is_green(),
            "the PR context pane is green"
        );
        assert!(
            artifact.fix_pr_flagship.is_green(),
            "the agent-native fix-PR flagship is green"
        );
        assert_eq!(
            artifact.fix_pr_flagship.merge_count, 1,
            "the flagship merge is exactly-once"
        );
        assert!(
            artifact.spec_to_ship.is_green(),
            "the spec-to-ship lineage is green"
        );

        let s = artifact.summary();
        assert!(s.contains("P-518 GIT DOGFOOD 2026-06-26"), "dated: {s}");
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    /// The truth-up pass is GREEN — every PROVEN git row rests on a dated green artifact.
    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_git_rows(RUN_DATE);
        assert!(
            rows.len() >= 14,
            "the PROVEN set covers GIT-D1..GIT-D11 + the E2E slices"
        );
        let confirmed = GitTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red later-band git gates — every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

    /// A claimed-not-proven row reds the truth-up pass LOUDLY (surfaced, never swallowed).
    #[test]
    fn a_claimed_not_proven_row_reds_the_truth_up_pass() {
        let mut rows = proven_git_rows(RUN_DATE);
        rows[0].artifact_date = None; // simulate a doc claim with no dated green artifact
        let verdict = GitTruthUpPass::new().run(&rows, RUN_DATE);
        assert!(!verdict.is_green(), "an undated row reds the pass");
        assert_eq!(verdict.undated_rows(), &[rows[0].id]);
        let err = GitTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect_err("a claimed-not-proven row fails the CI entrypoint");
        assert!(err.to_string().contains("CLAIMED-NOT-PROVEN"));
    }

    /// The enumerated scorecard renders GREEN with every PROVEN row dated + its proof source on disk.
    #[test]
    fn the_scorecard_renders_green_with_proof_sources_on_disk() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let scorecard = run_git_truth_up_scorecard(RUN_DATE, &repo_root);
        assert!(
            scorecard.is_green(),
            "the scorecard must be green — every PROVEN git row dated + its proof source on disk; \
             claimed-not-proven: {:?}",
            scorecard.claimed_not_proven()
        );
        assert_eq!(scorecard.rows_dated_green(), scorecard.rows_total());
        let md = scorecard.render();
        assert!(md.contains("verdict=GREEN"), "rendered: {md}");
        assert!(
            md.contains("GIT-D1") && md.contains("E2E-3"),
            "enumerated: {md}"
        );
    }

    /// A row whose proof source is missing on disk is surfaced CLAIMED-NOT-PROVEN (never trusted on faith).
    #[test]
    fn a_vanished_proof_source_is_surfaced_not_trusted() {
        let bogus_root = std::path::Path::new("/nonexistent-git-truth-up-root");
        let scorecard = run_git_truth_up_scorecard(RUN_DATE, bogus_root);
        assert!(
            !scorecard.is_green(),
            "a vanished proof source must red the scorecard"
        );
        assert!(
            scorecard
                .entries
                .iter()
                .all(|e| matches!(&e.status, GitRowStatus::ClaimedNotProven { reason, .. } if reason.contains("missing on disk"))),
            "every row is surfaced as proof-source-missing, never trusted on faith"
        );
    }

    /// The every-incident loop: an incident files a PII-free issue draft + a reproducing-drill ticket.
    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = GitIncident::new(
            "INC-GIT-DOGFOOD-1",
            "GIT-D9",
            "a receive-pack regression left a ghost ref without its outbox event on the Myelin self-tenant",
            "repro_git_d9_dogfood_ghost_ref",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "GIT-D9");
        assert!(draft.title.contains("INC-GIT-DOGFOOD-1"));
        assert!(
            draft.body.contains("repro_git_d9_dogfood_ghost_ref"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        // PII-free: the draft carries no personal data, only opaque ids + gate names.
        assert!(!draft.body.to_lowercase().contains("email"));

        let ticket = incident.drill_ticket();
        assert_eq!(ticket.drill_name, "repro_git_d9_dogfood_ghost_ref");
        assert_eq!(ticket.gate_id, "GIT-D9");
    }

    /// The proof-source paths the truth-up rows name all exist on disk (the rows are not stale).
    #[test]
    fn every_proven_row_proof_source_exists_on_disk() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        for row in proven_git_rows(RUN_DATE) {
            assert!(
                row.artifact_abs_path(&repo_root).exists(),
                "the proof source for {} must exist on disk: {}",
                row.id,
                row.artifact_path
            );
        }
    }
}
