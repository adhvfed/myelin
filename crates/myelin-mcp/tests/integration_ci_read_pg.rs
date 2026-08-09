#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_ci_controlplane::{ci_controlplane_migrations, CiRunStore};
use myelin_edge::repo_authz::GrantBackedRepos;
use myelin_edge::{DurableCiReadApi, DurableGitBackend, McpCiReadExecutor};
use myelin_events::{MonotonicMinter, OutboxStore, Timestamp};
use myelin_identity::{
    DataRole, DelegationCaveats, FailStaticBound, Principal, PrincipalId, PrincipalKind,
    PrincipalStatus, RunId, RuntimeRef,
};
use myelin_identity_service::delegation::DelegationInput;
use myelin_identity_service::machine_auth::{Authority, MachineKind, StructuralTokenVerifier};
use myelin_identity_service::mint::{RunTokenAuthorizer, RunTokenMinter, StructuralTokenSigner};
use myelin_identity_service::{ResolvedDelegationPolicy, RevocationStore};
use myelin_mcp::{
    GateApproverPolicy, GovernedRouter, McpServer, OutboxGovernanceAudit, RunPrincipal,
    SkeletonEffectApi, ToolRegistry,
};
use myelin_storage::hitl_gate_durable::HitlVerdictStore;
use myelin_storage::{with_tenant_tx, FsBlobStore, PgError, TenantScope};
use myelin_tenancy::{Region, TenantId};
use serde_json::{json, Value};
use sqlx::{Executor, PgPool};

const TENANT: &str = "mcp-ci-read";
const REGION: &str = "eu-north";
const RUN_ID: &str = "81000000-0000-4000-8000-000000000001";
const AGENT_ID: &str = "agent:mcp-ci-reader";
const NOW: &str = "2026-07-24T12:00:00Z";

static SCHEMA_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    std::env::var("DATABASE_MIGRATION_URL").unwrap_or_else(|_| {
        app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
    })
}

fn schema_name() -> String {
    format!(
        "mcp_ci_read_{}_{}",
        std::process::id(),
        SCHEMA_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

async fn pool(url: &str, schema: &str) -> Result<PgPool, sqlx::Error> {
    let schema = schema.to_string();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                connection
                    .execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
}

async fn setup_schema(admin: &PgPool, schema: &str) {
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated schema");
    for migration in myelin_flow::migrations::migrations()
        .0
        .iter()
        .chain(ci_controlplane_migrations().0.iter())
    {
        admin
            .execute(migration.ddl)
            .await
            .unwrap_or_else(|error| panic!("apply {}: {error}", migration.id));
    }
    admin
        .execute(format!("GRANT USAGE ON SCHEMA {schema} TO myelin_app").as_str())
        .await
        .expect("grant isolated schema usage");
    admin
        .execute(
            format!("GRANT SELECT, INSERT ON ALL TABLES IN SCHEMA {schema} TO myelin_app").as_str(),
        )
        .await
        .expect("grant fixture access");
}

struct CatchUnwind<F> {
    inner: std::pin::Pin<Box<F>>,
}

impl<F: std::future::Future> std::future::Future for CatchUnwind<F> {
    type Output = std::thread::Result<F::Output>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.inner.as_mut().poll(cx)
        })) {
            Ok(std::task::Poll::Ready(value)) => std::task::Poll::Ready(Ok(value)),
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Err(payload) => std::task::Poll::Ready(Err(payload)),
        }
    }
}

async fn with_schema_cleanup<Fut>(pool: &PgPool, schema: &str, body: impl FnOnce() -> Fut)
where
    Fut: std::future::Future<Output = ()>,
{
    let result = CatchUnwind {
        inner: Box::pin(body()),
    }
    .await;
    let _ = sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(pool)
        .await;
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

async fn insert_run(app: &PgPool) {
    with_tenant_tx(app, TENANT, REGION, |connection| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO ci_run (
                   tenant_id, region, run_id, project_id, repo_ref, commit_oid, pipeline_id,
                   wf_run_id, definition_snapshot, trigger_kind, trust_tier, state,
                   cost_settled, correlation_id, created_at
                 ) VALUES (
                   $1, $2, $3::uuid, '82000000-0000-4000-8000-000000000001'::uuid, $4,
                   '0123456789abcdef', '83000000-0000-4000-8000-000000000001'::uuid,
                   '84000000-0000-4000-8000-000000000001'::uuid, 'cas:mcp-ci-read', 'push',
                   'trusted', 'failed', TRUE, $3, '2026-07-24T11:59:00Z'::timestamptz
                 )",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(RUN_ID)
            .bind(format!("myelin://{TENANT}/git/repo/alpha"))
            .execute(&mut *connection)
            .await
            .map(|_| ())
            .map_err(|error| PgError::Query(error.to_string()))
        })
    })
    .await
    .expect("insert tenant-scoped run");
}

fn agent() -> Principal {
    Principal::new(
        TenantId(TENANT.into()),
        Region(REGION.into()),
        PrincipalId(AGENT_ID.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt:mcp-ci-read".into()),
            on_behalf_of: Some(PrincipalId("human:founder".into())),
        },
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

struct NoApprovers;

impl GateApproverPolicy for NoApprovers {
    fn eligible_approvers(&self, _tool: &str, _args: &Value) -> Result<Vec<PrincipalId>, String> {
        Ok(Vec::new())
    }
}

fn server(
    api: DurableCiReadApi,
    router_revocations: RevocationStore,
    boundary_revocations: RevocationStore,
) -> McpServer {
    let agent = agent();
    let trigger = Principal::new(
        TenantId(TENANT.into()),
        Region(REGION.into()),
        PrincipalId("human:founder".into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let delegator = trigger.clone();
    let scope = TenantScope::from_verified_token(&agent, Region(REGION.into()));
    let grants = ["run.view"];
    let run_id = RunId("run:mcp-ci-read-integration".into());
    let resolved_policy = ResolvedDelegationPolicy::synthetic_for_test(
        run_id.clone(),
        agent.principal_id.clone(),
        trigger.principal_id.clone(),
        DelegationInput {
            agent_policy: Authority::of(grants),
            delegation: Authority::of(grants),
            tenant_policy: Authority::of(grants),
            trigger_actor_held: Authority::of(grants),
        },
        1,
    );
    let minter = RunTokenMinter::with_signer_and_tuples(
        router_revocations,
        None,
        Arc::new(StructuralTokenSigner::new()),
    );
    let router = GovernedRouter::with_approver_policy(
        minter,
        RunPrincipal {
            scope,
            agent_id: agent.principal_id.clone(),
            agent,
            trigger_actor: trigger,
            trigger_credential_jti: "trigger:mcp-ci-read".into(),
            trigger_expires_at_unix: i64::MAX,
            run_id,
            resolved_policy,
            caveats: DelegationCaveats(vec!["run.view".into()]),
            kind: MachineKind::Agent,
            ttl: FailStaticBound {
                static_max_secs: 300,
            },
        },
        Box::new(SkeletonEffectApi::new()),
        HitlVerdictStore::new(),
        Arc::new(NoApprovers),
        Arc::new(OutboxGovernanceAudit::new(
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()),
        )),
    );
    let boundary = Arc::new(
        RunTokenAuthorizer::new(
            Arc::new(StructuralTokenVerifier::new()),
            boundary_revocations,
        )
        .with_clock(|| Timestamp(NOW.into())),
    );
    let reads = Arc::new(McpCiReadExecutor::new(api, boundary, delegator));
    McpServer::with_router_reads_and_clock(
        ToolRegistry::with_git_and_ci_reads().expect("valid shared catalogue"),
        router,
        reads,
        Arc::new(|| Timestamp(NOW.into())),
    )
}

fn call_read_run(server: &McpServer) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "ci.read_run",
            "arguments": { "run_id": RUN_ID }
        }
    });
    serde_json::from_str(
        &server
            .handle_line(&request.to_string())
            .expect("tools/call response"),
    )
    .expect("JSON-RPC response")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn governed_ci_read_reverifies_the_run_token_at_the_durable_boundary() {
    let schema = schema_name();
    let admin = match pool(&admin_url(), &schema).await {
        Ok(pool) => pool,
        Err(_) => {
            eprintln!("SKIP: dev PostgreSQL is unreachable");
            return;
        }
    };
    setup_schema(&admin, &schema).await;
    with_schema_cleanup(&admin, &schema, || async {
        let app = pool(&app_url(), &schema)
            .await
            .expect("connect app role to isolated schema");
        insert_run(&app).await;

        let git_root = std::env::temp_dir().join(format!("{schema}_git"));
        std::fs::create_dir_all(git_root.join(TENANT).join(REGION).join("alpha.git"))
            .expect("create visible repository");
        let grants = GrantBackedRepos::new().grant_read("human:founder", TENANT, "alpha");
        let git = Arc::new(
            DurableGitBackend::rooted_inmem_for_test(&git_root)
                .with_repo_authorizer(Arc::new(grants)),
        );
        let api = DurableCiReadApi::new(
            CiRunStore::with_pg(app),
            git,
            Arc::new(FsBlobStore::new()),
            tokio::runtime::Handle::current(),
        );

        let router_revocations = RevocationStore::new();
        let denied = call_read_run(&server(
            api.clone(),
            router_revocations.clone(),
            RevocationStore::new(),
        ));
        assert_eq!(denied["result"]["isError"], true);
        assert_eq!(denied["result"]["_meta"]["reason"], "denied");

        let visible = call_read_run(&server(api, router_revocations.clone(), router_revocations));
        assert_eq!(visible["result"]["isError"], false);
        assert_eq!(visible["result"]["_meta"]["tool"], "ci.read_run");
        assert!(
            visible["result"]["_meta"]["runToken"]
                .as_str()
                .is_some_and(|jti| !jti.is_empty()),
            "successful read is attributed to the signed run token"
        );
        let payload: Value =
            serde_json::from_str(visible["result"]["content"][0]["text"].as_str().unwrap())
                .expect("durable read payload");
        assert_eq!(payload["run"]["run_id"], RUN_ID);
        assert_eq!(
            payload["run"]["repo_ref"],
            format!("myelin://{TENANT}/git/repo/alpha")
        );

        let _ = std::fs::remove_dir_all(git_root);
    })
    .await;
}
