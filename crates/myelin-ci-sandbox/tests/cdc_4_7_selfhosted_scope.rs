use myelin_ci_sandbox::{
    mint_self_hosted_token, self_hosted_grant, AttestState, Attestation, SelfHostedMintError,
    SelfHostedRunner, StructuralAttestationVerifier, TrustTier, SELFHOSTED_GRANT_PREFIX,
};
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};
use myelin_tenancy::{Region, TenantId};

struct IdentitySelfHostedMintProvider {
    own_tenant: String,
}

impl RunTokenMinter for IdentitySelfHostedMintProvider {
    fn mint_run_token(
        &self,
        agent_id: &str,
        run_id: &str,
        caveats: &DelegationCaveats,
        ttl_secs: u64,
    ) -> Result<RunTokenHandle, RunTokenError> {
        if ttl_secs == 0 {
            return Err(RunTokenError("non-positive TTL".into()));
        }
        let own = format!("{SELFHOSTED_GRANT_PREFIX}{}", self.own_tenant);
        for g in &caveats.0 {
            if !g.starts_with(SELFHOSTED_GRANT_PREFIX) || g != &own {
                return Err(RunTokenError(format!(
                    "self-hosted scope violation: `{g}` is outside the own-tenant SelfHosted scope"
                )));
            }
        }
        Ok(RunTokenHandle {
            token: format!("runtok:{agent_id}:{run_id}|{}", caveats.0.join(",")),
            jti: format!("jti:{agent_id}:{run_id}"),
            ttl_secs,
        })
    }
}

fn region() -> Region {
    Region("fr-par".into())
}

fn attested_runner(tenant: &str) -> SelfHostedRunner {
    let mut r = SelfHostedRunner::register(TenantId(tenant.into()), region());
    let verifier = StructuralAttestationVerifier::new();
    let att = Attestation {
        tenant: TenantId(tenant.into()),
        material: StructuralAttestationVerifier::provisioned_material(
            &TenantId(tenant.into()),
            "n",
        ),
    };
    assert_eq!(r.attest(&att, &verifier), AttestState::Attested);
    r
}

#[test]
fn ci_consumer_mints_own_tenant_scope_provider_accepts() {
    let runner = attested_runner("acme");
    let provider = IdentitySelfHostedMintProvider {
        own_tenant: "acme".into(),
    };

    let token = mint_self_hosted_token(&runner, &provider, "svc:runner-acme", "run-1", 300)
        .expect("the own-tenant self-hosted token mints");

    assert!(token
        .handle()
        .token
        .contains(&self_hosted_grant(&TenantId("acme".into()))));
    assert!(!token.handle().token.contains("globex"));
    assert!(token.admits(TrustTier::SelfHosted, &TenantId("acme".into())));
    assert!(
        !token.admits(TrustTier::SelfHosted, &TenantId("globex".into())),
        "a token for acme cannot claim globex's job (cross-tenant refused)"
    );
}

#[test]
fn provider_ceiling_refuses_a_cross_tenant_grant() {
    let runner = attested_runner("acme");
    let provider = IdentitySelfHostedMintProvider {
        own_tenant: "globex".into(),
    };
    let r = mint_self_hosted_token(&runner, &provider, "svc:runner-acme", "run-1", 300);
    assert!(
        matches!(r, Err(SelfHostedMintError::MintFailed(_))),
        "the provider's self-hosted ceiling refuses a grant outside its own-tenant SelfHosted scope"
    );
}

#[test]
fn unattested_runner_never_reaches_the_provider() {
    let pending = SelfHostedRunner::register(TenantId("acme".into()), region());
    let provider = IdentitySelfHostedMintProvider {
        own_tenant: "acme".into(),
    };
    let r = mint_self_hosted_token(&pending, &provider, "svc:runner", "run-1", 300);
    assert_eq!(
        r.unwrap_err(),
        SelfHostedMintError::NotAttested(AttestState::Pending),
        "an un-attested runner gets no token - the gate is before the mint (fail-closed)"
    );
}
