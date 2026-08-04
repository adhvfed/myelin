use myelin_identity::{
    Credential, IdentityService, Principal, PrincipalId, PrincipalKind, PrincipalStatus,
};
use myelin_identity_service::{scheme, HumanSsoAuthenticator, PrincipalStore, StructuralVerifier};
use myelin_storage::{KmsEngine, TenantScope};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

fn material(tenant: &str, region: &str, subject_key: &str) -> String {
    format!("{tenant}|{region}|{subject_key}")
}

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
        .link_credential(
            &s,
            scheme::OIDC,
            "sub-alice",
            &PrincipalId("p:alice".into()),
        )
        .expect("link the OIDC credential");
    HumanSsoAuthenticator::with_verifier(store, Arc::new(StructuralVerifier::new()))
}

fn gateway_authenticates(provider: &dyn IdentityService, credential: &Credential) -> Principal {
    provider
        .authenticate(credential)
        .expect("the gateway resolves the verified credential to a Principal")
}

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
        "the consumer trusts the resolved tenant - the credential's, the trust root"
    );
    assert_eq!(principal.region, Region("eu-west".into()));
    assert_eq!(principal.kind, PrincipalKind::Human);

    assert_eq!(
        provider.telemetry().decision_count(),
        1,
        "the authenticate decision emitted one auth_decision_latency observation"
    );
}

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
