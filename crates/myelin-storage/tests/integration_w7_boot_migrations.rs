#![cfg(feature = "integration")]

use std::str::FromStr;

use myelin_config::MyelinConfig;
use myelin_storage::migration::HotTables;
use myelin_storage::{
    all_durable_migrations, foundation_migrations, DurablePrincipalBacking, DurablePrincipalRow,
    DurableReplayBacking, PgMigrator, SubstrateProvider,
};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};

fn admin_url() -> String {
    MyelinConfig::dev()
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn admin_config() -> MyelinConfig {
    let mut c = MyelinConfig::dev();
    c.database_url = admin_url();
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

async fn table_exists_in(pool: &PgPool, schema: &str, table: &str) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL")
        .bind(format!("{schema}.{table}"))
        .fetch_one(pool)
        .await
        .expect("probe table existence")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_schema_principal_table_absent_after_foundation_then_present_after_aggregate() {
    let schema = format!("w7_boot_{}", uniq());

    let bootstrap = match PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
    {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    sqlx::query(&format!("CREATE SCHEMA \"{schema}\""))
        .execute(&bootstrap)
        .await
        .expect("create the fresh isolation schema");

    let opts = PgConnectOptions::from_str(&admin_url())
        .expect("parse admin DSN")
        .options([("search_path", format!("\"{schema}\",public").as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await
        .expect("open the search_path-pinned admin pool");

    PgMigrator::apply(&pool, &foundation_migrations())
        .await
        .expect("foundation migrations apply into the fresh schema");
    assert!(
        table_exists_in(&pool, &schema, "outbox").await,
        "foundation created the outbox table in the fresh schema"
    );
    assert!(
        !table_exists_in(&pool, &schema, "principal").await,
        "DEFECT REPRODUCED: `principal` (identity 0012) is ABSENT after foundation-only - a \
         principal write here would fail at runtime"
    );

    PgMigrator::apply(&pool, &all_durable_migrations())
        .await
        .expect("the durable aggregate applies into the fresh schema");
    for (table, group) in [
        ("rebac_tuple", "identity 0010"),
        ("principal", "identity 0012"),
        ("pseudonym_map", "pseudonym 0020"),
        ("cell", "placement 0030"),
        ("kms_sealed_root", "kms 0040"),
        ("cost_reservation", "cost 0050"),
        ("restore_erasure_ledger", "restore 0051"),
        ("post_pit_erasure_ledger", "post-pit 0052"),
        ("bus_erasure_ledger", "bus-erasure 0053"),
        ("agent_hitl_gate", "hitl-gate 0054 (R2.4)"),
        ("auth_replay", "authentication replay 0070"),
        ("agent_wallet", "agent-wallet 0080"),
        ("agent_wallet_ledger", "agent-wallet-ledger 0080"),
    ] {
        assert!(
            table_exists_in(&pool, &schema, table).await,
            "the aggregate migrated `{table}` ({group}) into the fresh schema"
        );
    }

    PgMigrator::apply(&pool, &all_durable_migrations())
        .await
        .expect("re-applying the aggregate is idempotent");

    sqlx::query(&format!("DROP SCHEMA \"{schema}\" CASCADE"))
        .execute(&bootstrap)
        .await
        .expect("drop the fresh isolation schema");
    println!(
        "OK [1]: on a fresh schema, `principal` is ABSENT after foundation-only (the defect) and \
         PRESENT after all_durable_migrations (the fix); re-apply idempotent."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_principal_write_succeeds_after_the_boot_sequence() {
    let admin = match SubstrateProvider::connect(admin_config(), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate_foundation()
        .await
        .expect("boot step 1: foundation");
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("boot step 2: the durable aggregate (the W7.2 fix)");

    let app = SubstrateProvider::connect(MyelinConfig::dev(), 4)
        .await
        .expect("open the app-role provider");
    let region = app.config().region.clone();
    let suffix = uniq();
    let tenant = format!("w7-boot-{suffix}");

    let backing = DurablePrincipalBacking::new(app.clone());
    let row = DurablePrincipalRow {
        principal_id: "p:alice".into(),
        kind: "\"Human\"".into(),
        data_role: "\"Processor\"".into(),
        status: "\"Active\"".into(),
        profile: None,
    };
    backing
        .put_principal(&tenant, row.clone())
        .await
        .expect("the durable principal write COMMITS after the boot-migrations fix (was the defect)");

    let read = DurablePrincipalBacking::new(app.clone())
        .get_principal(&tenant, "p:alice")
        .await
        .expect("read the principal row back")
        .expect("the principal row is durable");
    assert_eq!(read.principal_id, "p:alice");
    assert_eq!(read.kind, "\"Human\"");

    let replay_a = DurableReplayBacking::new(app.clone());
    let replay_b = DurableReplayBacking::new(app.clone());
    let (a, b) = tokio::join!(
        replay_a.consume(&tenant, "oidc:test", "shared-jti", 200, 100),
        replay_b.consume(&tenant, "oidc:test", "shared-jti", 200, 100),
    );
    let winners = [a.unwrap(), b.unwrap()]
        .into_iter()
        .filter(|won| *won)
        .count();
    assert_eq!(winners, 1, "exactly one replica consumes a replay id");
    assert!(
        replay_a
            .consume(&tenant, "oidc:test", "shared-jti", 400, 201)
            .await
            .unwrap(),
        "the replay id is reusable only after its prior token expired"
    );

    let _ = sqlx::query("DELETE FROM principal WHERE tenant_id = $1 AND region = $2")
        .bind(&tenant)
        .bind(&region)
        .execute(admin.db_pool())
        .await;
    let _ = sqlx::query("DELETE FROM auth_replay WHERE tenant_id = $1 AND region = $2")
        .bind(&tenant)
        .bind(&region)
        .execute(admin.db_pool())
        .await;
    println!(
        "OK [2]: after `migrate_foundation` + `all_durable_migrations`, a durable principal write \
         commits + reads back - the doc-18 runtime write path is green."
    );
}
