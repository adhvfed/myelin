#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::agent_model_step::{
    agent_model_step_migrations, AgentModelStepStore, ModelStepBegin, ModelStepCompletion,
    ModelStepError,
};
use myelin_storage::migration::HotTables;
use myelin_storage::SubstrateProvider;
use myelin_tenancy::TenantId;

fn admin_config(config: &MyelinConfig) -> MyelinConfig {
    let mut admin = config.clone();
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
async fn completed_model_work_replays_but_ambiguous_work_stays_in_doubt() {
    let config = MyelinConfig::dev();
    let admin = match SubstrateProvider::connect(admin_config(&config), 4).await {
        Ok(provider) => provider,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate(&agent_model_step_migrations(), &HotTables::none())
        .await
        .expect("apply the durable model-step migration");

    let tenant = TenantId(format!("01J0MODEL{}", unique_suffix()));
    let run_id = format!("run-{}", unique_suffix());
    let request_hash = "a".repeat(64);
    let response = serde_json::json!({
        "reply": { "Final": { "content": "prepare the one-line fix" } },
        "usage": { "Reported": { "input": 10, "cached_input": 2, "output": 3 } }
    });
    let first = AgentModelStepStore::new(
        SubstrateProvider::connect(config.clone(), 4)
            .await
            .expect("connect the first host process"),
    );

    assert_eq!(
        first.begin(&tenant, &run_id, "model-turn/0", &request_hash),
        Ok(ModelStepBegin::Started),
        "the provider request earns a durable intent before it may leave the process",
    );

    let restarted = AgentModelStepStore::new(
        SubstrateProvider::connect(config.clone(), 4)
            .await
            .expect("restart with a fresh database pool"),
    );
    assert_eq!(
        restarted.begin(&tenant, &run_id, "model-turn/0", &request_hash),
        Ok(ModelStepBegin::InDoubt),
        "a crash after durable intent but before durable response is never guessed safe to retry",
    );
    assert_eq!(
        restarted.complete(
            &tenant,
            &run_id,
            "model-turn/0",
            &request_hash,
            &response,
        ),
        Ok(ModelStepCompletion::Applied),
        "the observed response completes the intent exactly once",
    );

    let replay = AgentModelStepStore::new(
        SubstrateProvider::connect(config, 4)
            .await
            .expect("replay from another fresh database pool"),
    );
    assert_eq!(
        replay.begin(&tenant, &run_id, "model-turn/0", &request_hash),
        Ok(ModelStepBegin::Completed(response.clone())),
        "completed work returns its durable response without another provider request",
    );
    assert_eq!(
        replay.complete(
            &tenant,
            &run_id,
            "model-turn/0",
            &request_hash,
            &response,
        ),
        Ok(ModelStepCompletion::Replayed),
    );
    assert_eq!(
        replay.begin(&tenant, &run_id, "model-turn/0", &"b".repeat(64)),
        Err(ModelStepError::Conflict),
        "the same turn cannot be rebound to a different prompt",
    );
    assert_eq!(
        replay.complete(
            &tenant,
            &run_id,
            "model-turn/0",
            &request_hash,
            &serde_json::json!({"different": "answer"}),
        ),
        Err(ModelStepError::Conflict),
        "a completed turn cannot be rewritten with a different answer",
    );
    assert_eq!(
        replay.complete(
            &tenant,
            &run_id,
            "model-turn/1",
            &request_hash,
            &response,
        ),
        Err(ModelStepError::Missing),
        "a response cannot appear without a durable intent",
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
        "UPDATE agent_model_step SET response = '{\"rewritten\":true}'::jsonb
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND step_key = 'model-turn/0'",
    )
    .bind(&tenant.0)
    .bind(&region)
    .bind(&run_id)
    .execute(&mut *mutation)
    .await;
    assert!(
        rewrite
            .expect_err("even the table owner cannot rewrite a completed provider response")
            .to_string()
            .contains("one-way completion transition"),
    );
    mutation.rollback().await.ok();
}
