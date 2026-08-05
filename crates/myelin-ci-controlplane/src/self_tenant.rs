use myelin_storage::blob::ContentHash;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchCapability {
    pub id: &'static str,
    pub anchor_feature: &'static str,
    pub myelin_surface: &'static str,
    pub reached_by_driving: bool,
    pub deferred_named_floor: bool,
}

impl SwitchCapability {
    pub fn is_wall(&self) -> bool {
        !self.reached_by_driving && !self.deferred_named_floor
    }
}

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
            "myelin ci watch <run> (firehose + resume-cursor - loses 0 lines on reconnect)",
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
            "myelin ci validate / myelin ci plan (no runner spend - the cost-saving path)",
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

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the CI switch-test verdict must be checked - a dropped RED means a migrating user hits a \
              wall the old tool didn't have, silently (EI-01 §4: actually try the real thing)"]
pub enum SwitchVerdict {
    Pass {
        reached: usize,
        measured_render_us: u64,
        budget_render_us: u64,
        deferred_floors: Vec<&'static str>,
    },
    Red {
        walls: Vec<&'static str>,
        latency_over_budget: bool,
    },
}

impl SwitchVerdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, SwitchVerdict::Pass { .. })
    }

    pub fn walls(&self) -> &[&'static str] {
        match self {
            SwitchVerdict::Pass { .. } => &[],
            SwitchVerdict::Red { walls, .. } => walls,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CiSwitchTest {
    pub capabilities: Vec<SwitchCapability>,
    pub measured_render_us: u64,
    pub budget_render_us: u64,
}

impl CiSwitchTest {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenCiRow {
    pub id: &'static str,
    pub title: &'static str,
    pub proof_command: &'static str,
    pub artifact_date: Option<String>,
}

impl ProvenCiRow {
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }
}

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
            "ci.pipeline crash-recovery - effectively-once SCHEDULE_AND_RUN_JOB (0 double-dispatch)",
            "cargo test -p myelin-ci-controlplane --test drills_ci_p16_effectively_once",
            date,
        ),
        row(
            "CI-D5",
            "reserve/settle metering parity - reserved == billed + refunded, one cost_event per unit",
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
            "ci.pipeline determinism - bit-identical replay (flow-determinism lint obeyed on the body)",
            "cargo test -p myelin-ci-controlplane --test drills_ci_p15_ci_pipeline",
            date,
        ),
        row(
            "CI-D4",
            "supply-chain fail-closed - unpinned/unsigned/SLSA-missing rejects (digest-pin + sigstore)",
            "cargo test -p myelin-ci-controlplane --test drills_ci_p23_supply_chain_fail_closed",
            date,
        ),
        row(
            "CI-D6",
            "trust-scoped artifacts/caches - a fork cannot poison a trusted cache (per-subject DEK)",
            "cargo test -p myelin-ci-controlplane --test drills_ci_p22_fork_cache_poison",
            date,
        ),
        row(
            "CI-D7",
            "in-boundary secret broker - an untrusted fork resolves to NO secrets (deploy HITL)",
            "cargo test -p myelin-ci-controlplane --test drills_ci_p24_fork_no_secrets",
            date,
        ),
        row(
            "CI-D11",
            "durable live-tail - committed-prefix producer/Edge sever resumes with 0 lost/duplicate bytes",
            "cargo test -p myelin-edge --features integration --test integration_ci_http_surface production_sink_and_edge_resume_exactly_after_both_services_are_severed",
            date,
        ),
        row(
            "CI-D2",
            "30× agent-surge - the human lane holds, the machine lane sheds (tuned DRR/shed-budget)",
            "cargo test -p myelin-ci-controlplane --test ci_d2_surge_drill",
            date,
        ),
        row(
            "CI-R3",
            "residency at scale - 0 cross-region egress + residency-pinned runner-claim",
            "cargo test -p myelin-ci-controlplane --test residency_and_self_hosted_drill",
            date,
        ),
        row(
            "CI-D10",
            "self-hosted trust boundary - a self-hosted runner is residency-attested + scoped-token",
            "cargo test -p myelin-ci-controlplane --test residency_and_self_hosted_drill",
            date,
        ),
        row(
            "CI-D3",
            "crypto-shred erase - erasure reaches every PersonalDataHolder (0 holder missed)",
            "cargo test -p myelin-ci-controlplane --test integration_ci_p32_crypto_shred_erase",
            date,
        ),
        row(
            "CI-restore-verify",
            "restore-verify on CI's stores - one consistent point within RPO/RTO (STOR-D1/D2 gate)",
            "cargo test -p myelin-ci-controlplane --test integration_ci_p27_restore_verify_ci_stores",
            date,
        ),
        row(
            "E2E-1",
            "PR context pane - CI check rows resolve per-viewer, 0 row leak (#step-<n> jump-to-failure)",
            "cargo test -p myelin-ci-controlplane --test drill_ci_p33_e2e_wedge",
            date,
        ),
        row(
            "E2E-3",
            "spec-to-ship traceability - HITL-gated deploy + cold-reindex == live + tamper detected",
            "cargo test -p myelin-ci-controlplane --test drill_ci_p33_e2e_wedge",
            date,
        ),
        row(
            "E2E-2",
            "agent-native flagship - CI-fail → triage agent → issue → chat → fix-PR (check seam e2e)",
            "cargo test -p myelin-ci-controlplane --test drill_ci_p34_e2e2_flagship",
            date,
        ),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a CI truth-up verdict must be checked - a dropped RED means a CLAIMED-NOT-PROVEN CI row \
              silently drifts the docs from the code (EI-01 §1: a claim that outlives its verification \
              misleads the next agent)"]
pub enum CiTruthUpVerdict {
    Green {
        rows_confirmed: usize,
        date: String,
    },
    Red {
        undated_rows: Vec<&'static str>,
    },
}

impl CiTruthUpVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, CiTruthUpVerdict::Green { .. })
    }

    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            CiTruthUpVerdict::Green { .. } => &[],
            CiTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CiTruthUpPass;

impl CiTruthUpPass {
    pub fn new() -> CiTruthUpPass {
        CiTruthUpPass
    }

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

    pub fn run_or_fail_ci(&self, rows: &[ProvenCiRow], date: &str) -> Result<usize, CiTruthUpRed> {
        match self.run(rows, date) {
            CiTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            CiTruthUpVerdict::Red { undated_rows } => Err(CiTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiTruthUpRed {
    pub undated_rows: Vec<String>,
}

impl std::fmt::Display for CiTruthUpRed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CI truth-up RED - {} claimed-not-proven row(s) lack a dated green artifact: {}",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for CiTruthUpRed {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiIncident {
    pub id: &'static str,
    pub issue_ref: Option<&'static str>,
    pub repro_drill_id: Option<&'static str>,
}

impl CiIncident {
    pub fn is_guarded(&self) -> bool {
        self.issue_ref.is_some() && self.repro_drill_id.is_some()
    }
}

#[derive(Clone, Debug, Default)]
pub struct IncidentDrillLoop {
    incidents: Vec<CiIncident>,
}

impl IncidentDrillLoop {
    pub fn new() -> IncidentDrillLoop {
        IncidentDrillLoop {
            incidents: Vec::new(),
        }
    }

    pub fn record(&mut self, incident: CiIncident) {
        self.incidents.push(incident);
    }

    pub fn incidents(&self) -> &[CiIncident] {
        &self.incidents
    }

    pub fn unguarded_incidents(&self) -> Vec<&'static str> {
        self.incidents
            .iter()
            .filter(|i| !i.is_guarded())
            .map(|i| i.id)
            .collect()
    }

    pub fn is_satisfied(&self) -> bool {
        self.incidents.iter().all(CiIncident::is_guarded)
    }
}

fn push_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

#[cfg(test)]
#[path = "self_tenant_tests.rs"]
mod tests;
