#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::Timestamp;
use myelin_identity::{Credential, DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::capability_crypto::{
    CapabilityMintSpec, CellTokenAuthority, PasetoCapabilityVerifier,
};
use myelin_identity_service::revocation::RevocationStore;
use myelin_identity_service::{CapabilityAuthenticator, PrincipalStore};
use myelin_storage::migration::HotTables;
use myelin_storage::{
    identity_durable_migrations, DurableRevocationBacking, KmsEngine, SubstrateProvider,
    TenantScope,
};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

const NOW_UNIX: i64 = 1_780_000_000;
const NOW_RFC3339: &str = "2026-06-26T00:00:00Z";

fn admin_config(cfg: &MyelinConfig) -> MyelinConfig {
    let mut c = cfg.clone();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

fn uniq() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn tenant_scope(tenant: &str, region: &str) -> TenantScope {
    let p = myelin_identity::Principal::stub(
        PrincipalId("p:admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region(region.into()))
}

async fn app_provider() -> Option<SubstrateProvider> {
    match SubstrateProvider::connect(MyelinConfig::dev(), 6).await {
        Ok(p) => Some(p),
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            None
        }
    }
}

async fn migrate() -> Option<SubstrateProvider> {
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return None;
        }
    };
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("identity durable migrations execute against the live DB");
    Some(admin)
}

async fn cleanup(admin: &SubstrateProvider, tenant: &str) {
    for sql in [
        "DELETE FROM revocation WHERE tenant_id = $1",
        "DELETE FROM run_token_teardown WHERE tenant_id = $1",
    ] {
        let _ = sqlx::query(sql).bind(tenant).execute(admin.db_pool()).await;
    }
}

fn seeded_principal_store(tenant: &str, region: &str, subject_key: &str) -> PrincipalStore {
    let st = PrincipalStore::new(Arc::new(KmsEngine::new()));
    let sc = tenant_scope(tenant, region);
    st.put_principal(
        &sc,
        PrincipalId("svc:ci".into()),
        PrincipalKind::Service,
        DataRole::Controller,
        PrincipalStatus::Active,
        None,
    )
    .unwrap();
    st.link_credential(&sc, "ci", subject_key, &PrincipalId("svc:ci".into()))
        .unwrap();
    st
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoked_machine_token_stays_denied_across_a_fresh_store_instance() {
    let Some(admin) = migrate().await else { return };
    let Some(app) = app_provider().await else {
        return;
    };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr011-{suffix}");

    let cell = Arc::new(CellTokenAuthority::from_seed(&[11u8; 32], &[22u8; 32]).expect("cell"));
    let anchor = cell.trust_anchor();
    let subject_key = "svc:ci";

    let material = cell.mint(&CapabilityMintSpec {
        tenant: tenant.clone(),
        region: region.clone(),
        subject_key: subject_key.into(),
        jti: "mr011-jti".into(),
        exp_unix: NOW_UNIX + 3600,
        authority: vec!["ci:run".into()],
        dpop_jkt: None,
        purpose: myelin_identity_service::CredentialPurpose::CiJob {
            run_id: "ci-run-durable".into(),
        },
        audience: myelin_identity_service::CredentialAudience::Edge,
    });
    let cred = Credential {
        scheme: "ci".into(),
        material,
    };

    let store1 =
        RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
    let sc = tenant_scope(&tenant, &region);
    store1.register_run_token_ttl(
        &sc,
        "mr011-jti",
        Timestamp(NOW_RFC3339.into()),
        Timestamp("2026-06-26T01:00:00Z".into()),
    );
    let verifier1: Arc<dyn myelin_identity_service::TokenVerifier> =
        Arc::new(PasetoCapabilityVerifier::new(anchor.clone()).with_clock(|| NOW_UNIX));
    let auth1 = CapabilityAuthenticator::with_verifier(
        seeded_principal_store(&tenant, &region, subject_key),
        verifier1,
        store1.clone(),
    )
    .with_clock(|| Timestamp(NOW_RFC3339.into()));

    let p = auth1
        .authenticate(&cred, None)
        .expect("a correctly-signed CI token authenticates");
    assert_eq!(p.principal_id, PrincipalId("svc:ci".into()));
    assert_eq!(
        p.tenant,
        TenantId(tenant.clone()),
        "tenant from the verified token"
    );

    store1.tear_down_run_token(&sc, "mr011-jti", Timestamp(NOW_RFC3339.into()));
    let denied = auth1.authenticate(&cred, None);
    assert!(
        matches!(denied, Err(myelin_identity::AuthzError::FailClosed(_))),
        "after revocation the SAME authenticator denies the token, got {denied:?}"
    );

    let store2 =
        RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
    let verifier2: Arc<dyn myelin_identity_service::TokenVerifier> =
        Arc::new(PasetoCapabilityVerifier::new(anchor).with_clock(|| NOW_UNIX));
    let auth2 = CapabilityAuthenticator::with_verifier(
        seeded_principal_store(&tenant, &region, subject_key),
        verifier2,
        store2.clone(),
    )
    .with_clock(|| Timestamp(NOW_RFC3339.into()));
    let still_denied = auth2.authenticate(&cred, None);
    assert!(
        matches!(
            still_denied,
            Err(myelin_identity::AuthzError::FailClosed(_))
        ),
        "a FRESH authenticator + store over the same pool STILL denies the revoked token (durable \
         revocation survives the 'restart') - got {still_denied:?}"
    );

    let fresh = cell_fresh_token(&tenant, &region, subject_key);
    let fresh_cred = Credential {
        scheme: "ci".into(),
        material: fresh,
    };
    store2.register_run_token_ttl(
        &sc,
        "mr011-jti-fresh",
        Timestamp(NOW_RFC3339.into()),
        Timestamp("2026-06-26T01:00:00Z".into()),
    );
    assert!(
        auth2.authenticate(&fresh_cred, None).is_ok(),
        "an un-revoked token from the same cell still authenticates (the deny is handle-specific)"
    );

    cleanup(&admin, &tenant).await;
    println!("OK: a revoked machine/capability token stays denied across a fresh store instance over the same pool (S7Denylist gap discharged).");
}

fn cell_fresh_token(tenant: &str, region: &str, subject_key: &str) -> String {
    let cell = CellTokenAuthority::from_seed(&[11u8; 32], &[22u8; 32]).unwrap();
    cell.mint(&CapabilityMintSpec {
        tenant: tenant.into(),
        region: region.into(),
        subject_key: subject_key.into(),
        jti: "mr011-jti-fresh".into(),
        exp_unix: NOW_UNIX + 3600,
        authority: vec!["ci:run".into()],
        dpop_jkt: None,
        purpose: myelin_identity_service::CredentialPurpose::CiJob {
            run_id: "ci-run-durable".into(),
        },
        audience: myelin_identity_service::CredentialAudience::Edge,
    })
}
