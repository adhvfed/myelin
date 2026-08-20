#![cfg(feature = "integration")]

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use myelin_config::MyelinConfig;
use myelin_edge::{
    serve_edge, AllowAll, EdgeError, EdgeResponse, Gateway, Handler, HandlerCtx, Method,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, HumanSsoAuthenticator,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore,
};
use myelin_storage::{connect_pool_with_reset, with_tenant_tx, KmsEngine, PgError, TenantScope};
use myelin_tenancy::{Region, TenantId};
use serde_json::json;
use sqlx::postgres::PgPool;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};

const TENANT: &str = "acme";
const REGION: &str = "eu-west";
const SCHEME: &str = "agent";

struct TenantScopeProbe {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

impl Handler for TenantScopeProbe {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let tenant = ctx.scope.tenant().0.clone();
        let region = ctx.scope.region().0.clone();
        let pool = self.pool.clone();
        let got: String = tokio::task::block_in_place(|| {
            self.rt.block_on(async move {
                with_tenant_tx(&pool, &tenant, &region, |conn| {
                    Box::pin(async move {
                        let row: (Option<String>,) =
                            sqlx::query_as("SELECT current_setting('myelin.tenant_id', true)")
                                .fetch_one(&mut *conn)
                                .await
                                .map_err(|e| PgError::Query(e.to_string()))?;
                        Ok(row.0.unwrap_or_default())
                    })
                })
                .await
            })
        })
        .map_err(|e| EdgeError::Internal(format!("tenant-scoped tx failed: {e}")))?;
        Ok(EdgeResponse::json(200, &json!({ "db_tenant_scope": got })))
    }
}

fn admin_scope(tenant: &str) -> TenantScope {
    TenantScope::from_verified_token(
        &Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        ),
        Region(REGION.into()),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edge_sets_the_tenant_scope_in_a_real_transaction() {
    let database_url = std::env::var("MYELIN_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| MyelinConfig::dev().database_url);
    let pool = connect_pool_with_reset(&database_url, REGION, 8)
        .await
        .expect("connect to the dev Postgres");

    let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell");
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    let scope = admin_scope(TENANT);
    store
        .put_principal(
            &scope,
            PrincipalId("svc:agent".into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .unwrap();
    store
        .link_credential(&scope, SCHEME, "subj-1", &PrincipalId("svc:agent".into()))
        .unwrap();
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        Arc::new(KmsEngine::new()),
    )));

    let probe = Arc::new(TenantScopeProbe {
        pool,
        rt: tokio::runtime::Handle::current(),
    });
    let gateway = Arc::new(
        Gateway::builder(authn, human, Arc::new(AllowAll))
            .route(Method::Get, "/v1/scope-probe", "edge.whoami", probe)
            .build(),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = serve_edge(listener, gateway).await;
    });

    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        + 3600;
    let token = cell.mint(&CapabilityMintSpec {
        tenant: TENANT.into(),
        region: REGION.into(),
        subject_key: "subj-1".into(),
        jti: "jti-scope".into(),
        exp_unix: exp,
        authority: vec!["edge.operator".into()],
        dpop_jkt: None,
        purpose: myelin_identity_service::CredentialPurpose::OperatorBootstrap,
        audience: myelin_identity_service::CredentialAudience::Edge,
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = Request::builder()
        .method("GET")
        .uri("/v1/scope-probe")
        .header("host", "edge.test")
        .header("authorization", format!("Bearer {token}"))
        .header("x-myelin-token-scheme", SCHEME)
        .body(Full::new(Bytes::new()))
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        v["db_tenant_scope"], TENANT,
        "the edge set the DB tenant scope (myelin.tenant_id) to the VERIFIED token's tenant"
    );
}
