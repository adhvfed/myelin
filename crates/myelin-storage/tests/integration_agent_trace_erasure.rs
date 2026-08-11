use myelin_config::MyelinConfig;
use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{
    all_durable_migrations, AgentModelStepStore, AgentToolEffectStore, AgentTraceError,
    AgentTraceSubjectState, AgentTraceWrite, AgentTraceWriter, DurableAgentTraceStore,
    DurableKmsBacking, HotTables, ModelStepError, PiiKeyRef, SealKey, SubstrateProvider,
    ToolEffectError,
};
use myelin_tenancy::{Region, TenantId};
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
    let mut config = app_config();
    config.database_url = config
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    config
}

fn unique(label: &str) -> String {
    format!(
        "{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock follows the epoch")
            .as_nanos()
    )
}

async fn test_provider() -> Option<SubstrateProvider> {
    let admin = match SubstrateProvider::connect(admin_config(), 2).await {
        Ok(provider) => provider,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return None;
        }
    };
    admin.migrate_foundation().await.unwrap();
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap();
    Some(
        SubstrateProvider::connect(app_config(), 1)
            .await
            .expect("open the deliberately single-connection app provider"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_database_has_no_plaintext_door_for_an_agent_answer() {
    let Some(provider) = test_provider().await else {
        return;
    };
    let tenant = unique("trace-plaintext-refused");
    let region = provider.config().region.clone();
    let transaction_tenant = tenant.clone();
    let attempted = provider
        .with_tenant_tx(&transaction_tenant, move |connection| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO knowledge_agent_trace \
                       (tenant_id, region, run_id, artifact_ref, agent_principal, requested_by, \
                        answer, trace_body, charged_micro) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 42)",
                )
                .bind(&tenant)
                .bind(&region)
                .bind("plaintext-run")
                .bind(format!("myelin://{tenant}/knowledge/doc/plaintext"))
                .bind("agent:privacy-test")
                .bind("founder")
                .bind("A result the database must refuse.")
                .bind(serde_json::json!({ "answer": "still plaintext" }))
                .execute(connection)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        })
        .await;

    assert!(
        attempted.is_err(),
        "an application path cannot persist a plaintext agent answer after the privacy migration"
    );
}

fn trace(run_id: &str) -> AgentTraceWrite {
    let answer = "A result that must stay erased.";
    AgentTraceWrite {
        run_id: run_id.into(),
        agent_principal: "agent:privacy-test".into(),
        requested_by: "founder".into(),
        answer: answer.into(),
        trace_body: serde_json::json!({
            "schema": "myelin.agent_trace.v1",
            "run_id": run_id,
            "actor": "agent:privacy-test",
            "requested_by": "founder",
            "answer": answer,
            "charged_micro": 42,
            "blocks": [{
                "type": "paragraph",
                "inline": {
                    "spans": [{"Text": {"text": answer, "marks": [], "link": null}}],
                    "nodes": []
                }
            }]
        }),
        charged_micro: 42,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_erasure_marker_refuses_to_resurrect_a_trace_on_worker_retry() {
    let Some(provider) = test_provider().await else {
        return;
    };
    let tenant = TenantId(unique("trace-erasure"));
    let run_id = "33333333-3333-4333-8333-333333333333";
    let erased_trace = trace(run_id);
    let artifact_ref = erased_trace.artifact_ref(&tenant).unwrap();
    let erased_tenant = tenant.0.clone();
    let erased_region = provider.config().region.clone();
    let erased_ref = artifact_ref.0.clone();
    provider
        .with_tenant_tx(&tenant.0, move |connection| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO knowledge_agent_trace_erasure \
                       (tenant_id, region, run_id, artifact_ref) VALUES ($1, $2, $3, $4)",
                )
                .bind(&erased_tenant)
                .bind(&erased_region)
                .bind(run_id)
                .bind(&erased_ref)
                .execute(&mut *connection)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        })
        .await
        .expect("record the durable erasure marker");

    let seal_key = SealKey::from_encoded(&"66".repeat(32)).expect("a 32-byte test seal key");
    let kms = DurableKmsBacking::new(provider.db_pool().clone(), unique("trace-erasure-cell"))
        .load_or_generate(&seal_key)
        .await
        .expect("the durable test KMS starts");
    let store = DurableAgentTraceStore::with_runtime(
        provider.clone(),
        tokio::runtime::Handle::current(),
        std::sync::Arc::new(kms),
    );
    assert_eq!(
        store.write(&tenant, erased_trace).unwrap_err(),
        AgentTraceError::Erased,
        "a replay cannot turn an erased final answer back into live product data"
    );

    let cleanup_tenant = tenant.0.clone();
    provider
        .with_tenant_tx(&tenant.0, move |connection| {
            Box::pin(async move {
                sqlx::query("DELETE FROM knowledge_agent_trace_erasure WHERE tenant_id = $1")
                    .bind(&cleanup_tenant)
                    .execute(&mut *connection)
                    .await
                    .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        })
        .await
        .expect("clean the isolated erasure story");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn erasing_a_subject_shreds_every_trace_and_permanently_suppresses_new_ones() {
    let Some(provider) = test_provider().await else {
        return;
    };
    let tenant = TenantId(unique("trace-subject-erasure"));
    let seal_key = SealKey::from_encoded(&"77".repeat(32)).expect("a 32-byte test seal key");
    let kms = Arc::new(
        DurableKmsBacking::new(provider.db_pool().clone(), unique("trace-subject-cell"))
            .load_or_generate(&seal_key)
            .await
            .expect("the durable test KMS starts"),
    );
    let store = DurableAgentTraceStore::with_runtime(
        provider.clone(),
        tokio::runtime::Handle::current(),
        kms.clone(),
    );
    let model_steps = AgentModelStepStore::new(provider.clone(), kms.clone());
    let tool_effects = AgentToolEffectStore::new(provider.clone(), kms.clone());
    let subject = SubjectRef::new(Principal::stub(
        PrincipalId("founder".into()),
        PrincipalKind::Human,
        tenant.clone(),
    ));
    let holder: &dyn PersonalDataHolder = &store;
    holder
        .restrict(&subject, true)
        .expect("restriction reaches the durable trace holder");
    assert_eq!(
        store
            .write(&tenant, trace("00000000-0000-4000-8000-000000000000"))
            .unwrap_err(),
        AgentTraceError::Restricted,
        "restricted subjects cannot enter agent trace processing"
    );
    assert_eq!(
        model_steps.begin(
            &tenant,
            "00000000-0000-4000-8000-000000000000",
            "model-turn/0",
            &"a".repeat(64),
            "founder",
        ),
        Err(ModelStepError::Restricted),
        "restriction is enforced before any model request can be journaled",
    );
    assert_eq!(
        tool_effects.begin(
            &tenant,
            "00000000-0000-4000-8000-000000000000",
            "model-turn/0/tool/0",
            &"b".repeat(64),
            "founder",
        ),
        Err(ToolEffectError::Restricted),
        "restriction is enforced before any external effect can be journaled",
    );
    holder
        .restrict(&subject, false)
        .expect("lifting restriction reaches the durable trace holder");
    for run_id in [
        "11111111-1111-4111-8111-111111111111",
        "22222222-2222-4222-8222-222222222222",
    ] {
        store
            .write(&tenant, trace(run_id))
            .expect("a subject-keyed trace is durable");
    }
    model_steps
        .begin(
            &tenant,
            "11111111-1111-4111-8111-111111111111",
            "model-turn/0",
            &"a".repeat(64),
            "founder",
        )
        .expect("the subject owns the model intent");
    model_steps
        .complete(
            &tenant,
            "11111111-1111-4111-8111-111111111111",
            "model-turn/0",
            &"a".repeat(64),
            "founder",
            &serde_json::json!({"answer": "private model reply"}),
        )
        .expect("the subject owns the replayable model answer");
    tool_effects
        .begin(
            &tenant,
            "22222222-2222-4222-8222-222222222222",
            "model-turn/0/tool/0",
            &"b".repeat(64),
            "founder",
        )
        .expect("the subject owns the tool intent");
    tool_effects
        .complete(
            &tenant,
            "22222222-2222-4222-8222-222222222222",
            "model-turn/0/tool/0",
            &"b".repeat(64),
            "founder",
            "private tool observation",
        )
        .expect("the subject owns the replayable tool result");
    let inspected_tenant = tenant.0.clone();
    let journal_key_ref = provider
        .with_tenant_tx(&tenant.0, move |connection| {
            Box::pin(async move {
                sqlx::query_scalar::<_, String>(
                    "SELECT response_key_ref FROM agent_model_step \
                      WHERE tenant_id = $1 AND run_id = \
                        '11111111-1111-4111-8111-111111111111' \
                        AND step_key = 'model-turn/0'",
                )
                .bind(&inspected_tenant)
                .fetch_one(connection)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
            })
        })
        .await
        .expect("capture the replay ciphertext's durable subject-key reference");
    let journal_key_ref =
        PiiKeyRef::parse(&journal_key_ref).expect("the stored key reference parses");
    let region = Region(provider.config().region.clone());
    assert!(
        kms.resolve_dek(&journal_key_ref, &region).is_ok(),
        "the subject can replay the encrypted result before erasure",
    );
    assert_eq!(
        store.count_for_subject(&tenant.0, "founder").await.unwrap(),
        4,
        "final traces and their replay journals are discoverable through one subject locator"
    );
    let before = store
        .summarize_subject(&tenant.0, "founder")
        .await
        .expect("the production privacy boundary can describe the subject's agent data");
    assert_eq!(before.state, AgentTraceSubjectState::Active);
    assert_eq!(before.recoverable_records, 4);
    assert_eq!(
        holder
            .locate(&subject, tenant.clone())
            .expect("the DSR holder locates durable traces")
            .receipt
            .operation,
        "locate"
    );

    let erased = store
        .erase_for_subject(&tenant.0, "founder")
        .await
        .expect("the subject erasure completes");
    assert_eq!(erased.traces_erased, 2);
    assert_eq!(erased.model_steps_erased, 1);
    assert_eq!(erased.tool_effects_erased, 1);
    assert!(!erased.already_erased);
    assert!(erased.key_destroyed, "the durable subject DEK existed");
    assert!(
        erased.key_unrecoverable,
        "the subject DEK no longer resolves"
    );
    assert!(
        kms.resolve_dek(&journal_key_ref, &region).is_err(),
        "captured journal ciphertext is unreadable after the subject key is destroyed",
    );
    assert_eq!(
        store.count_for_subject(&tenant.0, "founder").await.unwrap(),
        0,
        "no live trace or replay-journal rows remain"
    );
    let after = store
        .summarize_subject(&tenant.0, "founder")
        .await
        .expect("the erased state remains observable without reopening any data");
    assert_eq!(after.state, AgentTraceSubjectState::Erased);
    assert_eq!(after.recoverable_records, 0);
    assert_eq!(
        store
            .write(&tenant, trace("33333333-3333-4333-8333-333333333333"))
            .unwrap_err(),
        AgentTraceError::Erased,
        "the durable subject marker refuses a later worker retry before it can recreate a key"
    );
    assert_eq!(
        model_steps.begin(
            &tenant,
            "33333333-3333-4333-8333-333333333333",
            "model-turn/0",
            &"c".repeat(64),
            "founder",
        ),
        Err(ModelStepError::Erased),
        "erasure is checked before a replay journal can admit another provider call",
    );
    assert_eq!(
        tool_effects.begin(
            &tenant,
            "33333333-3333-4333-8333-333333333333",
            "model-turn/0/tool/0",
            &"d".repeat(64),
            "founder",
        ),
        Err(ToolEffectError::Erased),
        "erasure is checked before a replay journal can admit another tool effect",
    );
    let replay = store
        .erase_for_subject(&tenant.0, "founder")
        .await
        .expect("subject erasure is idempotent");
    assert!(replay.already_erased);
    assert_eq!(replay.traces_erased, 0);
    assert_eq!(replay.model_steps_erased, 0);
    assert_eq!(replay.tool_effects_erased, 0);
    assert!(replay.key_unrecoverable);
    assert_eq!(
        holder
            .erase(EraseScope::Subject {
                subject,
                tenant: tenant.clone(),
            })
            .expect("the PersonalDataHolder seam reaches the same idempotent erasure")
            .receipt
            .operation,
        "erase"
    );

    let cleanup_tenant = tenant.0.clone();
    provider
        .with_tenant_tx(&tenant.0, move |connection| {
            Box::pin(async move {
                for statement in [
                    "DELETE FROM knowledge_agent_trace_erasure WHERE tenant_id = $1",
                    "DELETE FROM knowledge_agent_trace_subject_erasure WHERE tenant_id = $1",
                    "DELETE FROM knowledge_agent_trace_subject_restriction WHERE tenant_id = $1",
                ] {
                    sqlx::query(statement)
                        .bind(&cleanup_tenant)
                        .execute(&mut *connection)
                        .await
                        .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                }
                Ok(())
            })
        })
        .await
        .expect("clean the isolated subject-erasure story");
}
