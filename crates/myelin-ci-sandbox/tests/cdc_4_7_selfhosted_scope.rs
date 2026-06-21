//! # The CDC pair for contract 4.7 — the SELF-HOSTED SCOPE (CI consumer ↔ Identity mint provider),
//! CI-P4 → P-240.
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 4.7
//! (`mint_run_token(agent_id, run_id, delegation_caveats, ttl) → token`) — Identity, §4: *"the
//! self-hosted-runner token is scoped to **one tenant's `SelfHosted` jobs** (cannot mint
//! cross-tenant)"*. Owning architecture:
//! `continuous-integration/architecture/01-tech-and-data-model.md` §3.4 (the `runner` row —
//! `attest_state`, the self-hosted `trust_tier` "only its own tenant's SelfHosted jobs") +
//! `00-reconciliation-decisions.md` §1 / X-6 (the self-hosted runner trust boundary + the scoped
//! token).
//!
//! ## What this pair pins (the CI ↔ Identity agreement of 4.7's self-hosted half)
//!
//! **The CI CONSUMER (CI-P4):** [`mint_self_hosted_token`] gates on the runner's `attest_state`
//! (fail-closed — an un-attested runner gets no token) and carries EXACTLY the own-tenant
//! `selfhosted:<tenant>` grant into the contract-4.7 mint surface
//! ([`RunTokenMinter`](myelin_flow::RunTokenMinter)). It NEVER names another tenant's scope.
//!
//! **The Identity PROVIDER (P-076 — modelled here on the frozen seam):** the mint applies the
//! self-hosted one-tenant CEILING (the same `SELFHOSTED_GRANT_PREFIX` `myelin_identity_service::mint`
//! enforces): a `PerJob` (self-hosted-runner) token whose authority names a grant outside its own
//! tenant's `SelfHosted` scope is REFUSED. The agreement: the SAME `selfhosted:<tenant>` ceiling
//! flows CI consumer → caveat → Identity mint, and the provider refuses any cross-tenant grant — so a
//! token for tenant A can never claim tenant B's job. CI REUSES the Identity mint; it never forks it.

use myelin_ci_sandbox::{
    mint_self_hosted_token, self_hosted_grant, Attestation, AttestState, SelfHostedMintError,
    SelfHostedRunner, StructuralAttestationVerifier, TrustTier, SELFHOSTED_GRANT_PREFIX,
};
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};
use myelin_tenancy::{Region, TenantId};

/// **PROVIDER side of 4.7 (the Identity mint's self-hosted ceiling — modelled on the frozen seam).**
/// Mints a per-run token, applying the SAME self-hosted one-tenant ceiling
/// `myelin_identity_service::mint::RunTokenMinter` enforces (`SELFHOSTED_GRANT_PREFIX`): a self-hosted
/// (`PerJob`) token whose caveat names a grant OUTSIDE `selfhosted:<own_tenant>` is REFUSED. The
/// provider is told its own tenant out-of-band (the verified `(tenant, region)` scope the real
/// service wires); here the CDC sets it to the runner's tenant (the attestation-bound tenant).
struct IdentitySelfHostedMintProvider {
    /// The verified own-tenant the mint scope is registered under (tenant-from-token, never a path).
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
        // A per-run token MUST have a positive life (life == run life).
        if ttl_secs == 0 {
            return Err(RunTokenError("non-positive TTL".into()));
        }
        // THE SELF-HOSTED ONE-TENANT CEILING (the SAME check the Identity mint applies AFTER the
        // delegation intersection): every grant a self-hosted token names must be `selfhosted:<own>`.
        // A grant outside the own-tenant SelfHosted scope is REFUSED (never silently dropped/widened).
        let own = format!("{SELFHOSTED_GRANT_PREFIX}{}", self.own_tenant);
        for g in &caveats.0 {
            if !g.starts_with(SELFHOSTED_GRANT_PREFIX) || g != &own {
                return Err(RunTokenError(format!(
                    "self-hosted scope violation: `{g}` is outside the own-tenant SelfHosted scope"
                )));
            }
        }
        Ok(RunTokenHandle {
            // The bearer material carries the (already-ceilinged) grants — the Identity envelope does
            // the same (grants ride in the material).
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
        material: StructuralAttestationVerifier::provisioned_material(&TenantId(tenant.into()), "n"),
    };
    assert_eq!(r.attest(&att, &verifier), AttestState::Attested);
    r
}

/// **The pair PINS: an attested self-hosted runner is minted a token scoped to ONLY its own tenant's
/// SelfHosted grant — the Identity provider mints it; the token's `admits` refuses cross-tenant.**
#[test]
fn ci_consumer_mints_own_tenant_scope_provider_accepts() {
    let runner = attested_runner("acme");
    let provider = IdentitySelfHostedMintProvider {
        own_tenant: "acme".into(),
    };

    let token = mint_self_hosted_token(&runner, &provider, "svc:runner-acme", "run-1", 300)
        .expect("the own-tenant self-hosted token mints");

    // The CI consumer carried EXACTLY the own-tenant grant into the mint.
    assert!(token.handle().token.contains(&self_hosted_grant(&TenantId("acme".into()))));
    assert!(!token.handle().token.contains("globex"));
    // The token admits its OWN tenant's SelfHosted job, refuses another tenant's.
    assert!(token.admits(TrustTier::SelfHosted, &TenantId("acme".into())));
    assert!(
        !token.admits(TrustTier::SelfHosted, &TenantId("globex".into())),
        "a token for acme cannot claim globex's job (cross-tenant refused)"
    );
}

/// **The pair PINS: a CROSS-TENANT grant is REFUSED at the Identity provider's self-hosted ceiling.**
/// (Defence-in-depth: the CI consumer never NAMES a cross-tenant grant, but were a caveat to name
/// another tenant's scope — a fork-attempt — the provider's ceiling refuses it. The two layers agree.)
#[test]
fn provider_ceiling_refuses_a_cross_tenant_grant() {
    let runner = attested_runner("acme");
    // A MISCONFIGURED provider whose own_tenant is acme, fed a runner whose tenant is acme, would
    // mint `selfhosted:acme`. To exercise the CEILING directly we stand the provider with own_tenant
    // = globex against an acme caveat (the cross-tenant case the ceiling MUST catch).
    let provider = IdentitySelfHostedMintProvider {
        own_tenant: "globex".into(),
    };
    let r = mint_self_hosted_token(&runner, &provider, "svc:runner-acme", "run-1", 300);
    assert!(
        matches!(r, Err(SelfHostedMintError::MintFailed(_))),
        "the provider's self-hosted ceiling refuses a grant outside its own-tenant SelfHosted scope"
    );
}

/// **The pair PINS the fail-closed attestation gate at the mint: an UN-attested runner is refused a
/// token BEFORE any mint is attempted (the gate is the CI consumer's, the ceiling is the provider's
/// — both fail-closed).**
#[test]
fn unattested_runner_never_reaches_the_provider() {
    let pending = SelfHostedRunner::register(TenantId("acme".into()), region()); // never attested.
    let provider = IdentitySelfHostedMintProvider {
        own_tenant: "acme".into(),
    };
    let r = mint_self_hosted_token(&pending, &provider, "svc:runner", "run-1", 300);
    assert_eq!(
        r.unwrap_err(),
        SelfHostedMintError::NotAttested(AttestState::Pending),
        "an un-attested runner gets no token — the gate is before the mint (fail-closed)"
    );
}
