use crate::TrustTier;
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};
use myelin_tenancy::{Region, TenantId};

pub const SELFHOSTED_GRANT_PREFIX: &str = "selfhosted:";

pub fn self_hosted_grant(tenant: &TenantId) -> String {
    format!("{SELFHOSTED_GRANT_PREFIX}{}", tenant.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttestState {
    Pending,
    Attested,
    Failed,
}

impl AttestState {
    pub fn admits_claim(self) -> bool {
        matches!(self, AttestState::Attested)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attestation {
    pub tenant: TenantId,
    pub material: String,
}

pub trait AttestationVerifier {
    fn verify(&self, attestation: &Attestation, claimed_tenant: &TenantId) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StructuralAttestationVerifier;

impl StructuralAttestationVerifier {
    pub const ENVELOPE_PREFIX: &'static str = "provsig:";

    pub fn new() -> StructuralAttestationVerifier {
        StructuralAttestationVerifier
    }

    pub fn provisioned_material(tenant: &TenantId, nonce: &str) -> String {
        format!("{}{}:{}", Self::ENVELOPE_PREFIX, tenant.0, nonce)
    }
}

impl AttestationVerifier for StructuralAttestationVerifier {
    fn verify(&self, attestation: &Attestation, claimed_tenant: &TenantId) -> bool {
        if &attestation.tenant != claimed_tenant {
            return false;
        }
        let Some(rest) = attestation.material.strip_prefix(Self::ENVELOPE_PREFIX) else {
            return false;
        };
        let Some((tenant_seg, nonce)) = rest.split_once(':') else {
            return false;
        };
        tenant_seg == claimed_tenant.0 && !nonce.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct SelfHostedRunner {
    tenant: TenantId,
    region: Region,
    attest_state: AttestState,
}

impl SelfHostedRunner {
    pub fn register(tenant: TenantId, region: Region) -> SelfHostedRunner {
        SelfHostedRunner {
            tenant,
            region,
            attest_state: AttestState::Pending,
        }
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn attest_state(&self) -> AttestState {
        self.attest_state
    }

    pub fn attest(
        &mut self,
        attestation: &Attestation,
        verifier: &dyn AttestationVerifier,
    ) -> AttestState {
        self.attest_state = if verifier.verify(attestation, &self.tenant) {
            AttestState::Attested
        } else {
            AttestState::Failed
        };
        self.attest_state
    }

    pub fn may_claim(
        &self,
        job_tier: TrustTier,
        job_tenant: &TenantId,
        job_region: &Region,
    ) -> bool {
        self.attest_state.admits_claim()
            && job_tier == TrustTier::SelfHosted
            && job_tenant == &self.tenant
            && job_region == &self.region
    }
}

#[derive(Clone, Debug)]
pub struct TenantScopedToken {
    tenant: TenantId,
    handle: RunTokenHandle,
}

impl TenantScopedToken {
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn handle(&self) -> &RunTokenHandle {
        &self.handle
    }

    pub fn admits(&self, job_tier: TrustTier, job_tenant: &TenantId) -> bool {
        job_tier == TrustTier::SelfHosted && job_tenant == &self.tenant
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelfHostedMintError {
    NotAttested(AttestState),
    MintFailed(RunTokenError),
}

impl core::fmt::Display for SelfHostedMintError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SelfHostedMintError::NotAttested(s) => write!(
                f,
                "a self-hosted run token cannot be minted for a runner in attest_state {s:?} - only \
                 an Attested runner receives a tenant-scoped token (fail-closed)"
            ),
            SelfHostedMintError::MintFailed(e) => {
                write!(f, "the contract-4.7 self-hosted token mint failed: {e}")
            }
        }
    }
}

impl std::error::Error for SelfHostedMintError {}

pub fn mint_self_hosted_token(
    runner: &SelfHostedRunner,
    minter: &dyn RunTokenMinter,
    agent_id: &str,
    run_id: &str,
    ttl_secs: u64,
) -> Result<TenantScopedToken, SelfHostedMintError> {
    if !runner.attest_state.admits_claim() {
        return Err(SelfHostedMintError::NotAttested(runner.attest_state));
    }

    let caveats = DelegationCaveats(vec![self_hosted_grant(&runner.tenant)]);

    let handle = minter
        .mint_run_token(agent_id, run_id, &caveats, ttl_secs)
        .map_err(SelfHostedMintError::MintFailed)?;

    Ok(TenantScopedToken {
        tenant: runner.tenant.clone(),
        handle,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn tenant(s: &str) -> TenantId {
        TenantId(s.into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct MintCall {
        agent_id: String,
        run_id: String,
        caveats: Vec<String>,
        ttl_secs: u64,
    }

    #[derive(Default)]
    struct RecordingMinter {
        calls: Mutex<Vec<MintCall>>,
    }
    impl RunTokenMinter for RecordingMinter {
        fn mint_run_token(
            &self,
            agent_id: &str,
            run_id: &str,
            caveats: &DelegationCaveats,
            ttl_secs: u64,
        ) -> Result<RunTokenHandle, RunTokenError> {
            self.calls.lock().unwrap().push(MintCall {
                agent_id: agent_id.into(),
                run_id: run_id.into(),
                caveats: caveats.0.clone(),
                ttl_secs,
            });
            Ok(RunTokenHandle {
                token: format!("runtok:{run_id}|{}", caveats.0.join(",")),
                jti: format!("jti:{agent_id}:{run_id}"),
                ttl_secs,
            })
        }
    }

    #[test]
    fn valid_attestation_admits_and_attested_runner_may_claim() {
        let mut runner = SelfHostedRunner::register(tenant("acme"), region());
        assert_eq!(runner.attest_state(), AttestState::Pending);
        assert!(
            !runner.may_claim(TrustTier::SelfHosted, &tenant("acme"), &region()),
            "a Pending (un-attested) runner cannot claim - fail-closed"
        );

        let verifier = StructuralAttestationVerifier::new();
        let att = Attestation {
            tenant: tenant("acme"),
            material: StructuralAttestationVerifier::provisioned_material(
                &tenant("acme"),
                "nonce-1",
            ),
        };
        assert_eq!(runner.attest(&att, &verifier), AttestState::Attested);
        assert!(runner.may_claim(TrustTier::SelfHosted, &tenant("acme"), &region()));
    }

    #[test]
    fn absent_attestation_fails_closed_cannot_claim() {
        let mut runner = SelfHostedRunner::register(tenant("acme"), region());
        let verifier = StructuralAttestationVerifier::new();
        let absent = Attestation {
            tenant: tenant("acme"),
            material: String::new(),
        };
        assert_eq!(runner.attest(&absent, &verifier), AttestState::Failed);
        assert!(
            !runner.may_claim(TrustTier::SelfHosted, &tenant("acme"), &region()),
            "an absent attestation ⇒ Failed ⇒ 0 claims (fail-closed)"
        );
    }

    #[test]
    fn forged_attestation_fails_closed_cannot_claim() {
        let verifier = StructuralAttestationVerifier::new();

        let mut r1 = SelfHostedRunner::register(tenant("acme"), region());
        let forged_scheme = Attestation {
            tenant: tenant("acme"),
            material: "totally-made-up-token".into(),
        };
        assert_eq!(r1.attest(&forged_scheme, &verifier), AttestState::Failed);
        assert!(!r1.may_claim(TrustTier::SelfHosted, &tenant("acme"), &region()));

        let mut r2 = SelfHostedRunner::register(tenant("acme"), region());
        let cross = Attestation {
            tenant: tenant("acme"),
            material: StructuralAttestationVerifier::provisioned_material(&tenant("globex"), "n"),
        };
        assert_eq!(r2.attest(&cross, &verifier), AttestState::Failed);
        assert!(!r2.may_claim(TrustTier::SelfHosted, &tenant("acme"), &region()));

        let mut r3 = SelfHostedRunner::register(tenant("acme"), region());
        let mismatched = Attestation {
            tenant: tenant("globex"),
            material: StructuralAttestationVerifier::provisioned_material(&tenant("globex"), "n"),
        };
        assert_eq!(r3.attest(&mismatched, &verifier), AttestState::Failed);
        assert!(!r3.may_claim(TrustTier::SelfHosted, &tenant("acme"), &region()));
    }

    #[test]
    fn attested_runner_gate_refuses_wrong_tier_tenant_or_region() {
        let mut runner = SelfHostedRunner::register(tenant("acme"), region());
        let verifier = StructuralAttestationVerifier::new();
        runner.attest(
            &Attestation {
                tenant: tenant("acme"),
                material: StructuralAttestationVerifier::provisioned_material(&tenant("acme"), "n"),
            },
            &verifier,
        );
        assert_eq!(runner.attest_state(), AttestState::Attested);

        assert!(!runner.may_claim(TrustTier::Trusted, &tenant("acme"), &region()));
        assert!(!runner.may_claim(TrustTier::UntrustedFork, &tenant("acme"), &region()));
        assert!(!runner.may_claim(TrustTier::SelfHosted, &tenant("globex"), &region()));
        assert!(!runner.may_claim(
            TrustTier::SelfHosted,
            &tenant("acme"),
            &Region("de-fra".into())
        ));
    }

    #[test]
    fn mint_scopes_to_own_tenant_and_cross_tenant_is_refused() {
        let mut runner = SelfHostedRunner::register(tenant("acme"), region());
        let verifier = StructuralAttestationVerifier::new();
        runner.attest(
            &Attestation {
                tenant: tenant("acme"),
                material: StructuralAttestationVerifier::provisioned_material(&tenant("acme"), "n"),
            },
            &verifier,
        );

        let minter = RecordingMinter::default();
        let token = mint_self_hosted_token(&runner, &minter, "svc:runner-acme", "run-1", 300)
            .expect("an attested runner is minted a tenant-scoped token");

        assert_eq!(token.tenant(), &tenant("acme"));
        let calls = minter.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].caveats, vec!["selfhosted:acme".to_string()]);
        assert_eq!(calls[0].ttl_secs, 300, "minted under the run-life TTL");
        assert!(token.handle().token.contains("selfhosted:acme"));
        assert!(!token.handle().token.contains("globex"));

        assert!(
            token.admits(TrustTier::SelfHosted, &tenant("acme")),
            "the token admits its OWN tenant's SelfHosted job"
        );
        assert!(
            !token.admits(TrustTier::SelfHosted, &tenant("globex")),
            "a token for tenant acme CANNOT claim tenant globex's job (cross-tenant refused)"
        );
        assert!(!token.admits(TrustTier::Trusted, &tenant("acme")));
    }

    #[test]
    fn unattested_runner_is_refused_a_token() {
        let runner = SelfHostedRunner::register(tenant("acme"), region());
        let minter = RecordingMinter::default();
        let r = mint_self_hosted_token(&runner, &minter, "svc:runner", "run-1", 300);
        assert_eq!(
            r.unwrap_err(),
            SelfHostedMintError::NotAttested(AttestState::Pending),
            "an un-attested runner receives NO token (fail-closed)"
        );
        assert_eq!(minter.calls.lock().unwrap().len(), 0);

        let mut failed = SelfHostedRunner::register(tenant("acme"), region());
        let verifier = StructuralAttestationVerifier::new();
        failed.attest(
            &Attestation {
                tenant: tenant("acme"),
                material: String::new(),
            },
            &verifier,
        );
        assert_eq!(failed.attest_state(), AttestState::Failed);
        let r2 = mint_self_hosted_token(&failed, &minter, "svc:runner", "run-1", 300);
        assert_eq!(
            r2.unwrap_err(),
            SelfHostedMintError::NotAttested(AttestState::Failed)
        );
    }

    #[test]
    fn mint_surfaces_identity_failure_loud() {
        struct FailingMinter;
        impl RunTokenMinter for FailingMinter {
            fn mint_run_token(
                &self,
                _a: &str,
                _r: &str,
                _c: &DelegationCaveats,
                _t: u64,
            ) -> Result<RunTokenHandle, RunTokenError> {
                Err(RunTokenError("identity unavailable (fail-static)".into()))
            }
        }
        let mut runner = SelfHostedRunner::register(tenant("acme"), region());
        let verifier = StructuralAttestationVerifier::new();
        runner.attest(
            &Attestation {
                tenant: tenant("acme"),
                material: StructuralAttestationVerifier::provisioned_material(&tenant("acme"), "n"),
            },
            &verifier,
        );
        let r = mint_self_hosted_token(&runner, &FailingMinter, "svc:runner", "run-1", 300);
        assert!(matches!(r, Err(SelfHostedMintError::MintFailed(_))));
    }
}
