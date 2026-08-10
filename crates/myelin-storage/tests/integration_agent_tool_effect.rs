#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::agent_tool_effect::{
    AgentToolEffectStore, ToolEffectBegin, ToolEffectCompletion, ToolEffectError,
};
use myelin_storage::migration::HotTables;
use myelin_storage::{all_durable_migrations, SubstrateProvider};
use myelin_tenancy::TenantId;

fn app_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            config.database_url = database_url;
        }
    }
    config
}

fn admin_config() -> MyelinConfig {
    let mut admin = app_config();
    admin.database_url = admin
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    admin
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch")
        .as_nanos()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restarted_agent_retries_pending_work_but_replays_completed_work() {
    let config = app_config();
    let admin = match SubstrateProvider::connect(admin_config(), 4).await {
        Ok(provider) => provider,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the durable tool-effect migrations");

    let tenant = TenantId(format!("01J0TOOL{}", unique_suffix()));
    let run_id = format!("run-{}", unique_suffix());
    let effect_key = "model-turn/0/tool/0";
    let request_hash = "a".repeat(64);
    let requested_by = "founder";
    let first_process = AgentToolEffectStore::new(
        SubstrateProvider::connect(config.clone(), 4)
            .await
            .expect("connect the first agent process"),
    );

    assert_eq!(
        first_process.begin(&tenant, &run_id, effect_key, &request_hash, requested_by,),
        Ok(ToolEffectBegin::Execute),
        "the external request earns a durable identity before it may leave the process",
    );

    let restarted_process = AgentToolEffectStore::new(
        SubstrateProvider::connect(config.clone(), 4)
            .await
            .expect("restart with a fresh database pool"),
    );
    assert_eq!(
        restarted_process.begin(
            &tenant,
            &run_id,
            effect_key,
            &request_hash,
            requested_by,
        ),
        Ok(ToolEffectBegin::Execute),
        "unfinished tool work retries with the same logical identity and downstream idempotency key",
    );
    assert_eq!(
        restarted_process.complete(
            &tenant,
            &run_id,
            effect_key,
            &request_hash,
            requested_by,
            "first snapshot",
        ),
        Ok(ToolEffectCompletion::Applied),
        "the first observed result completes the effect exactly once",
    );

    let replay_process = AgentToolEffectStore::new(
        SubstrateProvider::connect(config, 4)
            .await
            .expect("replay from another fresh database pool"),
    );
    assert_eq!(
        replay_process.begin(&tenant, &run_id, effect_key, &request_hash, requested_by,),
        Ok(ToolEffectBegin::Completed("first snapshot".into())),
        "completed work returns its exact bytes without another external request",
    );
    assert_eq!(
        replay_process.complete(
            &tenant,
            &run_id,
            effect_key,
            &request_hash,
            requested_by,
            "a later, nondeterministic snapshot",
        ),
        Ok(ToolEffectCompletion::Replayed("first snapshot".into())),
        "a racing completion cannot replace the first durable observation",
    );
    assert_eq!(
        replay_process.begin(&tenant, &run_id, effect_key, &"b".repeat(64), requested_by,),
        Err(ToolEffectError::Conflict),
        "one logical position cannot be rebound to different tool input",
    );
    assert_eq!(
        replay_process.begin(
            &tenant,
            &run_id,
            effect_key,
            &request_hash,
            "another-human",
        ),
        Err(ToolEffectError::Conflict),
        "a replay position cannot be reassigned to another data subject",
    );
    assert_eq!(
        replay_process.complete(
            &tenant,
            &run_id,
            "model-turn/0/tool/1",
            &request_hash,
            requested_by,
            "invented result",
        ),
        Err(ToolEffectError::Missing),
        "a result cannot appear without a durable intent",
    );

    let region = admin.config().region.clone();
    let mut mutation = admin.db_pool().begin().await.expect("begin owner mutation");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
    .bind(&tenant.0)
    .bind(&region)
    .execute(&mut *mutation)
    .await
    .expect("scope the owner mutation");
    let rewrite = sqlx::query(
        "UPDATE agent_tool_effect SET result_text = 'rewritten'
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND effect_key = $4",
    )
    .bind(&tenant.0)
    .bind(&region)
    .bind(&run_id)
    .bind(effect_key)
    .execute(&mut *mutation)
    .await;
    assert!(rewrite
        .expect_err("even the table owner cannot rewrite a completed external result")
        .to_string()
        .contains("one-way completion transition"),);
    mutation.rollback().await.ok();
}
