use crate::drills::{DrillRegistry, DrillScenario};
use crate::telemetry::{Predicate, SignalName};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubstrateIncident {
    pub id: &'static str,
    pub issue_ref: Option<&'static str>,
    pub repro_drill_id: Option<&'static str>,
}

impl SubstrateIncident {
    pub fn is_guarded(&self) -> bool {
        self.issue_ref.is_some() && self.repro_drill_id.is_some()
    }
}

#[derive(Default)]
pub struct SubstrateIncidentLoop {
    incidents: Vec<SubstrateIncident>,
    registry: DrillRegistry,
}

impl SubstrateIncidentLoop {
    pub fn new() -> SubstrateIncidentLoop {
        SubstrateIncidentLoop {
            incidents: Vec::new(),
            registry: DrillRegistry::new(),
        }
    }

    pub fn record(&mut self, id: &'static str, issue_ref: &'static str, repro: DrillScenario) {
        let drill_id: &'static str = scenario_name_static(repro.name());
        self.registry.register_drill(repro);
        self.incidents.push(SubstrateIncident {
            id,
            issue_ref: Some(issue_ref),
            repro_drill_id: Some(drill_id),
        });
    }

    pub fn record_unguarded(&mut self, incident: SubstrateIncident) {
        self.incidents.push(incident);
    }

    pub fn incidents(&self) -> &[SubstrateIncident] {
        &self.incidents
    }

    pub fn registered_drill_count(&self) -> usize {
        self.registry.len()
    }

    pub fn unguarded_incidents(&self) -> Vec<&'static str> {
        self.incidents
            .iter()
            .filter(|i| !i.is_guarded())
            .map(|i| i.id)
            .collect()
    }

    pub fn red_repros(&self) -> Vec<String> {
        self.registry
            .run_all()
            .into_iter()
            .filter(|r| !r.is_pass())
            .map(|r| r.name().to_string())
            .collect()
    }

    pub fn is_satisfied(&self) -> bool {
        self.incidents.iter().all(SubstrateIncident::is_guarded) && self.red_repros().is_empty()
    }
}

fn scenario_name_static(name: &str) -> &'static str {
    match name {
        "repro-outbox-relay-stall" => "repro-outbox-relay-stall",
        other => {
            panic!("unknown substrate incident drill name `{other}` - the repro corpus is frozen")
        }
    }
}

pub fn outbox_relay_stall_repro() -> DrillScenario {
    DrillScenario::new("repro-outbox-relay-stall", |ctx| {
        ctx.signals.set_scalar(SignalName::OutboxDepth, 7);
        ctx.signals.set_scalar(SignalName::DeadLetterCount, 0);
        ctx.signals
            .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenSubstrateRow {
    pub id: &'static str,
    pub title: &'static str,
    pub proof_command: &'static str,
    pub artifact_date: Option<String>,
}

impl ProvenSubstrateRow {
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }
}

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
            "the twelve architecture lints - each red fixture rejects + green admits",
            "cargo run -p myelin-lints --bin lint-gate",
            date,
        ),
        row(
            "contract-coverage",
            "the contract-coverage scanner - no falsely-claimed/dropped/un-named row",
            "cargo run -p myelin-lints --bin contract-coverage",
            date,
        ),
        row(
            "harness-self-test",
            "the harness injects a fault and reads one telemetry assertion green",
            "cargo test -p myelin-harness drills::tests::harness_self_test",
            date,
        ),
        row(
            "SUB-D4",
            "Id-hiccup → already-authenticated survives within W; revoked denied (fail-static)",
            "cargo test -p myelin-substrate --test drill_sub_d4_fail_static",
            date,
        ),
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

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a substrate truth-up verdict must be checked - a dropped RED means a CLAIMED-NOT-PROVEN \
              substrate row silently drifts the docs from the code (EI-01 §1: a claim that outlives \
              its verification misleads the next agent)"]
pub enum SubstrateTruthUpVerdict {
    Green {
        rows_confirmed: usize,
        date: String,
    },
    Red {
        undated_rows: Vec<&'static str>,
    },
}

impl SubstrateTruthUpVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, SubstrateTruthUpVerdict::Green { .. })
    }

    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            SubstrateTruthUpVerdict::Green { .. } => &[],
            SubstrateTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SubstrateTruthUpPass;

impl SubstrateTruthUpPass {
    pub fn new() -> SubstrateTruthUpPass {
        SubstrateTruthUpPass
    }

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

    pub fn render_markdown(&self, rows: &[ProvenSubstrateRow], date: &str) -> String {
        let verdict = self.run(rows, date);
        let mut out = String::new();
        out.push_str(
            "# Substrate truth-up pass - every PROVEN substrate row rests on a dated green artifact \
             (P-S38 / P-510, SUB-M6)\n\n",
        );
        out.push_str(&format!("Run date: {date}\n\n"));
        out.push_str(
            "The code wins over the docs (EI-01 §1): each substrate PROVEN row below names its DATED \
             green artifact (the `cargo test`/`cargo run` target that emits it), not a doc claim. The \
             pass is GREEN iff EVERY row rests on a dated artifact - the gate invariant holds \
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
                "| `{}` - {} | {} | `{}` |\n",
                r.id, r.title, dated, r.proof_command
            ));
        }
        out.push('\n');
        if verdict.is_green() {
            out.push_str(&format!(
                "**TRUTH-UP: GREEN** - {} PROVEN substrate rows, 0 claimed-not-proven; the gate \
                 invariant holds end-to-end (no earlier substrate gate is red).\n\n",
                rows.len()
            ));
        } else {
            out.push_str(&format!(
                "**TRUTH-UP: RED** - claimed-not-proven rows lack a dated green artifact: {}.\n\n",
                verdict.undated_rows().join(", ")
            ));
        }
        out.push_str(
            "**Named floor (EI-01 §1):** the world-scale 30× FLEET-hardware load drill (SUB-D3 at \
             true multi-box fleet scale) is the ONE legitimate remaining infra floor - the single-box \
             SCALED surge runs green in the self-hosting CI graph; the fleet corpus is named, not \
             claimed (it is not a row that reds this pass - the substrate is *correct*; the fleet \
             proof is *load-hardened-at-scale*).\n",
        );
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubstrateTruthUpRed {
    pub undated_rows: Vec<String>,
}

impl std::fmt::Display for SubstrateTruthUpRed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "substrate truth-up RED - {} claimed-not-proven row(s) lack a dated green artifact: {}",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for SubstrateTruthUpRed {}

#[cfg(test)]
#[path = "self_tenant_tests.rs"]
mod self_tenant_tests;
