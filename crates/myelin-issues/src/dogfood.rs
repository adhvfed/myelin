//! # `dogfood` — Myelin tracks its OWN issues + the truth-up pass (ISS-P37 / P-520, M6)
//!
//! **The Issues M6 dogfood prompt — THE DONE-BAR (issue-tracker roadmap §6 M6-I10).** M6 promotes
//! NOTHING and freezes NO new contract — the Issues engine is fixed at M4 and hardened through M5 (the
//! co-equal `ViewSpec` views ISS-P16, the SetExpr leak-free push-down ISS-P13/P14, the SLA calendar
//! ISS-P26, the real-time board sync ISS-P30, the erasure-reaches-every-holder ISS-P31, the world-scale
//! surge ISS-P33, the E2E legs ISS-P34/P35/P36). This prompt MIGRATES Myelin's OWN roadmap / gap-report
//! / scorecard into **Myelin ISSUES** (the team plans its own sprints on the platform's own board /
//! roadmap, VISION §5) and reaches the switch-test verdict + the truth-up pass.
//!
//! ## What this module IS (the dogfood DRIVER over the EXISTING surface — EI-01 §7)
//! This is a **caller that drives the already-shipped Issues surface over the Myelin self-tenant** —
//! never a second render / round-trip / view / E2E. It REUSES:
//! - [`crate::roundtrips_md`] → [`myelin_content::wasm`] — the ONE WASM render path (`render(parse(md))
//!   === md`, contract 13.1, the ISS-D10 gate). Every block of every Myelin issue body round-trips
//!   byte-identically — the team's own issues survive the editor with byte-fidelity.
//! - [`crate::e2e_wedge::run_e2e_1_pr_pane`] — the PR context pane (an Issues item resolves per-viewer
//!   through the SAME ACL chokepoint; a confidential issue's title/count never leaks, 0 leak), reframed
//!   onto Myelin's OWN issues.
//! - [`crate::e2e_flagship::run_e2e_2_issues_flagship`] — the agent-native flagship (a governed agent
//!   close is HITL-gated, applies EXACTLY ONCE across a crash + a duplicate approval, reserve/settle
//!   balanced), reframed onto Myelin's own triage loop.
//! - [`crate::e2e_lineage::run_e2e_3_lineage`] — the spec-to-ship lineage (a spec → initiative → issues
//!   → PR → CI run; cold-reindex == live byte-for-byte; audit tamper detected), reframed onto Myelin's
//!   own roadmap → scorecard lineage.
//!
//! ## Myelin's own work as Myelin ISSUES (the team plans its own sprints)
//! [`run_issues_over_myelins_own_work`] drives Myelin's own roadmap / gap-report / scorecard as Myelin
//! issues: each issue body is a real markdown-subset body whose every inline run round-trips
//! `render(parse(md)) === md` through the ONE WASM render path. The every-incident-adds-a-drill loop
//! ([`IssuesIncident`]) files a PII-free Myelin issue draft + a reproducing-drill ticket.
//!
//! ## The truth-up pass (the gate invariant — EI-01 §1)
//! [`IssuesTruthUpPass`] over [`proven_issues_rows`] — every PROVEN Issues row (ISS-D1..ISS-D13 + the
//! E2E slices E2E-1/E2E-2/E2E-3) rests on a DATED green artifact whose proof SOURCE exists on disk; no
//! later-band Issues gate is red. A row that names a vanished artifact is surfaced LOUDLY, never trusted
//! on faith (code-wins-over-docs).
//!
//! ## The switch test (split into [`crate::switch_test`])
//! The Issues switch test (the create→triage→plan→board→done loop without a manual, the primary-screen
//! render + `render(parse(md)) === md` + the state-pill / priority-badge / agent-pending / erased
//! overlays measured against the contrast/latency budgets, the primary-screen states reached, driven
//! over the real surface — EI-01 §4) lives in the sibling [`crate::switch_test`] module.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **None new** — M6 promotes nothing; it exercises the production-hardened Issues surface on real
//!   self-tenant data. The ONE legitimate remaining floor is the world-scale 30× fleet-hardware load
//!   drill (the shared §4.1 fleet drill; the CI variant runs a moderate corpus). The pixel-level browser
//!   drive over a mounted DOM (the live `<Views>` / `<Board>` component shell + a Playwright
//!   keyboard/drag/IME drive) is the UI follow-on prompt's named floor — recorded HONESTLY in
//!   [`crate::switch_test`], never claimed. Any switch-test WALL found (a place the old tool did better)
//!   is filed as a dated Myelin issue + a reproducing drill with its follow-on owner — none found in
//!   this run.
//!
//! **Owning architecture doc:** `planning/04-subsystem-architectures/issue-tracker/architecture/`
//! `04-views-cli-and-api.md` (the primary screens S1/S3/S5/S6/S9/S10/S13/S17/S19 + their states). **Roadmap:**
//! `planning/06-roadmaps/subsystems/issue-tracker.md` §"M6-I10" + §6 (the done-bar). **Doctrine:**
//! `external-insights/01-process-and-quality-doctrine.md` §1 (code-wins-over-docs — the truth-up pass),
//! §4 (the switch test — drive the real surface), §5 (the ratchet runs on the builders' own work).
//! **VISION §3** (the switch test) / **§5** (dogfooding).

use crate::e2e_wedge::IssuesE2eArtifact;

/// The Myelin self-tenant id (the platform self-hosts as exactly one cell — P-508 / CP-M6). Opaque,
/// PII-free — the dogfood Issues surface hosts the platform's OWN issues under this tenant.
pub const MYELIN_SELF_TENANT: &str = "myelin";

/// The region the Myelin self-tenant is pinned to (fr-par — the dev/prod residency pin; a config swap,
/// never a code change). The dogfood Issues surface resolves cell-local in this region.
pub const MYELIN_SELF_REGION: &str = "fr-par";

// ───────────────────────────── (1) Myelin's own work as Myelin ISSUES ─────────────────────────────

/// One of Myelin's OWN work items the team tracks as a Myelin ISSUE (PII-free — the roadmap /
/// gap-report / scorecard, the platform planning its own sprints on its own board, VISION §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MyelinIssue {
    /// The issue's opaque human-key (a stable token; the drills assert against the NAME, never a literal).
    pub key: &'static str,
    /// A one-line human title (what the issue is — the team's own work).
    pub title: &'static str,
    /// The markdown-subset body blocks the issue carries (each must round-trip through the ONE WASM
    /// render path — `render(parse(md)) === md`, contract 13.1 / ISS-D10).
    pub body_blocks: Vec<&'static str>,
}

impl MyelinIssue {
    /// `true` iff every body block round-trips `render(parse(md)) === md` through the ONE WASM render
    /// path (the issue survives the editor with byte-fidelity — the team's own issues are not silently
    /// rewritten). Drives [`crate::roundtrips_md`] (the editor-entry helper), never a second renderer.
    pub fn body_round_trips(&self) -> bool {
        self.body_blocks
            .iter()
            .all(|md| crate::roundtrips_md(md, &[]))
    }
}

/// **Myelin's OWN work as Myelin ISSUES (the roadmap / gap-report / scorecard).** The platform plans its
/// own sprints on its own board / roadmap (VISION §5). Every body block is a canonical markdown-subset
/// body that round-trips through the ONE WASM render path — the dogfood asserts the team's own issues
/// survive the editor with byte-fidelity. PII-free (opaque keys + the team's own process items).
pub fn myelin_issue_backlog() -> Vec<MyelinIssue> {
    vec![
        MyelinIssue {
            key: "MYL-1",
            title: "Myelin platform roadmap (M1..M6) as a tracked initiative",
            body_blocks: vec![
                "The bands run **M1** through **M6**; M6 is the `dogfood` done-bar.",
                "Each band closes on a dated green exit-gate scorecard.",
                "The roadmap is a *timeline* view over the one `issue` table.",
            ],
        },
        MyelinIssue {
            key: "MYL-2",
            title: "Myelin gap report — every named floor carries a follow-on",
            body_blocks: vec![
                "Every named floor carries a follow-on prompt id.",
                "The only remaining floor is the world-scale `30x` fleet-hardware load drill.",
                "~~Open~~ floors are triaged onto the backlog, never invisible.",
            ],
        },
        MyelinIssue {
            key: "MYL-3",
            title: "Myelin exit-gate scorecard — every PROVEN row dated-green",
            body_blocks: vec![
                "Every **PROVEN** row rests on a dated green drill artifact.",
                "A claim that outlives its verification is a `CLAIMED-NOT-PROVEN` red.",
                "See the truth-up pass: [scorecard](https://wiki.test/myelin/scorecard).",
            ],
        },
    ]
}

/// **The named green artifact the Issues dogfood run emits.** The production-hardened Issues surface
/// driven over Myelin's OWN work, across the four production faces:
/// - **Myelin's own work as Myelin ISSUES** ([`myelin_issue_backlog`]) — every body block round-trips
///   `render(parse(md)) === md` through the ONE WASM render path (the team's issues survive the editor);
/// - **the PR context pane** (an issue resolves per-viewer; a confidential issue's title/count never
///   leaks) — REUSES [`crate::e2e_wedge::run_e2e_1_pr_pane`];
/// - **the agent-native flagship** (a governed agent close is HITL-gated, applies exactly once across a
///   crash + a duplicate approval, reserve/settle balanced) — REUSES
///   [`crate::e2e_flagship::run_e2e_2_issues_flagship`];
/// - **the spec-to-ship lineage** (spec → initiative → issues → PR → CI; cold-reindex == live
///   byte-for-byte; audit tamper detected) — REUSES [`crate::e2e_lineage::run_e2e_3_lineage`].
///
/// Issues is GREEN on the platform's own work iff every face is green AND 0 leak AND every Myelin issue
/// body round-trips. A face that did not reach green fails LOUDLY ([`IssuesDogfoodArtifact::is_green`] is
/// false) — never a claimed-but-unearned green.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the dogfood artifact must be checked — an unread RED face silently claims a green Issues \
              did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct IssuesDogfoodArtifact {
    /// The date the dogfood run was asserted (every face is dated at this run).
    pub date: String,
    /// How many of Myelin's own issues round-tripped through the ONE WASM render path (must == `issues_total`).
    pub issues_round_tripped: usize,
    /// How many of Myelin's own issues the backlog carries.
    pub issues_total: usize,
    /// The PR-context-pane face (the per-viewer leak-free issue resolution) — E2E-1.
    pub pr_context_pane: IssuesE2eArtifact,
    /// The agent-native flagship face (HITL-gated governed close, exactly-once across crash) — E2E-2.
    pub agent_flagship: IssuesE2eArtifact,
    /// The spec-to-ship lineage face (cold-reindex == live byte-for-byte; audit tamper detected) — E2E-3.
    pub spec_to_ship: IssuesE2eArtifact,
}

impl IssuesDogfoodArtifact {
    /// `true` iff Issues is GREEN on Myelin's own work — every Myelin issue body round-trips AND every
    /// E2E face green AND 0 leak. The ONLY way to read the dogfood run (a RED face is never silently a
    /// pass).
    pub fn is_green(&self) -> bool {
        self.issues_total > 0
            && self.issues_round_tripped == self.issues_total
            && self.pr_context_pane.is_green()
            && self.agent_flagship.is_green()
            && self.spec_to_ship.is_green()
            && self.total_leaks() == 0
    }

    /// The total leak/tamper counter across the three E2E faces (the F1 leak spine — must be 0).
    pub fn total_leaks(&self) -> u64 {
        self.pr_context_pane.leaks + self.agent_flagship.leaks + self.spec_to_ship.leaks
    }

    /// The dated one-line summary (the artifact body the dogfood CI run prints).
    pub fn summary(&self) -> String {
        format!(
            "P-520 ISSUES DOGFOOD {} — tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             own-issues-round-trip={}/{} pr-context-pane={} agent-flagship={} spec-to-ship={} \
             total-leaks={} verdict={}",
            self.date,
            self.issues_round_tripped,
            self.issues_total,
            self.pr_context_pane.is_green(),
            self.agent_flagship.is_green(),
            self.spec_to_ship.is_green(),
            self.total_leaks(),
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

/// **Run Issues over Myelin's OWN work (ISS-P37).** Drives the production Issues surface across the four
/// faces (Myelin's own work as Myelin issues, the PR context pane, the agent-native flagship, the
/// spec-to-ship lineage) on the Myelin self-tenant, REUSING the existing E2E runners (the SAME ACL
/// chokepoint / governance FSM / reindex engine — EI-01 §7, never a second implementation). `date` is
/// the run stamp.
///
/// **MR-009b W6b2 — `#[cfg(any(test, feature = "test-support"))]`:** it calls the now-gated
/// `run_e2e_2_issues_flagship` (which builds the in-memory `CostLedger`); gated with it. Its callers
/// (this crate's dogfood unit tests + `tests/iss_p37_dogfood_drill.rs`) reach it via the
/// `myelin-issues/test-support` self dev-dependency.
#[cfg(any(test, feature = "test-support"))]
pub fn run_issues_over_myelins_own_work(date: &str) -> IssuesDogfoodArtifact {
    let backlog = myelin_issue_backlog();
    let issues_total = backlog.len();
    let issues_round_tripped = backlog.iter().filter(|i| i.body_round_trips()).count();
    IssuesDogfoodArtifact {
        date: date.to_string(),
        issues_round_tripped,
        issues_total,
        pr_context_pane: crate::e2e_wedge::run_e2e_1_pr_pane(),
        agent_flagship: crate::e2e_flagship::run_e2e_2_issues_flagship(),
        spec_to_ship: crate::e2e_lineage::run_e2e_3_lineage(),
    }
}

// ───────────────────────── (2) the truth-up pass over the PROVEN Issues rows ─────────────────────────

/// One PROVEN Issues row the truth-up pass enumerates. A gate/drill the ledger claims PROVEN, with the
/// proof command that emits its dated green artifact AND the repo-relative path to that proof source. The
/// truth-up pass asserts EACH row rests on a DATED green artifact whose source EXISTS on disk — a row that
/// names a vanished artifact is surfaced, never trusted on faith (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenIssuesRow {
    /// The stable gate/drill id (e.g. `"ISS-D1"`, `"E2E-1"`).
    pub id: &'static str,
    /// The contract SECTION the row's gate belongs to (the §x.y / drill face — e.g. `"13.3"` the views,
    /// `"13.1"` the round-trip). The scorecard groups by section so coverage is visible at a glance.
    pub section: &'static str,
    /// A one-line human title (what the row proves).
    pub title: &'static str,
    /// The proof command that emits this row's dated green artifact (the `cargo test` target that lives
    /// with the feature prompt — named so the artifact is reproducible).
    pub proof_command: &'static str,
    /// The repo-RELATIVE path to the proof source (the test that the `proof_command` runs). The truth-up
    /// pass asserts this file EXISTS on disk — a row that names a vanished artifact is surfaced as
    /// CLAIMED-NOT-PROVEN, never swallowed (EI-01 §1).
    pub artifact_path: &'static str,
    /// The DATE the row's green artifact was last emitted, if any. `Some(date)` ⇒ dated + proven;
    /// `None` ⇒ CLAIMED-NOT-PROVEN (recorded honestly with a date, surfaced as a loud red).
    pub artifact_date: Option<String>,
}

impl ProvenIssuesRow {
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

/// **The FROZEN set of PROVEN Issues rows the truth-up pass enumerates (ISS-P37).** Every Issues gate the
/// ledger claims PROVEN: the thirteen engine/M-spanning drills **ISS-D1..ISS-D13** (co-equal-views /
/// cost-bounding / leak-free-pushdown / create-storm / reorder-zero-clobber / SLA-business-calendar /
/// stateful-trigger / rollup / import / erase-reaches-every-holder / round-trip / guard / board-sync)
/// **plus** the whole-system E2E slices **E2E-1 / E2E-2 / E2E-3** (ISS-P34/P35/P36). The truth-up pass
/// asserts EVERY id here rests on a dated green artifact whose proof source exists on disk; a row without
/// one is a loud failure. `date` is supplied by the runner so a claim never outlives its verification
/// (EI-01 §1).
pub fn proven_issues_rows(date: &str) -> Vec<ProvenIssuesRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenIssuesRow {
        ProvenIssuesRow {
            id,
            section,
            title,
            proof_command: cmd,
            artifact_path,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        // ── The co-equal views + the cost-bounded query (ISS-D1, ISS-D2). ──
        row(
            "ISS-D1",
            "13.3",
            "the board and the roadmap are two ViewSpecs over the SAME rows — editing one patches the other live (0 parallel reality, the co-equal projection)",
            "cargo test -p myelin-issues --test e2e_iss_p16_coequal_views",
            "crates/myelin-issues/tests/e2e_iss_p16_coequal_views.rs",
            date,
        ),
        row(
            "ISS-D2",
            "4.4",
            "a deep cross-subsystem board query is cost-bounded — the three-tier escalation holds, p99 within budget at cell scale (0 unbounded scan)",
            "cargo test -p myelin-issues --test drill_iss_d2_cost_bounding",
            "crates/myelin-issues/tests/drill_iss_d2_cost_bounding.rs",
            date,
        ),
        // ── The leak-free SetExpr push-down + the create storm (ISS-D3, ISS-D4). ──
        row(
            "ISS-D3",
            "4.3",
            "a confidential issue / field-hidden column never leaks — incl. COUNT across the board/search/My-Work (0 leak, the SetExpr pre-filter conjoined into every tier)",
            "cargo test -p myelin-issues --test drill_iss_d3_setexpr_zero_leak",
            "crates/myelin-issues/tests/drill_iss_d3_setexpr_zero_leak.rs",
            date,
        ),
        row(
            "ISS-D4",
            "5.1",
            "a create storm against the Hi/Lo human-key allocator — 0 duplicate / 0 gap key under concurrency (the monotonic per-tenant counter)",
            "cargo test -p myelin-issues --test drill_iss_d4_create_storm",
            "crates/myelin-issues/tests/drill_iss_d4_create_storm.rs",
            date,
        ),
        // ── The reorder CAS + the SLA business-calendar (ISS-D5, ISS-D6). ──
        row(
            "ISS-D5",
            "13.3",
            "concurrent drag-to-rank against the same gap — the LexoRank CAS rejects the loser with current state (0 silent clobber, 0 duplicate rank)",
            "cargo test -p myelin-issues --test drill_iss_d5_reorder_zero_clobber",
            "crates/myelin-issues/tests/drill_iss_d5_reorder_zero_clobber.rs",
            date,
        ),
        row(
            "ISS-D6",
            "7.5",
            "an SLA timer over a business calendar (working hours / holidays / pauses) — the breach fires at the calendar-correct instant; the escalation reflex pages on-call (0 phantom breach)",
            "cargo test -p myelin-issues --test drill_iss_d6_sla_business_calendar",
            "crates/myelin-issues/tests/drill_iss_d6_sla_business_calendar.rs",
            date,
        ),
        // ── The stateful trigger + the incremental rollup (ISS-D7, ISS-D8). ──
        row(
            "ISS-D7",
            "3.3",
            "a stateful Trigger fires exactly once per qualifying transition (debounced across replays / duplicate events; 0 double-fire)",
            "cargo test -p myelin-issues --test drill_iss_d7_stateful_trigger",
            "crates/myelin-issues/tests/drill_iss_d7_stateful_trigger.rs",
            date,
        ),
        row(
            "ISS-D8",
            "11.6",
            "the incremental rollup consumer maintains burndown / counts; the OLAP feed reconciles cold == live byte-for-byte (0 drift)",
            "cargo test -p myelin-issues --test drill_iss_d8_rollup",
            "crates/myelin-issues/tests/drill_iss_d8_rollup.rs",
            date,
        ),
        // ── The import round-trip + the holder-erase (ISS-D9, ISS-D11). ──
        row(
            "ISS-D9",
            "13.2",
            "an ADF/Jira import → the canonical core → re-export — the lossy-map is recorded; the canonical subset round-trips (0 silent data loss)",
            "cargo test -p myelin-issues --test drill_iss_d9_import",
            "crates/myelin-issues/tests/drill_iss_d9_import.rs",
            date,
        ),
        row(
            "ISS-D11",
            "10.1",
            "erase a subject — every holder (issue rows / comments / OLAP / search / agent traces) is reached; the per-subject DEK is crypto-shredded (0 recoverable PII)",
            "cargo test -p myelin-issues --test drill_iss_d11_erase_reaches_every_holder",
            "crates/myelin-issues/tests/drill_iss_d11_erase_reaches_every_holder.rs",
            date,
        ),
        // ── The OLAP feed + the round-trip + the board sync (ISS-D8b, ISS-D10, ISS-D13). ──
        row(
            "ISS-D8b",
            "11.6",
            "the OLAP read store is fed from the same committed log; a wiped OLAP store replays cold == live (the only recovery path)",
            "cargo test -p myelin-issues --test drill_iss_d8b_olap_feed",
            "crates/myelin-issues/tests/drill_iss_d8b_olap_feed.rs",
            date,
        ),
        row(
            "ISS-D10",
            "13.1",
            "render(parse(md)) === md 100% round-trip over an issue body + comment corpus through the ONE WASM render path (0 regressions; read + edit use the IDENTICAL parser)",
            "cargo test -p myelin-issues --test roundtrip_iss_d10",
            "crates/myelin-issues/tests/roundtrip_iss_d10.rs",
            date,
        ),
        row(
            "ISS-D13",
            "3.5",
            "real-time board sync — kill a client mid-drag + sever the connection during a sustained multi-author board edit; the resume cursor re-binds (0 lost / 0 dup move)",
            "cargo test -p myelin-issues --test drill_iss_d13_board_sync",
            "crates/myelin-issues/tests/drill_iss_d13_board_sync.rs",
            date,
        ),
        row(
            "ISS-D12",
            "13.3",
            "a workflow FSM transition guarded by a QueryAst guard + a CheckStatus guard — an ungated transition is rejected (the FSM interpreter is Issues' slice of the agent ∩)",
            "cargo test -p myelin-issues --test e2e_iss_p27_ci_guard",
            "crates/myelin-issues/tests/e2e_iss_p27_ci_guard.rs",
            date,
        ),
        // ── The whole-system E2E wedge slices (E2E-1 / E2E-2 / E2E-3 — ISS-P34/P35/P36). ──
        row(
            "E2E-1",
            "5.6",
            "the PR context pane — an Issues item resolves per-viewer through the ACL chokepoint; a confidential issue's title/count never leaks (0 leak; mid-flight erase honoured live)",
            "cargo test -p myelin-issues --test e2e_wedge_iss_p34",
            "crates/myelin-issues/tests/e2e_wedge_iss_p34.rs",
            date,
        ),
        row(
            "E2E-2",
            "8.2",
            "the agent-native flagship — a governed agent close is HITL-gated; it applies EXACTLY ONCE across a crash + a duplicate approval; reserve/settle balanced (0 ungoverned mutation)",
            "cargo test -p myelin-issues --test e2e_flagship_iss_p35",
            "crates/myelin-issues/tests/e2e_flagship_iss_p35.rs",
            date,
        ),
        row(
            "E2E-3",
            "2.6",
            "spec-to-ship traceability — a spec → initiative → issues → PR → CI run; cold-reindex == live byte-for-byte; audit tamper detected (0 silent tamper)",
            "cargo test -p myelin-issues --test e2e_lineage_iss_p36",
            "crates/myelin-issues/tests/e2e_lineage_iss_p36.rs",
            date,
        ),
    ]
}

/// The verdict of the Issues truth-up pass — Green (every PROVEN row dated) or Red (the undated rows
/// named). Never a swallowed bool — a RED points at exactly which Issues claim outran its verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssuesTruthUpVerdict {
    /// Every enumerated PROVEN Issues row rests on a dated green artifact (no later-band gate red).
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

impl IssuesTruthUpVerdict {
    /// `true` iff the truth-up pass is green (every PROVEN row dated). The ONLY way to read a pass.
    pub fn is_green(&self) -> bool {
        matches!(self, IssuesTruthUpVerdict::Green { .. })
    }

    /// The ids of any CLAIMED-NOT-PROVEN rows (empty on a green pass).
    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            IssuesTruthUpVerdict::Green { .. } => &[],
            IssuesTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

/// **The Issues truth-up pass (ISS-P37 / EI-01 §1).** Enumerates every PROVEN Issues row and confirms
/// each rests on a DATED green artifact. A row WITHOUT one is a LOUD failure
/// ([`IssuesTruthUpVerdict::Red`]), never a silent pass — the code-wins-over-docs discipline made
/// mechanical. A zero-sized orchestrator.
#[derive(Clone, Copy, Debug, Default)]
pub struct IssuesTruthUpPass;

impl IssuesTruthUpPass {
    /// A new truth-up pass (stateless).
    pub fn new() -> IssuesTruthUpPass {
        IssuesTruthUpPass
    }

    /// **Run the truth-up pass over `rows`.** Returns [`IssuesTruthUpVerdict::Green`] (every row dated)
    /// or [`IssuesTruthUpVerdict::Red`] (the undated rows named). `date` stamps the green verdict.
    pub fn run(&self, rows: &[ProvenIssuesRow], date: &str) -> IssuesTruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            IssuesTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            IssuesTruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    /// **The loud-never-swallowed truth-up CI entrypoint (EI-01 §5).** Run the pass and turn a RED
    /// verdict into a process-failing `Err` — so a CI invocation `pass.run_or_fail_ci(&rows, date)?`
    /// FAILS the dogfood truth-up job if ANY PROVEN Issues row lacks a dated green artifact. On GREEN it
    /// returns the number of confirmed rows.
    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenIssuesRow],
        date: &str,
    ) -> Result<usize, IssuesTruthUpRed> {
        match self.run(rows, date) {
            IssuesTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            IssuesTruthUpVerdict::Red { undated_rows } => Err(IssuesTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

/// A RED truth-up pass surfaced as an `Err` — the CLAIMED-NOT-PROVEN Issues rows, loud + specific (the
/// process exits non-zero, never a silent docs drift).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesTruthUpRed {
    /// The ids of the rows lacking a dated green artifact.
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for IssuesTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL — {} Issues row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} — a \
             claim that outlives its verification misleads the next agent (EI-01 §1); fix the doc or \
             re-run the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for IssuesTruthUpRed {}

// ───────────────────────── the enumerated truth-up scorecard (the green artifact) ─────────────────────────

/// How a PROVEN Issues row's proof stands at truth-up time: a dated green artifact, or an
/// honestly-recorded CLAIMED-NOT-PROVEN note. Either way the status carries a DATE (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssuesRowStatus {
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

impl IssuesRowStatus {
    /// `true` iff this is a dated green artifact (the per-row truth-up invariant).
    pub fn is_dated_green(&self) -> bool {
        matches!(self, IssuesRowStatus::DatedGreen { .. })
    }
}

/// One scorecard line: a PROVEN Issues row resolved to its [`IssuesRowStatus`] at truth-up time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesScorecardEntry {
    /// The row this line scores.
    pub row: ProvenIssuesRow,
    /// Its resolved status (dated-green or claimed-not-proven, both dated).
    pub status: IssuesRowStatus,
}

/// **The enumerated Issues truth-up scorecard (the GATE/DRILLS green artifact, ISS-P37).** Every PROVEN
/// Issues row → its dated green artifact (or a dated CLAIMED-NOT-PROVEN note). The scorecard itself is
/// the closing-honesty-pass artifact: rendering it produces the section-grouped table the prompt's GATE
/// demands, and [`Self::is_green`] is true iff NO later-band Issues gate is red.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked — an unread CLAIMED-NOT-PROVEN row silently drifts \
              the docs from the code (EI-01 §1)"]
pub struct IssuesTruthUpScorecard {
    /// The run date the scorecard is stamped with.
    pub date: String,
    /// One entry per PROVEN Issues row, in section order.
    pub entries: Vec<IssuesScorecardEntry>,
}

impl IssuesTruthUpScorecard {
    /// `true` iff every row rests on a dated green artifact (the gate invariant: no Issues gate red).
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
            "GREEN (no later-band Issues gate red)"
        } else {
            "RED (an Issues claim outran its verification)"
        };
        out.push_str(&format!(
            "P-520 ISSUES TRUTH-UP SCORECARD {} — {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                IssuesRowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                IssuesRowStatus::ClaimedNotProven { date, reason } => {
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

/// **Run the Issues truth-up pass and produce the enumerated [`IssuesTruthUpScorecard`] (ISS-P37).** For
/// each PROVEN Issues row this resolves a dated [`IssuesRowStatus`]: a row is DATED-GREEN iff it carries
/// an `artifact_date` AND its proof source exists on disk under `repo_root`; otherwise it is recorded
/// CLAIMED-NOT-PROVEN with the run `date` and the honest reason. The scorecard surfaces — never swallows
/// — any gap (EI-01 §1). `repo_root` is the workspace root the `artifact_path`s are relative to.
pub fn run_issues_truth_up_scorecard(
    date: &str,
    repo_root: &std::path::Path,
) -> IssuesTruthUpScorecard {
    let entries = proven_issues_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => IssuesRowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    IssuesRowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => IssuesRowStatus::DatedGreen { date: d.clone() },
            };
            IssuesScorecardEntry { row, status }
        })
        .collect();
    IssuesTruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

// ───────────────────────────── (3) the every-incident-adds-a-drill loop ─────────────────────────────

/// **An Issues incident on Myelin's own development (the every-incident-adds-a-drill loop, EI-01 §3/§5).**
/// A real incident ends by filing a PII-free Myelin issue draft AND a reproducing-drill ticket — both
/// reference-linked (the moat thesis: the issue points at the drill that reproduces it). The integration
/// drill registers the repro so it re-runs forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesIncident {
    /// The incident id (PII-free, e.g. `"INC-ISS-DOGFOOD-1"`).
    pub incident_id: String,
    /// The Issues gate the incident regressed (e.g. `"ISS-D10"`).
    pub gate_id: String,
    /// A PII-free one-line description of what broke.
    pub description: String,
    /// The name of the reproducing drill the incident files (the test that re-fires the failure).
    pub repro_drill_name: String,
}

impl IssuesIncident {
    /// A new Issues incident (every field PII-free).
    pub fn new(
        incident_id: &str,
        gate_id: &str,
        description: &str,
        repro_drill_name: &str,
    ) -> IssuesIncident {
        IssuesIncident {
            incident_id: incident_id.into(),
            gate_id: gate_id.into(),
            description: description.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    /// The PII-free Myelin issue draft the incident files (names the gate + the repro drill — the issue
    /// is reference-linked to the drift that reproduces it).
    pub fn issue_draft(&self) -> IssuesIncidentIssueDraft {
        IssuesIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!(
                "[{}] Issues gate {} regressed",
                self.incident_id, self.gate_id
            ),
            body: format!(
                "Issues incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked — every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    /// The reproducing-drill ticket the incident files (the test that joins the permanent suite).
    pub fn drill_ticket(&self) -> IssuesIncidentDrillTicket {
        IssuesIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

/// The PII-free Myelin issue draft an [`IssuesIncident`] files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesIncidentIssueDraft {
    /// The Issues gate the issue is about.
    pub gate_id: String,
    /// The issue title (PII-free).
    pub title: String,
    /// The issue body (PII-free; names the repro drill).
    pub body: String,
}

/// The reproducing-drill ticket an [`IssuesIncident`] files (the drill that joins the permanent suite).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesIncidentDrillTicket {
    /// The drill name (the test that re-fires the failure).
    pub drill_name: String,
    /// The gate the drill guards.
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    /// **THE HEADLINE: Issues is GREEN on Myelin's OWN work.** Myelin's own issues round-trip through the
    /// ONE WASM render path, the PR context pane resolves per-viewer (0 title/count leak), the
    /// agent-native flagship is HITL-gated + exactly-once, and the spec-to-ship lineage is cold == live
    /// + tamper-detected — all over the Myelin self-tenant, 0 leak.
    #[test]
    fn issues_greens_on_myelins_own_work() {
        let artifact = run_issues_over_myelins_own_work(RUN_DATE);

        assert!(
            artifact.is_green(),
            "Issues must be green on Myelin's own work: {}",
            artifact.summary()
        );
        assert_eq!(
            artifact.issues_round_tripped,
            artifact.issues_total,
            "every one of Myelin's own issues round-trips through the ONE WASM render path: {}",
            artifact.summary()
        );
        assert!(
            artifact.issues_total >= 3,
            "the roadmap/gap-report/scorecard"
        );
        assert!(
            artifact.pr_context_pane.is_green(),
            "the PR context pane is green"
        );
        assert!(
            artifact.agent_flagship.is_green(),
            "the agent-native flagship is green"
        );
        assert!(
            artifact.spec_to_ship.is_green(),
            "the spec-to-ship lineage is green"
        );
        assert_eq!(
            artifact.total_leaks(),
            0,
            "0 leak across the three E2E faces: {}",
            artifact.summary()
        );

        let s = artifact.summary();
        assert!(s.contains("P-520 ISSUES DOGFOOD 2026-06-26"), "dated: {s}");
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    /// Every one of Myelin's own issue bodies round-trips `render(parse(md)) === md` (the team's issues
    /// survive the editor with byte-fidelity — the ISS-D10 gate / contract 13.1).
    #[test]
    fn myelins_own_issues_round_trip_through_the_one_render_path() {
        let backlog = myelin_issue_backlog();
        assert!(backlog.len() >= 3, "the roadmap/gap-report/scorecard");
        for issue in &backlog {
            assert!(
                issue.body_round_trips(),
                "the Myelin issue {} must round-trip through the ONE WASM render path",
                issue.key
            );
            assert!(!issue.body_blocks.is_empty(), "{}", issue.key);
        }
    }

    /// The truth-up pass is GREEN — every PROVEN Issues row rests on a dated green artifact.
    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_issues_rows(RUN_DATE);
        assert!(
            rows.len() >= 16,
            "the PROVEN set covers ISS-D1..ISS-D13 + the E2E slices E2E-1/E2E-2/E2E-3"
        );
        let confirmed = IssuesTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red later-band Issues gates — every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

    /// A claimed-not-proven row reds the truth-up pass LOUDLY (surfaced, never swallowed).
    #[test]
    fn a_claimed_not_proven_row_reds_the_truth_up_pass() {
        let mut rows = proven_issues_rows(RUN_DATE);
        rows[0].artifact_date = None; // simulate a doc claim with no dated green artifact
        let verdict = IssuesTruthUpPass::new().run(&rows, RUN_DATE);
        assert!(!verdict.is_green(), "an undated row reds the pass");
        assert_eq!(verdict.undated_rows(), &[rows[0].id]);
        let err = IssuesTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect_err("a claimed-not-proven row fails the CI entrypoint");
        assert!(err.to_string().contains("CLAIMED-NOT-PROVEN"));
    }

    /// The enumerated scorecard renders GREEN with every PROVEN row dated + its proof source on disk.
    #[test]
    fn the_scorecard_renders_green_with_proof_sources_on_disk() {
        let scorecard = run_issues_truth_up_scorecard(RUN_DATE, &repo_root());
        assert!(
            scorecard.is_green(),
            "the scorecard must be green — every PROVEN Issues row dated + its proof source on disk; \
             claimed-not-proven: {:?}",
            scorecard.claimed_not_proven()
        );
        assert_eq!(scorecard.rows_dated_green(), scorecard.rows_total());
        let md = scorecard.render();
        assert!(md.contains("verdict=GREEN"), "rendered: {md}");
        assert!(
            md.contains("ISS-D1") && md.contains("E2E-3"),
            "enumerated: {md}"
        );
    }

    /// A row whose proof source is missing on disk is surfaced CLAIMED-NOT-PROVEN (never trusted on faith).
    #[test]
    fn a_vanished_proof_source_is_surfaced_not_trusted() {
        let bogus_root = std::path::Path::new("/nonexistent-iss-truth-up-root");
        let scorecard = run_issues_truth_up_scorecard(RUN_DATE, bogus_root);
        assert!(
            !scorecard.is_green(),
            "a vanished proof source must red the scorecard"
        );
        assert!(
            scorecard.entries.iter().all(|e| matches!(
                &e.status,
                IssuesRowStatus::ClaimedNotProven { reason, .. } if reason.contains("missing on disk")
            )),
            "every row is surfaced as proof-source-missing, never trusted on faith"
        );
    }

    /// The every-incident loop: an incident files a PII-free issue draft + a reproducing-drill ticket.
    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = IssuesIncident::new(
            "INC-ISS-DOGFOOD-1",
            "ISS-D10",
            "an issue-body corpus fixture silently round-tripped non-canonically on the Myelin self-tenant",
            "repro_iss_d10_dogfood_non_canonical_round_trip",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "ISS-D10");
        assert!(draft.title.contains("INC-ISS-DOGFOOD-1"));
        assert!(
            draft
                .body
                .contains("repro_iss_d10_dogfood_non_canonical_round_trip"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        // PII-free: the draft carries no personal data, only opaque ids + gate names.
        assert!(!draft.body.to_lowercase().contains("email"));

        let ticket = incident.drill_ticket();
        assert_eq!(
            ticket.drill_name,
            "repro_iss_d10_dogfood_non_canonical_round_trip"
        );
        assert_eq!(ticket.gate_id, "ISS-D10");
    }

    /// The proof-source paths the truth-up rows name all exist on disk (the rows are not stale).
    #[test]
    fn every_proven_row_proof_source_exists_on_disk() {
        for row in proven_issues_rows(RUN_DATE) {
            assert!(
                row.artifact_abs_path(&repo_root()).exists(),
                "the proof source for {} must exist on disk: {}",
                row.id,
                row.artifact_path
            );
        }
    }

    /// The workspace root the `artifact_path`s are relative to (two parents up from this crate's manifest).
    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf()
    }
}
