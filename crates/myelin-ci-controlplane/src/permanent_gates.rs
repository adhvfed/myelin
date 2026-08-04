use myelin_storage::{GateInputs, GateVerdict, GreenArtifact, RestoreVerifyGate};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermanentGateKind {
    EscapeDrill,
    RestoreVerify,
}

impl PermanentGateKind {
    pub fn contract_row(self) -> &'static str {
        match self {
            PermanentGateKind::EscapeDrill => "8.4",
            PermanentGateKind::RestoreVerify => "11.5",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PermanentGate {
    pub kind: PermanentGateKind,
    pub committed_test: &'static str,
    pub drill_rows: &'static [&'static str],
    pub pass_condition: &'static str,
}

impl PermanentGate {
    pub fn is_floor(&self) -> bool {
        false
    }
}

pub fn m4_boundary_permanent_gates() -> [PermanentGate; 2] {
    [
        PermanentGate {
            kind: PermanentGateKind::EscapeDrill,
            committed_test:
                "myelin-ci-sandbox/tests/escape_drill_ci_committed_gate_reconfirm_test.rs",
            drill_rows: &["AG-D4", "CI-T1"],
            pass_condition: "ZERO escapes on the PRODUCTION CI runner image (prod backend + image \
                             digest + kernel version) - a dated GREEN escape attestation, or CI is \
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

pub fn ci_restore_verify_stores() -> &'static [&'static str] {
    &[
        "ci_run",
        "ci_job",
        "job_queue",
        "log_segment",
        "artifact",
        "cache_entry",
        "ci_event_log_offset",
    ]
}

pub fn run_ci_restore_verify_or_fail(inputs: &GateInputs<'_>) -> Result<GreenArtifact, String> {
    match RestoreVerifyGate::new().run(inputs) {
        GateVerdict::Green(artifact) => Ok(artifact),
        GateVerdict::Red(failure) => Err(format!(
            "STOR-D1/D2 restore-verify over the CI stores is RED - this is a DATED NO-GO that blocks \
             M5, NOT a weakened threshold: {failure}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        for g in &gates {
            assert!(
                !g.is_floor(),
                "a permanent gate has NO floor below it - it re-runs forever ({:?})",
                g.kind
            );
        }
    }

    #[test]
    fn the_restore_verify_stores_cover_every_ci_store_the_prompt_names() {
        let stores = ci_restore_verify_stores();
        for required in [
            "ci_run",
            "ci_job",
            "job_queue",
            "log_segment",
            "artifact",
            "cache_entry",
            "ci_event_log_offset",
        ] {
            assert!(
                stores.contains(&required),
                "the restore-verify gate must wire the CI store `{required}` (CI-P27 DELIVERABLE)"
            );
        }
    }
}
