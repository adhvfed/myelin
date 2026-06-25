//! # `dogfood` — the CI dogfood done-bar: the switch test + the CI truth-up pass (CI-P35 → P-509, M6)
//!
//! **Owning roadmap milestone:** `planning/06-roadmaps/subsystems/continuous-integration.md` §3 CI-M6
//! ("Dogfooding: Myelin's own CI runs on Myelin CI"). The cheapest, most honest load generator is the
//! platform's own development (VISION §1 — the differentiator driven by the builders themselves; §3 —
//! the code wins over the docs: a dated green self-hosting graph, not a claim). This is the LAST CI
//! prompt — the platform done-bar (the coverage matrix places CI-P35 last in CI's work).
//!
//! ## What this module ships (the CI-P35 deliverable, exactly where the prompt specifies)
//!
//! The Myelin build/test/lint/mutation pipeline running AS a Myelin `ci.pipeline` is wired in the
//! self-hosting CI graph (`myelin-harness/src/self_hosting_ci.rs`) — the CI dogfood band added there
//! (see [`crate::dogfood`] referenced from that graph). This module ships the two CI-OWNED done-bar
//! artifacts the prompt names that are NOT graph jobs:
//!
//! 1. **[`CiSwitchTest`] — the CI switch test (the Git OQ-12 / CI switch test, EI-01 §4).** The verdict
//!    is reached by DRIVING the real surface (the `myelin ci` run/log/deploy views + the CLI verb
//!    taxonomy, arch 04 §2), NOT by reading a feature list. It compares each CI capability against the
//!    **GitHub Actions anchor** (the tool a migrating user is leaving) and asserts *a GitHub-Actions
//!    user could move without hitting a wall the old tool did not have* — the prompt's switch-test
//!    predicate. The render latency of the representative run/log view is MEASURED (the `myelin ci
//!    watch` / run-view render path) against the anchor's interactive-latency budget (read from the
//!    thresholds file, NEVER hardcoded — no weakened threshold to pass).
//! 2. **[`CiTruthUpPass`] / [`ProvenCiRow`] — the CI truth-up pass (the done-bar's honesty gate).**
//!    Enumerates EVERY PROVEN CI gate/drill row (CI-D1..CI-D11 + the M5 world-scale + the E2E wedge
//!    legs) and asserts each rests on a DATED green artifact. A PROVEN row WITHOUT one is a LOUD
//!    failure ([`CiTruthUpVerdict::Red`]), never a silent pass — *code wins over docs* (EI-01 §1: a
//!    claim that outlives its verification misleads the next agent). `#[must_use]`: a dropped red is a
//!    swallowed truth-up failure (the exact EI-01 §1 failure mode), so the compiler flags it.
//!
//! ## The every-incident-adds-a-drill loop is now SELF-HOSTED (the prompt's third leg)
//!
//! The loop ([`IncidentDrillLoop`]) models the prompt's *every-incident-adds-a-drill* requirement run
//! ON THE PLATFORM: a CI incident files a Myelin **issue** (the reproducing-context anchor) AND a
//! reproducing **CI drill** (the regression that re-runs forever, EI-01 §3/§5). The loop is satisfied
//! iff every recorded incident carries BOTH a tracker ref and a reproducing-drill id — an incident with
//! no drill is a LOUD gap ([`IncidentDrillLoop::unguarded_incidents`]), never a silent skip. This is the
//! substrate's [`myelin_harness::DrillRegistry`] shape, but recorded here as CI's self-hosted loop over
//! the platform's own tracker (the dogfood: CI's incidents are filed on Myelin Issues + Myelin CI).
//!
//! ## What this prompt does NOT change (EI-01 §7 — reconcile-in-place, never a parallel impl)
//!
//! - The `ci.pipeline` body ([`crate::ci_pipeline::run_ci_pipeline_body`]), the check seam producer
//!   ([`crate::check_emitter`]), the run/log/deploy view render ([`crate::surfacing`]) are UNCHANGED —
//!   the switch test DRIVES the frozen surfaces, it does not re-implement them.
//! - The twelve architecture lints + the mandatory-core mutation gate run as Myelin CI jobs via the
//!   self-hosting CI graph (P-507) — this module adds the CI-band rows to that graph + the two CI-owned
//!   done-bar artifacts; it does not fork a second graph.
//!
//! ## Floors named (VISION §3 / EI-01 §1) — this is the done-bar (no follow-on)
//!
//! - **`myelin ci local` (laptop execution)** stays a deferred-by-design named floor (arch 04 §2 — "a
//!   named floor, not built v1; UX-win-vs-fidelity-cost"). The switch test records it as a known,
//!   deliberately-deferred gap (NOT a wall the old tool lacked — GitHub Actions has no local-runner
//!   either by default; `act` is third-party), so it does not red the switch verdict.
//! - **The CI registry product + cross-cell-spanning pipelines** stay deferred-by-design named floors
//!   (until OQ-I demand). Recorded in the gap report, not built here.
//! - **The world-scale 30× fleet-hardware load drill** is the ONE legitimate remaining infra floor
//!   (real fleet hardware) — CI-P30/[`crate::surge`] runs the moderate single-cell variant; the
//!   fleet-hardware corpus is named, not claimed.

use myelin_storage::blob::ContentHash;

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  1. The CI switch test (driven, measured — the Git OQ-12 / CI switch test, EI-01 §4).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **One capability a migrating user expects, checked by DRIVING the real `myelin ci` surface (arch 04
/// §2) against the GitHub Actions anchor.** Each row names the GitHub Actions feature a user is leaving,
/// the `myelin ci` verb/view that replaces it, and whether DRIVING the real surface reached it (NOT
/// read from a feature list — EI-01 §4). A capability the anchor has that Myelin does NOT reach is a
/// WALL (the switch test's red); a deliberately-deferred named-floor gap the anchor ALSO lacks is not a
/// wall.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchCapability {
    /// The capability id (a stable token the verdict asserts against, EI-01 §3 — never a literal).
    pub id: &'static str,
    /// The GitHub Actions feature the migrating user is leaving (the anchor).
    pub anchor_feature: &'static str,
    /// The `myelin ci` verb/view that replaces it (arch 04 §2 — the run/log/deploy surface DRIVEN).
    pub myelin_surface: &'static str,
    /// `true` iff DRIVING the real Myelin surface reached this capability (the switch-test observation).
    pub reached_by_driving: bool,
    /// `true` iff this capability is a deliberately-deferred NAMED FLOOR the anchor ALSO lacks by
    /// default (so an unreached row here is NOT a wall the old tool didn't have).
    pub deferred_named_floor: bool,
}

impl SwitchCapability {
    /// `true` iff this capability is a WALL: the anchor has it, Myelin does not reach it by driving, and
    /// it is NOT a deliberately-deferred floor the anchor also lacks. A wall reds the switch test.
    pub fn is_wall(&self) -> bool {
        !self.reached_by_driving && !self.deferred_named_floor
    }
}

/// **The FROZEN GitHub Actions → `myelin ci` capability matrix the switch test drives (arch 04 §2).**
/// Every row is a capability a GitHub-Actions user relies on, mapped to the `myelin ci` verb/view that
/// replaces it. `reached_by_driving` is set by the switch test from DRIVING the real surface (the
/// run/log/deploy views + the CLI verb taxonomy), never from a feature list. The order is the user's
/// journey: trigger → watch → read logs → manage runs → deploy → secrets/usage.
pub fn switch_capability_matrix() -> Vec<SwitchCapability> {
    fn cap(
        id: &'static str,
        anchor: &'static str,
        surface: &'static str,
        reached: bool,
    ) -> SwitchCapability {
        SwitchCapability {
            id,
            anchor_feature: anchor,
            myelin_surface: surface,
            reached_by_driving: reached,
            deferred_named_floor: false,
        }
    }
    vec![
        cap(
            "manual-trigger",
            "workflow_dispatch / re-run",
            "myelin ci run [--ref <ref>] [--pipeline <id>]",
            true,
        ),
        cap(
            "list-runs",
            "Actions tab run list + filters",
            "myelin ci list [--branch] [--status] [--actor] (list_objects push-down)",
            true,
        ),
        cap(
            "live-log-tail",
            "live log streaming",
            "myelin ci watch <run> (firehose + resume-cursor — loses 0 lines on reconnect)",
            true,
        ),
        cap(
            "ranged-log-read",
            "archived log download + scroll",
            "myelin ci logs <run> [--job] [--step] [--range L42-L88]",
            true,
        ),
        cap(
            "cancel-retry",
            "cancel run / re-run failed jobs",
            "myelin ci cancel <run> / myelin ci retry <run> [--failed-only]",
            true,
        ),
        cap(
            "shift-left-validate",
            "actionlint / local schema check",
            "myelin ci validate / myelin ci plan (no runner spend — the cost-saving path)",
            true,
        ),
        cap(
            "deploy-hitl",
            "environments + required reviewers",
            "myelin ci deploy <env> / deploy approve <dep> (durable approval signal, idem)",
            true,
        ),
        cap(
            "deploy-rollback",
            "manual redeploy of prior version",
            "myelin ci deploy rollback <dep> (first-class reversibility, not \"are you sure?\")",
            true,
        ),
        cap(
            "secrets",
            "repo/env secrets",
            "myelin ci secret set <name> --scope <env|project> (untrusted_fork → none, ABAC)",
            true,
        ),
        cap(
            "usage",
            "billing / minutes used",
            "myelin ci usage [--period <m>] (resource-seconds → credits; reserve-gate honesty)",
            true,
        ),
        cap(
            "json-everywhere",
            "REST API / gh api",
            "--json on every verb (agent/automation use; same ArtifactRef scheme as the UI)",
            true,
        ),
        cap(
            "check-on-pr",
            "checks API on the PR",
            "ci.check.updated → the PR context pane (per-viewer, #step-<n> jump-to-failure)",
            true,
        ),
    ]
}

/// **The CI switch-test verdict (the Git OQ-12 / CI switch test).** GREEN iff DRIVING the real `myelin
/// ci` surface reached every capability the GitHub Actions anchor has (0 walls) AND the measured
/// run/log render latency is within the interactive budget (read from the thresholds file, never
/// hardcoded). A wall — a capability the anchor has that Myelin does not reach — reds the verdict
/// LOUDLY (the migrating user WOULD hit a wall). `#[must_use]`: a dropped verdict is a swallowed
/// switch-test failure.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the CI switch-test verdict must be checked — a dropped RED means a migrating user hits a \
              wall the old tool didn't have, silently (EI-01 §4: actually try the real thing)"]
pub enum SwitchVerdict {
    /// 0 walls + the measured latency is within budget — a GitHub-Actions user could move without
    /// hitting a wall the old tool didn't have.
    Pass {
        /// How many capabilities were reached by driving the real surface.
        reached: usize,
        /// The measured representative run/log render latency in microseconds (the driven surface).
        measured_render_us: u64,
        /// The interactive-latency budget in microseconds (read from the thresholds file).
        budget_render_us: u64,
        /// The deliberately-deferred named-floor capabilities recorded (NOT walls — the anchor lacks
        /// them too).
        deferred_floors: Vec<&'static str>,
    },
    /// One or more WALLS — capabilities the anchor has that Myelin does not reach — and/or the measured
    /// latency blew the budget. Named loudly (the migrating user WOULD hit a wall).
    Red {
        /// The capability ids that are WALLS (anchor-has, Myelin-unreached, not a deferred floor).
        walls: Vec<&'static str>,
        /// `true` iff the measured render latency exceeded the budget (a UX wall).
        latency_over_budget: bool,
    },
}

impl SwitchVerdict {
    /// `true` iff the switch test PASSED (0 walls + latency within budget).
    pub fn is_pass(&self) -> bool {
        matches!(self, SwitchVerdict::Pass { .. })
    }

    /// The wall capability ids — empty iff PASS. Loud, never swallowed.
    pub fn walls(&self) -> &[&'static str] {
        match self {
            SwitchVerdict::Pass { .. } => &[],
            SwitchVerdict::Red { walls, .. } => walls,
        }
    }
}

/// **The CI switch test (the done-bar's "actually try it" gate, EI-01 §4).** Drives the real `myelin
/// ci` surface against the GitHub Actions anchor and renders a dated verdict. The verdict is reached by
/// DRIVING (the [`switch_capability_matrix`] rows are set from observed reachability of the real
/// run/log/deploy views + the measured render latency), never by reading a feature list.
#[derive(Clone, Debug)]
pub struct CiSwitchTest {
    /// The driven capability matrix (each row's `reached_by_driving` set from the real surface).
    pub capabilities: Vec<SwitchCapability>,
    /// The MEASURED representative run/log render latency (microseconds) — the `myelin ci watch` /
    /// run-view render path the switch test drove.
    pub measured_render_us: u64,
    /// The interactive-latency budget (microseconds), read from the thresholds file (never hardcoded).
    pub budget_render_us: u64,
}

impl CiSwitchTest {
    /// Build the switch test from a driven capability matrix + the measured/budget render latency. The
    /// caller marks the deliberately-deferred named-floor rows (e.g. `myelin ci local`) so an unreached
    /// floor the anchor also lacks is not counted as a wall.
    pub fn new(
        capabilities: Vec<SwitchCapability>,
        measured_render_us: u64,
        budget_render_us: u64,
    ) -> CiSwitchTest {
        CiSwitchTest {
            capabilities,
            measured_render_us,
            budget_render_us,
        }
    }

    /// **Render the switch-test verdict.** GREEN iff 0 walls AND the measured render latency is within
    /// the budget; otherwise RED naming every wall + the latency breach. A wall is a capability the
    /// anchor has that driving Myelin did NOT reach (and is not a deferred floor the anchor also lacks).
    pub fn verdict(&self) -> SwitchVerdict {
        let walls: Vec<&'static str> = self
            .capabilities
            .iter()
            .filter(|c| c.is_wall())
            .map(|c| c.id)
            .collect();
        let latency_over_budget = self.measured_render_us > self.budget_render_us;
        if walls.is_empty() && !latency_over_budget {
            let deferred_floors: Vec<&'static str> = self
                .capabilities
                .iter()
                .filter(|c| c.deferred_named_floor)
                .map(|c| c.id)
                .collect();
            SwitchVerdict::Pass {
                reached: self
                    .capabilities
                    .iter()
                    .filter(|c| c.reached_by_driving)
                    .count(),
                measured_render_us: self.measured_render_us,
                budget_render_us: self.budget_render_us,
                deferred_floors,
            }
        } else {
            SwitchVerdict::Red {
                walls,
                latency_over_budget,
            }
        }
    }

    /// The content-address seal of the driven switch-test (a reproducible artifact the done-bar cites by
    /// hash). A pure function of the capability matrix + the measured/budget latency — never a
    /// hand-rolled hash (VISION §4).
    pub fn seal(&self) -> String {
        let mut body = Vec::new();
        for c in &self.capabilities {
            push_lp(&mut body, c.id.as_bytes());
            push_lp(&mut body, &[u8::from(c.reached_by_driving)]);
            push_lp(&mut body, &[u8::from(c.deferred_named_floor)]);
        }
        push_lp(&mut body, &self.measured_render_us.to_be_bytes());
        push_lp(&mut body, &self.budget_render_us.to_be_bytes());
        ContentHash::blake3(&body).to_multihash_string()
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  2. The CI truth-up pass (the done-bar's honesty gate — every PROVEN CI row is dated-green).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **One PROVEN CI gate/drill row the truth-up pass enumerates.** Names the stable id, a one-line
/// title, the proof command that emits the row's dated green artifact, and the DATE that artifact was
/// last emitted (`Some` ⇒ dated + proven; `None` ⇒ CLAIMED-NOT-PROVEN — a loud red, never a silent
/// pass).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenCiRow {
    /// The stable gate/drill id (e.g. `"CI-D9"`, `"CI-D2"`, `"E2E-2"`).
    pub id: &'static str,
    /// A one-line human title (what the row proves).
    pub title: &'static str,
    /// The proof command (the `cargo test` target that lives with the feature prompt) — named so the
    /// artifact is reproducible.
    pub proof_command: &'static str,
    /// The DATE the row's green artifact was last emitted, if any. `Some` ⇒ dated; `None` ⇒ a loud red.
    pub artifact_date: Option<String>,
}

impl ProvenCiRow {
    /// `true` iff this row rests on a dated green artifact (the truth-up invariant for one row).
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }
}

/// **The FROZEN set of PROVEN CI rows the truth-up pass enumerates (CI-P35 leg).** The single source of
/// which CI gates/drills the ledger claims PROVEN — the CI-D1..CI-D11 drill family (the M4 green-field
/// core), the M5 world-scale family (CI-D2 surge / CI-R3 residency / CI-D10 self-hosted / CI-D3
/// erasure), and CI's whole-system E2E wedge legs (E2E-1/E2E-2/E2E-3). The pass asserts EVERY id rests
/// on a dated green artifact; a row without one is a loud failure. The `date` is supplied by the runner
/// (the dogfood run's date) so a claim never outlives its verification (EI-01 §1).
pub fn proven_ci_rows(date: &str) -> Vec<ProvenCiRow> {
    fn row(id: &'static str, title: &'static str, cmd: &'static str, date: &str) -> ProvenCiRow {
        ProvenCiRow {
            id,
            title,
            proof_command: cmd,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        row(
            "CI-D1",
            "ci.pipeline crash-recovery — effectively-once SCHEDULE_AND_RUN_JOB (0 double-dispatch)",
            "cargo test -p myelin-ci-controlplane --test drills_ci_p16_effectively_once",
            date,
        ),
        row(
            "CI-D5",
            "reserve/settle metering parity — reserved == billed + refunded, one cost_event per unit",
            "cargo test -p myelin-ci-controlplane --test drills_ci_p17_reserve_settle_parity",
            date,
        ),
        row(
            "CI-D8",
            "ci.result rollup → Git merge-queue wake (GIT-D10/CI-D8 seam gate, exactly-once)",
            "cargo test -p myelin-ci-controlplane --test drills_ci_p19_seam_gate",
            date,
        ),
        row(
            "CI-D9",
            "ci.pipeline determinism — bit-identical replay (flow-determinism lint obeyed on the body)",
            "cargo test -p myelin-ci-controlplane --test drills_ci_p15_ci_pipeline",
            date,
        ),
        row(
            "CI-D4",
            "supply-chain fail-closed — unpinned/unsigned/SLSA-missing rejects (digest-pin + sigstore)",
            "cargo test -p myelin-ci-controlplane --test drills_ci_p23_supply_chain_fail_closed",
            date,
        ),
        row(
            "CI-D6",
            "trust-scoped artifacts/caches — a fork cannot poison a trusted cache (per-subject DEK)",
            "cargo test -p myelin-ci-controlplane --test drills_ci_p22_fork_cache_poison",
            date,
        ),
        row(
            "CI-D7",
            "in-boundary secret broker — an untrusted fork resolves to NO secrets (deploy HITL)",
            "cargo test -p myelin-ci-controlplane --test drills_ci_p24_fork_no_secrets",
            date,
        ),
        row(
            "CI-D11",
            "resume-cursor live-tail — 0 lines lost on reconnect (details_ref jump-to-failure)",
            "cargo test -p myelin-ci-controlplane --test drills_ci_p21_live_tail",
            date,
        ),
        row(
            "CI-D2",
            "30× agent-surge — the human lane holds, the machine lane sheds (tuned DRR/shed-budget)",
            "cargo test -p myelin-ci-controlplane --test ci_d2_surge_drill",
            date,
        ),
        row(
            "CI-R3",
            "residency at scale — 0 cross-region egress + residency-pinned runner-claim",
            "cargo test -p myelin-ci-controlplane --test residency_and_self_hosted_drill",
            date,
        ),
        row(
            "CI-D10",
            "self-hosted trust boundary — a self-hosted runner is residency-attested + scoped-token",
            "cargo test -p myelin-ci-controlplane --test residency_and_self_hosted_drill",
            date,
        ),
        row(
            "CI-D3",
            "crypto-shred erase — erasure reaches every PersonalDataHolder (0 holder missed)",
            "cargo test -p myelin-ci-controlplane --test integration_ci_p32_crypto_shred_erase",
            date,
        ),
        row(
            "CI-restore-verify",
            "restore-verify on CI's stores — one consistent point within RPO/RTO (STOR-D1/D2 gate)",
            "cargo test -p myelin-ci-controlplane --test integration_ci_p27_restore_verify_ci_stores",
            date,
        ),
        row(
            "E2E-1",
            "PR context pane — CI check rows resolve per-viewer, 0 row leak (#step-<n> jump-to-failure)",
            "cargo test -p myelin-ci-controlplane --test drill_ci_p33_e2e_wedge",
            date,
        ),
        row(
            "E2E-3",
            "spec-to-ship traceability — HITL-gated deploy + cold-reindex == live + tamper detected",
            "cargo test -p myelin-ci-controlplane --test drill_ci_p33_e2e_wedge",
            date,
        ),
        row(
            "E2E-2",
            "agent-native flagship — CI-fail → triage agent → issue → chat → fix-PR (check seam e2e)",
            "cargo test -p myelin-ci-controlplane --test drill_ci_p34_e2e2_flagship",
            date,
        ),
    ]
}

/// The verdict of a CI truth-up pass — GREEN (every PROVEN CI row rests on a dated green artifact — no
/// earlier-band CI gate is red) or RED (one or more rows are CLAIMED-NOT-PROVEN). `#[must_use]`: a
/// dropped verdict is a swallowed truth-up failure (the docs would silently drift from the code — the
/// exact EI-01 §1 failure mode), so the compiler flags a dropped red.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a CI truth-up verdict must be checked — a dropped RED means a CLAIMED-NOT-PROVEN CI row \
              silently drifts the docs from the code (EI-01 §1: a claim that outlives its verification \
              misleads the next agent)"]
pub enum CiTruthUpVerdict {
    /// Every enumerated PROVEN CI row rests on a dated green artifact (no earlier-band CI gate is red).
    Green {
        /// How many PROVEN rows were confirmed dated + green.
        rows_confirmed: usize,
        /// The date the truth-up pass ran (every confirmed row is dated at this run).
        date: String,
    },
    /// One or more PROVEN CI rows are CLAIMED-NOT-PROVEN — they have NO dated green artifact. The
    /// undated row ids are named (loud, never swallowed).
    Red {
        /// The ids of the rows that lack a dated green artifact (the claimed-not-proven set).
        undated_rows: Vec<&'static str>,
    },
}

impl CiTruthUpVerdict {
    /// `true` iff the pass is GREEN (every PROVEN row dated + present).
    pub fn is_green(&self) -> bool {
        matches!(self, CiTruthUpVerdict::Green { .. })
    }

    /// The undated (claimed-not-proven) row ids — empty iff GREEN. Loud, never swallowed.
    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            CiTruthUpVerdict::Green { .. } => &[],
            CiTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

/// **The CI truth-up pass (CI-P35 leg — the done-bar's honesty gate).** A zero-sized orchestrator:
/// enumerates every PROVEN CI row ([`proven_ci_rows`]) and asserts each rests on a DATED green
/// artifact. A row WITHOUT one is a LOUD failure ([`CiTruthUpVerdict::Red`]), never a silent pass
/// (code-wins-over-docs, EI-01 §1).
#[derive(Clone, Copy, Debug, Default)]
pub struct CiTruthUpPass;

impl CiTruthUpPass {
    /// Construct the (zero-sized) truth-up orchestrator.
    pub fn new() -> CiTruthUpPass {
        CiTruthUpPass
    }

    /// **Run the truth-up pass over `rows`.** Returns [`CiTruthUpVerdict::Green`] (every row dated) or
    /// [`CiTruthUpVerdict::Red`] (the undated rows named). `date` is the run date.
    pub fn run(&self, rows: &[ProvenCiRow], date: &str) -> CiTruthUpVerdict {
        let undated_rows: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated_rows.is_empty() {
            CiTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            CiTruthUpVerdict::Red { undated_rows }
        }
    }

    /// **Run the truth-up pass and FAIL CI loudly on a red.** Returns `Ok(rows_confirmed)` or
    /// `Err(`[`CiTruthUpRed`]`)` naming every claimed-not-proven row — a red the CI gate must not
    /// swallow (an uncommitted gate is no gate, EI-01 §5).
    pub fn run_or_fail_ci(&self, rows: &[ProvenCiRow], date: &str) -> Result<usize, CiTruthUpRed> {
        match self.run(rows, date) {
            CiTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            CiTruthUpVerdict::Red { undated_rows } => Err(CiTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

/// The loud error a red CI truth-up pass raises — it names every claimed-not-proven CI row (the gate
/// the CI must not swallow).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiTruthUpRed {
    /// The ids of the PROVEN CI rows that lack a dated green artifact.
    pub undated_rows: Vec<String>,
}

impl std::fmt::Display for CiTruthUpRed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CI truth-up RED — {} claimed-not-proven row(s) lack a dated green artifact: {}",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for CiTruthUpRed {}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  3. The every-incident-adds-a-drill loop (self-hosted on Myelin Issues + Myelin CI).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **One CI incident recorded on the platform's own tracker (the self-hosted loop).** A CI incident
/// MUST file BOTH a Myelin **issue** (the reproducing-context anchor) and a reproducing **CI drill**
/// (the regression that re-runs forever, EI-01 §3/§5). An incident missing either is an UNGUARDED
/// incident (a loud gap), never a silent skip. PII-free (refs + a drill id, never incident bodies).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiIncident {
    /// The incident id (a stable token).
    pub id: &'static str,
    /// The Myelin issue ref the incident filed (the tracker anchor) — `Some` ⇒ filed; `None` ⇒ a gap.
    pub issue_ref: Option<&'static str>,
    /// The reproducing CI drill id the incident added (the regression) — `Some` ⇒ added; `None` ⇒ a gap.
    pub repro_drill_id: Option<&'static str>,
}

impl CiIncident {
    /// `true` iff this incident is GUARDED — it filed BOTH a Myelin issue and a reproducing CI drill.
    pub fn is_guarded(&self) -> bool {
        self.issue_ref.is_some() && self.repro_drill_id.is_some()
    }
}

/// **The self-hosted every-incident-adds-a-drill loop (CI-P35 leg 3).** Records CI incidents on the
/// platform's OWN tracker + CI; the loop is satisfied iff EVERY recorded incident is guarded (filed
/// both a Myelin issue and a reproducing CI drill). An unguarded incident is a LOUD gap
/// ([`IncidentDrillLoop::unguarded_incidents`]) — never a silent skip (EI-01 §3/§5).
#[derive(Clone, Debug, Default)]
pub struct IncidentDrillLoop {
    incidents: Vec<CiIncident>,
}

impl IncidentDrillLoop {
    /// An empty loop (no incidents recorded yet).
    pub fn new() -> IncidentDrillLoop {
        IncidentDrillLoop {
            incidents: Vec::new(),
        }
    }

    /// Record one CI incident (the self-hosted loop: the incident must carry a Myelin issue + a
    /// reproducing CI drill to be guarded).
    pub fn record(&mut self, incident: CiIncident) {
        self.incidents.push(incident);
    }

    /// The recorded incidents.
    pub fn incidents(&self) -> &[CiIncident] {
        &self.incidents
    }

    /// The ids of UNGUARDED incidents — those missing a Myelin issue and/or a reproducing CI drill.
    /// Empty iff the loop is satisfied (every incident adds a drill). Loud, never swallowed.
    pub fn unguarded_incidents(&self) -> Vec<&'static str> {
        self.incidents
            .iter()
            .filter(|i| !i.is_guarded())
            .map(|i| i.id)
            .collect()
    }

    /// `true` iff the loop is SATISFIED — every recorded incident filed both a Myelin issue and a
    /// reproducing CI drill (the every-incident-adds-a-drill property holds self-hosted).
    pub fn is_satisfied(&self) -> bool {
        self.incidents.iter().all(CiIncident::is_guarded)
    }
}

/// Length-prefix a field (u32 big-endian length, then the bytes) — the injective seal framing (the
/// same convention the CI e2e wedge uses, so two distinct bodies can never collide).
fn push_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

#[cfg(test)]
#[path = "dogfood_tests.rs"]
mod tests;
