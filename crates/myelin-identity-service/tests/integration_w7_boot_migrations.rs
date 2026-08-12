#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::principal_store::{PrincipalProfile, PrincipalStore};
use myelin_storage::migration::HotTables;
use myelin_storage::{
    all_durable_migrations, DurablePrincipalBacking, KmsEngine, SubstrateProvider,
};
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn principal_store_with_pg_write_succeeds_after_the_aggregate_boot() {
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
        .expect("boot step 2: the FULL durable aggregate (identity 0010–0019 included - the fix)");

    let app = SubstrateProvider::connect(MyelinConfig::dev(), 4)
        .await
        .expect("open the app-role provider");
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let kms = Arc::new(KmsEngine::new());
    let suffix = uniq();
    let tenant = format!("w7id-{suffix}");
    let s = scope(&tenant, &region);

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
        .expect(
            "PrincipalStore::with_pg write COMMITS after the boot-migrations fix (was the defect)",
        );
    assert!(
        written.profile_ref.is_some(),
        "a profiled principal has a profile_ref"
    );

    let read = PrincipalStore::with_pg(kms, DurablePrincipalBacking::new(app.clone()), handle)
        .get_principal(&s, &alice)
        .expect("the principal row is durable");
    assert_eq!(read.principal_id, alice);
    assert_eq!(read.kind, PrincipalKind::Human);

    for sql in [
        "DELETE FROM principal WHERE tenant_id = $1",
        "DELETE FROM credential_link WHERE tenant_id = $1",
    ] {
        let _ = sqlx::query(sql)
            .bind(&tenant)
            .execute(admin.db_pool())
            .await;
    }
    println!(
        "OK: after migrate_foundation + all_durable_migrations, the first PrincipalStore::with_pg \
         write commits + reads back - doc-18 defect green."
    );
}
