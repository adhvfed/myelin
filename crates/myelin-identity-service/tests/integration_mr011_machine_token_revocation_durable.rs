//! # MR-011 — a revoked MACHINE/capability token stays denied across a restart, proven against LIVE
//! Postgres (the carried-forward S7Denylist fix, discharged end-to-end through the auth path).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo test` stays DB-free. Runs ONLY
//! against the docker-compose dev stack:
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     cargo test -p myelin-identity-service --features integration \
//!       --test integration_mr011_machine_token_revocation_durable -- --nocapture
//!
//! The old `S7Denylist` was a tenant-less in-process `Mutex<BTreeSet>` rebuilt EMPTY on construction —
//! a machine-token `jti` revoked there RE-VALIDATED after a restart (a real revocation gap, census
//! SI-020). MR-011 routes [`CapabilityAuthenticator`] through the durable, `(tenant, region)`-
//! partitioned [`RevocationStore`] (`with_pg`). This test proves the discharge END TO END through the
//! REAL PASETO capability verifier:
//!   (1) a genuinely-signed capability token authenticates to its Principal;
//!   (2) revoke its `jti` in the durable store → the SAME authenticator now DENIES it;
//!   (3) a BRAND-NEW `RevocationStore` instance + a fresh authenticator over the SAME pool (a restart
//!       simulation; the real kill-9 is MR-009) STILL denies it — the revocation was read back from PG,
//!       not an in-process set that died with the process. THIS is the durability the stub lacked.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::Timestamp;
use myelin_identity::{Credential, DataRole, PrincipalId, PrincipalKind, PrincipalStatus, RevokeTarget};
use myelin_identity_service::capability_crypto::{CapabilityMintSpec, CellTokenAuthority, PasetoCapabilityVerifier};
use myelin_identity_service::revocation::RevocationStore;
use myelin_identity_service::{CapabilityAuthenticator, PrincipalStore};
use myelin_storage::migration::HotTables;
use myelin_storage::{identity_durable_migrations, DurableRevocationBacking, KmsEngine, SubstrateProvider, TenantScope};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

/// A pinned "now" (Unix seconds) for the verifier's exp check, and its RFC-3339 form for the consult.
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

/// Seed an in-memory PrincipalStore with a CI machine principal + its credential link (the S1 lookup
/// the auth path does after verifying the token; the principal store's OWN durability is MR-007's
/// concern — this test isolates the REVOCATION durability).
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
    let Some(app) = app_provider().await else { return };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr011-{suffix}");

    // The cell's REAL token authority (Ed25519 + macaroon secret) + the verifier's trust anchor.
    let cell = Arc::new(CellTokenAuthority::from_seed(&[11u8; 32], &[22u8; 32]).expect("cell"));
    let anchor = cell.trust_anchor();
    let subject_key = "svc:ci";

    // A genuinely-signed CI capability token (PASETO v4.public), exp in the future of the pinned clock.
    let material = cell.mint(&CapabilityMintSpec {
        tenant: tenant.clone(),
        region: region.clone(),
        subject_key: subject_key.into(),
        jti: "mr011-jti".into(),
        exp_unix: NOW_UNIX + 3600,
        authority: vec!["ci:run".into()],
        dpop_jkt: None,
    });
    let cred = Credential {
        scheme: "ci".into(),
        material,
    };

    // ---- Instance #1: the durable store + an authenticator wired to the REAL PASETO verifier. ----
    let store1 = RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
    let verifier1: Arc<dyn myelin_identity_service::TokenVerifier> =
        Arc::new(PasetoCapabilityVerifier::new(anchor.clone()).with_clock(|| NOW_UNIX));
    let auth1 = CapabilityAuthenticator::with_verifier(
        seeded_principal_store(&tenant, &region, subject_key),
        verifier1,
        store1.clone(),
    )
    .with_clock(|| Timestamp(NOW_RFC3339.into()));

    // (1) Before revocation: the genuinely-signed token authenticates to its Principal.
    let p = auth1
        .authenticate(&cred, None)
        .expect("a correctly-signed CI token authenticates");
    assert_eq!(p.principal_id, PrincipalId("svc:ci".into()));
    assert_eq!(p.tenant, TenantId(tenant.clone()), "tenant from the verified token");

    // (2) Revoke the jti in the durable store (the token's verified partition) → auth1 now DENIES.
    let sc = tenant_scope(&tenant, &region);
    store1.revoke(&sc, &RevokeTarget::Jti("mr011-jti".into()), Timestamp(NOW_RFC3339.into()));
    let denied = auth1.authenticate(&cred, None);
    assert!(
        matches!(denied, Err(myelin_identity::AuthzError::FailClosed(_))),
        "after revocation the SAME authenticator denies the token, got {denied:?}"
    );

    // (3) THE DURABILITY: a BRAND-NEW RevocationStore + a fresh authenticator over the SAME pool (a
    //     restart simulation) STILL denies — the revocation was read back from PG, not an in-process
    //     set. This is exactly what the old tenant-less S7Denylist could NOT do.
    let store2 = RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
    let verifier2: Arc<dyn myelin_identity_service::TokenVerifier> =
        Arc::new(PasetoCapabilityVerifier::new(anchor).with_clock(|| NOW_UNIX));
    let auth2 = CapabilityAuthenticator::with_verifier(
        seeded_principal_store(&tenant, &region, subject_key),
        verifier2,
        store2,
    )
    .with_clock(|| Timestamp(NOW_RFC3339.into()));
    let still_denied = auth2.authenticate(&cred, None);
    assert!(
        matches!(still_denied, Err(myelin_identity::AuthzError::FailClosed(_))),
        "a FRESH authenticator + store over the same pool STILL denies the revoked token (durable \
         revocation survives the 'restart') — got {still_denied:?}"
    );

    // Sanity: a DIFFERENT, un-revoked jti from the same cell still authenticates through auth2 (the
    // deny is specific to the revoked handle, not a blanket failure).
    let fresh = cell_fresh_token(&tenant, &region, subject_key);
    let fresh_cred = Credential { scheme: "ci".into(), material: fresh };
    assert!(
        auth2.authenticate(&fresh_cred, None).is_ok(),
        "an un-revoked token from the same cell still authenticates (the deny is handle-specific)"
    );

    cleanup(&admin, &tenant).await;
    println!("OK: a revoked machine/capability token stays denied across a fresh store instance over the same pool (S7Denylist gap discharged).");
}

/// A second genuinely-signed CI token with a DISTINCT jti (re-mints the cell material — the cell is
/// re-derived from the same seed so the verifier anchor matches).
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
    })
}
