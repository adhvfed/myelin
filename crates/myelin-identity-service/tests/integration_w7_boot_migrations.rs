//! # W7.2 — the boot-migrations aggregate proves the LITERAL doc-18 defect fix, against LIVE PG.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo test` stays DB-free. Runs ONLY
//! against the docker-compose dev stack (the make-it-real env):
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     cargo test -p myelin-identity-service --features integration \
//!       --test integration_w7_boot_migrations -- --nocapture
//!
//! doc-18 Part 5 names the LIVE DEFECT at the edge/identity mains precisely: a main constructs
//! `PrincipalStore::with_pg` but the identity tables `0010`–`0019` are NEVER migrated at boot, so the
//! FIRST principal write fails at runtime on a fresh DB. W7.2 folds every durable group into
//! `myelin_storage::all_durable_migrations()` and applies it (after the foundation) at every such
//! main. This test runs that exact boot sequence and then does the previously-broken write through
//! the LITERAL `PrincipalStore::with_pg` path — it must COMMIT.
#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::principal_store::{PrincipalProfile, PrincipalStore};
use myelin_storage::migration::HotTables;
use myelin_storage::{all_durable_migrations, DurablePrincipalBacking, KmsEngine, SubstrateProvider};
use myelin_tenancy::{Region, TenantId};

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

fn scope(tenant: &str, region: &str) -> myelin_storage::TenantScope {
    let p = Principal::stub(
        PrincipalId("p:admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    myelin_storage::TenantScope::from_verified_token(&p, Region(region.into()))
}

/// The doc-18 red-to-green: run the FIXED boot sequence (`migrate_foundation` + the durable
/// aggregate), then the FIRST `PrincipalStore::with_pg` write — which used to fail because
/// `0010`–`0019` were never migrated — now COMMITS and reads back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn principal_store_with_pg_write_succeeds_after_the_aggregate_boot() {
    // The boot sequence a service main now runs (admin role owns the DDL; idempotent on re-run).
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate_foundation()
        .await
        .expect("boot step 1: foundation (outbox/consumer_dedup)");
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("boot step 2: the FULL durable aggregate (identity 0010–0019 included — the fix)");

    // The store binds through the app role (NOBYPASSRLS, reset-on-release) — the production path.
    let app = SubstrateProvider::connect(MyelinConfig::dev(), 4)
        .await
        .expect("open the app-role provider");
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let kms = Arc::new(KmsEngine::new());
    let suffix = uniq();
    let tenant = format!("w7id-{suffix}");
    let s = scope(&tenant, &region);

    // THE previously-broken path, verbatim (doc-18): `PrincipalStore::with_pg` + the first write.
    let pstore = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(app.clone()),
        handle.clone(),
    );
    let alice = PrincipalId("p:alice".into());
    let written = pstore
        .put_principal(
            &s,
            alice.clone(),
            PrincipalKind::Human,
            DataRole::Processor,
            PrincipalStatus::Active,
            Some(&PrincipalProfile {
                email: "alice@w7.test".into(),
                display_name: "Alice".into(),
            }),
        )
        .expect("PrincipalStore::with_pg write COMMITS after the boot-migrations fix (was the defect)");
    assert!(written.profile_ref.is_some(), "a profiled principal has a profile_ref");

    // It reads back through a fresh store instance over the SAME pool (durable).
    let read = PrincipalStore::with_pg(kms, DurablePrincipalBacking::new(app.clone()), handle)
        .get_principal(&s, &alice)
        .expect("the principal row is durable");
    assert_eq!(read.principal_id, alice);
    assert_eq!(read.kind, PrincipalKind::Human);

    // Cleanup (admin role — RLS-bypassing owner).
    for sql in [
        "DELETE FROM principal WHERE tenant_id = $1",
        "DELETE FROM credential_link WHERE tenant_id = $1",
    ] {
        let _ = sqlx::query(sql).bind(&tenant).execute(admin.db_pool()).await;
    }
    println!(
        "OK: after migrate_foundation + all_durable_migrations, the first PrincipalStore::with_pg \
         write commits + reads back — doc-18 defect green."
    );
}
