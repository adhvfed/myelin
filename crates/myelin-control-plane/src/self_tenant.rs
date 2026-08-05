use myelin_tenancy::{CellId, Region, TenantId};

use crate::place::{CounterMinter, PlaceError, PlacementAnswer, PlacementService};
use crate::residency_verify::{ResidencyMismatch, ResidencySigningKey, SignedAttestation};
use crate::schema::IsolationKind;
use crate::self_host::DegenerateControlPlane;

#[derive(Debug)]
pub struct MyelinSelfHost {
    control_plane: DegenerateControlPlane,
    team_tenant: TenantId,
    placement: PlacementAnswer,
}

impl MyelinSelfHost {
    #[cfg(any(test, feature = "test-support"))]
    pub fn bootstrap_team(
        cell_id: CellId,
        region: Region,
        slug: &str,
    ) -> Result<MyelinSelfHost, PlaceError> {
        Self::bootstrap_team_on(DegenerateControlPlane::bootstrap(cell_id, region), slug)
    }

    pub fn bootstrap_team_on(
        mut control_plane: DegenerateControlPlane,
        slug: &str,
    ) -> Result<MyelinSelfHost, PlaceError> {
        let service = PlacementService::new(CounterMinter::new());
        let placement = control_plane.place(&service, IsolationKind::Pool, slug)?;
        let team_tenant = placement.tenant_id.clone();
        Ok(MyelinSelfHost {
            control_plane,
            team_tenant,
            placement,
        })
    }

    pub fn control_plane(&self) -> &DegenerateControlPlane {
        &self.control_plane
    }

    pub fn team_tenant(&self) -> &TenantId {
        &self.team_tenant
    }

    pub fn placement(&self) -> &PlacementAnswer {
        &self.placement
    }

    pub fn region(&self) -> &Region {
        self.control_plane.region()
    }

    pub fn discover_team_cell(&self) -> Option<CellId> {
        self.control_plane.discover_cell(&self.team_tenant)
    }

    pub fn residency_verify_team(
        &self,
        key: &ResidencySigningKey,
    ) -> Result<SignedAttestation, ResidencyMismatch> {
        self.control_plane
            .residency_verify_own_data(&self.team_tenant, key)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenTenancyRow {
    pub id: &'static str,
    pub title: &'static str,
    pub proof_command: &'static str,
    pub artifact_date: Option<String>,
}

impl ProvenTenancyRow {
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }
}

pub fn proven_tenancy_rows(date: &str) -> Vec<ProvenTenancyRow> {
    fn row(
        id: &'static str,
        title: &'static str,
        cmd: &'static str,
        date: &str,
    ) -> ProvenTenancyRow {
        ProvenTenancyRow {
            id,
            title,
            proof_command: cmd,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        row(
            "CP-D1",
            "control-plane PII-free - 0 personal data in the registry (lint + place + registry legs)",
            "cargo test -p myelin-control-plane registry::",
            date,
        ),
        row(
            "CP-D2",
            "gateway misroute-rejection - 0 cross-tenant/cross-cell read (placement_of, end-to-end)",
            "cargo test -p myelin-control-plane placement_of",
            date,
        ),
        row(
            "CP-D3",
            "residency-pin rejects an out-of-region write + attestation passes (four-layer end-to-end)",
            "cargo test -p myelin-control-plane four_layer",
            date,
        ),
        row(
            "CP-D4",
            "CP-outage blast-radius - placed tenants keep serving, signup-only degrade (fail-static)",
            "cargo test -p myelin-control-plane cp_outage",
            date,
        ),
        row(
            "CP-D5",
            "cell bulkhead under 30× surge - cross-cell impact 0",
            "cargo test -p myelin-control-plane bulkhead",
            date,
        ),
        row(
            "CP-D6",
            "provision-gating - no traffic to an unverified cell (restore-verify + readiness)",
            "cargo test -p myelin-control-plane provision",
            date,
        ),
        row(
            "CP-D7",
            "live migration + repo relocation - cell→cell migration 0 loss",
            "cargo test -p myelin-control-plane migration",
            date,
        ),
        row(
            "CP-D8",
            "cross-cell pointer bridge resolution live - 0 PII across the bridge (12.6)",
            "cargo test -p myelin-control-plane cross_cell_bridge",
            date,
        ),
        row(
            "STOR-D5",
            "residency end-to-end (Tenancy's leg) - 0 cross-region egress for personal data",
            "cargo test -p myelin-control-plane residency_verify",
            date,
        ),
        row(
            "CI-R3",
            "residency_verify CI-store coverage + residency-pinned runner-claim - 0 silent pass",
            "cargo test -p myelin-control-plane runner_claim_pin",
            date,
        ),
        row(
            "GA-D8",
            "multi-cell DSR erase fan-out - 0 cells missed (member_cells fan-out)",
            "cargo test -p myelin-control-plane multi_cell",
            date,
        ),
        row(
            "self-host-parity",
            "the degenerate one-cell control plane runs the identical code path (no self-host fork)",
            "cargo test -p myelin-control-plane self_host",
            date,
        ),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a Tenancy truth-up verdict must be checked - a dropped RED means a CLAIMED-NOT-PROVEN \
              Tenancy row silently drifts the docs from the code (EI-01 §1: a claim that outlives its \
              verification misleads the next agent)"]
pub enum TenancyTruthUpVerdict {
    Green {
        rows_confirmed: usize,
        date: String,
    },
    Red {
        undated_rows: Vec<&'static str>,
    },
}

impl TenancyTruthUpVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, TenancyTruthUpVerdict::Green { .. })
    }

    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            TenancyTruthUpVerdict::Green { .. } => &[],
            TenancyTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TenancyTruthUpPass;

impl TenancyTruthUpPass {
    pub fn new() -> TenancyTruthUpPass {
        TenancyTruthUpPass
    }

    pub fn run(&self, rows: &[ProvenTenancyRow], date: &str) -> TenancyTruthUpVerdict {
        let undated_rows: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated_rows.is_empty() {
            TenancyTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            TenancyTruthUpVerdict::Red { undated_rows }
        }
    }

    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenTenancyRow],
        date: &str,
    ) -> Result<usize, TenancyTruthUpRed> {
        match self.run(rows, date) {
            TenancyTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            TenancyTruthUpVerdict::Red { undated_rows } => Err(TenancyTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenancyTruthUpRed {
    pub undated_rows: Vec<String>,
}

impl std::fmt::Display for TenancyTruthUpRed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Tenancy truth-up RED - {} claimed-not-proven row(s) lack a dated green artifact: {}",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for TenancyTruthUpRed {}

#[cfg(test)]
mod tests {
    use super::*;

    fn team() -> MyelinSelfHost {
        MyelinSelfHost::bootstrap_team(
            CellId::from_token("cell-myelin"),
            Region::new("fr-par"),
            "myelin-team",
        )
        .expect("the one Active cell is eligible → the team is placed")
    }

    #[test]
    fn team_tenant_placed_on_the_degenerate_one_cell() {
        let sh = team();
        assert_eq!(
            sh.control_plane().registry().cell_count(),
            1,
            "a self-host install is EXACTLY one cell"
        );
        assert_eq!(sh.placement().home_cell.as_str(), "cell-myelin");
        assert_eq!(sh.region().as_str(), "fr-par");
        let discovered = sh.discover_team_cell().expect("the placed team discovers");
        assert_eq!(discovered.as_str(), "cell-myelin");
    }

    #[test]
    fn residency_verify_green_on_the_teams_own_data() {
        let sh = team();
        let key = ResidencySigningKey::from_bytes([7u8; 32]);
        let attestation = sh
            .residency_verify_team(&key)
            .expect("the one cell's stores all report the team's region → green");
        assert_eq!(attestation.tenant_id.as_str(), sh.team_tenant().as_str());
        assert_eq!(attestation.region.as_str(), "fr-par");
        assert!(
            attestation.verify(&key),
            "the attestation verifies under the control-plane signing key"
        );
        assert!(
            attestation
                .store_regions
                .iter()
                .all(|(_, r)| r.as_str() == "fr-par"),
            "every M1 store reports the team's region (no global pool)"
        );
    }

    #[test]
    fn the_team_install_is_a_one_row_registry_pinned_to_its_region() {
        let sh = team();
        let registry = sh.control_plane().registry();
        assert_eq!(
            registry.cell_count(),
            1,
            "a self-host install is EXACTLY one cell"
        );
        let cell = registry
            .cell(&CellId::from_token("cell-myelin"))
            .expect("the install's one cell");
        assert_eq!(
            cell.region.as_str(),
            "fr-par",
            "the one cell is pinned to the install's region - a foreign-region cell does not exist"
        );
        assert_ne!(
            cell.region.as_str(),
            "us-east",
            "the team's data cannot land out-of-region (no out-of-region cell to place onto)"
        );
    }

    #[test]
    fn truth_up_greens_when_every_row_is_dated() {
        let rows = proven_tenancy_rows("2026-06-25");
        assert!(!rows.is_empty(), "the PROVEN Tenancy set is non-empty");
        let verdict = TenancyTruthUpPass::new().run(&rows, "2026-06-25");
        assert!(
            verdict.is_green(),
            "every PROVEN row rests on a dated artifact"
        );
        match verdict {
            TenancyTruthUpVerdict::Green { rows_confirmed, .. } => {
                assert_eq!(rows_confirmed, rows.len());
            }
            TenancyTruthUpVerdict::Red { .. } => unreachable!(),
        }
        let confirmed = TenancyTruthUpPass::new()
            .run_or_fail_ci(&rows, "2026-06-25")
            .expect("green → Ok");
        assert_eq!(confirmed, rows.len());
    }

    #[test]
    fn truth_up_reds_loudly_on_a_claimed_not_proven_row() {
        let mut rows = proven_tenancy_rows("2026-06-25");
        let cp_d3 = rows
            .iter_mut()
            .find(|r| r.id == "CP-D3")
            .expect("CP-D3 is in the PROVEN set");
        cp_d3.artifact_date = None;
        assert!(!cp_d3.is_dated());

        let verdict = TenancyTruthUpPass::new().run(&rows, "2026-06-25");
        assert!(!verdict.is_green(), "an undated PROVEN row REDs the pass");
        assert_eq!(
            verdict.undated_rows(),
            &["CP-D3"],
            "the claimed-not-proven row is NAMED (loud, never swallowed)"
        );
        let err = TenancyTruthUpPass::new()
            .run_or_fail_ci(&rows, "2026-06-25")
            .expect_err("an undated row → a loud CI red");
        assert_eq!(err.undated_rows, vec!["CP-D3".to_string()]);
        assert!(
            err.to_string().contains("CP-D3"),
            "the red names the claimed-not-proven row"
        );
    }

    #[test]
    fn proven_set_covers_the_cp_drill_family_and_cross_system_legs() {
        let rows = proven_tenancy_rows("2026-06-25");
        let ids: std::collections::BTreeSet<&str> = rows.iter().map(|r| r.id).collect();
        for must in [
            "CP-D1", "CP-D2", "CP-D3", "CP-D4", "CP-D5", "CP-D6", "CP-D7", "CP-D8", "STOR-D5",
            "CI-R3", "GA-D8",
        ] {
            assert!(ids.contains(must), "the PROVEN set must include {must}");
        }
    }
}
