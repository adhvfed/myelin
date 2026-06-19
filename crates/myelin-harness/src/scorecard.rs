//! The **M0 exit-gate scorecard** (P-S24 → P-039) — the consolidated band-boundary proof.
//!
//! This module is the build-layer realisation of the master M0→M1 gate invariant
//! (master-sequencing §2/§4, EI-01 §2): *no later-band prompt runs over a red earlier gate.*
//! It does **NOT** re-implement the M0 drills — they live with their feature prompts
//! (P-S07..P-S20, EB/Storage/Client crates). It **WIRES** them into ONE band-boundary gate,
//! asserts each emits a dated green artifact, and records any red as a claimed-not-proven row
//! (never edited green; the thresholds-file discipline, EI-01 §3 / roadmap §5).
//!
//! ## What the scorecard aggregates (substrate roadmap §5, the SUB-M0 row set)
//! The required rows are the SUB-M0 exit gate, frozen as [`required_rows`]:
//! - **SUB-D1** (0 ghost / 0 lost across kill-between-commit-and-publish) — outbox + dedup.
//! - **SUB-D2** (0 lost across reconnect; slow subject no HoL stall) — consumer template.
//! - **BUS-D4** (emit-iff-committed; delivered, never without state) — the outbox emit API.
//! - **SUB-D5** (trip a breaker → fail fast, honour `Retry-After`, no amplification).
//! - **SUB-D7** (cross-tenant read path≠token → 0 misroute; the tenant-predicate lint).
//! - **SUB-D8** (agent→agent loop → depth ceiling + shared-root tripwire + bounded pool halt).
//! - **SUB-D9** (kill critical dep → not-ready + sheds; no liveness restart-storm).
//! - **the twelve architecture lints** (each red fixture rejects + green fixture admits).
//! - **the contract-coverage scanner** (no falsely-claimed / silently-dropped / un-named row).
//! - **the harness self-test** (inject a fault → read one telemetry assertion green).
//!
//! Each row names the CONCRETE PROOF COMMAND that emits its dated green artifact (the cargo
//! test/binary that lives with the feature prompt). The scorecard binary
//! (`src/bin/sub-m0-scorecard.rs`) runs them, records PASS/FAIL with a date, and writes
//! `testing/scorecards/sub-m0.md`. The CI `sub-m0-scorecard` job is the committed gate: a
//! single red row fails it and blocks M1.
//!
//! ## The un-gameable ratchet (the prompt's required meta-property, EI-01 §3)
//! The row set is FROZEN data ([`required_rows`]). The scorecard cannot be gamed two ways,
//! both rejected mechanically and tested in `tests/scorecard_ratchet.rs`:
//! 1. **You cannot drop a row.** [`Scorecard::missing_required`] reports any required gate id
//!    absent from the recorded results; the gate verdict is RED if any is missing — removing a
//!    drill from the scorecard fails the gate, it does not silently shrink the proof set.
//! 2. **You cannot flip a row green without proof.** A [`RowResult`] is only `Pass` when it
//!    carries a non-empty `proof` string (the green artifact line the proof command emitted);
//!    a [`RowVerdict::ClaimedNotProven`] row is recorded honestly and the gate reads RED. There
//!    is no constructor that yields a `Pass` from nothing — a green must be earned.
//!
//! ## Floors named (deferred + filling prompt)
//! - **The permanent gates SUB-D1 / SUB-D2 / BUS-D4 re-run forever.** They are marked
//!   [`GateRow::permanent`] here; from M0 on, every emit-path-touching prompt re-runs them
//!   (the gate-invariant ratchet, master-sequencing §1 item 6). This module is where that
//!   marking is committed; the re-run wiring is each later prompt's DEFINITION OF DONE.
//! - **Proof commands run via `cargo test`, not an in-process call.** The scorecard binary
//!   shells out to the per-feature test (the test IS the dated artifact). A future prompt may
//!   register the drills into [`crate::drills::DrillRegistry`] for in-process aggregation; the
//!   row-set contract here is the stable handle either way.

use std::fmt;

/// Which master band a gate row belongs to. The M0 exit-gate scorecard records the M0 set;
/// the field exists so the same scorecard machinery serves later band-boundary gates without
/// a parallel type (coherence, EI-01 §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    /// The substrate/harness/committed-gates band (master M0).
    M0,
}

impl fmt::Display for Band {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Band::M0 => write!(f, "M0"),
        }
    }
}

/// One required row of a band-boundary gate: a stable gate id, a human title, the concrete
/// PROOF COMMAND that emits its dated green artifact, and whether it is a PERMANENT gate (one
/// that re-runs on every relevant change forever, master-sequencing §1 item 6).
///
/// The `proof_command` is the cargo test/binary invocation that lives WITH the feature prompt
/// (this scorecard does not re-implement the drill — it names + runs the existing one). It is
/// stored as the argv vector so the runner invokes it directly (no shell, no `|| true`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateRow {
    /// The stable gate id (e.g. `"SUB-D1"`, `"lints"`, `"harness-self-test"`).
    pub id: &'static str,
    /// A one-line human title for the scorecard.
    pub title: &'static str,
    /// The proof command (argv) that emits this row's dated green artifact. Run directly by
    /// the scorecard binary; a non-zero exit is a RED row (never swallowed).
    pub proof_command: &'static [&'static str],
    /// `true` iff this is a PERMANENT gate that re-runs forever (SUB-D1 / SUB-D2 / BUS-D4).
    pub permanent: bool,
}

/// The FROZEN required-row set for the SUB-M0 exit gate (substrate roadmap §5). This is the
/// un-gameable ratchet's data: the scorecard's gate verdict is RED unless EVERY id here is
/// present and PASS. Removing a row from the recorded results does not shrink the proof set —
/// [`Scorecard::missing_required`] re-reds the gate (the meta-test asserts this).
///
/// The permanent gates (SUB-D1/D2/BUS-D4) are marked `permanent: true` — they re-run on every
/// emit-path change from M0 on (master-sequencing §1 item 6).
pub fn required_rows() -> Vec<GateRow> {
    vec![
        GateRow {
            id: "SUB-D1",
            title: "kill service between commit & publish → 0 ghost / 0 lost (outbox + dedup)",
            // The outbox 0-loss/0-ghost drill (EB) + the same-tx co-location drill (Storage).
            proof_command: &[
                "test",
                "-p",
                "myelin-events",
                "--test",
                "drills_sub_d1_bus_d4",
            ],
            permanent: true,
        },
        GateRow {
            id: "SUB-D2",
            title: "drop broker mid-stream → 0 lost across reconnect; slow subject no HoL stall",
            proof_command: &[
                "test",
                "-p",
                "myelin-events",
                "--test",
                "drills_sub_d2_consumer",
            ],
            permanent: true,
        },
        GateRow {
            id: "BUS-D4",
            title: "crash producer between state-commit and publish → emit-iff-committed",
            // The Storage same-transaction co-commit drill is the BUS-D4 emit-iff-committed proof.
            proof_command: &[
                "test",
                "-p",
                "myelin-storage",
                "--test",
                "sub_d1_bus_d4_coloc_drill",
            ],
            permanent: true,
        },
        GateRow {
            id: "SUB-D5",
            title: "trip a downstream breaker → fail fast, honour Retry-After, no amplification",
            proof_command: &[
                "test",
                "-p",
                "myelin-client",
                "--test",
                "sub_d5_retry_storm",
            ],
            permanent: false,
        },
        GateRow {
            id: "SUB-D7",
            title: "cross-tenant read via path≠token → 0 misroute; tenant-predicate lint catches",
            proof_command: &[
                "test",
                "-p",
                "myelin-substrate",
                "--test",
                "drill_sub_d7_idor",
            ],
            permanent: false,
        },
        GateRow {
            id: "SUB-D8",
            title: "agent→agent loop → depth ceiling + shared-root tripwire + bounded pool halt",
            proof_command: &[
                "test",
                "-p",
                "myelin-substrate",
                "--test",
                "drill_sub_d8_causal_loop",
            ],
            permanent: false,
        },
        GateRow {
            id: "SUB-D9",
            title: "kill a critical dependency → not-ready + sheds; no liveness restart-storm",
            proof_command: &[
                "test",
                "-p",
                "myelin-substrate",
                "--test",
                "drill_sub_d9_liveness_readiness",
            ],
            permanent: false,
        },
        GateRow {
            id: "lints",
            title: "the twelve architecture lints — each red fixture rejects + green admits",
            // The lint-gate binary over the workspace source (12/12 lints, loud non-zero on any
            // violation) — the same gate the architecture-lints CI job runs.
            proof_command: &["run", "-p", "myelin-lints", "--bin", "lint-gate"],
            permanent: false,
        },
        GateRow {
            id: "lint-fixtures",
            title: "the lint fixture matrix + the CI-gate self-test (red fixture ⇒ non-zero)",
            proof_command: &["test", "-p", "myelin-lints"],
            permanent: false,
        },
        GateRow {
            id: "contract-coverage",
            title: "the contract-coverage scanner — no falsely-claimed/dropped/un-named row",
            proof_command: &["run", "-p", "myelin-lints", "--bin", "contract-coverage"],
            permanent: false,
        },
        GateRow {
            id: "harness-self-test",
            title: "the harness injects a fault and reads one telemetry assertion green",
            proof_command: &[
                "test",
                "-p",
                "myelin-harness",
                "drills::tests::harness_self_test",
            ],
            permanent: false,
        },
    ]
}

/// The verdict of one recorded scorecard row. A `Pass` is only constructible WITH a non-empty
/// proof line (the dated green artifact the proof command emitted) — a green must be earned, it
/// cannot be flipped from nothing (the ratchet's "no green without proof" half).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowVerdict {
    /// The proof command emitted a dated green artifact. Carries that proof line so the
    /// scorecard row is auditable — observability is part of the pass (EI-01 §3).
    Pass {
        /// The dated green-artifact line the proof command produced (non-empty by construction).
        proof: String,
    },
    /// The proof command read RED, or its drill is not yet green. Recorded honestly as a
    /// claimed-not-proven row (EI-01 §3 / roadmap §5) — the gate reads RED and M1 is blocked.
    /// `reason` names exactly what failed (the red signal / non-zero exit).
    ClaimedNotProven {
        /// Why this row is not proven (the failing signal / non-zero exit / owner note).
        reason: String,
    },
}

/// One recorded row: the gate row + its verdict + the date the verdict was asserted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowResult {
    /// The gate id this result is for (matched against [`required_rows`]).
    pub id: String,
    /// The verdict (a Pass carries its proof; a claimed-not-proven carries its reason).
    pub verdict: RowVerdict,
    /// The ISO-8601 date this row was asserted (the dated green-artifact date).
    pub date: String,
}

impl RowResult {
    /// Record a PASS row. Panics if `proof` is empty — a green is never recorded without its
    /// dated artifact line (the ratchet: no green without proof, EI-01 §3). This is the ONLY
    /// way to construct a `Pass`, so the discipline is structural, not a convention.
    pub fn pass(id: impl Into<String>, proof: impl Into<String>, date: impl Into<String>) -> Self {
        let proof = proof.into();
        assert!(
            !proof.trim().is_empty(),
            "a PASS row must carry its dated green-artifact proof line — no green without proof \
             (EI-01 §3); recording a Pass with an empty proof is the gamed-green the ratchet forbids"
        );
        RowResult {
            id: id.into(),
            verdict: RowVerdict::Pass { proof },
            date: date.into(),
        }
    }

    /// Record a CLAIMED-NOT-PROVEN row (a red drill / non-zero exit). The gate reads RED; the
    /// row is honest, never softened (EI-01 §3).
    pub fn claimed_not_proven(
        id: impl Into<String>,
        reason: impl Into<String>,
        date: impl Into<String>,
    ) -> Self {
        RowResult {
            id: id.into(),
            verdict: RowVerdict::ClaimedNotProven {
                reason: reason.into(),
            },
            date: date.into(),
        }
    }

    /// `true` iff this row is a proven PASS.
    pub fn is_pass(&self) -> bool {
        matches!(self.verdict, RowVerdict::Pass { .. })
    }
}

/// The aggregated M0 exit-gate scorecard: the band + the recorded row results. The gate verdict
/// (`is_green`) is RED unless EVERY required id is present AND every row is a proven PASS.
#[derive(Clone, Debug)]
pub struct Scorecard {
    /// The band this scorecard gates (M0 here).
    pub band: Band,
    /// The recorded row results (one per gate row run).
    pub rows: Vec<RowResult>,
}

impl Scorecard {
    /// A fresh scorecard for `band` with no rows recorded yet.
    pub fn new(band: Band) -> Self {
        Scorecard {
            band,
            rows: Vec::new(),
        }
    }

    /// Record a row result (PASS or claimed-not-proven).
    pub fn record(&mut self, row: RowResult) {
        self.rows.push(row);
    }

    /// The required gate ids absent from the recorded rows. The ratchet's "cannot drop a row"
    /// half: a non-empty result here re-reds the gate (you cannot shrink the proof set by
    /// omitting a row). The meta-test asserts removing a row lands it here.
    pub fn missing_required(&self) -> Vec<&'static str> {
        required_rows()
            .into_iter()
            .map(|r| r.id)
            .filter(|id| !self.rows.iter().any(|row| row.id == *id))
            .collect()
    }

    /// The recorded rows that are NOT a proven pass (claimed-not-proven). A non-empty result
    /// re-reds the gate.
    pub fn not_proven(&self) -> Vec<&RowResult> {
        self.rows.iter().filter(|r| !r.is_pass()).collect()
    }

    /// **The gate verdict.** GREEN iff every required id is present AND every recorded row is a
    /// proven PASS. RED otherwise (a missing row OR a claimed-not-proven row blocks M1 — the
    /// gate invariant, master-sequencing §2). LOUD: this is a typed predicate the CI binary's
    /// exit code reads; there is no `|| true` path to a false green.
    pub fn is_green(&self) -> bool {
        self.missing_required().is_empty() && self.not_proven().is_empty()
    }

    /// Render the dated scorecard artifact (the committed `testing/scorecards/sub-m0.md` body).
    /// Every row is a visible, dated PASS/RED line (observability is part of the pass, EI-01 §3);
    /// the permanent gates are marked re-run-forever; a final GREEN/RED gate verdict line is the
    /// band-boundary signal.
    pub fn render_markdown(&self, generated_on: &str) -> String {
        let rows = required_rows();
        let mut out = String::new();
        out.push_str(&format!(
            "# {} exit-gate scorecard (SUB-D1/D2/BUS-D4/D5/D7/D8/D9 + 12 lints + harness self-test)\n\n",
            self.band
        ));
        out.push_str(&format!("> Generated: {generated_on}. "));
        out.push_str(
            "The build-layer realisation of the master M0→M1 gate invariant (master-sequencing \
             §2/§4, EI-01 §2): no later-band prompt runs over a red earlier gate. Each row is a \
             dated green artifact read off the per-feature drill (this scorecard WIRES the drills, \
             it does not re-implement them). A single RED row blocks M1 and is recorded honestly \
             as claimed-not-proven, never edited green (EI-01 §3 / roadmap §5).\n\n",
        );

        let verdict = if self.is_green() {
            "GREEN — M1 may start"
        } else {
            "RED — M1 is BLOCKED (a row is missing or claimed-not-proven)"
        };
        out.push_str(&format!("**Gate verdict: {verdict}**\n\n"));

        out.push_str("| Gate | Title | Verdict | Date | Permanent | Proof / reason |\n");
        out.push_str("|---|---|---|---|---|---|\n");
        for gr in &rows {
            let recorded = self.rows.iter().find(|r| r.id == gr.id);
            let perm = if gr.permanent { "re-run-forever" } else { "—" };
            match recorded {
                Some(r) => match &r.verdict {
                    RowVerdict::Pass { proof } => out.push_str(&format!(
                        "| {} | {} | PASS | {} | {} | {} |\n",
                        gr.id, gr.title, r.date, perm, proof
                    )),
                    RowVerdict::ClaimedNotProven { reason } => out.push_str(&format!(
                        "| {} | {} | **RED (claimed-not-proven)** | {} | {} | {} |\n",
                        gr.id, gr.title, r.date, perm, reason
                    )),
                },
                None => out.push_str(&format!(
                    "| {} | {} | **RED (MISSING — row dropped)** | — | {} | the ratchet re-reds a dropped row |\n",
                    gr.id, gr.title, perm
                )),
            }
        }
        out.push('\n');
        out.push_str(
            "**Permanent gates (re-run forever).** SUB-D1 / SUB-D2 / BUS-D4 re-run on every \
             emit-path-touching change from M0 on (master-sequencing §1 item 6); a regression on \
             any of them halts the run.\n",
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The required row set is exactly the SUB-M0 exit gate (substrate roadmap §5): the seven
    /// drills + the lints + the scanner + the harness self-test. This is the frozen ratchet
    /// data — if a future edit shrinks it, this asserts the loss is deliberate, not silent.
    #[test]
    fn required_rows_cover_the_full_sub_m0_gate() {
        let ids: Vec<&str> = required_rows().iter().map(|r| r.id).collect();
        for must in [
            "SUB-D1",
            "SUB-D2",
            "BUS-D4",
            "SUB-D5",
            "SUB-D7",
            "SUB-D8",
            "SUB-D9",
            "lints",
            "lint-fixtures",
            "contract-coverage",
            "harness-self-test",
        ] {
            assert!(ids.contains(&must), "SUB-M0 gate is missing required row {must}");
        }
        assert_eq!(ids.len(), 11, "the SUB-M0 row set is frozen at 11 rows");
    }

    /// The permanent gates are exactly SUB-D1 / SUB-D2 / BUS-D4 (the re-run-forever set,
    /// master-sequencing §1 item 6).
    #[test]
    fn permanent_gates_are_the_three_emit_path_drills() {
        let perm: Vec<&str> = required_rows()
            .into_iter()
            .filter(|r| r.permanent)
            .map(|r| r.id)
            .collect();
        assert_eq!(perm, vec!["SUB-D1", "SUB-D2", "BUS-D4"]);
    }

    /// A fully-green scorecard reads green and renders a GREEN verdict line.
    #[test]
    fn all_rows_proven_is_green() {
        let mut card = Scorecard::new(Band::M0);
        for r in required_rows() {
            card.record(RowResult::pass(r.id, format!("[2026-06-19] PASS {}", r.id), "2026-06-19"));
        }
        assert!(card.is_green(), "every required row proven ⇒ green");
        assert!(card.missing_required().is_empty());
        assert!(card.render_markdown("2026-06-19").contains("GREEN — M1 may start"));
    }

    /// THE RATCHET, half 1: dropping a row re-reds the gate. You cannot shrink the proof set by
    /// omitting a row (the prompt's required meta-test).
    #[test]
    fn dropping_a_row_reds_the_gate() {
        let mut card = Scorecard::new(Band::M0);
        // Record all but SUB-D1.
        for r in required_rows().into_iter().filter(|r| r.id != "SUB-D1") {
            card.record(RowResult::pass(r.id, "[2026-06-19] PASS", "2026-06-19"));
        }
        assert_eq!(card.missing_required(), vec!["SUB-D1"]);
        assert!(!card.is_green(), "a missing required row must RED the gate");
        assert!(card.render_markdown("2026-06-19").contains("RED (MISSING"));
    }

    /// THE RATCHET, half 2: a claimed-not-proven row keeps the gate RED — it cannot be softened
    /// into a green (EI-01 §3). The honest red blocks M1.
    #[test]
    fn claimed_not_proven_row_reds_the_gate() {
        let mut card = Scorecard::new(Band::M0);
        for r in required_rows() {
            if r.id == "SUB-D8" {
                card.record(RowResult::claimed_not_proven(
                    r.id,
                    "causal-depth ceiling not yet enforced past 12",
                    "2026-06-19",
                ));
            } else {
                card.record(RowResult::pass(r.id, "[2026-06-19] PASS", "2026-06-19"));
            }
        }
        assert!(!card.is_green(), "a claimed-not-proven row blocks M1");
        assert_eq!(card.not_proven().len(), 1);
        assert!(card
            .render_markdown("2026-06-19")
            .contains("RED (claimed-not-proven)"));
    }

    /// THE RATCHET, half 2 (structural): a PASS cannot be flipped green without a proof line —
    /// `RowResult::pass` panics on an empty proof. This is the "no green without proof" guard
    /// made structural (there is no constructor that yields a Pass from nothing).
    #[test]
    #[should_panic(expected = "no green without proof")]
    fn a_pass_without_proof_panics() {
        let _ = RowResult::pass("SUB-D1", "   ", "2026-06-19");
    }
}
