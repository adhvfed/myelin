#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_config::{Mode, MyelinConfig};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType, IdMinter,
    OutboxStore, OutboxTx, Region, TenantId, Timestamp, UlidMinter, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::issues_app_spec;
use myelin_storage::{PgOutboxBacking, SubstrateProvider};
use myelin_substrate::{boot, Config};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn ctx_base(tenant: &str) -> EmitContextBase {
    let principal = Principal::stub(
        PrincipalId("p:w3b4".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    EmitContextBase {
        tenant: TenantId(tenant.into()),
        region: Region("fr-par".into()),
        actor: Actor(principal),
        schema_ver: 1,
        occurred_at: Timestamp("2026-07-15T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-15T00:00:01Z".into()),
        caused_by: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spec_built_app_emit_lands_in_pg_without_claiming_the_shared_relay() {
    let mut migration_config = MyelinConfig::from_env(Mode::DevDefaults).expect("dev config");
    migration_config.database_url = admin_url();
    let migrator = SubstrateProvider::connect(migration_config.clone(), 2)
        .await
        .expect("connect the migration-owner pool (is the dev stack up?)");
    migrator
        .migrate_foundation()
        .await
        .expect("apply the foundation migrations (outbox/consumer_dedup)");

    migration_config.database_url = app_url();
    let provider = SubstrateProvider::connect(migration_config, 4)
        .await
        .expect("connect the RLS-enforced runtime pool");
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));

    let handle = boot(issues_app_spec(Config::default(), outbox.clone())).expect("boot");

    let minter: Arc<dyn IdMinter> = Arc::new(UlidMinter::new());
    let run_tag = format!(
        "w3b4-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let aggregate = format!("issue:{run_tag}");
    let mut tx = handle.outbox().begin(Arc::clone(&minter), ctx_base("acme"));
    tx.stage_state_change("w3b4 durable-root proof");
    let event_id = tx
        .emit(
            EventDraft {
                type_: EventType("issues.issue.created".into()),
                subject: ArtifactRef(format!("myelin://acme/issues/issue/{run_tag}")),
                aggregate: AggregateKey(aggregate.clone()),
                payload: serde_json::json!({ "ref": run_tag }),
                data_role: DataRole::Controller,
                visibility: Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            },
            None,
        )
        .expect("emit");
    tx.commit().expect("durable co-commit");

    let (n, published): (i64, Option<bool>) = {
        let row: (i64,) =
            sqlx::query_as("SELECT count(*) FROM outbox WHERE event_id = $1 AND aggregate = $2")
                .bind(&event_id.0)
                .bind(&aggregate)
                .fetch_one(provider.db_pool())
                .await
                .expect("select committed row");
        let sent: Option<(bool,)> =
            sqlx::query_as("SELECT published_at IS NOT NULL FROM outbox WHERE event_id = $1")
                .bind(&event_id.0)
                .fetch_optional(provider.db_pool())
                .await
                .expect("select published flag");
        (row.0, sent.map(|s| s.0))
    };
    assert_eq!(n, 1, "the emit landed in the PG outbox exactly once");
    assert_eq!(
        published,
        Some(false),
        "a freshly committed row is unsent (published_at IS NULL)"
    );

    handle.tick();

    let sent_after: (bool,) =
        sqlx::query_as("SELECT published_at IS NOT NULL FROM outbox WHERE event_id = $1")
            .bind(&event_id.0)
            .fetch_one(provider.db_pool())
            .await
            .expect("select published flag after drain");
    assert!(
        !sent_after.0,
        "the producer lifecycle must leave publication to the elected cell relay"
    );

    let row = handle
        .outbox()
        .row(&event_id)
        .expect("the durable store re-reads the row from PG");
    assert!(
        row.published_at.is_none(),
        "store read confirms the shared row was not claimed locally"
    );

    handle.signal_drain();
}
