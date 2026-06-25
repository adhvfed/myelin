//! # CI-R3 + CI-D10 — world-scale hardening at CELL scale: residency at scale + the self-hosted
//! runner trust boundary (CI-P31 / P-491, M5).
//!
//! **Drill catalogue:** `01-whole-system-e2e-and-drill-catalogue.md` rows **CI-R3** (*"an EU-resident
//! tenant's run → claimed ONLY by an in-region runner; logs/artifacts/caches never leave region;
//! `residency_verify` attests; the residency-pin lint passes on every CI write"*) and **CI-D10**
//! (*"a compromised self-hosted runner → the scoped job token bounds it to its own tenant's
//! `SelfHosted` jobs only; 0 cross-tenant job/secret reads; attestation failure → cannot claim"*),
//! cadence SCHED. **Architecture:** continuous-integration `00-overview.md` §5 (cell topology — NO
//! global runner pool; an EU-resident tenant's job claimed only by an in-region runner),
//! `05-hard-problems.md` HP-2 (runner-fleet elasticity on EU infra). **Reconciliation:**
//! `00-reconciliation-decisions.md` §10 (the CI no-global-pool residency attestation), §1 / X-6 (the
//! self-hosted scoped token — one tenant's `SelfHosted` jobs only). **Contract-index:** 12.4
//! (`residency_verify` — at cell scale), 4.7 (the self-hosted scoped token), 1.6 (the residency-pin
//! lint), 1.8 (the telemetry). **Doctrine:** EI-01 §3 (prove-it — residency + the self-hosted boundary
//! are QUANTIFIED drills; the green is EARNED via the refused/leaked counter-case, never a weakened
//! threshold), §2 (the leak families — residency leak + cross-tenant escape).
//!
//! ## What these two drills prove (the dated green artifacts the DoD names)
//! - **CI-R3:** an EU-resident tenant's CI run lands ENTIRELY in-region — the runner pool is
//!   provisioned region-pinned (an out-of-region provision is REFUSED); the logs / artifacts / caches
//!   are written through the residency-pin write boundary (an out-of-region write is REFUSED → 0
//!   cross-region writes admitted); the within-EU CDN clone class excludes EVERY extra-EU edge (the EU
//!   tenant's bundles never reach an extra-EU POP); and `residency_verify` attests (every CI store's
//!   region agrees with the region of record).
//! - **CI-D10:** a COMPROMISED self-hosted runner (attested only for its own tenant) is offered the
//!   whole cell's job set; its scoped job token (contract 4.7) bounds it to its OWN tenant's
//!   `SelfHosted` jobs only — 0 cross-tenant job reads, 0 cross-tenant secret reads; and an UNATTESTED
//!   runner is refused a claim AND a token (fail-closed: attestation failure → cannot claim).
//!
//! ## The mutation-score floor (mandatory-core — the cross-tenant isolation boundary)
//! The self-hosted-token-scoping path (`TenantScopedToken::admits` / `mint_self_hosted_token` /
//! `SelfHostedRunner::may_claim`) carries a **100% (zero surviving mutants)** cargo-mutants floor — a
//! surviving mutant is either an unattested runner that CAN claim or a token for tenant A that admits
//! tenant B's job (the cross-tenant escape). This drill is the cell-scale exercise of that exact gate.
//!
//! ## Floors named (the prompt's honesty register)
//! - **Cross-cell-spanning runs** (a CI run that fans across two cells, inheriting the 12.6 cross-cell
//!   PII-free pointer bridge) are a **deferred-until-demand named floor**, handled at **CI-P29** by
//!   reference (`floor_followons::DEFERRED_BY_REFERENCE_FLOORS`, the `cross-cell-spanning-pipelines`
//!   row). This drill is single-cell.
//! - **The REAL 30× world-scale FLEET-hardware load** is the ONE legitimate remaining floor (real
//!   fleet, named platform-wide). Here the cell is a deterministic multi-runner/multi-tenant fixture;
//!   the residency + self-hosted-boundary PROPERTIES are complete and do not change shape under real
//!   cell load.

use myelin_ci_controlplane::{drive_ci_d10_self_hosted_boundary, drive_ci_r3_residency, CellJob};
use myelin_ci_sandbox::TrustTier;
use myelin_storage::cdn::CdnEdgePop;
use myelin_tenancy::{Region, TenantId};

fn tenant(s: &str) -> TenantId {
    TenantId(s.into())
}
fn region(s: &str) -> Region {
    Region(s.into())
}

/// **THE CI-R3 RESIDENCY-AT-CELL-SCALE PROOF (the dated green artifact the DoD names).** An EU-resident
/// tenant (region of record `fr-par`) issues a CI run into a cell pinned to that region. The drill
/// proves: the runner pool is provisioned in-region only (an out-of-region provision REFUSED); the
/// logs/artifacts/caches are written through the residency-pin boundary (0 cross-region writes
/// admitted); the within-EU CDN clone class excludes the extra-EU POP (0 extra-EU edges); and
/// `residency_verify` attests (every CI store agrees with the region of record).
#[test]
fn ci_r3_eu_run_is_in_region_only_residency_verify_attests() {
    let t = tenant("eu-resident-acme");
    let region_of_record = region("fr-par");
    let out_of_region = region("us-east-1");
    // The cell's candidate CDN POP set: within-EU POPs (eligible) + an extra-EU POP (must be excluded).
    let cdn_candidates = vec![
        CdnEdgePop::new("par-1", region("fr-par"), true),
        CdnEdgePop::new("ams-1", region("nl-ams"), true),
        CdnEdgePop::new("fra-1", region("de-fra"), true),
        // The extra-EU POP the EU tenant's bundles must NEVER reach.
        CdnEdgePop::new("iad-1", region("us-east-1"), false),
    ];

    let report = drive_ci_r3_residency(&t, &region_of_record, &out_of_region, &cdn_candidates);

    assert!(
        report.is_green(),
        "CI-R3 must be GREEN: {}",
        report.summary()
    );
    assert!(
        report.claimed_by_in_region_runner,
        "the EU tenant's run is claimed ONLY by an in-region runner (no global pool)"
    );
    assert_eq!(
        report.cross_region_writes_admitted, 0,
        "the residency-pin lint admitted 0 cross-region CI writes (logs/artifacts/caches never leave region)"
    );
    assert_eq!(
        report.extra_eu_cdn_edges_admitted, 0,
        "the within-EU CDN clone class admitted 0 extra-EU edges (the EU tenant's bundles never leave the EU)"
    );
    assert_eq!(
        report.within_eu_cdn_edges, 3,
        "exactly the three within-EU POPs are eligible (the within-EU CDN is genuinely serving)"
    );
    assert!(
        report.disagreeing_stores().is_empty(),
        "residency_verify attests: every CI store's region agrees with the region of record"
    );

    println!(
        "[P-491 CI-R3 GREEN 2026-06-25] {} (cell={}, runner pool + logs + artifacts + caches + cdn \
         all in-region; out-of-region provision + writes REFUSED)",
        report.summary(),
        report.region_of_record.as_str()
    );
}

/// **The CI-R3 green is EARNED (EI-01 §3) — a leaked store FAILs the attestation.** A CI store that
/// reports a region ≠ the region of record is the residency breach `residency_verify` exists to catch;
/// the predicate goes RED. (The counter-case proves the green is not vacuous.)
#[test]
fn ci_r3_a_leaked_store_fails_residency_verify() {
    let t = tenant("eu-resident-acme");
    let region_of_record = region("fr-par");
    let out_of_region = region("us-east-1");
    let cdn_candidates = vec![CdnEdgePop::new("par-1", region("fr-par"), true)];
    let mut report = drive_ci_r3_residency(&t, &region_of_record, &out_of_region, &cdn_candidates);
    // Inject a store that leaked into the wrong region (the failure injection).
    report
        .store_reports
        .push(myelin_ci_controlplane::CiStoreResidency {
            store: "leaked-artifact-store".into(),
            region: region("us-east-1"),
        });
    assert!(
        !report.is_green(),
        "a CI store in the wrong region FAILs residency_verify — the green is earned"
    );
    assert_eq!(report.disagreeing_stores().len(), 1);
    println!(
        "[P-491 CI-R3 counter-case 2026-06-25] a leaked store → RED ({} disagreeing) — the green is earned",
        report.disagreeing_stores().len()
    );
}

/// **THE CI-D10 SELF-HOSTED TRUST-BOUNDARY PROOF (the dated green artifact the DoD names).** A
/// COMPROMISED self-hosted runner (belonging to `acme`, attested only for `acme`) is offered the whole
/// cell's job set — its own SelfHosted jobs PLUS other tenants' jobs (incl. another tenant's SelfHosted
/// job, the prime cross-tenant target). Its scoped job token bounds it to its OWN tenant's SelfHosted
/// jobs only: 0 cross-tenant job reads, 0 cross-tenant secret reads. An unattested sibling is refused a
/// claim AND a token (fail-closed).
#[test]
fn ci_d10_compromised_self_hosted_runner_is_bounded_zero_cross_tenant_reads() {
    let compromised = tenant("acme");
    let r = region("fr-par");
    // The cell's job set: acme's own SelfHosted jobs + globex's SelfHosted job (the prime target) +
    // cross-tier jobs. The compromised runner tries to claim + read EVERY one.
    let cell_jobs = vec![
        CellJob {
            tenant: tenant("acme"),
            tier: TrustTier::SelfHosted,
            job_id: "acme-sh-1".into(),
        },
        CellJob {
            tenant: tenant("acme"),
            tier: TrustTier::SelfHosted,
            job_id: "acme-sh-2".into(),
        },
        CellJob {
            tenant: tenant("acme"),
            tier: TrustTier::SelfHosted,
            job_id: "acme-sh-3".into(),
        },
        // The cross-tenant SelfHosted job — the prime breach target. MUST be refused.
        CellJob {
            tenant: tenant("globex"),
            tier: TrustTier::SelfHosted,
            job_id: "globex-sh-1".into(),
        },
        // A second cross-tenant SelfHosted job (another tenant) — refused.
        CellJob {
            tenant: tenant("initech"),
            tier: TrustTier::SelfHosted,
            job_id: "initech-sh-1".into(),
        },
        // A cross-tenant Trusted job — refused (wrong tenant AND wrong tier).
        CellJob {
            tenant: tenant("globex"),
            tier: TrustTier::Trusted,
            job_id: "globex-trusted-1".into(),
        },
        // The runner's OWN Trusted job — refused (a self-hosted runner serves only SelfHosted).
        CellJob {
            tenant: tenant("acme"),
            tier: TrustTier::Trusted,
            job_id: "acme-trusted-1".into(),
        },
    ];

    let report = drive_ci_d10_self_hosted_boundary(&compromised, &r, &cell_jobs);

    assert!(
        report.is_green(),
        "CI-D10 must be GREEN: {}",
        report.summary()
    );
    assert_eq!(
        report.own_tenant_jobs_admitted, 3,
        "exactly the compromised runner's OWN three SelfHosted jobs are admitted"
    );
    assert_eq!(
        report.cross_tenant_jobs_admitted, 0,
        "0 cross-tenant jobs — the scoped token bounds the compromised runner to its own tenant"
    );
    assert_eq!(
        report.cross_tenant_secret_reads, 0,
        "0 cross-tenant secret reads — every secret is gated on the same scoped token"
    );
    assert!(
        report.unattested_runner_refused,
        "an unattested runner cannot claim (fail-closed: attestation failure → no claim, no token)"
    );
    assert!(
        report.token_scoped_to_own_tenant,
        "the minted token is scoped to EXACTLY the own tenant's SelfHosted grant (no cross-tenant grant)"
    );

    println!(
        "[P-491 CI-D10 GREEN 2026-06-25] {} (a compromised self-hosted runner offered {} jobs across \
         3 tenants reads ONLY its own {} SelfHosted jobs; 0 cross-tenant job/secret reads; unattested → no claim)",
        report.summary(),
        report.jobs_offered,
        report.own_tenant_jobs_admitted
    );
}

/// **The CI-D10 green is EARNED (EI-01 §3) — a cell of ONLY cross-tenant jobs admits NOTHING, and the
/// boundary is not vacuously green.** The compromised runner reads 0 of other tenants' jobs; with no
/// own-tenant job to serve the green is not reachable (the boundary must be genuinely exercised).
#[test]
fn ci_d10_only_cross_tenant_jobs_reads_zero_and_is_not_vacuously_green() {
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
        "the compromised runner reads 0 of OTHER tenants' jobs"
    );
    assert_eq!(report.own_tenant_jobs_admitted, 0);
    assert!(
        !report.is_green(),
        "with 0 own-tenant jobs the boundary is not exercised → not a vacuous green"
    );
    println!(
        "[P-491 CI-D10 counter-case 2026-06-25] only cross-tenant jobs → 0 reads, not a vacuous green"
    );
}
