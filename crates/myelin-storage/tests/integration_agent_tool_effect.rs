#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::agent_tool_effect::{
    AgentToolEffectStore, ToolEffectBegin, ToolEffectCompletion, ToolEffectError,
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
    let first_provider = SubstrateProvider::connect(config.clone(), 4)
        .await
        .expect("connect the first agent process");
    let seal_key = SealKey::from_encoded(&"55".repeat(32)).expect("a 32-byte test seal key");
    let kms_cell = format!("tool-effect-cell-{}", unique_suffix());
    let kms = Arc::new(
        DurableKmsBacking::new(first_provider.db_pool().clone(), &kms_cell)
            .load_or_generate(&seal_key)
            .await
            .expect("the tool replay KMS starts"),
    );
    let first_process = AgentToolEffectStore::new(first_provider, kms.clone());

    assert_eq!(
        first_process.begin(&tenant, &run_id, effect_key, &request_hash, requested_by,),
        Ok(ToolEffectBegin::Execute),
        "the external request earns a durable identity before it may leave the process",
    );

    let restarted_process = AgentToolEffectStore::new(
        SubstrateProvider::connect(config.clone(), 4)
            .await
            .expect("restart with a fresh database pool"),
        kms.clone(),
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

    let inspected_tenant = tenant.0.clone();
    let inspected_run = run_id.clone();
    let (plaintext, key_ref, nonce, ciphertext) = admin
        .with_tenant_tx(&tenant.0, move |connection| {
            Box::pin(async move {
                sqlx::query_as::<
                    _,
                    (
                        Option<String>,
                        Option<String>,
                        Option<Vec<u8>>,
                        Option<Vec<u8>>,
                    ),
                >(
                    "SELECT result_text, result_key_ref, result_nonce, result_ciphertext \
                       FROM agent_tool_effect \
                      WHERE tenant_id = $1 AND run_id = $2 AND effect_key = $3",
                )
                .bind(&inspected_tenant)
                .bind(&inspected_run)
                .bind(effect_key)
                .fetch_one(connection)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
            })
        })
        .await
        .expect("inspect the durable tool-result envelope");
    assert!(
        plaintext.is_none(),
        "the tool result never rests in its legacy column"
    );
    assert!(
        key_ref
            .as_deref()
            .is_some_and(|value| value.contains("subject:founder")),
        "the replay result is wrapped by the requesting human's subject key",
    );
    assert_eq!(nonce.expect("the encrypted result has a nonce").len(), 12);
    let ciphertext = ciphertext.expect("the encrypted result has ciphertext");
    assert!(
        !ciphertext
            .windows(b"first snapshot".len())
            .any(|window| window == b"first snapshot"),
        "the tool observation has no plaintext copy in its durable replay row",
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
    let replay_process = AgentToolEffectStore::new(replay_provider, replay_kms);
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
        replay_process.begin(
            &tenant,
            &run_id,
            "model-turn/0/tool/empty",
            &request_hash,
            requested_by,
        ),
        Ok(ToolEffectBegin::Execute),
        "an empty but successful external observation still earns a durable identity",
    );
    assert_eq!(
        replay_process.complete(
            &tenant,
            &run_id,
            "model-turn/0/tool/empty",
            &request_hash,
            requested_by,
            "",
        ),
        Ok(ToolEffectCompletion::Applied),
        "even a zero-byte result has a valid authenticated-encryption envelope",
    );
    assert_eq!(
        replay_process.begin(
            &tenant,
            &run_id,
            "model-turn/0/tool/empty",
            &request_hash,
            requested_by,
        ),
        Ok(ToolEffectBegin::Completed(String::new())),
        "the empty result replays without repeating the external effect",
    );
    assert_eq!(
        replay_process.begin(&tenant, &run_id, effect_key, &"b".repeat(64), requested_by,),
        Err(ToolEffectError::Conflict),
        "one logical position cannot be rebound to different tool input",
    );
    assert_eq!(
        replay_process.begin(&tenant, &run_id, effect_key, &request_hash, "another-human",),
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
        "INSERT INTO agent_tool_effect \
           (tenant_id, region, run_id, effect_key, request_hash, requested_by, state, completed_at) \
         VALUES ($1, $2, $3, 'model-turn/0/tool/malformed', $4, $5, 'completed', now())",
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
            .expect_err("completed tool work must carry a complete ciphertext envelope")
            .to_string()
            .contains("agent_tool_effect_encrypted_payload_shape"),
        "the database, not only the host, enforces ciphertext-only completion",
    );
    malformed.rollback().await.ok();

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
