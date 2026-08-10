#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::agent_model_step::{
    AgentModelStepStore, ModelStepBegin, ModelStepCompletion, ModelStepError,
};
use myelin_storage::migration::HotTables;
use myelin_storage::{all_durable_migrations, DurableKmsBacking, SealKey, SubstrateProvider};
use myelin_tenancy::TenantId;
use std::sync::Arc;

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
async fn completed_model_work_replays_but_ambiguous_work_stays_in_doubt() {
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
        .expect("apply the durable model-step migration");

    let tenant = TenantId(format!("01J0MODEL{}", unique_suffix()));
    let run_id = format!("run-{}", unique_suffix());
    let request_hash = "a".repeat(64);
    let requested_by = "founder";
    let response = serde_json::json!({
        "reply": { "Final": { "content": "prepare the one-line fix" } },
        "usage": { "Reported": { "input": 10, "cached_input": 2, "output": 3 } }
    });
    let first_provider = SubstrateProvider::connect(config.clone(), 4)
        .await
        .expect("connect the first host process");
    let seal_key = SealKey::from_encoded(&"44".repeat(32)).expect("a 32-byte test seal key");
    let kms_cell = format!("model-step-cell-{}", unique_suffix());
    let kms = Arc::new(
        DurableKmsBacking::new(first_provider.db_pool().clone(), &kms_cell)
            .load_or_generate(&seal_key)
            .await
            .expect("the model replay KMS starts"),
    );
    let first = AgentModelStepStore::new(first_provider, kms.clone());

    assert_eq!(
        first.begin(
            &tenant,
            &run_id,
            "model-turn/0",
            &request_hash,
            requested_by,
        ),
        Ok(ModelStepBegin::Started),
        "the provider request earns a durable intent before it may leave the process",
    );

    let restarted = AgentModelStepStore::new(
        SubstrateProvider::connect(config.clone(), 4)
            .await
            .expect("restart with a fresh database pool"),
        kms.clone(),
    );
    assert_eq!(
        restarted.begin(
            &tenant,
            &run_id,
            "model-turn/0",
            &request_hash,
            requested_by,
        ),
        Ok(ModelStepBegin::InDoubt),
        "a crash after durable intent but before durable response is never guessed safe to retry",
    );
    assert_eq!(
        restarted.complete(
            &tenant,
            &run_id,
            "model-turn/0",
            &request_hash,
            requested_by,
            &response,
        ),
        Ok(ModelStepCompletion::Applied),
        "the observed response completes the intent exactly once",
    );

    let inspected_tenant = tenant.0.clone();
    let inspected_run = run_id.clone();
    let (plaintext, key_ref, nonce, ciphertext) = admin
        .with_tenant_tx(&tenant.0, move |connection| {
            Box::pin(async move {
                sqlx::query_as::<
                    _,
                    (
                        Option<serde_json::Value>,
                        Option<String>,
                        Option<Vec<u8>>,
                        Option<Vec<u8>>,
                    ),
                >(
                    "SELECT response, response_key_ref, response_nonce, response_ciphertext \
                       FROM agent_model_step \
                      WHERE tenant_id = $1 AND run_id = $2 AND step_key = 'model-turn/0'",
                )
                .bind(&inspected_tenant)
                .bind(&inspected_run)
                .fetch_one(connection)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
            })
        })
        .await
        .expect("inspect the durable model response envelope");
    assert!(
        plaintext.is_none(),
        "the JSON response never rests in its legacy column"
    );
    assert!(
        key_ref
            .as_deref()
            .is_some_and(|value| value.contains("subject:founder")),
        "the replay response is wrapped by the requesting human's subject key",
    );
    assert_eq!(nonce.expect("the encrypted response has a nonce").len(), 12);
    let ciphertext = ciphertext.expect("the encrypted response has ciphertext");
    assert!(
        !ciphertext
            .windows(b"prepare the one-line fix".len())
            .any(|window| window == b"prepare the one-line fix"),
        "the model answer has no plaintext copy in its durable replay row",
    );

    let replay_provider = SubstrateProvider::connect(config, 4)
        .await
        .expect("replay from another fresh database pool");
    let replay_kms = Arc::new(
        DurableKmsBacking::new(replay_provider.db_pool().clone(), kms_cell)
            .load_or_generate(&seal_key)
            .await
            .expect("a reconstructed KMS opens the durable subject key"),
    );
    let replay = AgentModelStepStore::new(replay_provider, replay_kms);
    assert_eq!(
        replay.begin(
            &tenant,
            &run_id,
            "model-turn/0",
            &request_hash,
            requested_by,
        ),
        Ok(ModelStepBegin::Completed(response.clone())),
        "completed work returns its durable response without another provider request",
    );
    assert_eq!(
        replay.complete(
            &tenant,
            &run_id,
            "model-turn/0",
            &request_hash,
            requested_by,
            &response,
        ),
        Ok(ModelStepCompletion::Replayed),
    );
    assert_eq!(
        replay.begin(
            &tenant,
            &run_id,
            "model-turn/0",
            &"b".repeat(64),
            requested_by,
        ),
        Err(ModelStepError::Conflict),
        "the same turn cannot be rebound to a different prompt",
    );
    assert_eq!(
        replay.begin(
            &tenant,
            &run_id,
            "model-turn/0",
            &request_hash,
            "another-human",
        ),
        Err(ModelStepError::Conflict),
        "a replay position cannot be reassigned to another data subject",
    );
    assert_eq!(
        replay.complete(
            &tenant,
            &run_id,
            "model-turn/0",
            &request_hash,
            requested_by,
            &serde_json::json!({"different": "answer"}),
        ),
        Err(ModelStepError::Conflict),
        "a completed turn cannot be rewritten with a different answer",
    );

    let region = admin.config().region.clone();
    let mut malformed = admin
        .db_pool()
        .begin()
        .await
        .expect("begin malformed-row probe");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
    .bind(&tenant.0)
    .bind(&region)
    .execute(&mut *malformed)
    .await
    .expect("scope the malformed-row probe");
    let missing_envelope = sqlx::query(
        "INSERT INTO agent_model_step \
           (tenant_id, region, run_id, step_key, request_hash, requested_by, state, completed_at) \
         VALUES ($1, $2, $3, 'model-turn/malformed', $4, $5, 'completed', now())",
    )
    .bind(&tenant.0)
    .bind(&region)
    .bind(&run_id)
    .bind(&request_hash)
    .bind(requested_by)
    .execute(&mut *malformed)
    .await;
    assert!(
        missing_envelope
            .expect_err("completed model work must carry a complete ciphertext envelope")
            .to_string()
            .contains("agent_model_step_encrypted_payload_shape"),
        "the database, not only the host, enforces ciphertext-only completion",
    );
    malformed.rollback().await.ok();
    assert_eq!(
        replay.complete(
            &tenant,
            &run_id,
            "model-turn/1",
            &request_hash,
            requested_by,
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
    assert!(rewrite
        .expect_err("even the table owner cannot rewrite a completed provider response")
        .to_string()
        .contains("one-way completion transition"),);
    mutation.rollback().await.ok();
}
