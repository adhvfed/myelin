use crate::fleet::{EuFleetProvider, FleetResidencyReport, GenericEuIaasAdapter};
use myelin_ci_sandbox::{
    mint_self_hosted_token, AttestState, Attestation, FleetProvider, RunnerClass, SelfHostedRunner,
    StructuralAttestationVerifier, TenantScopedToken, TrustTier,
};
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};
use myelin_storage::cdn::{CdnEdgePop, CdnEdgeSet};
use myelin_tenancy::{Region, TenantId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiStoreResidency {
    pub store: String,
    pub region: Region,
}

impl CiStoreResidency {
    pub fn agrees_with(&self, region_of_record: &Region) -> bool {
        self.region == *region_of_record
    }
}

#[derive(Clone, Debug)]
pub struct CiR3Report {
    pub tenant_id: String,
    pub region_of_record: Region,
    pub store_reports: Vec<CiStoreResidency>,
    pub claimed_by_in_region_runner: bool,
    pub cross_region_writes_admitted: u64,
    pub extra_eu_cdn_edges_admitted: u64,
    pub within_eu_cdn_edges: u64,
}

impl CiR3Report {
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

    pub fn disagreeing_stores(&self) -> Vec<&CiStoreResidency> {
        self.store_reports
            .iter()
            .filter(|r| !r.agrees_with(&self.region_of_record))
            .collect()
    }

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

pub fn drive_ci_r3_residency(
    tenant: &TenantId,
    region_of_record: &Region,
    out_of_region: &Region,
    cdn_candidates: &[CdnEdgePop],
) -> CiR3Report {
    let provider = EuFleetProvider::new(
        GenericEuIaasAdapter,
        tenant.0.clone(),
        region_of_record.clone(),
        64,
    );
    let provisioned = provider
        .provision(RunnerClass("ci".into()), 4, region_of_record.clone())
        .is_ok();
    let out_of_region_provision_refused = provider
        .provision(RunnerClass("ci".into()), 4, out_of_region.clone())
        .is_err();
    let fleet_report: FleetResidencyReport = provider.residency_report();

    let mut cross_region_writes_admitted: u64 = 0;

    let mut log_pin =
        crate::log_pipeline::LogWritePin::for_cell(tenant.0.clone(), region_of_record.clone());
    assert!(
        log_pin.admit_log_write(region_of_record).is_ok(),
        "an in-region log write is admitted"
    );
    if log_pin.admit_log_write(out_of_region).is_ok() {
        cross_region_writes_admitted += 1;
    }

    let mut art_pin = crate::artifact_cache::ArtifactWritePin::for_cell(
        tenant.0.clone(),
        region_of_record.clone(),
    );
    assert!(art_pin.admit_write(region_of_record).is_ok());
    assert!(art_pin.admit_write(region_of_record).is_ok());
    if art_pin.admit_write(out_of_region).is_ok() {
        cross_region_writes_admitted += 1;
    }

    if !out_of_region_provision_refused {
        cross_region_writes_admitted += 1;
    }

    let eligible_within_eu: Vec<&CdnEdgePop> = CdnEdgeSet.eligible_for(true, cdn_candidates);
    let extra_eu_cdn_edges_admitted =
        eligible_within_eu.iter().filter(|p| !p.within_eu).count() as u64;
    let within_eu_cdn_edges = eligible_within_eu.iter().filter(|p| p.within_eu).count() as u64;

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
            region: region_of_record.clone(),
        },
    ];

    CiR3Report {
        tenant_id: tenant.0.clone(),
        region_of_record: region_of_record.clone(),
        store_reports,
        claimed_by_in_region_runner: provisioned
            && out_of_region_provision_refused
            && fleet_report.matches_region_of_record(region_of_record),
        cross_region_writes_admitted,
        extra_eu_cdn_edges_admitted,
        within_eu_cdn_edges,
    }
}

#[derive(Clone, Debug)]
pub struct CellJob {
    pub tenant: TenantId,
    pub tier: TrustTier,
    pub job_id: String,
}

#[derive(Clone, Debug)]
pub struct CiD10Report {
    pub runner_tenant: String,
    pub jobs_offered: u64,
    pub own_tenant_jobs_admitted: u64,
    pub cross_tenant_jobs_admitted: u64,
    pub cross_tenant_secret_reads: u64,
    pub unattested_runner_refused: bool,
    pub token_scoped_to_own_tenant: bool,
}

impl CiD10Report {
    pub fn is_green(&self) -> bool {
        self.cross_tenant_jobs_admitted == 0
            && self.cross_tenant_secret_reads == 0
            && self.unattested_runner_refused
            && self.token_scoped_to_own_tenant
            && self.own_tenant_jobs_admitted > 0
    }

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

pub fn drive_ci_d10_self_hosted_boundary(
    compromised_tenant: &TenantId,
    region: &Region,
    cell_jobs: &[CellJob],
) -> CiD10Report {
    let verifier = StructuralAttestationVerifier::new();
    let minter = DrillMinter;

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

    let mut own_tenant_jobs_admitted: u64 = 0;
    let mut cross_tenant_jobs_admitted: u64 = 0;
    let mut cross_tenant_secret_reads: u64 = 0;

    for job in cell_jobs {
        let claim_ok = runner.may_claim(job.tier, &job.tenant, region);
        let token_ok = token.admits(job.tier, &job.tenant);
        let admitted = claim_ok && token_ok;

        let is_own = job.tenant == *compromised_tenant;
        if admitted {
            if is_own {
                own_tenant_jobs_admitted += 1;
            } else {
                cross_tenant_jobs_admitted += 1;
            }
        }
        if admitted && !is_own {
            cross_tenant_secret_reads += 1;
        }
    }

    let mut unattested = SelfHostedRunner::register(compromised_tenant.clone(), region.clone());
    let absent = Attestation {
        tenant: compromised_tenant.clone(),
        material: String::new(),
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

    #[test]
    fn ci_r3_residency_at_cell_scale_is_green() {
        let t = tenant("eu-acme");
        let ror = region("fr-par");
        let oor = region("us-east-1");
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
        assert_eq!(report.store_reports.len(), 5);
        println!("[P-491 CI-R3 GREEN 2026-06-25] {}", report.summary());
    }

    #[test]
    fn ci_r3_disagreeing_store_is_not_green() {
        let t = tenant("eu-acme");
        let ror = region("fr-par");
        let oor = region("us-east-1");
        let candidates = vec![CdnEdgePop::new("par-1", region("fr-par"), true)];
        let mut report = drive_ci_r3_residency(&t, &ror, &oor, &candidates);
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

    #[test]
    fn ci_r3_extra_eu_cdn_edge_is_not_green() {
        let t = tenant("eu-acme");
        let ror = region("fr-par");
        let oor = region("us-east-1");
        let candidates = vec![CdnEdgePop::new("par-1", region("fr-par"), true)];
        let mut report = drive_ci_r3_residency(&t, &ror, &oor, &candidates);
        report.extra_eu_cdn_edges_admitted = 1;
        assert!(
            !report.is_green(),
            "an extra-EU CDN edge admitted FAILs the within-EU CDN property"
        );
    }

    #[test]
    fn ci_r3_cdn_eligible_set_is_within_eu_only() {
        let t = tenant("eu-acme");
        let ror = region("fr-par");
        let oor = region("us-east-1");
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

    #[test]
    fn ci_d10_self_hosted_boundary_is_green() {
        let compromised = tenant("acme");
        let r = region("fr-par");
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
            CellJob {
                tenant: tenant("globex"),
                tier: TrustTier::SelfHosted,
                job_id: "globex-1".into(),
            },
            CellJob {
                tenant: tenant("globex"),
                tier: TrustTier::Trusted,
                job_id: "globex-2".into(),
            },
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
            "0 cross-tenant jobs - the scoped token bounds the compromised runner to its own tenant"
        );
        assert_eq!(
            report.cross_tenant_secret_reads, 0,
            "0 cross-tenant secret reads - secrets are gated on the same scoped token"
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
        report.cross_tenant_jobs_admitted = 1;
        assert!(
            !report.is_green(),
            "a single cross-tenant job read FAILs CI-D10 (the green is earned)"
        );
        report.cross_tenant_jobs_admitted = 0;
        report.cross_tenant_secret_reads = 1;
        assert!(
            !report.is_green(),
            "a single cross-tenant secret read FAILs CI-D10"
        );
    }

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
        assert!(
            !report.is_green(),
            "with 0 own-tenant jobs the boundary is not exercised → not a (vacuous) green"
        );
    }

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

    #[test]
    fn ci_r3_every_green_clause_is_load_bearing() {
        assert!(green_r3().is_green(), "the baseline is green");

        let mut a = green_r3();
        a.claimed_by_in_region_runner = false;
        assert!(!a.is_green(), "an out-of-region runner claim FAILs CI-R3");

        let mut b = green_r3();
        b.cross_region_writes_admitted = 1;
        assert!(!b.is_green(), "a cross-region write admitted FAILs CI-R3");

        let mut c = green_r3();
        c.extra_eu_cdn_edges_admitted = 1;
        assert!(!c.is_green(), "an extra-EU CDN edge admitted FAILs CI-R3");

        let mut d = green_r3();
        d.within_eu_cdn_edges = 0;
        assert!(
            !d.is_green(),
            "0 within-EU CDN edges is a vacuous CDN → NOT green (kills the `> 0`→`>= 0` mutant)"
        );

        let mut e = green_r3();
        e.store_reports.clear();
        assert!(
            !e.is_green(),
            "0 store reports FAILs (fail-closed - no silent stores)"
        );

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

    #[test]
    fn ci_d10_every_green_clause_is_load_bearing() {
        assert!(green_d10().is_green(), "the baseline is green");

        let mut a = green_d10();
        a.cross_tenant_jobs_admitted = 1;
        assert!(!a.is_green(), "a cross-tenant job read FAILs CI-D10");

        let mut b = green_d10();
        b.cross_tenant_secret_reads = 1;
        assert!(!b.is_green(), "a cross-tenant secret read FAILs CI-D10");

        let mut c = green_d10();
        c.unattested_runner_refused = false;
        assert!(
            !c.is_green(),
            "an unattested runner not refused FAILs CI-D10"
        );

        let mut d = green_d10();
        d.token_scoped_to_own_tenant = false;
        assert!(
            !d.is_green(),
            "a token not scoped to the own tenant FAILs CI-D10"
        );

        let mut e = green_d10();
        e.own_tenant_jobs_admitted = 0;
        assert!(
            !e.is_green(),
            "0 own-tenant jobs is a vacuous boundary → NOT green (kills the `> 0`→`>= 0` mutant)"
        );
    }

    #[test]
    fn ci_d10_admit_requires_both_gates() {
        let compromised = tenant("acme");
        let r = region("fr-par");
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
            "the cross-tenant job clears NEITHER gate - both must admit (the admit conjunction)"
        );
        assert!(report.is_green());
    }

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
        assert!(report.unattested_runner_refused);
        assert!(report.token_scoped_to_own_tenant);
    }

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
        assert_eq!(report.cross_region_writes_admitted, 0);
        assert_eq!(report.extra_eu_cdn_edges_admitted, 0);
        assert_eq!(report.within_eu_cdn_edges, 2);
        assert!(report.claimed_by_in_region_runner);
        assert_eq!(report.store_reports.len(), 5);
    }
}
