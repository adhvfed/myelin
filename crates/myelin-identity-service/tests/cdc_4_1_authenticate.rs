//! # The CDC pair for contract 4.1 — `authenticate(credential) → Principal` (P-ID-06 / P-065)
//!
//! **Contract-index row 4.1** (`authenticate -> Principal`), the **human/SSO half** (OIDC / SAML /
//! SCIM / passkey / SSH). This is the dedicated provider+consumer pair the P-ID-06 TESTS field names
//! — the in-CI evidence the two sides of the `authenticate` seam cannot drift apart:
//!
//! - the **PROVIDER** ([`HumanSsoAuthenticator`], through the frozen [`IdentityService`] ABI) resolves
//!   a verified human/SSO credential to the one polymorphic `Principal{kind, tenant, region, …}` over
//!   the S1 store, **tenant-from-credential** (ID-3, never the URL path);
//! - the **CONSUMER** (a gateway-side caller — the stateless gateway that authenticates an inbound
//!   request before injecting a trusted identity header) hands the credential to `authenticate` and
//!   reads back the resolved `Principal`, asserting the tenant is the credential's even when the URL
//!   path lies.
//!
//! The provider's promise (resolve the verified credential's tenant, never the path) and the
//! consumer's promise (use the resolved `Principal.tenant` as the trust root) are pinned here so a
//! change to either side fails this test in the same CI job. The capability-token / machine-identity
//! half of 4.1 is P-ID-07 (P-066); this pair is the human/SSO CDC the prompt requires.

use myelin_identity::{
    Credential, IdentityService, Principal, PrincipalId, PrincipalKind, PrincipalStatus,
};
use myelin_identity_service::{scheme, HumanSsoAuthenticator, PrincipalStore};
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

/// The frozen floor verified-assertion envelope `<tenant>|<region>|<subject_key>`.
fn material(tenant: &str, region: &str, subject_key: &str) -> String {
    format!("{tenant}|{region}|{subject_key}")
}

/// Build the PROVIDER: an authenticator over an S1 store seeded with one SSO-linked principal.
fn provider() -> HumanSsoAuthenticator {
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    let s = scope("acme");
    store
        .put_principal(
            &s,
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            myelin_identity::DataRole::Processor,
            PrincipalStatus::Active,
            None,
        )
        .expect("seed the principal");
    store
        .link_credential(&s, scheme::OIDC, "sub-alice", &PrincipalId("p:alice".into()))
        .expect("link the OIDC credential");
    HumanSsoAuthenticator::new(store)
}

/// The CONSUMER side: a gateway authenticates an inbound credential through the frozen
/// `IdentityService` ABI and returns the resolved `Principal` (the trusted identity it injects).
fn gateway_authenticates(provider: &dyn IdentityService, credential: &Credential) -> Principal {
    provider
        .authenticate(credential)
        .expect("the gateway resolves the verified credential to a Principal")
}

/// **The 4.1 provider+consumer CDC pair.** The gateway consumer authenticates a verified OIDC
/// credential through the frozen ABI; the provider resolves it to the polymorphic Principal from S1
/// with the tenant taken from the credential.
#[test]
fn cdc_4_1_authenticate_provider_resolves_consumer_trusts_principal() {
    let provider = provider();
    let principal = gateway_authenticates(
        &provider,
        &Credential {
            scheme: scheme::OIDC.into(),
            material: material("acme", "eu-west", "sub-alice"),
        },
    );
    assert_eq!(principal.principal_id, PrincipalId("p:alice".into()));
    assert_eq!(
        principal.tenant,
        TenantId("acme".into()),
        "the consumer trusts the resolved tenant — the credential's, the trust root"
    );
    assert_eq!(principal.region, Region("eu-west".into()));
    assert_eq!(principal.kind, PrincipalKind::Human);

    // Observability is part of the pass: the decision emitted its auth_decision_latency signal.
    assert_eq!(
        provider.telemetry().decision_count(),
        1,
        "the authenticate decision emitted one auth_decision_latency observation"
    );
}

/// **The IDOR floor through the gateway-facing form (ID-3).** The gateway's path-aware form takes
/// the URL-path tenant; even when the path LIES (asserts globex), the resolved tenant is the
/// credential's (acme) and `path_derived_tenant_count == 0`. A drift to path-derived tenant fails
/// here in the same CI job.
#[test]
fn cdc_4_1_tenant_is_from_credential_not_path() {
    let provider = provider();
    let principal = provider
        .authenticate(
            &Credential {
                scheme: scheme::OIDC.into(),
                material: material("acme", "eu-west", "sub-alice"),
            },
            Some(&TenantId("globex".into())),
        )
        .expect("resolve");
    assert_eq!(
        principal.tenant,
        TenantId("acme".into()),
        "tenant from the credential (acme), never the path (globex)"
    );
    assert_eq!(
        provider.idor_counters().path_derived_tenant_count(),
        0,
        "path_derived_tenant_count == 0 (the IDOR floor)"
    );
}
