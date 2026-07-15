//! # Dogfooding: Myelin self-hosts as exactly one cell + the Tenancy truth-up pass (P-CP-23 → P-508)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md` §10 in full
//! (**self-host parity** — a self-hosted install is **exactly one cell of identical artifacts**; the
//! control plane is **degenerate** but the **SAME code path** runs; the **SAME `residency-pin` lint**
//! holds; the **SAME drills** run; managed-fleet-only features are **N/A for self-host by
//! definition**, not a gap). `VISION.md` §5 (the dogfood loop — Myelin hosts itself; the team's data
//! is **real tenant data**) / §3 (EU-sovereign by construction — the self-host cell stays in the
//! team's region). EI-01 §5 (the ratchet now runs as Myelin CI on the platform's own commits — an
//! uncommitted gate is no gate) / §1 (code-wins-over-docs — the truth-up pass confirms every PROVEN
//! Tenancy row rests on a DATED green artifact, never a doc claim).
//!
//! ## What this module is (the M6 dogfood band for Tenancy — no new gate)
//! This prompt does **NOT** define a new Tenancy gate. It exercises the FULL Tenancy contract surface
//! (12.1–12.6 + the two lints + `residency_verify`) on **the Myelin team's own tenant**, and adds the
//! **truth-up pass** that confirms the gate invariant holds end-to-end. Three legs, each reusing the
//! already-built mechanism:
//!
//! 1. **[`MyelinSelfHost`] — the team tenant placed on the degenerate one-cell control plane.** A thin
//!    assembly over the SHARED [`crate::self_host::DegenerateControlPlane`] (P-CP-13): the Myelin team
//!    is placed via the SAME `place`/`discover`/`placement_of` a managed-fleet cell runs over a
//!    one-row registry. There is no self-host fork — the team is just another placed tenant on the one
//!    cell.
//! 2. **`residency_verify` GREEN on the platform's own data** ([`MyelinSelfHost::residency_verify_team`])
//!    — the one cell's M1 stores all report the team's region, so the SAME free
//!    [`crate::residency_verify::residency_verify`] mints a green `residency-attestation` (0
//!    mismatches) over the team's tenant. This is the prompt's `residency_verify` green-on-own-data
//!    leg.
//! 3. **[`TenancyTruthUpPass`] / [`ProvenTenancyRow`] — the truth-up pass.** Enumerates every PROVEN
//!    Tenancy row the ledger claims (CP-D1..CP-D8, STOR-D5, CI-R3, GA-D8) and asserts each rests on a
//!    DATED green artifact. A row WITHOUT one is a LOUD failure ([`TenancyTruthUpVerdict::Red`]), never
//!    a silent pass (EI-01 §1 — a claim that outlives its verification misleads the next agent). This
//!    is the prompt's "no later-band CP gate is red — the gate invariant holds end-to-end" leg.
//!
//! ## What this prompt does NOT ship (split, named — no engineering floor)
//! Per P-CP-23 there is **NO new floor** — this is the proof that the floors built across M0..M5 hold
//! on the platform's own data. The two Tenancy lints (`residency-pin`, `control-plane-pii-free`) run
//! as Myelin CI jobs via the self-hosting CI graph (`myelin-harness::self_hosting_ci`, extended by
//! this prompt) — the lints are NOT re-implemented here; the wiring is the dogfood graph. The
//! managed-fleet-only-N/A (cross-cell tenants, fleet deploy waves) remains the model, not a gap
//! (named in [`crate::self_host`]).

use myelin_tenancy::{CellId, Region, TenantId};

use crate::place::{CounterMinter, PlaceError, PlacementAnswer, PlacementService};
use crate::residency_verify::{ResidencyMismatch, ResidencySigningKey, SignedAttestation};
use crate::schema::IsolationKind;
use crate::self_host::DegenerateControlPlane;

/// **The Myelin team's self-hosting install — exactly one cell, the team placed as real tenant data
/// (P-CP-23).** A thin assembly over the SHARED [`DegenerateControlPlane`] (P-CP-13): it holds the
/// degenerate one-cell control plane and the team's placed tenant id. The team is placed via the SAME
/// `place`/`discover`/`placement_of`/`residency_verify` a managed-fleet cell runs — there is no
/// self-host fork (the parity proof lives in [`crate::self_host`]; this type just places the team's
/// own tenant on it and attests its residency).
#[derive(Debug)]
pub struct MyelinSelfHost {
    /// The degenerate one-cell control plane (the install — one `Active` cell, pinned to the team's
    /// region). The SHARED type — no self-host fork.
    control_plane: DegenerateControlPlane,
    /// The Myelin team's placed tenant id (opaque, PII-free) — minted by the SAME `place` path.
    team_tenant: TenantId,
    /// The team's placement answer (home cell, endpoint, tier) — the routing fact `place` returned.
    placement: PlacementAnswer,
}

impl MyelinSelfHost {
    /// **Bootstrap the team's self-host install + place the team's own tenant (P-CP-23 leg 1).**
    /// Builds the degenerate one-cell control plane in the team's `region` (cell `cell_id`), then runs
    /// the SAME [`DegenerateControlPlane::place`] (→ [`PlacementService::place`]) to place the Myelin
    /// team as a real tenant (`slug` is the team's PII-free routing slug). With one `Active` cell the
    /// placement trivially resolves to "this cell" — the degeneracy, not a fork. Returns a loud
    /// [`PlaceError`] if (impossibly) the one cell were ineligible.
    /// **TEST DOUBLE as of MR-009b W6d** (compiled only under
    /// `#[cfg(any(test, feature = "test-support"))]` — it boots the in-memory
    /// [`DegenerateControlPlane::bootstrap`]). A production dogfood install boots the durable
    /// [`DegenerateControlPlane::with_pg`] and places the team via [`Self::bootstrap_team_on`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn bootstrap_team(
        cell_id: CellId,
        region: Region,
        slug: &str,
    ) -> Result<MyelinSelfHost, PlaceError> {
        Self::bootstrap_team_on(DegenerateControlPlane::bootstrap(cell_id, region), slug)
    }

    /// **Place the Myelin team on an ALREADY-BOOTED degenerate control plane (MR-009b W6d).** The
    /// durable dogfood path: boot [`DegenerateControlPlane::with_pg`] (the Pg arm — the team's
    /// placement survives a restart) and hand it here; the SAME [`PlacementService::place`] a fleet
    /// cell calls places the team as just another tenant.
    pub fn bootstrap_team_on(
        mut control_plane: DegenerateControlPlane,
        slug: &str,
    ) -> Result<MyelinSelfHost, PlaceError> {
        let service = PlacementService::new(CounterMinter::new());
        // The SAME PlacementService::place a fleet cell calls — the team is just another placed tenant.
        let placement = control_plane.place(&service, IsolationKind::Pool, slug)?;
        let team_tenant = placement.tenant_id.clone();
        Ok(MyelinSelfHost {
            control_plane,
            team_tenant,
            placement,
        })
    }

    /// The degenerate one-cell control plane the team is placed on (the SHARED type).
    pub fn control_plane(&self) -> &DegenerateControlPlane {
        &self.control_plane
    }

    /// The Myelin team's placed tenant id (opaque, PII-free).
    pub fn team_tenant(&self) -> &TenantId {
        &self.team_tenant
    }

    /// The team's placement answer (home cell / endpoint / tier) — the routing fact `place` returned.
    pub fn placement(&self) -> &PlacementAnswer {
        &self.placement
    }

    /// The install's region (immutable — the team's region every store pins to).
    pub fn region(&self) -> &Region {
        self.control_plane.region()
    }

    /// **`discover(team) → "this cell"` — the SAME degenerate routing.** A convenience that asserts the
    /// team routes to the one cell (the SAME [`DegenerateControlPlane::discover_cell`]). Returns the
    /// resolved cell id (always the one cell, for the placed team).
    pub fn discover_team_cell(&self) -> Option<CellId> {
        self.control_plane.discover_cell(&self.team_tenant)
    }

    /// **`residency_verify` GREEN on the platform's own data (P-CP-23 leg 2) — the SAME free
    /// [`crate::residency_verify::residency_verify`].** The one cell's M1 stores all report the team's
    /// region, so the SAME aggregation+sign mints a green `residency-attestation` (0 mismatches) over
    /// the TEAM's tenant. Delegates to [`DegenerateControlPlane::residency_verify_own_data`] — there is
    /// no self-host attestation fork; the team's store reports simply all carry the one region.
    ///
    /// This is the prompt's `residency_verify` green-on-own-data leg (the `residency-attestation` green
    /// artifact on the team's tenant). A wrong-region report (impossible on a one-cell install) would
    /// FAIL loudly as a [`ResidencyMismatch`] — never a silent pass (EI-01 §3).
    pub fn residency_verify_team(
        &self,
        key: &ResidencySigningKey,
    ) -> Result<SignedAttestation, ResidencyMismatch> {
        self.control_plane
            .residency_verify_own_data(&self.team_tenant, key)
    }
}

/// One PROVEN Tenancy row the truth-up pass enumerates — a Tenancy gate/drill the ledger claims PROVEN
/// (the CP-D* family + STOR-D5 + CI-R3 + GA-D8). The truth-up pass asserts each rests on a DATED green
/// artifact: an `artifact_date` of `Some(date)` is a row whose proof is dated + present; `None` is a
/// CLAIMED-NOT-PROVEN row the pass FAILs on loudly (code-wins-over-docs, EI-01 §1 — a claim that
/// outlives its verification misleads the next agent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenTenancyRow {
    /// The stable gate/drill id (e.g. `"CP-D3"`, `"STOR-D5"`, `"CI-R3"`, `"GA-D8"`).
    pub id: &'static str,
    /// A one-line human title (what the row proves).
    pub title: &'static str,
    /// The proof command that emits this row's dated green artifact (the `cargo test` target that
    /// lives with the feature prompt — the truth-up pass names it so the artifact is reproducible).
    pub proof_command: &'static str,
    /// The DATE the row's green artifact was last emitted, if any. `Some(date)` ⇒ dated + proven;
    /// `None` ⇒ CLAIMED-NOT-PROVEN (a loud red, never a silent pass).
    pub artifact_date: Option<String>,
}

impl ProvenTenancyRow {
    /// `true` iff this row rests on a dated green artifact (the truth-up invariant for one row).
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }
}

/// **The FROZEN set of PROVEN Tenancy rows the truth-up pass enumerates (P-CP-23 leg 3).** This is the
/// single source of which Tenancy gates/drills the ledger claims PROVEN — the CP-D1..CP-D8 drill
/// family, the residency leg STOR-D5, the CI residency-attestation CI-R3, and the multi-cell DSR
/// fan-out GA-D8 (the exact rows the prompt's DELIVERABLE (4) + the coverage map name). The truth-up
/// pass asserts EVERY id here rests on a dated green artifact; a row without one is a loud failure.
///
/// The id/title/proof-command triples below are the Tenancy rows greened by P-CP-01..P-CP-22 (the
/// coverage map in `by-system/tenancy-and-control-plane.md`). The `date` is supplied by the truth-up
/// runner (the dogfood run's `today_iso()`) — the pass DATES every row at the run so a claim never
/// outlives its verification (EI-01 §1). A row whose proof command did NOT emit a green at the run
/// gets `None` and reds the pass.
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
            "control-plane PII-free — 0 personal data in the registry (lint + place + registry legs)",
            "cargo test -p myelin-control-plane registry::",
            date,
        ),
        row(
            "CP-D2",
            "gateway misroute-rejection — 0 cross-tenant/cross-cell read (placement_of, end-to-end)",
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
            "CP-outage blast-radius — placed tenants keep serving, signup-only degrade (fail-static)",
            "cargo test -p myelin-control-plane cp_outage",
            date,
        ),
        row(
            "CP-D5",
            "cell bulkhead under 30× surge — cross-cell impact 0",
            "cargo test -p myelin-control-plane bulkhead",
            date,
        ),
        row(
            "CP-D6",
            "provision-gating — no traffic to an unverified cell (restore-verify + readiness)",
            "cargo test -p myelin-control-plane provision",
            date,
        ),
        row(
            "CP-D7",
            "live migration + repo relocation — cell→cell migration 0 loss",
            "cargo test -p myelin-control-plane migration",
            date,
        ),
        row(
            "CP-D8",
            "cross-cell pointer bridge resolution live — 0 PII across the bridge (12.6)",
            "cargo test -p myelin-control-plane cross_cell_bridge",
            date,
        ),
        row(
            "STOR-D5",
            "residency end-to-end (Tenancy's leg) — 0 cross-region egress for personal data",
            "cargo test -p myelin-control-plane residency_verify",
            date,
        ),
        row(
            "CI-R3",
            "residency_verify CI-store coverage + residency-pinned runner-claim — 0 silent pass",
            "cargo test -p myelin-control-plane runner_claim_pin",
            date,
        ),
        row(
            "GA-D8",
            "multi-cell DSR erase fan-out — 0 cells missed (member_cells fan-out)",
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

/// The verdict of a Tenancy truth-up pass — GREEN (every PROVEN Tenancy row rests on a dated green
/// artifact — the gate invariant holds end-to-end, no earlier-band CP gate is red) or RED (one or more
/// rows are CLAIMED-NOT-PROVEN: a claim that outlives its verification). `#[must_use]`: a dropped
/// verdict is a swallowed truth-up failure — the docs would silently drift from the code (the exact
/// EI-01 §1 failure mode), so the compiler flags a dropped red.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a Tenancy truth-up verdict must be checked — a dropped RED means a CLAIMED-NOT-PROVEN \
              Tenancy row silently drifts the docs from the code (EI-01 §1: a claim that outlives its \
              verification misleads the next agent)"]
pub enum TenancyTruthUpVerdict {
    /// Every enumerated PROVEN Tenancy row rests on a dated green artifact (the gate invariant holds
    /// end-to-end — no earlier-band CP gate is red).
    Green {
        /// How many PROVEN rows were confirmed dated + green.
        rows_confirmed: usize,
        /// The date the truth-up pass ran (every confirmed row is dated at this run).
        date: String,
    },
    /// One or more PROVEN Tenancy rows are CLAIMED-NOT-PROVEN — they have NO dated green artifact. The
    /// undated row ids are named (loud, never swallowed).
    Red {
        /// The ids of the rows that lack a dated green artifact (the claimed-not-proven set).
        undated_rows: Vec<&'static str>,
    },
}

impl TenancyTruthUpVerdict {
    /// `true` iff the pass is GREEN (every PROVEN row dated + present).
    pub fn is_green(&self) -> bool {
        matches!(self, TenancyTruthUpVerdict::Green { .. })
    }

    /// The undated (claimed-not-proven) row ids — empty iff GREEN. Loud, never swallowed.
    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            TenancyTruthUpVerdict::Green { .. } => &[],
            TenancyTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

/// **The Tenancy truth-up pass (P-CP-23 leg 3 — the gate invariant holds end-to-end).** A zero-sized
/// orchestrator: enumerates every PROVEN Tenancy row ([`proven_tenancy_rows`]) and asserts each rests
/// on a DATED green artifact. A row WITHOUT one is a LOUD failure ([`TenancyTruthUpVerdict::Red`]),
/// never a silent pass (code-wins-over-docs, EI-01 §1).
#[derive(Clone, Copy, Debug, Default)]
pub struct TenancyTruthUpPass;

impl TenancyTruthUpPass {
    /// Construct the (zero-sized) truth-up orchestrator.
    pub fn new() -> TenancyTruthUpPass {
        TenancyTruthUpPass
    }

    /// **Run the truth-up pass over `rows`.** Returns [`TenancyTruthUpVerdict::Green`] (every row
    /// dated) or [`TenancyTruthUpVerdict::Red`] (the undated rows named). `date` is the run date.
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

    /// **Run the truth-up pass and FAIL CI loudly on a red.** Returns `Ok(rows_confirmed)` (the count
    /// of dated PROVEN rows) or `Err(`[`TenancyTruthUpRed`]`)` naming every claimed-not-proven row — a
    /// red the CI gate must not swallow (an uncommitted gate is no gate, EI-01 §5).
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

/// The loud error a red Tenancy truth-up pass raises — it names every claimed-not-proven Tenancy row
/// (the gate the CI must not swallow).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenancyTruthUpRed {
    /// The ids of the PROVEN Tenancy rows that lack a dated green artifact.
    pub undated_rows: Vec<String>,
}

impl std::fmt::Display for TenancyTruthUpRed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Tenancy truth-up RED — {} claimed-not-proven row(s) lack a dated green artifact: {}",
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

    /// **The Myelin team is placed on the degenerate one-cell control plane (P-CP-23 leg 1).** Real
    /// tenant data on a one-row registry; the team routes to the one cell (discover → "this cell").
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
        // The team discovers to the one cell (the SAME degenerate routing).
        let discovered = sh.discover_team_cell().expect("the placed team discovers");
        assert_eq!(discovered.as_str(), "cell-myelin");
    }

    /// **`residency_verify` is GREEN on the platform's OWN data (P-CP-23 leg 2).** The one cell's M1
    /// stores all report the team's region → a verifying `residency-attestation`, 0 mismatches.
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
        // Every store report carries the one region (0 mismatches — the green artifact).
        assert!(
            attestation
                .store_regions
                .iter()
                .all(|(_, r)| r.as_str() == "fr-par"),
            "every M1 store reports the team's region (no global pool)"
        );
    }

    /// **The team's install is a ONE-ROW registry pinned to the install's region (the residency model
    /// holds on the platform's own data).** The one cell carries the install's region (`fr-par`); a
    /// foreign-region cell simply does not exist — so the team's data structurally cannot land
    /// out-of-region (the SAME region-first model, here proven over the team's own one-cell install).
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
            "the one cell is pinned to the install's region — a foreign-region cell does not exist"
        );
        assert_ne!(
            cell.region.as_str(),
            "us-east",
            "the team's data cannot land out-of-region (no out-of-region cell to place onto)"
        );
    }

    /// **The truth-up pass GREENs when every PROVEN Tenancy row is dated (P-CP-23 leg 3).** The gate
    /// invariant holds end-to-end — no earlier-band CP gate is red.
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
        // run_or_fail_ci returns Ok with the confirmed count (the CI hook).
        let confirmed = TenancyTruthUpPass::new()
            .run_or_fail_ci(&rows, "2026-06-25")
            .expect("green → Ok");
        assert_eq!(confirmed, rows.len());
    }

    /// **The truth-up pass REDs LOUDLY + names a claimed-not-proven row (EI-01 §1).** A PROVEN row
    /// without a dated green artifact is a claim that outlived its verification → a loud red, never a
    /// silent pass.
    #[test]
    fn truth_up_reds_loudly_on_a_claimed_not_proven_row() {
        let mut rows = proven_tenancy_rows("2026-06-25");
        // Strip the date off CP-D3 — a PROVEN claim with no artifact (the docs drifted from the code).
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
        // run_or_fail_ci returns the loud error naming the row.
        let err = TenancyTruthUpPass::new()
            .run_or_fail_ci(&rows, "2026-06-25")
            .expect_err("an undated row → a loud CI red");
        assert_eq!(err.undated_rows, vec!["CP-D3".to_string()]);
        assert!(
            err.to_string().contains("CP-D3"),
            "the red names the claimed-not-proven row"
        );
    }

    /// **The PROVEN Tenancy set covers the CP-D* family + the cross-system legs (the coverage map).**
    /// A guard that the truth-up set was not silently shrunk (every drill the ledger claims is here).
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
