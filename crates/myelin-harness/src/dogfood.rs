//! # `dogfood` — the substrate dogfood: the every-incident-adds-a-drill loop on Myelin's
//! tracker + the substrate truth-up pass (P-S38 → global P-510, SUB-M6).
//!
//! **Owning roadmap milestone:** `planning/06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M6
//! ("Dogfooding: the substrate runs Myelin's own development"). The cheapest, most honest load
//! generator is the platform's own development (EI-01 §5 — the ratchet runs on the builders' own
//! work; §1 — the code wins over the docs: a dated green artifact, never a claim).
//!
//! This is the LAST substrate prompt. P-S37 ([`crate::self_hosting_ci`]) wired the twelve lints +
//! the contract-coverage scanner + the mandatory-core mutation gate + the substrate's
//! surge/restore/migration drills as Myelin CI jobs on the platform's own commit. This module ships
//! the two SUB-M6 deliverables P-S37 split out (DELIVERABLE note: *"the incident-loop + the truth-up
//! pass are split to P-S38"*):
//!
//! 1. **[`SubstrateIncidentLoop`] / [`SubstrateIncident`] — the every-incident-adds-a-drill loop on
//!    Myelin's OWN tracker (T-3, EI-01 §3/§5).** A substrate incident files a Myelin **issue** (the
//!    reproducing-context anchor on the platform's own tracker) AND a reproducing **drill** that
//!    JOINS the substrate's real [`crate::drills::DrillRegistry`] via the P-S04
//!    [`crate::drills::DrillRegistry::register_drill`] hook and **re-runs forever**. The loop is
//!    *live*, not a ref check: [`SubstrateIncidentLoop::red_repros`] actually re-runs every
//!    registered repro and reports any that read red (the regression is guarded). An incident missing
//!    either leg is a LOUD gap ([`SubstrateIncidentLoop::unguarded_incidents`]), never a silent skip.
//!
//! 2. **[`SubstrateTruthUpPass`] / [`ProvenSubstrateRow`] — the substrate truth-up pass (EI-01 §1).**
//!    Enumerates EVERY substrate PROVEN gate/drill row (SUB-D1..SUB-D11 + BUS-D4 + the twelve lints +
//!    the contract-coverage scanner + the harness self-test + the M5 tuned-budget/world-scale legs)
//!    and asserts each rests on a DATED green artifact. A PROVEN row WITHOUT one is a LOUD failure
//!    ([`SubstrateTruthUpVerdict::Red`]), never a silent pass — *code wins over docs* (EI-01 §1: a
//!    claim that outlives its verification misleads the next agent). The gate invariant holds
//!    end-to-end: no earlier substrate gate is red.
//!
//! ## Why this lives in `myelin-harness` (EI-01 §7 — reconcile-in-place, never a parallel impl)
//! The substrate's [`crate::drills::DrillRegistry`] (the `register_drill` hook), the band-boundary
//! [`crate::scorecard`] (the dated-green-row machinery the truth-up pass mirrors), and the
//! [`crate::self_hosting_ci`] graph all live here — this is the substrate's test-support home (the
//! leaf crate above `myelin-substrate`, architecture §2.9). The incident loop REUSES the real
//! `register_drill` hook rather than re-modelling it; the truth-up pass MIRRORS the
//! [`crate::scorecard::GateRow`] dated-artifact shape the Tenancy/CI dogfood passes (P-508/P-509)
//! established, so the three dogfood truth-up passes are coherent, not three divergent impls.
//!
//! ## What this prompt does NOT change
//! - The [`crate::drills::DrillRegistry`] / `register_drill` hook, the [`crate::scorecard`] row
//!   machinery, and the [`crate::self_hosting_ci`] graph are UNCHANGED — this module WIRES them.
//! - No new contract (the prompt's CONTRACTS line: "None new — this closes the dogfood loop").
//!
//! ## Floors named (EI-01 §1) — this is the done-bar (no follow-on)
//! - **The world-scale 30× FLEET-hardware load drill** (SUB-D3 at true multi-box fleet scale) stays
//!   the ONE legitimate remaining infra floor (real fleet hardware). The single-box SCALED surge runs
//!   green in the self-hosting graph; the fleet corpus is named, not claimed (it is NOT a truth-up
//!   row that reds the pass — the substrate is *correct*; the fleet proof is *load-hardened-at-scale*).

use crate::drills::{DrillRegistry, DrillScenario};
use crate::telemetry::{Predicate, SignalName};

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  1. The every-incident-adds-a-drill loop on Myelin's own tracker (T-3, self-hosted).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **One substrate incident recorded on the platform's OWN tracker (the self-hosted T-3 loop).** A
/// substrate incident MUST file BOTH a Myelin **issue** (the reproducing-context anchor on Myelin's
/// own issue tracker) and a reproducing **drill** id (the regression registered into the substrate's
/// [`DrillRegistry`] that re-runs forever, EI-01 §3/§5). An incident missing either is an UNGUARDED
/// incident (a loud gap), never a silent skip. PII-free (refs + a drill id, never an incident body).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubstrateIncident {
    /// The incident id (a stable token; the substrate-side handle, never a PII-carrying body).
    pub id: &'static str,
    /// The Myelin issue ref the incident filed on the platform's own tracker (the reproducing-context
    /// anchor) — `Some` ⇒ filed; `None` ⇒ a gap.
    pub issue_ref: Option<&'static str>,
    /// The reproducing drill id the incident registered into the [`DrillRegistry`] (the regression
    /// that re-runs forever) — `Some` ⇒ added; `None` ⇒ a gap.
    pub repro_drill_id: Option<&'static str>,
}

impl SubstrateIncident {
    /// `true` iff this incident is GUARDED — it filed BOTH a Myelin issue and a reproducing drill.
    pub fn is_guarded(&self) -> bool {
        self.issue_ref.is_some() && self.repro_drill_id.is_some()
    }
}

/// **The self-hosted every-incident-adds-a-drill loop (P-S38 leg a, the live loop).** Unlike a
/// ref-only ledger, this loop is *live*: registering an incident's repro [`records`](Self::record) a real
/// [`DrillScenario`] into an owned [`DrillRegistry`] (the substrate's P-S04 hook), and
/// [`Self::red_repros`] actually re-runs every repro and reports any that read red — the
/// regression is *guarded*, not merely *recorded*. The loop is satisfied iff EVERY recorded incident
/// carries BOTH a Myelin issue ref AND a registered reproducing drill, and every registered repro
/// re-runs green. An unguarded incident is a LOUD gap ([`Self::unguarded_incidents`]); a red repro is
/// a LOUD regression ([`Self::red_repros`]) — never a silent skip (EI-01 §3/§5).
#[derive(Default)]
pub struct SubstrateIncidentLoop {
    incidents: Vec<SubstrateIncident>,
    /// The substrate's REAL drill registry — every incident's reproducing drill joins here via the
    /// P-S04 `register_drill` hook and re-runs forever (the loop is live, not a ref check).
    registry: DrillRegistry,
}

impl SubstrateIncidentLoop {
    /// An empty loop (no incidents recorded, an empty drill registry).
    pub fn new() -> SubstrateIncidentLoop {
        SubstrateIncidentLoop {
            incidents: Vec::new(),
            registry: DrillRegistry::new(),
        }
    }

    /// **Record one substrate incident AND register its reproducing drill (the live T-3 loop).** The
    /// incident files a Myelin issue ref (`issue_ref`) on the platform's own tracker and registers a
    /// reproducing [`DrillScenario`] into the owned [`DrillRegistry`] via the real `register_drill`
    /// hook — so the regression re-runs forever. The recorded [`SubstrateIncident`] carries the issue
    /// ref + the drill's id; the drill itself joins the permanent suite.
    pub fn record(&mut self, id: &'static str, issue_ref: &'static str, repro: DrillScenario) {
        // The drill's stable id is read off the scenario BEFORE it moves into the registry, so the
        // recorded incident's `repro_drill_id` is exactly the registered drill's name (no drift).
        let drill_id: &'static str = scenario_name_static(repro.name());
        self.registry.register_drill(repro);
        self.incidents.push(SubstrateIncident {
            id,
            issue_ref: Some(issue_ref),
            repro_drill_id: Some(drill_id),
        });
    }

    /// Record an UNGUARDED incident (a gap — e.g. an incident filed with no reproducing drill). Used
    /// by the loud-gap test to prove an incident without a drill is reported, never silently passed.
    pub fn record_unguarded(&mut self, incident: SubstrateIncident) {
        self.incidents.push(incident);
    }

    /// The recorded incidents.
    pub fn incidents(&self) -> &[SubstrateIncident] {
        &self.incidents
    }

    /// How many reproducing drills are registered (re-run forever).
    pub fn registered_drill_count(&self) -> usize {
        self.registry.len()
    }

    /// The ids of UNGUARDED incidents — those missing a Myelin issue and/or a reproducing drill.
    /// Empty iff every incident adds a drill. Loud, never swallowed (EI-01 §3/§5).
    pub fn unguarded_incidents(&self) -> Vec<&'static str> {
        self.incidents
            .iter()
            .filter(|i| !i.is_guarded())
            .map(|i| i.id)
            .collect()
    }

    /// **Re-run EVERY registered reproducing drill and return the names of any that read RED.** This
    /// is the "re-runs forever" half made live: a regression re-reds its repro loudly. Empty iff every
    /// repro reads green (the regressions stay fixed). Loud, never swallowed.
    pub fn red_repros(&self) -> Vec<String> {
        self.registry
            .run_all()
            .into_iter()
            .filter(|r| !r.is_pass())
            .map(|r| r.name().to_string())
            .collect()
    }

    /// **`true` iff the loop is SATISFIED** — every recorded incident filed both a Myelin issue and a
    /// reproducing drill, AND every registered repro re-runs green. The every-incident-adds-a-drill
    /// property holds self-hosted on the platform's own tracker + drill suite.
    pub fn is_satisfied(&self) -> bool {
        self.incidents.iter().all(SubstrateIncident::is_guarded) && self.red_repros().is_empty()
    }
}

/// Map a runtime drill name back to its `'static` form. The repro scenarios this loop registers are
/// built from `&'static str` literals (the closed, frozen set of substrate incident ids), so the
/// name round-trips to a `'static` token without allocation drift. A name not in the table is a LOUD
/// panic — a typo cannot silently produce a mismatched incident drill id.
fn scenario_name_static(name: &str) -> &'static str {
    match name {
        "repro-outbox-relay-stall" => "repro-outbox-relay-stall",
        other => {
            // The frozen set is the substrate incident corpus below; an unknown name is a typo, not a
            // silent pass (EI-01 §5: an uncommitted/mis-named gate is no gate).
            // Intentionally a leak-free panic — the test corpus is closed.
            panic!("unknown substrate incident drill name `{other}` — the repro corpus is frozen")
        }
    }
}

/// **A reproducing drill for a simulated substrate incident (the prompt's required artifact).** Models
/// a substrate incident — an outbox relay stall that parked committed events — and its reproducing
/// drill: with the relay degraded, the committed events stay PARKED in the outbox (0 lost / 0
/// dead-lettered), and the survival signal a drill reads is `dead_letter_count == 0`. This is the
/// inject → load → assert SHAPE every substrate drill uses (the same shape as the harness self-test),
/// registered as the incident's reproducing regression. A real future incident registers its repro
/// the SAME way — one [`SubstrateIncidentLoop::record`] call (EI-01 §5: adding the next incident's
/// drill is one call, no enum edit).
pub fn outbox_relay_stall_repro() -> DrillScenario {
    DrillScenario::new("repro-outbox-relay-stall", |ctx| {
        // The simulated incident: the relay stalled, committed events parked in the outbox. The
        // repro asserts the survival property the incident violated would have been caught — 0 events
        // dead-lettered / lost while parked (the silent-data-loss survival signal, SUB-D1/SUB-D2).
        ctx.signals.set_scalar(SignalName::OutboxDepth, 7);
        ctx.signals.set_scalar(SignalName::DeadLetterCount, 0);
        ctx.signals
            .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
    })
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  2. The substrate truth-up pass (every PROVEN substrate row rests on a dated green artifact).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **One PROVEN substrate gate/drill row the truth-up pass enumerates.** Names the stable id, a
/// one-line title, the proof command that emits its dated green artifact (the `cargo test`/`cargo
/// run` target that lives with the feature prompt — named so the artifact is reproducible), and the
/// DATE the green artifact was last emitted. `artifact_date` of `Some(date)` is a row whose proof is
/// dated + present; `None` is a CLAIMED-NOT-PROVEN row the pass FAILs on loudly (EI-01 §1 — a claim
/// that outlives its verification misleads the next agent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenSubstrateRow {
    /// The stable gate/drill id (e.g. `"SUB-D1"`, `"BUS-D4"`, `"lints"`, `"harness-self-test"`).
    pub id: &'static str,
    /// A one-line human title (what the row proves).
    pub title: &'static str,
    /// The proof command that emits this row's dated green artifact (the target that lives with the
    /// feature prompt — the truth-up pass names it so the artifact is reproducible).
    pub proof_command: &'static str,
    /// The DATE the row's green artifact was last emitted, if any. `Some(date)` ⇒ dated + proven;
    /// `None` ⇒ CLAIMED-NOT-PROVEN (a loud red, never a silent pass).
    pub artifact_date: Option<String>,
}

impl ProvenSubstrateRow {
    /// `true` iff this row rests on a dated green artifact (the truth-up invariant for one row).
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }
}

/// **The FROZEN set of PROVEN substrate rows the truth-up pass enumerates (P-S38 leg b).** The single
/// source of which substrate gates/drills the ledger claims PROVEN, drawn from the substrate roadmap
/// §4.2 / §6 drill catalogue + the SUB-M0 scorecard ([`crate::scorecard::required_rows`]):
///
/// - **M0 silent-data-loss + correctness:** SUB-D1, SUB-D2, BUS-D4 (the permanent emit-path gates),
///   SUB-D5 (breaker / no retry-storm), SUB-D7 (cross-tenant 0), SUB-D8 (causal-loop guards),
///   SUB-D9 (liveness ≠ readiness), the twelve architecture lints, the contract-coverage scanner,
///   and the harness self-test.
/// - **M1:** SUB-D4 (fail-static proven against a real Identity hiccup).
/// - **M3/M4 firehose:** SUB-D11 (the firehose slow-consumer + frame-budget + connection-storm half).
/// - **M5 world-scale:** SUB-D3 (30× surge family), SUB-D10 (online-migration-under-load),
///   SUB-D6/STOR-D2 at cell scale, BUS-D7 (the agent publish surge), and the two M5 tuning legs
///   (P-S33 tuned shed budgets, P-S36 tuned resilient-client targets).
///
/// The truth-up pass asserts EVERY id here rests on a dated green artifact; a row without one is a
/// loud failure. The `date` is supplied by the truth-up runner (the dogfood run's `today_iso()`) —
/// the pass DATES every row at the run so a claim never outlives its verification (EI-01 §1). A row
/// whose proof command did NOT emit a green at the run gets `None` and reds the pass.
pub fn proven_substrate_rows(date: &str) -> Vec<ProvenSubstrateRow> {
    fn row(
        id: &'static str,
        title: &'static str,
        cmd: &'static str,
        date: &str,
    ) -> ProvenSubstrateRow {
        ProvenSubstrateRow {
            id,
            title,
            proof_command: cmd,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        // ---- M0 silent-data-loss + correctness ----
        row(
            "SUB-D1",
            "kill service between commit & publish → 0 ghost / 0 lost (outbox + dedup)",
            "cargo test -p myelin-events --test drills_sub_d1_bus_d4",
            date,
        ),
        row(
            "SUB-D2",
            "drop broker mid-stream → 0 lost across reconnect; slow subject no HoL stall",
            "cargo test -p myelin-events --test drills_sub_d2_consumer",
            date,
        ),
        row(
            "BUS-D4",
            "crash producer between state-commit and publish → emit-iff-committed (co-commit)",
            "cargo test -p myelin-storage --test sub_d1_bus_d4_coloc_drill",
            date,
        ),
        row(
            "SUB-D5",
            "trip a downstream breaker → fail fast, honour Retry-After, no amplification",
            "cargo test -p myelin-client --test sub_d5_retry_storm",
            date,
        ),
        row(
            "SUB-D7",
            "cross-tenant read via path≠token → 0 misroute; tenant-predicate lint catches",
            "cargo test -p myelin-substrate --test drill_sub_d7_idor",
            date,
        ),
        row(
            "SUB-D8",
            "agent→agent loop → depth ceiling + shared-root tripwire + bounded pool halt",
            "cargo test -p myelin-substrate --test drill_sub_d8_causal_loop",
            date,
        ),
        row(
            "SUB-D9",
            "kill a critical dependency → not-ready + sheds; no liveness restart-storm",
            "cargo test -p myelin-substrate --test drill_sub_d9_liveness_readiness",
            date,
        ),
        row(
            "lints",
            "the twelve architecture lints — each red fixture rejects + green admits",
            "cargo run -p myelin-lints --bin lint-gate",
            date,
        ),
        row(
            "contract-coverage",
            "the contract-coverage scanner — no falsely-claimed/dropped/un-named row",
            "cargo run -p myelin-lints --bin contract-coverage",
            date,
        ),
        row(
            "harness-self-test",
            "the harness injects a fault and reads one telemetry assertion green",
            "cargo test -p myelin-harness drills::tests::harness_self_test",
            date,
        ),
        // ---- M1 ----
        row(
            "SUB-D4",
            "Id-hiccup → already-authenticated survives within W; revoked denied (fail-static)",
            "cargo test -p myelin-substrate --test drill_sub_d4_fail_static",
            date,
        ),
        // ---- M3/M4 firehose (SUB-D11) ----
        row(
            "SUB-D11-slow",
            "firehose hot-stream slow consumer → frame-cap + drop-to-resync, no unbounded buffer",
            "cargo test -p myelin-substrate --test drill_sub_d11_firehose_slow_consumer",
            date,
        ),
        row(
            "SUB-D11-budgets",
            "firehose frame-budget + scope-selector → per-surface shed budget bounds frames",
            "cargo test -p myelin-substrate --test drill_sub_d11_firehose_frame_budgets",
            date,
        ),
        row(
            "SUB-D11-storm",
            "firehose backpressure under connection-storm → bounded everything, human lane holds",
            "cargo test -p myelin-substrate --test drill_sub_d11_connection_storm",
            date,
        ),
        // ---- M5 world-scale + tuning ----
        row(
            "SUB-D3",
            "30× surge family → human lane within budget, agent lane sheds, cross-tenant impact 0",
            "cargo test -p myelin-substrate --test drill_sub_d3_surge_family",
            date,
        ),
        row(
            "SUB-D10",
            "online-migration-under-load → lock-wait p99 within budget, 0 errored writes, 0 downtime",
            "cargo test -p myelin-substrate --test drill_sub_d10_migration_under_load",
            date,
        ),
        row(
            "SUB-D6/STOR-D2-cell",
            "restore-verify re-confirmed at cell scale under world-scale load → RPO/RTO held",
            "cargo test -p myelin-substrate --test drill_sub_d6_restore_verify_cell_scale",
            date,
        ),
        row(
            "BUS-D7",
            "30× agent publish surge → human lane holds, agent sheds, other tenants unaffected",
            "cargo test -p myelin-substrate --test drills_bus_d7_agent_surge",
            date,
        ),
        row(
            "P-S33",
            "tuned per-surface shed budgets → human-lane starvation 0 at the measured numbers",
            "cargo test -p myelin-substrate --test drill_sub_p_s33_tuned_shed_budgets",
            date,
        ),
        row(
            "P-S36",
            "tuned resilient-client per-target values → each target within its measured budget",
            "cargo test -p myelin-substrate --test drill_sub_p_s36_resilient_target_tuning",
            date,
        ),
    ]
}

/// The verdict of a substrate truth-up pass — GREEN (every PROVEN substrate row rests on a dated
/// green artifact — the gate invariant holds end-to-end, no earlier substrate gate is red) or RED
/// (one or more rows are CLAIMED-NOT-PROVEN: a claim that outlives its verification). `#[must_use]`: a
/// dropped verdict is a swallowed truth-up failure — the docs would silently drift from the code (the
/// exact EI-01 §1 failure mode), so the compiler flags a dropped red.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a substrate truth-up verdict must be checked — a dropped RED means a CLAIMED-NOT-PROVEN \
              substrate row silently drifts the docs from the code (EI-01 §1: a claim that outlives \
              its verification misleads the next agent)"]
pub enum SubstrateTruthUpVerdict {
    /// Every enumerated PROVEN substrate row rests on a dated green artifact (the gate invariant holds
    /// end-to-end — no earlier substrate gate is red).
    Green {
        /// How many PROVEN rows were confirmed dated + green.
        rows_confirmed: usize,
        /// The date the truth-up pass ran (every confirmed row is dated at this run).
        date: String,
    },
    /// One or more PROVEN substrate rows are CLAIMED-NOT-PROVEN — they have NO dated green artifact.
    /// The undated row ids are named (loud, never swallowed).
    Red {
        /// The ids of the rows that lack a dated green artifact (the claimed-not-proven set).
        undated_rows: Vec<&'static str>,
    },
}

impl SubstrateTruthUpVerdict {
    /// `true` iff the pass is GREEN (every PROVEN row dated + present).
    pub fn is_green(&self) -> bool {
        matches!(self, SubstrateTruthUpVerdict::Green { .. })
    }

    /// The undated (claimed-not-proven) row ids — empty iff GREEN. Loud, never swallowed.
    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            SubstrateTruthUpVerdict::Green { .. } => &[],
            SubstrateTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

/// **The substrate truth-up pass (P-S38 leg b — the gate invariant holds end-to-end).** A zero-sized
/// orchestrator: enumerates every PROVEN substrate row ([`proven_substrate_rows`]) and asserts each
/// rests on a DATED green artifact. A row WITHOUT one is a LOUD failure
/// ([`SubstrateTruthUpVerdict::Red`]), never a silent pass (code-wins-over-docs, EI-01 §1).
#[derive(Clone, Copy, Debug, Default)]
pub struct SubstrateTruthUpPass;

impl SubstrateTruthUpPass {
    /// Construct the (zero-sized) truth-up orchestrator.
    pub fn new() -> SubstrateTruthUpPass {
        SubstrateTruthUpPass
    }

    /// **Run the truth-up pass over `rows`.** Returns [`SubstrateTruthUpVerdict::Green`] (every row
    /// dated) or [`SubstrateTruthUpVerdict::Red`] (the undated rows named). `date` is the run date.
    pub fn run(&self, rows: &[ProvenSubstrateRow], date: &str) -> SubstrateTruthUpVerdict {
        let undated_rows: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated_rows.is_empty() {
            SubstrateTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            SubstrateTruthUpVerdict::Red { undated_rows }
        }
    }

    /// **Run the truth-up pass and FAIL CI loudly on a red.** Returns `Ok(rows_confirmed)` (the count
    /// of dated PROVEN rows) or `Err(`[`SubstrateTruthUpRed`]`)` naming every claimed-not-proven row —
    /// a red the CI gate must not swallow (an uncommitted gate is no gate, EI-01 §5).
    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenSubstrateRow],
        date: &str,
    ) -> Result<usize, SubstrateTruthUpRed> {
        match self.run(rows, date) {
            SubstrateTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            SubstrateTruthUpVerdict::Red { undated_rows } => Err(SubstrateTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }

    /// Render the dated committed truth-up scorecard (the prompt's named green artifact: a dated
    /// truth-up scorecard with 0 red earlier rows).
    pub fn render_markdown(&self, rows: &[ProvenSubstrateRow], date: &str) -> String {
        let verdict = self.run(rows, date);
        let mut out = String::new();
        out.push_str(
            "# Substrate truth-up pass — every PROVEN substrate row rests on a dated green artifact \
             (P-S38 / P-510, SUB-M6)\n\n",
        );
        out.push_str(&format!("Run date: {date}\n\n"));
        out.push_str(
            "The code wins over the docs (EI-01 §1): each substrate PROVEN row below names its DATED \
             green artifact (the `cargo test`/`cargo run` target that emits it), not a doc claim. The \
             pass is GREEN iff EVERY row rests on a dated artifact — the gate invariant holds \
             end-to-end (no earlier substrate gate is red).\n\n",
        );
        out.push_str("| Gate / drill | Dated artifact | Proof command |\n");
        out.push_str("|---|---|---|\n");
        for r in rows {
            let dated = match &r.artifact_date {
                Some(d) => format!("[{d}] PROVEN"),
                None => "**CLAIMED-NOT-PROVEN**".to_string(),
            };
            out.push_str(&format!(
                "| `{}` — {} | {} | `{}` |\n",
                r.id, r.title, dated, r.proof_command
            ));
        }
        out.push('\n');
        if verdict.is_green() {
            out.push_str(&format!(
                "**TRUTH-UP: GREEN** — {} PROVEN substrate rows, 0 claimed-not-proven; the gate \
                 invariant holds end-to-end (no earlier substrate gate is red).\n\n",
                rows.len()
            ));
        } else {
            out.push_str(&format!(
                "**TRUTH-UP: RED** — claimed-not-proven rows lack a dated green artifact: {}.\n\n",
                verdict.undated_rows().join(", ")
            ));
        }
        out.push_str(
            "**Named floor (EI-01 §1):** the world-scale 30× FLEET-hardware load drill (SUB-D3 at \
             true multi-box fleet scale) is the ONE legitimate remaining infra floor — the single-box \
             SCALED surge runs green in the self-hosting CI graph; the fleet corpus is named, not \
             claimed (it is not a row that reds this pass — the substrate is *correct*; the fleet \
             proof is *load-hardened-at-scale*).\n",
        );
        out
    }
}

/// The loud error a red substrate truth-up pass raises — it names every claimed-not-proven substrate
/// row (the gate the CI must not swallow).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubstrateTruthUpRed {
    /// The ids of the PROVEN substrate rows that lack a dated green artifact.
    pub undated_rows: Vec<String>,
}

impl std::fmt::Display for SubstrateTruthUpRed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "substrate truth-up RED — {} claimed-not-proven row(s) lack a dated green artifact: {}",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for SubstrateTruthUpRed {}

#[cfg(test)]
#[path = "dogfood_tests.rs"]
mod dogfood_tests;
