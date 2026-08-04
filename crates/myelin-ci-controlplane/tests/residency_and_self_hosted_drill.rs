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

#[test]
fn ci_r3_eu_run_is_in_region_only_residency_verify_attests() {
    let t = tenant("eu-resident-acme");
    let region_of_record = region("fr-par");
    let out_of_region = region("us-east-1");
    let cdn_candidates = vec![
        CdnEdgePop::new("par-1", region("fr-par"), true),
        CdnEdgePop::new("ams-1", region("nl-ams"), true),
        CdnEdgePop::new("fra-1", region("de-fra"), true),
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

#[test]
fn ci_r3_a_leaked_store_fails_residency_verify() {
    let t = tenant("eu-resident-acme");
    let region_of_record = region("fr-par");
    let out_of_region = region("us-east-1");
    let cdn_candidates = vec![CdnEdgePop::new("par-1", region("fr-par"), true)];
    let mut report = drive_ci_r3_residency(&t, &region_of_record, &out_of_region, &cdn_candidates);
    report
        .store_reports
        .push(myelin_ci_controlplane::CiStoreResidency {
            store: "leaked-artifact-store".into(),
            region: region("us-east-1"),
        });
    assert!(
        !report.is_green(),
        "a CI store in the wrong region FAILs residency_verify - the green is earned"
    );
    assert_eq!(report.disagreeing_stores().len(), 1);
    println!(
        "[P-491 CI-R3 counter-case 2026-06-25] a leaked store → RED ({} disagreeing) - the green is earned",
        report.disagreeing_stores().len()
    );
}

#[test]
fn ci_d10_compromised_self_hosted_runner_is_bounded_zero_cross_tenant_reads() {
    let compromised = tenant("acme");
    let r = region("fr-par");
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
        CellJob {
            tenant: tenant("globex"),
            tier: TrustTier::SelfHosted,
            job_id: "globex-sh-1".into(),
        },
        CellJob {
            tenant: tenant("initech"),
            tier: TrustTier::SelfHosted,
            job_id: "initech-sh-1".into(),
        },
        CellJob {
            tenant: tenant("globex"),
            tier: TrustTier::Trusted,
            job_id: "globex-trusted-1".into(),
        },
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
        "0 cross-tenant jobs - the scoped token bounds the compromised runner to its own tenant"
    );
    assert_eq!(
        report.cross_tenant_secret_reads, 0,
        "0 cross-tenant secret reads - every secret is gated on the same scoped token"
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
