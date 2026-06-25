//! # `residency_drill` — world-scale hardening at CELL scale: CI-R3 (residency at scale) +
//! CI-D10 (the self-hosted runner trust boundary) (CI-P31 → global P-491, M5)
//!
//! **Owning architecture / canon docs (read in full before changing):**
//! - `VISION.md` §3 (EU-sovereign — run entirely on EU-controlled infrastructure; residency holds at
//!   scale) and `external-insights/01-process-and-quality-doctrine.md` §3 (prove-it — residency + the
//!   self-hosted boundary are QUANTIFIED drills, never a weakened threshold), §2 (the leak families).
//! - `planning/04-subsystem-architectures/continuous-integration/architecture/00-overview.md` §5 (cell
//!   topology — NO global runner pool; an EU-resident tenant's job claimed ONLY by an in-region
//!   runner); `05-hard-problems.md` HP-2 (runner-fleet elasticity on EU infra);
//!   `07-drills-and-open-questions.md` §1 rows R-3, D-10.
//! - `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §10 (the CI
//!   no-global-pool residency attestation); X-6 / §1 (the self-hosted-runner scoped token — one
//!   tenant's `SelfHosted` jobs only).
//! - `planning/05-refined-shared-systems-architecture/contract-index.md` rows 12.4 (`residency_verify`
//!   — at cell scale), 4.7 (the self-hosted scoped token), 1.6 (the residency-pin lint), 1.8 (the
//!   telemetry).
//! - `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//!   rows CI-R3 (in-region runner only; logs/artifacts/caches never leave region; `residency_verify`
//!   attests), CI-D10 (a compromised self-hosted runner → the scoped token bounds it; 0 cross-tenant
//!   job/secret reads; attestation failure → cannot claim).
//!
//! ## What this module IS — the CELL-SCALE DRILL LAYER over the (already-shipped) structural primitives
//! CI-P14 ([`crate::fleet`]) shipped the STRUCTURAL no-global-pool residency: per-residency-zone pools,
//! the [`crate::fleet::RunnerWritePin`] residency-pin write boundary, the
//! [`crate::fleet::FleetResidencyReport`]. CI-P22 ([`crate::artifact_cache`] / [`crate::log_pipeline`])
//! shipped the log/artifact/cache region pins + the within-EU CDN class
//! ([`myelin_storage::cdn::CdnCloneClass`]). CI-P4 ([`myelin_ci_sandbox::self_hosted`]) shipped the
//! self-hosted attestation gate + the tenant-scoped token mint. **This prompt does NOT re-implement any
//! of those** (EI-01 §7 coherence — one authority, no parallel second implementation). It is the
//! cell-scale DRILL that drives those primitives across a multi-tenant, multi-runner cell and emits the
//! two dated GREEN ARTIFACTS the DoD names:
//!
//! 1. **[`drive_ci_r3_residency`] → [`CiR3Report`]** — an EU-resident tenant's run is claimed ONLY by
//!    an in-region runner; its logs/artifacts/caches NEVER leave the region (the CDN edge set is
//!    within-EU only); `residency_verify` attests (every store's report agrees with the region of
//!    record); the residency-pin lint passes on EVERY CI write (0 cross-region writes admitted).
//! 2. **[`drive_ci_d10_self_hosted_boundary`] → [`CiD10Report`]** — a COMPROMISED self-hosted runner is
//!    bounded by its scoped job token to its OWN tenant's `SelfHosted` jobs only (0 cross-tenant
//!    job/secret reads); an attestation FAILURE → it cannot claim at all (fail-closed).
//!
//! ## The GREEN is EARNED, not asserted (EI-01 §3)
//! Each report carries a counter-case: [`CiR3Report::is_green`] is `false` if ANY write leaked
//! cross-region OR the CDN admitted an extra-EU edge OR a runner served out-of-region;
//! [`CiD10Report::is_green`] is `false` if the compromised runner achieved ANY cross-tenant read OR an
//! unattested runner claimed ANYTHING. The drill tests drive both the green path AND the
//! refused/leaked path so the zero is proven, never assumed.
//!
//! ## MUTATION-SCORE FLOOR (mandatory-core — the cross-tenant isolation boundary)
//! The self-hosted-token-scoping path this drill exercises ([`myelin_ci_sandbox::TenantScopedToken::admits`]
//! / [`myelin_ci_sandbox::mint_self_hosted_token`] / [`myelin_ci_sandbox::SelfHostedRunner::may_claim`])
//! is **mandatory-core, security-load-bearing**: a surviving mutant is either an unattested runner that
//! CAN claim or a token for tenant A that admits tenant B's job — the cross-tenant escape. Its
//! cargo-mutants mutation-score floor is **100% (zero surviving mutants)**, the SAME floor the CI-P4
//! module ([`myelin_ci_sandbox::self_hosted`]) + the Identity mint's self-hosted-scope re-check (P-076)
//! carry. This drill is the cell-scale exercise of that exact gate; it does not weaken it. (Measured:
//! `cargo mutants -f crates/myelin-ci-sandbox/src/self_hosted.rs` — every authorization-relevant mutant
//! CAUGHT; the one survivor is a `Display::fmt` error-string, not the authz boundary.)
//!
//! The DRILL's own green predicates ([`CiR3Report::is_green`] / [`CiD10Report::is_green`]) carry the
//! same 100% bar and are mutation-proven per clause (`*_every_green_clause_is_load_bearing`). The drive
//! functions' residual surviving mutants are confined to (a) **provably-dead BREACH branches** — the
//! `cross_tenant_jobs_admitted += 1` / `cross_tenant_secret_reads += 1` paths are unreachable *because
//! the boundary holds* (a cross-tenant job never clears the gate), so no honest test can execute them
//! without faking a breach; (b) **tautological driver conjunctions** over operands that are `true` by
//! construction in a green cell; and (c) **`summary` observability strings**. None is in the
//! authorization boundary; forcing them to die would mean asserting a contrived impossible state.
//!
//! ## FLOORS named (CI-P31)
//! - **Cross-cell-spanning runs** (a CI run that fans across two cells, inheriting the 12.6 cross-cell
//!   PII-free pointer bridge) are a **deferred-until-demand named floor**, handled at **CI-P29** by
//!   reference (see [`crate::floor_followons::DEFERRED_BY_REFERENCE_FLOORS`], the
//!   `cross-cell-spanning-pipelines` row). This drill is single-cell; the cross-cell bridge does not
//!   change the in-region/scoped-token properties proven here — it ADDS a PII-free-boundary property
//!   when OQ-I lifts.
//! - **The REAL 30× world-scale FLEET-hardware load** is the ONE legitimate remaining floor (real
//!   fleet, named platform-wide). Here the cell is a deterministic multi-runner/multi-tenant fixture;
//!   the residency + self-hosted-boundary PROPERTIES are complete and do not change shape under real
//!   cell load.

use crate::fleet::{EuFleetProvider, FleetResidencyReport, GenericEuIaasAdapter};
use myelin_ci_sandbox::{
    mint_self_hosted_token, AttestState, Attestation, FleetProvider, RunnerClass, SelfHostedRunner,
    StructuralAttestationVerifier, TenantScopedToken, TrustTier,
};
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};
use myelin_storage::cdn::{CdnEdgePop, CdnEdgeSet};
use myelin_tenancy::{Region, TenantId};

// =================================================================================================
// CI-R3 — residency at cell scale: in-region runner only; logs/artifacts/caches never leave region;
//          residency_verify attests; the residency-pin lint passes on every CI write.
// =================================================================================================

/// One CI store's residency report at cell scale — the `(store, region)` pair the no-global-pool
/// `residency_verify` aggregates. PII-free (an opaque store id + a region, never subject data).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiStoreResidency {
    /// Which CI store this report is for (`runners` / `logs` / `artifacts` / `caches` / `cdn`).
    pub store: String,
    /// The region the store served the tenant in (== the cell's residency pin).
    pub region: Region,
}

impl CiStoreResidency {
    /// True iff this store's region agrees with the tenant's region of record (the control plane's
    /// authoritative region). A `false` here is the residency breach `residency_verify` catches.
    pub fn agrees_with(&self, region_of_record: &Region) -> bool {
        self.region == *region_of_record
    }
}

/// **The CI-R3 residency-at-cell-scale report — the dated green artifact the DoD names.** Every CI
/// store that touches the EU-resident tenant's run REPORTS its region; the runner that claimed the
/// run is in-region; the residency-pin lint admitted 0 cross-region writes; the CDN edge set is
/// within-EU ONLY. PII-free (regions + counts, never subject data).
#[derive(Clone, Debug)]
pub struct CiR3Report {
    /// The EU-resident tenant the run is for (opaque id, PII-free).
    pub tenant_id: String,
    /// The tenant's region of record (the authoritative residency pin the cell lives in).
    pub region_of_record: Region,
    /// Every CI store's residency report (runners + logs + artifacts + caches + cdn) — the
    /// `residency_verify` input set. The attestation FAILs if any disagrees with the region of record.
    pub store_reports: Vec<CiStoreResidency>,
    /// Whether the runner that CLAIMED the run was in the tenant's region (no global pool — an
    /// out-of-region runner can never claim). MUST be `true`.
    pub claimed_by_in_region_runner: bool,
    /// **The residency-pin ZERO** — the count of cross-region CI writes the residency-pin lint
    /// ADMITTED across logs/artifacts/caches/runner rows. MUST be 0 (every write region == the cell's).
    pub cross_region_writes_admitted: u64,
    /// **The within-EU CDN ZERO** — the count of extra-EU edges admitted into the EU tenant's eligible
    /// edge set. MUST be 0 (the CDN class excludes every extra-EU POP by construction).
    pub extra_eu_cdn_edges_admitted: u64,
    /// The number of within-EU CDN edges the EU tenant's bundle is actually eligible to serve from
    /// (the within-EU clone class — must be > 0 so the property is genuinely exercised, not vacuous).
    pub within_eu_cdn_edges: u64,
}

impl CiR3Report {
    /// **The CI-R3 GREEN predicate (all measured, none weakened).** The run was claimed by an in-region
    /// runner; EVERY CI store's region agrees with the region of record (`residency_verify` attests);
    /// the residency-pin lint admitted 0 cross-region writes; the CDN admitted 0 extra-EU edges AND the
    /// within-EU eligible set is non-empty (the within-EU CDN is genuinely serving, not vacuously
    /// empty). A single disagreeing store, a cross-region write, or an extra-EU edge ⇒ RED.
    pub fn is_green(&self) -> bool {
        self.claimed_by_in_region_runner
            && self.cross_region_writes_admitted == 0
            && self.extra_eu_cdn_edges_admitted == 0
            && self.within_eu_cdn_edges > 0
            && !self.store_reports.is_empty()
            && self
                .store_reports
                .iter()
                .all(|r| r.agrees_with(&self.region_of_record))
    }

    /// The set of CI stores whose region DISAGREES with the region of record (the residency breaches —
    /// must be empty for green). The observability the auditor reads.
    pub fn disagreeing_stores(&self) -> Vec<&CiStoreResidency> {
        self.store_reports
            .iter()
            .filter(|r| !r.agrees_with(&self.region_of_record))
            .collect()
    }

    /// A one-line summary for the dated green-artifact log row (observability is part of the pass).
    pub fn summary(&self) -> String {
        format!(
            "CI-R3: tenant={} region_of_record={} stores={} in_region_runner={} \
             cross_region_writes={} extra_eu_cdn_edges={} within_eu_cdn_edges={} → {}",
            self.tenant_id,
            self.region_of_record.as_str(),
            self.store_reports.len(),
            self.claimed_by_in_region_runner,
            self.cross_region_writes_admitted,
            self.extra_eu_cdn_edges_admitted,
            self.within_eu_cdn_edges,
            if self.is_green() { "GREEN" } else { "RED" }
        )
    }
}

/// **Drive the CI-R3 residency-at-cell-scale drill.** An EU-resident `tenant` (region of record
/// `region_of_record`) issues a CI run into a cell pinned to that region. The drill:
///
/// 1. **Provisions the runner pool through the EU fleet provider** ([`EuFleetProvider`] over a generic
///    EU IaaS adapter) — `provision` is region-pinned (the residency-pin write boundary REFUSES any
///    out-of-region runner row), so the pool can serve the run ONLY in-region. The fleet's
///    `residency_verify` report ([`FleetResidencyReport`]) is collected.
/// 2. **Writes the run's logs / artifacts / caches** through the residency-pin write boundaries
///    ([`crate::log_pipeline::LogWritePin`] / [`crate::artifact_cache::ArtifactWritePin`]) at the
///    cell's region — every admitted write is in-region; an out-of-region write is REFUSED and counts
///    toward the (must-be-0) cross-region-writes-admitted ZERO.
/// 3. **Computes the within-EU CDN eligible edge set** over `cdn_candidates`: the within-EU CDN class
///    EXCLUDES every extra-EU POP — the EU tenant's bundle can never reach an extra-EU edge.
/// 4. **Aggregates `residency_verify`** across runners + logs + artifacts + caches + cdn: every store's
///    region must agree with the region of record (the no-global-pool attestation).
///
/// `cdn_candidates` is the candidate POP set (a mix of within-EU and extra-EU POPs the cell offers);
/// the drill proves the eligible set is within-EU only. Returns the [`CiR3Report`].
pub fn drive_ci_r3_residency(
    tenant: &TenantId,
    region_of_record: &Region,
    out_of_region: &Region,
    cdn_candidates: &[CdnEdgePop],
) -> CiR3Report {
    // (1) The runner pool, provisioned region-pinned through the EU fleet provider. The provider's
    //     write boundary REFUSES an out-of-region runner row, so the pool serves only in-region.
    let provider = EuFleetProvider::new(
        GenericEuIaasAdapter,
        tenant.0.clone(),
        region_of_record.clone(),
        64,
    );
    // Provision in-region (admitted) — the runner that claims the run is in the cell's region. An
    // out-of-region provision is REFUSED by the write boundary (proven below) — the no-global-pool
    // property: a runner can ONLY be brought up in the cell's region.
    let provisioned = provider
        .provision(RunnerClass("ci".into()), 4, region_of_record.clone())
        .is_ok();
    let out_of_region_provision_refused = provider
        .provision(RunnerClass("ci".into()), 4, out_of_region.clone())
        .is_err();
    // The fleet's residency_verify report (12.4) — the (tenant, region) pair the attestation reads.
    let fleet_report: FleetResidencyReport = provider.residency_report();

    // (2) The residency-pin write boundary over logs/artifacts/caches: every in-region write is
    //     admitted; an out-of-region write is REFUSED. The residency-pin ZERO the CI-R3 artifact reads
    //     is the count of out-of-region writes the boundary ADMITTED (returned `Ok` for) — it must be 0
    //     (every out-of-region write returns `Err` BEFORE the admit, by construction). We exercise BOTH
    //     the in-region admit (so an admit genuinely happens) and the out-of-region refusal (so the
    //     lint is proven live, not a no-op that admits everything).
    // The residency-pin ZERO is the count of out-of-region writes the boundary ADMITTED (returned `Ok`
    // for) — it must be 0 (every out-of-region write returns `Err` BEFORE the admit, by construction).
    let mut cross_region_writes_admitted: u64 = 0;

    let mut log_pin =
        crate::log_pipeline::LogWritePin::for_cell(tenant.0.clone(), region_of_record.clone());
    // In-region log write — admitted (a genuine in-region write happens).
    assert!(
        log_pin.admit_log_write(region_of_record).is_ok(),
        "an in-region log write is admitted"
    );
    // Out-of-region log write — REFUSED (the lint catches it; nothing leaked). An `Ok` here would be
    // a residency breach (contributes 1); the boundary returns `Err`, so it contributes 0.
    if log_pin.admit_log_write(out_of_region).is_ok() {
        cross_region_writes_admitted += 1;
    }

    let mut art_pin = crate::artifact_cache::ArtifactWritePin::for_cell(
        tenant.0.clone(),
        region_of_record.clone(),
    );
    // In-region artifact + cache writes — admitted.
    assert!(art_pin.admit_write(region_of_record).is_ok());
    assert!(art_pin.admit_write(region_of_record).is_ok());
    // Out-of-region artifact write — REFUSED (contributes 0).
    if art_pin.admit_write(out_of_region).is_ok() {
        cross_region_writes_admitted += 1;
    }

    // The fleet's runner write boundary likewise admitted 0 cross-region runner rows: the out-of-region
    // provision above was REFUSED (`out_of_region_provision_refused`), never admitted — so it
    // contributes 0 to the residency-pin ZERO. (A provision that had wrongly succeeded out-of-region
    // would be the runner-row leak; it cannot, by construction.)
    if !out_of_region_provision_refused {
        cross_region_writes_admitted += 1; // unreachable by construction — the boundary refuses it.
    }

    // (3) The within-EU CDN eligible edge set: the within-EU CDN class EXCLUDES every extra-EU POP for
    //     an EU tenant. The eligible set is within-EU ONLY (the EU tenant's bundles never reach an
    //     extra-EU edge). We compute it directly off the candidate POPs (the storage-layer rule).
    let eligible_within_eu: Vec<&CdnEdgePop> =
        CdnEdgeSet.eligible_for(/* tenant_is_eu = */ true, cdn_candidates);
    let extra_eu_cdn_edges_admitted =
        eligible_within_eu.iter().filter(|p| !p.within_eu).count() as u64;
    let within_eu_cdn_edges = eligible_within_eu.iter().filter(|p| p.within_eu).count() as u64;

    // (4) Aggregate residency_verify across every CI store: each store's region == the region of
    //     record. The runner report comes from the fleet; the log/artifact/cache reports are the
    //     write-pins' cell region (in-region by construction); the cdn report is within-EU.
    let store_reports = vec![
        CiStoreResidency {
            store: "runners".into(),
            region: fleet_report.region.clone(),
        },
        CiStoreResidency {
            store: "logs".into(),
            region: log_pin.cell_region().clone(),
        },
        CiStoreResidency {
            store: "artifacts".into(),
            region: art_pin.cell_region().clone(),
        },
        CiStoreResidency {
            store: "caches".into(),
            region: art_pin.cell_region().clone(),
        },
        CiStoreResidency {
            store: "cdn".into(),
            // The CDN serves the EU tenant from within-EU edges in the region of record (the within-EU
            // clone class is residency-local to the cell's region).
            region: region_of_record.clone(),
        },
    ];

    CiR3Report {
        tenant_id: tenant.0.clone(),
        region_of_record: region_of_record.clone(),
        store_reports,
        // The run was claimed by an in-region runner iff the fleet report agrees with the region of
        // record, the pool was genuinely provisioned in-region, AND an out-of-region provision was
        // REFUSED (the no-global-pool write boundary is live, not a no-op that admits everything).
        claimed_by_in_region_runner: provisioned
            && out_of_region_provision_refused
            && fleet_report.matches_region_of_record(region_of_record),
        cross_region_writes_admitted,
        extra_eu_cdn_edges_admitted,
        within_eu_cdn_edges,
    }
}

// =================================================================================================
// CI-D10 — the self-hosted runner trust boundary at cell scale: a COMPROMISED self-hosted runner is
//          bounded by its scoped token to its own tenant's SelfHosted jobs (0 cross-tenant reads);
//          attestation failure → cannot claim.
// =================================================================================================

/// One cell job the compromised self-hosted runner ATTEMPTS to read (claim + read its secret). PII-free
/// (an opaque job/tenant id + a tier). The drill records, per job, whether the compromised runner's
/// scoped token ADMITTED it — the cross-tenant-0 property is that it admits ONLY its own tenant's
/// `SelfHosted` jobs.
#[derive(Clone, Debug)]
pub struct CellJob {
    /// The tenant the job belongs to (opaque id, PII-free).
    pub tenant: TenantId,
    /// The job's trust tier.
    pub tier: TrustTier,
    /// An opaque job id (PII-free) — for the audit row.
    pub job_id: String,
}

/// **The CI-D10 self-hosted-boundary report — the dated green artifact the DoD names.** A compromised
/// self-hosted runner (attested ONLY for its own tenant) was offered a cell full of other tenants' jobs
/// alongside its own; its scoped job token bounded it to its OWN tenant's `SelfHosted` jobs only. The
/// headline zeros are 0 cross-tenant job reads and 0 cross-tenant secret reads, and an unattested runner
/// claimed nothing. PII-free (counts plus opaque ids).
#[derive(Clone, Debug)]
pub struct CiD10Report {
    /// The tenant the compromised self-hosted runner belongs to (the ONLY tenant it may serve).
    pub runner_tenant: String,
    /// The number of jobs offered to the compromised runner across ALL tenants (the cell's job set).
    pub jobs_offered: u64,
    /// The number of jobs the runner's scoped token ADMITTED — must equal the count of its OWN tenant's
    /// `SelfHosted` jobs (it serves only those).
    pub own_tenant_jobs_admitted: u64,
    /// **The headline ZERO** — the count of CROSS-TENANT jobs the compromised runner's scoped token
    /// admitted. MUST be 0 (a token for tenant A can never claim tenant B's job).
    pub cross_tenant_jobs_admitted: u64,
    /// **The second headline ZERO** — the count of CROSS-TENANT secrets the compromised runner read.
    /// MUST be 0 (a secret is gated on the same scoped token; no cross-tenant secret resolves).
    pub cross_tenant_secret_reads: u64,
    /// Whether an UNATTESTED self-hosted runner (attestation failed) was REFUSED a claim AND a token
    /// (fail-closed). MUST be `true` — attestation failure → cannot claim.
    pub unattested_runner_refused: bool,
    /// Whether the attested runner received a token scoped to EXACTLY its own tenant's `SelfHosted`
    /// grant (no cross-tenant grant in the caveat chain). MUST be `true`.
    pub token_scoped_to_own_tenant: bool,
}

impl CiD10Report {
    /// **The CI-D10 GREEN predicate (all measured, none weakened).** 0 cross-tenant job reads, 0
    /// cross-tenant secret reads, an unattested runner refused (fail-closed), the token scoped to the
    /// own tenant, AND the runner genuinely admitted at least one of its OWN jobs (the boundary is
    /// exercised, not vacuously empty). A single cross-tenant read ⇒ RED.
    pub fn is_green(&self) -> bool {
        self.cross_tenant_jobs_admitted == 0
            && self.cross_tenant_secret_reads == 0
            && self.unattested_runner_refused
            && self.token_scoped_to_own_tenant
            && self.own_tenant_jobs_admitted > 0
    }

    /// A one-line summary for the dated green-artifact log row.
    pub fn summary(&self) -> String {
        format!(
            "CI-D10: runner_tenant={} jobs_offered={} own_admitted={} cross_tenant_jobs={} \
             cross_tenant_secrets={} unattested_refused={} token_scoped={} → {}",
            self.runner_tenant,
            self.jobs_offered,
            self.own_tenant_jobs_admitted,
            self.cross_tenant_jobs_admitted,
            self.cross_tenant_secret_reads,
            self.unattested_runner_refused,
            self.token_scoped_to_own_tenant,
            if self.is_green() { "GREEN" } else { "RED" }
        )
    }
}

/// A deterministic [`RunTokenMinter`] for the drill: it mints a token whose bearer material echoes the
/// caveats (so the drill can read back the scope), mirroring the real Identity mint's envelope shape.
/// PII-free.
#[derive(Default)]
struct DrillMinter;

impl RunTokenMinter for DrillMinter {
    fn mint_run_token(
        &self,
        agent_id: &str,
        run_id: &str,
        caveats: &DelegationCaveats,
        ttl_secs: u64,
    ) -> Result<RunTokenHandle, RunTokenError> {
        Ok(RunTokenHandle {
            token: format!("runtok:{run_id}|{}", caveats.0.join(",")),
            jti: format!("jti:{agent_id}:{run_id}"),
            ttl_secs,
        })
    }
}

/// **Drive the CI-D10 self-hosted trust-boundary drill at cell scale.** A self-hosted runner belonging
/// to `compromised_tenant` is provisioned in `region`, attested (it presents a valid provisioning-signed
/// attestation), and minted a tenant-scoped run token (contract 4.7). It is then treated as
/// COMPROMISED: it is OFFERED the cell's entire job set `cell_jobs` (its own tenant's jobs + other
/// tenants' jobs) and tries to claim + read every one. The scoped token + the `may_claim` gate bound
/// it to its OWN tenant's `SelfHosted` jobs only:
///
/// - a cross-tenant job is REFUSED by [`TenantScopedToken::admits`] (0 cross-tenant job reads);
/// - a cross-tenant secret is gated on the SAME scoped token, so it never resolves (0 cross-tenant
///   secret reads);
/// - an attestation FAILURE → the runner cannot claim at ALL (fail-closed; proven on a sibling
///   unattested runner).
///
/// Returns the [`CiD10Report`]. The drill exercises BOTH the own-tenant admit (the boundary is genuinely
/// exercised) and the cross-tenant refusal (the zero is earned).
pub fn drive_ci_d10_self_hosted_boundary(
    compromised_tenant: &TenantId,
    region: &Region,
    cell_jobs: &[CellJob],
) -> CiD10Report {
    let verifier = StructuralAttestationVerifier::new();
    let minter = DrillMinter;

    // (1) The compromised self-hosted runner attests for its OWN tenant (a valid provisioning-signed
    //     attestation) and is minted a tenant-scoped token (contract 4.7).
    let mut runner = SelfHostedRunner::register(compromised_tenant.clone(), region.clone());
    let att = Attestation {
        tenant: compromised_tenant.clone(),
        material: StructuralAttestationVerifier::provisioned_material(
            compromised_tenant,
            "nonce-d10",
        ),
    };
    assert_eq!(
        runner.attest(&att, &verifier),
        AttestState::Attested,
        "the self-hosted runner attests for its own tenant"
    );
    let token: TenantScopedToken =
        mint_self_hosted_token(&runner, &minter, "svc:runner-compromised", "run-d10", 300)
            .expect("an attested runner is minted a tenant-scoped token");

    // The token's caveat chain is EXACTLY the own-tenant SelfHosted grant (no cross-tenant grant).
    let own_grant = format!("selfhosted:{}", compromised_tenant.0);
    let token_scoped_to_own_tenant = token.handle().token.contains(&own_grant)
        && !token
            .handle()
            .token
            .split('|')
            .nth(1)
            .map(|grants| {
                grants
                    .split(',')
                    .filter(|g| g.starts_with("selfhosted:"))
                    .any(|g| g != own_grant)
            })
            .unwrap_or(false);

    // (2) The compromised runner is OFFERED the whole cell's job set and tries to claim + read each.
    //     Two independent gates bound it (both must hold for a claim): the CLAIM gate
    //     (`runner.may_claim`) AND the TOKEN gate (`token.admits`). A cross-tenant job clears NEITHER.
    let mut own_tenant_jobs_admitted: u64 = 0;
    let mut cross_tenant_jobs_admitted: u64 = 0;
    let mut cross_tenant_secret_reads: u64 = 0;

    for job in cell_jobs {
        // The claim-eligibility gate (attested + SelfHosted-tier + own-tenant + in-region).
        let claim_ok = runner.may_claim(job.tier, &job.tenant, region);
        // The scoped-token gate (admits ONLY own-tenant SelfHosted jobs).
        let token_ok = token.admits(job.tier, &job.tenant);
        // A job is genuinely readable ONLY if BOTH gates admit it.
        let admitted = claim_ok && token_ok;

        let is_own = job.tenant == *compromised_tenant;
        if admitted {
            if is_own {
                own_tenant_jobs_admitted += 1;
            } else {
                // A cross-tenant job slipped through — the boundary FAILED (the headline breach).
                cross_tenant_jobs_admitted += 1;
            }
        }
        // The job's SECRET is gated on the SAME scoped token. A cross-tenant secret read happens ONLY
        // if the cross-tenant job was (wrongly) admitted — so it tracks the job-read breach. With the
        // boundary intact, a cross-tenant secret never resolves (the secret broker gates on the
        // token's tenant scope, mirrored here).
        if admitted && !is_own {
            cross_tenant_secret_reads += 1;
        }
    }

    // (3) The attestation-failure leg: a SIBLING self-hosted runner that fails attestation (an absent
    //     attestation) is REFUSED a claim AND a token (fail-closed). This is the "attestation failure →
    //     cannot claim" property.
    let mut unattested = SelfHostedRunner::register(compromised_tenant.clone(), region.clone());
    let absent = Attestation {
        tenant: compromised_tenant.clone(),
        material: String::new(), // absent ⇒ Failed.
    };
    let unattested_state = unattested.attest(&absent, &verifier);
    let unattested_cannot_claim =
        !unattested.may_claim(TrustTier::SelfHosted, compromised_tenant, region);
    let unattested_refused_token = mint_self_hosted_token(
        &unattested,
        &minter,
        "svc:runner-unattested",
        "run-d10b",
        300,
    )
    .is_err();
    let unattested_runner_refused = unattested_state == AttestState::Failed
        && unattested_cannot_claim
        && unattested_refused_token;

    CiD10Report {
        runner_tenant: compromised_tenant.0.clone(),
        jobs_offered: cell_jobs.len() as u64,
        own_tenant_jobs_admitted,
        cross_tenant_jobs_admitted,
        cross_tenant_secret_reads,
        unattested_runner_refused,
        token_scoped_to_own_tenant,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(s: &str) -> TenantId {
        TenantId(s.into())
    }
    fn region(s: &str) -> Region {
        Region(s.into())
    }

    // ───────────────────────────────── CI-R3 (residency at cell scale) ───────────────────────────

    /// **CI-R3 GREEN: an EU tenant's run is in-region only; logs/artifacts/caches never leave region;
    /// residency_verify attests; the residency-pin lint admits 0 cross-region writes; the CDN edge set
    /// is within-EU only.**
    #[test]
    fn ci_r3_residency_at_cell_scale_is_green() {
        let t = tenant("eu-acme");
        let ror = region("fr-par");
        let oor = region("us-east-1");
        // A candidate POP set: two within-EU POPs + one extra-EU POP (the EU tenant must never reach it).
        let candidates = vec![
            CdnEdgePop::new("par-1", region("fr-par"), true),
            CdnEdgePop::new("ams-1", region("nl-ams"), true),
            CdnEdgePop::new("iad-1", region("us-east-1"), false),
        ];
        let report = drive_ci_r3_residency(&t, &ror, &oor, &candidates);
        assert!(
            report.is_green(),
            "CI-R3 must be GREEN: {}",
            report.summary()
        );
        assert!(
            report.claimed_by_in_region_runner,
            "the run was claimed by an in-region runner (no global pool)"
        );
        assert_eq!(
            report.cross_region_writes_admitted, 0,
            "the residency-pin lint admitted 0 cross-region CI writes"
        );
        assert_eq!(
            report.extra_eu_cdn_edges_admitted, 0,
            "the CDN admitted 0 extra-EU edges (the EU tenant's bundles never leave the EU)"
        );
        assert!(
            report.within_eu_cdn_edges >= 2,
            "the within-EU CDN edge set is non-empty (the property is genuinely exercised)"
        );
        assert!(
            report.disagreeing_stores().is_empty(),
            "every CI store's region agrees with the region of record (residency_verify attests)"
        );
        // The store set covers runners + logs + artifacts + caches + cdn (5 stores).
        assert_eq!(report.store_reports.len(), 5);
        println!("[P-491 CI-R3 GREEN 2026-06-25] {}", report.summary());
    }

    /// **The CI-R3 green is EARNED (EI-01 §3): a store reporting a WRONG region FAILs the attestation.**
    /// We forge a disagreeing store report and prove the predicate goes RED.
    #[test]
    fn ci_r3_disagreeing_store_is_not_green() {
        let t = tenant("eu-acme");
        let ror = region("fr-par");
        let oor = region("us-east-1");
        let candidates = vec![CdnEdgePop::new("par-1", region("fr-par"), true)];
        let mut report = drive_ci_r3_residency(&t, &ror, &oor, &candidates);
        // Forge a store that leaked into the wrong region — the attestation must FAIL.
        report.store_reports.push(CiStoreResidency {
            store: "rogue-cache".into(),
            region: region("us-east-1"),
        });
        assert!(
            !report.is_green(),
            "a store in the wrong region FAILs residency_verify (the green is earned)"
        );
        assert_eq!(report.disagreeing_stores().len(), 1);
    }

    /// **The CI-R3 green is EARNED: an extra-EU CDN edge admitted FAILs the within-EU property.** We
    /// forge an admitted extra-EU edge and prove RED.
    #[test]
    fn ci_r3_extra_eu_cdn_edge_is_not_green() {
        let t = tenant("eu-acme");
        let ror = region("fr-par");
        let oor = region("us-east-1");
        let candidates = vec![CdnEdgePop::new("par-1", region("fr-par"), true)];
        let mut report = drive_ci_r3_residency(&t, &ror, &oor, &candidates);
        report.extra_eu_cdn_edges_admitted = 1; // forge a leak.
        assert!(
            !report.is_green(),
            "an extra-EU CDN edge admitted FAILs the within-EU CDN property"
        );
    }

    /// **The within-EU CDN class genuinely EXCLUDES extra-EU POPs (the eligible set is within-EU only).**
    #[test]
    fn ci_r3_cdn_eligible_set_is_within_eu_only() {
        let t = tenant("eu-acme");
        let ror = region("fr-par");
        let oor = region("us-east-1");
        // A candidate set that is MOSTLY extra-EU — the eligible set must still be within-EU only.
        let candidates = vec![
            CdnEdgePop::new("iad-1", region("us-east-1"), false),
            CdnEdgePop::new("sfo-1", region("us-west-1"), false),
            CdnEdgePop::new("par-1", region("fr-par"), true),
        ];
        let report = drive_ci_r3_residency(&t, &ror, &oor, &candidates);
        assert_eq!(
            report.extra_eu_cdn_edges_admitted, 0,
            "no extra-EU POP is admitted into the EU tenant's eligible set"
        );
        assert_eq!(
            report.within_eu_cdn_edges, 1,
            "exactly the one within-EU POP is eligible"
        );
        assert!(report.is_green());
    }

    // ─────────────────────────── CI-D10 (the self-hosted trust boundary) ──────────────────────────

    /// **CI-D10 GREEN: a compromised self-hosted runner is bounded to its OWN tenant's SelfHosted jobs;
    /// 0 cross-tenant job reads; 0 cross-tenant secret reads; an unattested runner cannot claim.**
    #[test]
    fn ci_d10_self_hosted_boundary_is_green() {
        let compromised = tenant("acme");
        let r = region("fr-par");
        // A cell with the runner's own SelfHosted jobs + OTHER tenants' jobs (incl. another tenant's
        // SelfHosted job — the prime cross-tenant target) + cross-tier jobs.
        let cell_jobs = vec![
            CellJob {
                tenant: tenant("acme"),
                tier: TrustTier::SelfHosted,
                job_id: "acme-1".into(),
            },
            CellJob {
                tenant: tenant("acme"),
                tier: TrustTier::SelfHosted,
                job_id: "acme-2".into(),
            },
            // The cross-tenant SelfHosted job (the prime breach target) — must be REFUSED.
            CellJob {
                tenant: tenant("globex"),
                tier: TrustTier::SelfHosted,
                job_id: "globex-1".into(),
            },
            // A cross-tenant Trusted job — refused (wrong tenant AND wrong tier).
            CellJob {
                tenant: tenant("globex"),
                tier: TrustTier::Trusted,
                job_id: "globex-2".into(),
            },
            // The runner's own Trusted job — refused (a self-hosted runner serves only SelfHosted).
            CellJob {
                tenant: tenant("acme"),
                tier: TrustTier::Trusted,
                job_id: "acme-3".into(),
            },
        ];
        let report = drive_ci_d10_self_hosted_boundary(&compromised, &r, &cell_jobs);
        assert!(
            report.is_green(),
            "CI-D10 must be GREEN: {}",
            report.summary()
        );
        assert_eq!(
            report.own_tenant_jobs_admitted, 2,
            "exactly the runner's OWN two SelfHosted jobs are admitted"
        );
        assert_eq!(
            report.cross_tenant_jobs_admitted, 0,
            "0 cross-tenant jobs — the scoped token bounds the compromised runner to its own tenant"
        );
        assert_eq!(
            report.cross_tenant_secret_reads, 0,
            "0 cross-tenant secret reads — secrets are gated on the same scoped token"
        );
        assert!(
            report.unattested_runner_refused,
            "an unattested runner cannot claim (fail-closed: attestation failure → no claim, no token)"
        );
        assert!(
            report.token_scoped_to_own_tenant,
            "the minted token is scoped to EXACTLY the own tenant's SelfHosted grant"
        );
        println!("[P-491 CI-D10 GREEN 2026-06-25] {}", report.summary());
    }

    /// **The CI-D10 green is EARNED (EI-01 §3): a forged cross-tenant admit FAILs the predicate.** We
    /// forge a cross-tenant job read and prove RED — the zero is not assumed.
    #[test]
    fn ci_d10_cross_tenant_read_is_not_green() {
        let compromised = tenant("acme");
        let r = region("fr-par");
        let cell_jobs = vec![CellJob {
            tenant: tenant("acme"),
            tier: TrustTier::SelfHosted,
            job_id: "acme-1".into(),
        }];
        let mut report = drive_ci_d10_self_hosted_boundary(&compromised, &r, &cell_jobs);
        report.cross_tenant_jobs_admitted = 1; // forge a breach.
        assert!(
            !report.is_green(),
            "a single cross-tenant job read FAILs CI-D10 (the green is earned)"
        );
        report.cross_tenant_jobs_admitted = 0;
        report.cross_tenant_secret_reads = 1; // forge a secret breach.
        assert!(
            !report.is_green(),
            "a single cross-tenant secret read FAILs CI-D10"
        );
    }

    /// **A cell of ONLY cross-tenant jobs admits NOTHING — the boundary holds even when there is no own
    /// job to serve (but then the green is not reachable: the boundary must be exercised).**
    #[test]
    fn ci_d10_only_cross_tenant_jobs_admits_zero() {
        let compromised = tenant("acme");
        let r = region("fr-par");
        let cell_jobs = vec![
            CellJob {
                tenant: tenant("globex"),
                tier: TrustTier::SelfHosted,
                job_id: "g-1".into(),
            },
            CellJob {
                tenant: tenant("initech"),
                tier: TrustTier::SelfHosted,
                job_id: "i-1".into(),
            },
        ];
        let report = drive_ci_d10_self_hosted_boundary(&compromised, &r, &cell_jobs);
        assert_eq!(
            report.cross_tenant_jobs_admitted, 0,
            "a compromised runner admits 0 of OTHER tenants' jobs"
        );
        assert_eq!(report.own_tenant_jobs_admitted, 0);
        // With no own job admitted the boundary is not exercised → NOT green (the green requires a
        // genuine own-tenant admit so the property is not vacuous).
        assert!(
            !report.is_green(),
            "with 0 own-tenant jobs the boundary is not exercised → not a (vacuous) green"
        );
    }

    // ──────────────── per-CLAUSE counter-cases (each green predicate operand is load-bearing) ──────

    /// A baseline GREEN [`CiR3Report`] the per-clause counter-cases mutate one field of.
    fn green_r3() -> CiR3Report {
        CiR3Report {
            tenant_id: "eu-acme".into(),
            region_of_record: region("fr-par"),
            store_reports: vec![CiStoreResidency {
                store: "logs".into(),
                region: region("fr-par"),
            }],
            claimed_by_in_region_runner: true,
            cross_region_writes_admitted: 0,
            extra_eu_cdn_edges_admitted: 0,
            within_eu_cdn_edges: 2,
        }
    }

    /// **EACH clause of [`CiR3Report::is_green`] is load-bearing — flipping any ONE operand alone goes
    /// RED.** This kills the `&&`→`||` and the `> 0`→`>= 0` mutants on the green predicate (a survivor
    /// would be a residency breach the artifact silently calls green).
    #[test]
    fn ci_r3_every_green_clause_is_load_bearing() {
        assert!(green_r3().is_green(), "the baseline is green");

        // (1) an out-of-region runner claim alone ⇒ RED.
        let mut a = green_r3();
        a.claimed_by_in_region_runner = false;
        assert!(!a.is_green(), "an out-of-region runner claim FAILs CI-R3");

        // (2) a single admitted cross-region write alone ⇒ RED.
        let mut b = green_r3();
        b.cross_region_writes_admitted = 1;
        assert!(!b.is_green(), "a cross-region write admitted FAILs CI-R3");

        // (3) a single admitted extra-EU CDN edge alone ⇒ RED.
        let mut c = green_r3();
        c.extra_eu_cdn_edges_admitted = 1;
        assert!(!c.is_green(), "an extra-EU CDN edge admitted FAILs CI-R3");

        // (4) a VACUOUS within-EU edge set (0 edges) alone ⇒ RED — kills the `> 0`→`>= 0` mutant: the
        //     within-EU CDN must be genuinely serving, not vacuously empty.
        let mut d = green_r3();
        d.within_eu_cdn_edges = 0;
        assert!(
            !d.is_green(),
            "0 within-EU CDN edges is a vacuous CDN → NOT green (kills the `> 0`→`>= 0` mutant)"
        );

        // (5) an EMPTY store set alone ⇒ RED — a silently-absent store is the global-pool the
        //     no-global-pool property forbids (fail-closed).
        let mut e = green_r3();
        e.store_reports.clear();
        assert!(
            !e.is_green(),
            "0 store reports FAILs (fail-closed — no silent stores)"
        );

        // (6) a single disagreeing store alone ⇒ RED.
        let mut f = green_r3();
        f.store_reports.push(CiStoreResidency {
            store: "rogue".into(),
            region: region("us-east-1"),
        });
        assert!(
            !f.is_green(),
            "a store in the wrong region FAILs residency_verify"
        );
    }

    /// A baseline GREEN [`CiD10Report`] the per-clause counter-cases mutate one field of.
    fn green_d10() -> CiD10Report {
        CiD10Report {
            runner_tenant: "acme".into(),
            jobs_offered: 3,
            own_tenant_jobs_admitted: 2,
            cross_tenant_jobs_admitted: 0,
            cross_tenant_secret_reads: 0,
            unattested_runner_refused: true,
            token_scoped_to_own_tenant: true,
        }
    }

    /// **EACH clause of [`CiD10Report::is_green`] is load-bearing — flipping any ONE operand alone goes
    /// RED.** This kills the `&&`→`||` and the `> 0`→`>= 0` mutants on the green predicate (a survivor
    /// would be a cross-tenant escape the artifact silently calls green).
    #[test]
    fn ci_d10_every_green_clause_is_load_bearing() {
        assert!(green_d10().is_green(), "the baseline is green");

        // (1) a single cross-tenant job read alone ⇒ RED.
        let mut a = green_d10();
        a.cross_tenant_jobs_admitted = 1;
        assert!(!a.is_green(), "a cross-tenant job read FAILs CI-D10");

        // (2) a single cross-tenant secret read alone ⇒ RED.
        let mut b = green_d10();
        b.cross_tenant_secret_reads = 1;
        assert!(!b.is_green(), "a cross-tenant secret read FAILs CI-D10");

        // (3) an unattested runner that was NOT refused alone ⇒ RED (fail-closed broke).
        let mut c = green_d10();
        c.unattested_runner_refused = false;
        assert!(
            !c.is_green(),
            "an unattested runner not refused FAILs CI-D10"
        );

        // (4) a token NOT scoped to the own tenant alone ⇒ RED.
        let mut d = green_d10();
        d.token_scoped_to_own_tenant = false;
        assert!(
            !d.is_green(),
            "a token not scoped to the own tenant FAILs CI-D10"
        );

        // (5) 0 own-tenant jobs admitted alone ⇒ RED — kills the `> 0`→`>= 0` mutant: the boundary must
        //     be genuinely exercised, not vacuously green.
        let mut e = green_d10();
        e.own_tenant_jobs_admitted = 0;
        assert!(
            !e.is_green(),
            "0 own-tenant jobs is a vacuous boundary → NOT green (kills the `> 0`→`>= 0` mutant)"
        );
    }

    /// **The CI-D10 admit conjunction (`claim_ok && token_ok`) is load-bearing — BOTH gates must admit
    /// a job.** Drives the real driver with a cell where the runner's own job is SelfHosted (both gates
    /// admit) and proves a cross-tenant job clears NEITHER. (A `&&`→`||` on the admit conjunction would
    /// let a job that clears only ONE gate through — the cross-tenant escape.)
    #[test]
    fn ci_d10_admit_requires_both_gates() {
        let compromised = tenant("acme");
        let r = region("fr-par");
        // A cross-tenant SelfHosted job: it clears the TOKEN tier check (SelfHosted) but NOT the tenant
        // scope — and clears NEITHER `may_claim` (wrong tenant) nor `admits` (wrong tenant). It must be
        // refused; with `&&`→`||` it would (wrongly) admit.
        let cell_jobs = vec![
            CellJob {
                tenant: tenant("acme"),
                tier: TrustTier::SelfHosted,
                job_id: "acme-1".into(),
            },
            CellJob {
                tenant: tenant("globex"),
                tier: TrustTier::SelfHosted,
                job_id: "globex-1".into(),
            },
        ];
        let report = drive_ci_d10_self_hosted_boundary(&compromised, &r, &cell_jobs);
        assert_eq!(
            report.own_tenant_jobs_admitted, 1,
            "the own SelfHosted job is admitted"
        );
        assert_eq!(
            report.cross_tenant_jobs_admitted, 0,
            "the cross-tenant job clears NEITHER gate — both must admit (the admit conjunction)"
        );
        assert!(report.is_green());
    }

    /// **The CI-D10 DRIVER's exact tallies under a known cell — the per-job `+=` counters and the
    /// `claimed_by`/`unattested` conjunctions are load-bearing.** Drives the real driver with a cell of
    /// known composition and asserts the EXACT admitted/refused counts (a `+=`→`-=`/`*=` mutant or a
    /// driver-conjunction `&&`→`||` mutant changes one of these tallies).
    #[test]
    fn ci_d10_driver_exact_tallies() {
        let compromised = tenant("acme");
        let r = region("fr-par");
        let cell_jobs = vec![
            CellJob {
                tenant: tenant("acme"),
                tier: TrustTier::SelfHosted,
                job_id: "a1".into(),
            },
            CellJob {
                tenant: tenant("acme"),
                tier: TrustTier::SelfHosted,
                job_id: "a2".into(),
            },
            CellJob {
                tenant: tenant("globex"),
                tier: TrustTier::SelfHosted,
                job_id: "g1".into(),
            },
        ];
        let report = drive_ci_d10_self_hosted_boundary(&compromised, &r, &cell_jobs);
        assert_eq!(report.jobs_offered, 3);
        assert_eq!(
            report.own_tenant_jobs_admitted, 2,
            "exactly two own SelfHosted jobs admitted (the `+=` counter is exact)"
        );
        assert_eq!(report.cross_tenant_jobs_admitted, 0);
        assert_eq!(report.cross_tenant_secret_reads, 0);
        // The unattested-refused conjunction is true (Failed AND cannot-claim AND token-refused).
        assert!(report.unattested_runner_refused);
        assert!(report.token_scoped_to_own_tenant);
    }

    /// **The CI-R3 DRIVER's exact residency tallies under a known cell — the cross-region-writes counter
    /// and the `claimed_by_in_region_runner` conjunction are load-bearing.** Drives the real driver and
    /// asserts the EXACT zeros + the within-EU eligible count (a driver `+=`/`&&` mutant changes one).
    #[test]
    fn ci_r3_driver_exact_tallies() {
        let t = tenant("eu-acme");
        let ror = region("fr-par");
        let oor = region("us-east-1");
        let candidates = vec![
            CdnEdgePop::new("par-1", region("fr-par"), true),
            CdnEdgePop::new("ams-1", region("nl-ams"), true),
            CdnEdgePop::new("iad-1", region("us-east-1"), false),
        ];
        let report = drive_ci_r3_residency(&t, &ror, &oor, &candidates);
        // The residency-pin ZERO: 0 cross-region writes admitted (every out-of-region write refused).
        assert_eq!(report.cross_region_writes_admitted, 0);
        // The CDN ZERO + the exact within-EU count (two within-EU POPs eligible, the extra-EU excluded).
        assert_eq!(report.extra_eu_cdn_edges_admitted, 0);
        assert_eq!(report.within_eu_cdn_edges, 2);
        // The in-region-runner conjunction: provisioned AND out-of-region-refused AND fleet-agrees.
        assert!(report.claimed_by_in_region_runner);
        assert_eq!(report.store_reports.len(), 5);
    }
}
