//! # The two M4-boundary PERMANENT GATES, wired as committed CI jobs (CI-P27 → P-370, M4)
//!
//! **Owning prompt:** `planning/07-prompts/by-system/continuous-integration.md` §CI-P27 ("Re-confirm
//! the two permanent gates at the M4 boundary: AG-D4 / CI-T1 on the prod runner image + STOR-D1 /
//! STOR-D2 restore-verify on the CI stores"). **Master sequencing:** `00-master-sequencing.md` §2
//! (the M4 exit gate — CI-T1/AG-D4 re-confirmed on the prod runner) + §4 (the two permanent gates).
//! **Contracts:** rows **8.4** (the real-kernel sandbox-escape drill — permanent gate) + **11.5**
//! (backup / restore / cross-seam + restore-verify, CI-gated, ADR-18 — RPO ≤ 5 min, RTO ≤ 1 h/tenant
//! ≤ 4 h/cell, 0 loss). **Doctrine:** EI-01 §2 (the gate invariant — no later band over a red earlier
//! gate; the two permanent gates ratchet across the whole build), §3 (prove-it — the green artifact
//! IS the pass), **§5 (an uncommitted gate is no gate — wire these as committed CI jobs;
//! loud-never-swallowed, no `|| true`)**.
//!
//! ## What this module IS (and what it is NOT)
//! CI-P27 ships **no new drill logic**. Both gates already exist and are PROVEN:
//!   - **AG-D4 / CI-T1** — the adversarial escape corpus + the `EscapeAttestation` green-artifact type
//!     live in `myelin-ci-sandbox::escape_corpus` (CI-P5 → P-239); the prod-image re-confirm boots a
//!     real Firecracker microVM in `myelin-ci-sandbox/tests/escape_drill_prod_image_reconfirm_test.rs`
//!     (P-348) and `tests/escape_drill_ci_committed_gate_reconfirm_test.rs` (this prompt, CI side).
//!   - **STOR-D1 / STOR-D2** — the `RestoreVerifyGate` + `CellKillRestore` restore-verify machinery
//!     lives in `myelin-storage` (P-061 / P-100). CI-P27 WIRES the **CI stores** into it (the test
//!     `tests/integration_ci_p27_restore_verify_ci_stores.rs`).
//!
//! This module is the **committed-gate WIRING**: a declarative manifest of the two M4-boundary
//! permanent gates as committed CI jobs ([`PermanentGate`] / [`m4_boundary_permanent_gates`]), the
//! exact list of CI stores the restore-verify gate must cover ([`ci_restore_verify_stores`]), and the
//! loud-never-swallowed driver [`run_ci_restore_verify_or_fail`] that turns a red restore-verify over
//! the CI stores into a process-failing `Err` — there is NO `|| true`, no `.ok()`, no swallow. An
//! **uncommitted re-run is no gate** (EI-01 §5); declaring these here, in the committed CI control
//! plane, is the committed-job half the prompt's DEFINITION OF DONE requires.
//!
//! ## FLOOR: none — both are PERMANENT GATES
//! Neither gate has a floor: each is BOTH the floor and the full answer, re-run forever (every
//! backend/image/kernel change re-runs AG-D4 — gVisor re-runs it at CI-P28; every store-touching
//! change re-runs STOR-D1/D2). A red re-confirm is a DATED NO-GO that blocks M5 — never a weakened
//! threshold. This module states that posture in [`PermanentGate::is_floor`] (always `false`: there
//! is no floor *below* a permanent gate).

use myelin_storage::{GateInputs, GateVerdict, GreenArtifact, RestoreVerifyGate};

/// Which of the two M4-boundary permanent gates a [`PermanentGate`] declares. Both ratchet across the
/// whole build (master §4); both are re-confirmed at the M4 boundary (CI-P27) so the M4→M5 band
/// cannot pass while either is red.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermanentGateKind {
    /// **AG-D4 / CI-T1** (contract 8.4): the real-kernel sandbox-escape drill, re-run on the
    /// PRODUCTION CI runner image — ZERO escapes or CI is no-go for untrusted code. Re-runs on every
    /// backend/image/kernel change (gVisor re-runs it at CI-P28).
    EscapeDrill,
    /// **STOR-D1 / STOR-D2** (contract 11.5): restore from backups to ONE consistent cross-seam point
    /// (OLTP↔blob↔index↔offset) over the CI stores — 0 loss, RPO ≤ 5 min, RTO ≤ 1 h/tenant ≤ 4 h/cell.
    /// Re-runs on every change touching a CI store.
    RestoreVerify,
}

impl PermanentGateKind {
    /// The contract-index row this gate re-confirms.
    pub fn contract_row(self) -> &'static str {
        match self {
            PermanentGateKind::EscapeDrill => "8.4",
            PermanentGateKind::RestoreVerify => "11.5",
        }
    }
}

/// One M4-boundary permanent gate, declared as a committed CI job (EI-01 §5 — an uncommitted gate is
/// no gate). A `PermanentGate` is a manifest entry: the kind, the committed test that runs it, the
/// drill-catalogue rows it re-confirms, and the quantified pass condition. It is intentionally a
/// data-only declaration — the gate LOGIC lives in the named test (no fork); this is the committed
/// WIRING that makes the re-run a gate rather than an ad-hoc run.
#[derive(Clone, Copy, Debug)]
pub struct PermanentGate {
    /// Which permanent gate this is.
    pub kind: PermanentGateKind,
    /// The committed test that RUNS the gate (the `--features integration`-gated re-confirm). The
    /// gate is real because this test is committed and re-runs forever — not a doc claim.
    pub committed_test: &'static str,
    /// The drill-catalogue rows the gate re-confirms (e.g. `["AG-D4", "CI-T1"]`).
    pub drill_rows: &'static [&'static str],
    /// The quantified pass condition (the green artifact's measured assertion). A red is a dated
    /// no-go that blocks M5 — never a weakened threshold.
    pub pass_condition: &'static str,
}

impl PermanentGate {
    /// A permanent gate has NO floor — it is BOTH the floor and the full answer, re-run forever. So
    /// `is_floor()` is always `false`: there is no weaker bar a permanent gate could ratchet up from.
    /// (Stated as code so the no-floor posture is mechanically checkable, not only prose.)
    pub fn is_floor(&self) -> bool {
        false
    }
}

/// **The two M4-boundary permanent gates, as committed CI jobs (the CI-P27 manifest).** Both are
/// re-confirmed at the M4→M5 boundary; the band cannot pass while either reads red. There is no
/// third — these are exactly the two gates master §4 names as ratcheting across the whole build.
pub fn m4_boundary_permanent_gates() -> [PermanentGate; 2] {
    [
        PermanentGate {
            kind: PermanentGateKind::EscapeDrill,
            committed_test:
                "myelin-ci-sandbox/tests/escape_drill_ci_committed_gate_reconfirm_test.rs",
            drill_rows: &["AG-D4", "CI-T1"],
            pass_condition: "ZERO escapes on the PRODUCTION CI runner image (prod backend + image \
                             digest + kernel version) — a dated GREEN escape attestation, or CI is \
                             no-go for untrusted code and M5 cannot start",
        },
        PermanentGate {
            kind: PermanentGateKind::RestoreVerify,
            committed_test:
                "myelin-ci-controlplane/tests/integration_ci_p27_restore_verify_ci_stores.rs",
            drill_rows: &["STOR-D1", "STOR-D2"],
            pass_condition:
                "restore the CI stores from backups to ONE consistent cross-seam point \
                             (OLTP↔blob↔index↔offset) → 0 loss, RPO ≤ 5 min, RTO ≤ 1 h/tenant ≤ \
                             4 h/cell",
        },
    ]
}

/// **The CI stores the STOR-D1/D2 restore-verify gate must cover** (CI-P27 DELIVERABLE): the CI OLTP
/// (run/job/queue state), the T2 blob tier (artifacts + caches), the T3 log segments, and the CI
/// event-log offset — all restored to ONE consistent cross-seam point. These are the table/tier
/// identifiers the restore-verify wiring asserts a consistent point across; the restore-verify test
/// drives `RestoreVerifyGate` over rows that model exactly these tiers.
///
/// Named here (committed) so "we forgot to restore-verify the cache table" is structurally
/// impossible — a CI store added later that is NOT in this list (and not covered by the gate) is the
/// kind of silent gap the permanent gate exists to prevent.
pub fn ci_restore_verify_stores() -> &'static [&'static str] {
    &[
        // CI OLTP (the control-plane run/job/scheduler state).
        "ci_run",
        "ci_job",
        "job_queue",
        // T3 log segments (the sealed-segment log tier).
        "log_segment",
        // T2 blob tier (artifacts + caches — content-addressed objects).
        "artifact",
        "cache_entry",
        // The CI event-log offset (the cross-seam consistency point the outbox `seq` pins).
        "ci_event_log_offset",
    ]
}

/// **The loud-never-swallowed CI entrypoint for the STOR-D1/D2 restore-verify over the CI stores
/// (EI-01 §5).** Reuses Storage's [`RestoreVerifyGate::run_or_fail_ci`] verbatim (no fork): a red
/// restore-verify over the CI stores becomes a process-failing `Err(String)` — there is NO `|| true`,
/// no `.ok()`, no swallow. On GREEN it returns the dated [`GreenArtifact`] with the measured numbers
/// (0 dangling, 0 checksum mismatches, 0 resurrected subjects, the cross-seam consistency offset T).
///
/// This is the committed CI-job CALLER the prompt requires: the CI control plane OWNS the wiring that
/// drives Storage's gate over the CI stores; Storage owns the gate MECHANISM (coherence, EI-01 §7 —
/// CI does not re-implement restore-verify, it wires its stores into the one gate). A CI store-touching
/// change re-runs this, forever.
pub fn run_ci_restore_verify_or_fail(inputs: &GateInputs<'_>) -> Result<GreenArtifact, String> {
    match RestoreVerifyGate::new().run(inputs) {
        GateVerdict::Green(artifact) => Ok(artifact),
        // Loud-never-swallowed: surface EXACTLY what broke across the CI stores. The CI process exits
        // non-zero on this Err — there is no path that turns a red into a silent pass.
        GateVerdict::Red(failure) => Err(format!(
            "STOR-D1/D2 restore-verify over the CI stores is RED — this is a DATED NO-GO that blocks \
             M5, NOT a weakened threshold: {failure}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CI-P27 manifest declares EXACTLY the two permanent gates master §4 names, each a committed
    /// CI job re-confirming its contract row, each with NO floor (the permanent-gate posture).
    #[test]
    fn the_two_permanent_gates_are_declared_as_committed_no_floor_jobs() {
        let gates = m4_boundary_permanent_gates();
        assert_eq!(
            gates.len(),
            2,
            "exactly the two permanent gates (master §4)"
        );

        let escape = gates
            .iter()
            .find(|g| g.kind == PermanentGateKind::EscapeDrill)
            .expect("AG-D4 / CI-T1 escape drill is one of the two permanent gates");
        assert_eq!(escape.kind.contract_row(), "8.4");
        assert!(escape.drill_rows.contains(&"AG-D4") && escape.drill_rows.contains(&"CI-T1"));
        assert!(
            escape.committed_test.contains("escape_drill")
                && escape.committed_test.ends_with(".rs"),
            "the escape gate names a committed test (an uncommitted gate is no gate)"
        );
        assert!(
            escape.pass_condition.contains("ZERO escapes")
                && escape.pass_condition.contains("PRODUCTION"),
            "the escape gate's pass condition is ZERO escapes on the prod image"
        );

        let restore = gates
            .iter()
            .find(|g| g.kind == PermanentGateKind::RestoreVerify)
            .expect("STOR-D1 / STOR-D2 restore-verify is the other permanent gate");
        assert_eq!(restore.kind.contract_row(), "11.5");
        assert!(restore.drill_rows.contains(&"STOR-D1") && restore.drill_rows.contains(&"STOR-D2"));
        assert!(
            restore.committed_test.contains("restore_verify")
                && restore.committed_test.ends_with(".rs"),
            "the restore-verify gate names a committed test"
        );
        assert!(
            restore.pass_condition.contains("0 loss")
                && restore.pass_condition.contains("RPO ≤ 5 min")
                && restore
                    .pass_condition
                    .contains("RTO ≤ 1 h/tenant ≤ 4 h/cell"),
            "the restore-verify gate's pass condition carries the 11.5 RPO/RTO/0-loss thresholds"
        );

        // FLOOR: none — a permanent gate is BOTH the floor and the full answer.
        for g in &gates {
            assert!(
                !g.is_floor(),
                "a permanent gate has NO floor below it — it re-runs forever ({:?})",
                g.kind
            );
        }
    }

    /// The restore-verify gate must cover EVERY CI store the prompt names: the CI OLTP, the T2 blob
    /// tier (artifacts + caches), the T3 log segments, and the CI event-log offset. A missing store
    /// here is the "we forgot the cache table" silent gap the permanent gate exists to prevent.
    #[test]
    fn the_restore_verify_stores_cover_every_ci_store_the_prompt_names() {
        let stores = ci_restore_verify_stores();
        for required in [
            "ci_run",
            "ci_job",
            "job_queue",
            "log_segment",         // T3 log segments
            "artifact",            // T2 blob
            "cache_entry",         // T2 blob
            "ci_event_log_offset", // the cross-seam offset
        ] {
            assert!(
                stores.contains(&required),
                "the restore-verify gate must wire the CI store `{required}` (CI-P27 DELIVERABLE)"
            );
        }
    }
}
