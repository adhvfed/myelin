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

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

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

fn dispatcher_authenticates(
    provider: &CapabilityAuthenticator,
    credential: &Credential,
) -> Result<Principal, AuthzError> {
    provider.authenticate_trait(credential)
}

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
        "the consumer trusts the resolved tenant - the token's, the trust root"
    );
    assert_eq!(principal.region, Region("eu-west".into()));
    assert_eq!(
        principal.kind,
        PrincipalKind::Service,
        "a machine identity → Service principal"
    );

    assert_eq!(
        provider.telemetry().decision_count(),
        1,
        "the authenticate decision emitted one auth_decision_latency observation"
    );
}

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
