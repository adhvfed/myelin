//! # The CDC pair for contract 4.1 — the token/machine-identity half (P-ID-07 / P-066)
//!
//! **Contract-index row 4.1** (`authenticate -> Principal`), the **capability-token + machine-identity
//! half** (PAT / CI-job / agent-run / deploy-key / per-job). The P-ID-07 TESTS field requires the 4.1
//! provider+consumer pair to **re-affirm it exercises a token credential** — this file is that
//! evidence, the in-CI proof the two sides of the `authenticate` seam cannot drift apart for the
//! machine surfaces:
//!
//! - the **PROVIDER** ([`CapabilityAuthenticator`]) resolves a verified capability/machine token to
//!   the one polymorphic `Principal{kind = Service, tenant, region, …}` over the S1 store,
//!   **tenant-from-token** (ID-3, never the URL path), with the C6 scope ceilings (deploy-key repo
//!   scope, self-hosted-runner one-tenant `SelfHosted` scope);
//! - the **CONSUMER** (a CI-dispatch / gateway-side caller — the surface that authenticates a per-job
//!   or deploy-key token before injecting a trusted machine identity) hands the credential to
//!   `authenticate` and reads back the resolved machine `Principal`, asserting the tenant is the
//!   token's and the runner cannot cross tenants.
//!
//! The provider's promise (resolve the verified token's tenant, bound to its ceiling, never the path)
//! and the consumer's promise (use the resolved machine `Principal` as the trust root) are pinned
//! here so a change to either side fails this test in the same CI job. This complements the human/SSO
//! CDC pair (`cdc_4_1_authenticate.rs`, P-065): the two pairs together cover both halves of 4.1.

use myelin_identity::{
    AuthzError, Credential, Principal, PrincipalId, PrincipalKind, PrincipalStatus,
};
use myelin_identity_service::machine_auth::scheme as machine_scheme;
use myelin_identity_service::{
    CapabilityAuthenticator, PrincipalStore, RevocationStore, StructuralTokenVerifier,
};
use myelin_storage::{KmsEngine, TenantScope};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

/// A verified `(tenant, region)` scope (minted from a verified token — never a path).
fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

/// The frozen verified-token envelope `<tenant>|<region>|<subject_key>|<jti>|<dpop>|<grants>`.
fn material(
    tenant: &str,
    region: &str,
    subject_key: &str,
    jti: &str,
    dpop: bool,
    grants: &[&str],
) -> String {
    format!(
        "{tenant}|{region}|{subject_key}|{jti}|{}|{}|per_job|edge|{subject_key}|",
        if dpop { "1" } else { "0" },
        grants.join(",")
    )
}

/// Build the PROVIDER: a capability authenticator over an S1 store seeded with one self-hosted-runner
/// (per-job) token record and one deploy-key record in `acme`.
fn provider() -> CapabilityAuthenticator {
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    let s = scope("acme");
    store
        .put_principal(
            &s,
            PrincipalId("svc:runner".into()),
            PrincipalKind::Service,
            myelin_identity::DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .expect("seed the runner principal");
    store
        .link_credential(
            &s,
            machine_scheme::PER_JOB,
            "run-1",
            &PrincipalId("svc:runner".into()),
        )
        .expect("link the per-job token record");
    // MR-012: the Structural-defaulting `new` is now a `#[cfg(test)]` test-double of the lib; this
    // CDC test exercises the resolution body (tenant-from-token / scope ceiling / revocation consult)
    // over the mock floor verifier, injected explicitly via the production `with_verifier` seam
    // (constructing a `Structural*` double in a `tests/` file is admitted — the scanner excludes tests/).
    let revocations = RevocationStore::new();
    for jti in ["jti-1", "jti-2"] {
        revocations.register_run_token_ttl(
            &s,
            jti,
            myelin_events::Timestamp("2020-01-01T00:00:00Z".into()),
            myelin_events::Timestamp("2099-01-01T00:00:00Z".into()),
        );
    }
    CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(StructuralTokenVerifier::new()),
        revocations,
    )
}

/// The CONSUMER side: a CI-dispatch / gateway authenticates an inbound machine token and returns the
/// resolved machine `Principal` (the trusted identity it injects for the job).
fn dispatcher_authenticates(
    provider: &CapabilityAuthenticator,
    credential: &Credential,
) -> Result<Principal, AuthzError> {
    provider.authenticate_trait(credential)
}

/// **The 4.1 provider+consumer CDC pair (the token/machine half).** The dispatcher consumer
/// authenticates a verified per-job (self-hosted-runner) token; the provider resolves it to the
/// polymorphic machine `Principal` from S1 with the tenant taken from the token, bounded to the
/// runner's one-tenant `SelfHosted` scope.
#[test]
fn cdc_4_1_machine_provider_resolves_consumer_trusts_machine_principal() {
    let provider = provider();
    let principal = dispatcher_authenticates(
        &provider,
        &Credential {
            scheme: machine_scheme::PER_JOB.into(),
            material: material(
                "acme",
                "eu-west",
                "run-1",
                "jti-1",
                false,
                &["selfhosted:acme"],
            ),
        },
    )
    .expect("the dispatcher resolves the verified per-job token to a machine Principal");

    assert_eq!(principal.principal_id, PrincipalId("svc:runner".into()));
    assert_eq!(
        principal.tenant,
        TenantId("acme".into()),
        "the consumer trusts the resolved tenant — the token's, the trust root"
    );
    assert_eq!(principal.region, Region("eu-west".into()));
    assert_eq!(
        principal.kind,
        PrincipalKind::Service,
        "a machine identity → Service principal"
    );

    // Observability is part of the pass: the decision emitted its auth_decision_latency signal.
    assert_eq!(
        provider.telemetry().decision_count(),
        1,
        "the authenticate decision emitted one auth_decision_latency observation"
    );
}

/// **The self-hosted-runner one-tenant scope through the consumer seam (C6).** A per-job token whose
/// authority names ANOTHER tenant's `SelfHosted` scope is refused by the provider — the consumer
/// never receives a cross-tenant machine identity. A drift to a cross-tenant runner fails here in the
/// same CI job.
#[test]
fn cdc_4_1_runner_token_cannot_cross_tenant() {
    let provider = provider();
    let r = dispatcher_authenticates(
        &provider,
        &Credential {
            scheme: machine_scheme::PER_JOB.into(),
            material: material(
                "acme",
                "eu-west",
                "run-1",
                "jti-2",
                false,
                &["selfhosted:globex"],
            ),
        },
    );
    assert!(
        matches!(r, Err(AuthzError::FailClosed(_))),
        "a runner token naming another tenant's scope is refused (C6, no-global-pool)"
    );
    assert_eq!(
        provider.idor_counters().path_derived_tenant_count(),
        0,
        "0 cross-tenant runner resolutions (the C6 mandatory-core)"
    );
}
