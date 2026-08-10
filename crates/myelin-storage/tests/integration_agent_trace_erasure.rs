use myelin_config::MyelinConfig;
use myelin_storage::{
    all_durable_migrations, AgentTraceError, AgentTraceWrite, AgentTraceWriter,
    DurableAgentTraceStore, DurableKmsBacking, HotTables, SealKey, SubstrateProvider,
};
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
        SubstrateProvider::connect(app_config(), 4)
            .await
            .expect("open the constrained app provider"),
    )
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
