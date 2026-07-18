//! Live PostgreSQL proof for the R4.4 durable Issue-store increment.
#![cfg(feature = "integration")]

use myelin_config::{Mode, MyelinConfig};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_issues::{
    issues_hot_tables, issues_migrations, CreateIssue, IssueAuthorizer, IssuePageRequest,
    IssuePermission, IssueStoreError, PgIssueStore, VisibleIssues,
};
use myelin_storage::{KmsEngine, PgBootstrap};
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;
use std::sync::Arc;

fn admin_url() -> String {
    std::env::var("DATABASE_MIGRATION_URL")
        .unwrap_or_else(|_| "postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin".into())
}

#[derive(Clone)]
struct ExplicitAuthz;

impl IssueAuthorizer for ExplicitAuthz {
    fn may_create(&self, _principal: &Principal, _project_id: &str) -> bool {
        true
    }

    fn may_access(
        &self,
        _principal: &Principal,
        _issue_id: &str,
        _permission: IssuePermission,
    ) -> bool {
        true
    }

    fn visible_issues(&self, _principal: &Principal) -> Result<VisibleIssues, String> {
        Ok(VisibleIssues::All)
    }
}

fn principal(tenant: &str, id: &str) -> Principal {
    Principal::new(
        TenantId::from_token(tenant),
        Region::new("fr-par"),
        PrincipalId(id.into()),
        PrincipalKind::Service,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn durable_create_list_view_close_is_rls_isolated_and_ciphertext_at_rest() {
    let mut config = MyelinConfig::from_env(Mode::DevDefaults).expect("dev config");
    config.region = "fr-par".into();
    let bootstrap = PgBootstrap::connect(config, 4)
        .await
        .expect("validate split database roles (is docker-compose.dev.yml up?)");
    bootstrap
        .migrate_foundation()
        .await
        .expect("apply shared outbox/dedup foundation");
    bootstrap
        .migrate(&issues_migrations(), &issues_hot_tables())
        .await
        .expect("apply boot-applicable Issues table/index/expand migrations");

    // Runtime uses the non-owner NOBYPASSRLS app role; handoff has closed the privileged pool and
    // erased its credential from retained config before the Issue store exists.
    let provider = bootstrap
        .into_runtime()
        .await
        .expect("handoff to the RLS-enforced app provider");

    let store = PgIssueStore::new(provider, Arc::new(KmsEngine::new()), ExplicitAuthz);
    let suffix = format!("{}_{}", std::process::id(), now_nanos());
    let tenant_a = format!("r44_a_{suffix}");
    let tenant_b = format!("r44_b_{suffix}");
    let alice = principal(&tenant_a, &format!("svc:alice:{suffix}"));
    let bob = principal(&tenant_b, &format!("svc:bob:{suffix}"));
    let project = "11111111-1111-1111-1111-111111111111";
    let issue_type = "22222222-2222-2222-2222-222222222222";
    let secret_title = format!("private title {suffix}");

    let a = store
        .create(
            &alice,
            CreateIssue {
                project_id: project.into(),
                type_id: issue_type.into(),
                prefix: "ENG".into(),
                title: secret_title.clone(),
            },
        )
        .await
        .expect("tenant A create");
    let b = store
        .create(
            &bob,
            CreateIssue {
                project_id: project.into(),
                type_id: issue_type.into(),
                prefix: "ENG".into(),
                title: "tenant B title".into(),
            },
        )
        .await
        .expect("tenant B create");

    let page_a = store
        .list(&alice, IssuePageRequest::new(10, None).unwrap())
        .await
        .expect("tenant A list");
    assert!(page_a.items.iter().any(|item| item.id == a.id));
    assert!(page_a.items.iter().all(|item| item.id != b.id));
    assert_eq!(store.view(&alice, &a.id).await.unwrap().title, secret_title);
    assert_eq!(
        store.view(&bob, &a.id).await,
        Err(IssueStoreError::NotFound),
        "even an intentionally permissive test authorizer cannot cross the FORCE-RLS tenant boundary"
    );
    assert_eq!(
        store.close(&bob, &a.id).await,
        Err(IssueStoreError::NotFound)
    );

    let closed = store.close(&alice, &a.id).await.expect("durable close");
    assert_eq!(closed.state_category, "completed");
    let version = closed.version;
    assert_eq!(
        store
            .close(&alice, &a.id)
            .await
            .expect("idempotent close")
            .version,
        version,
        "an already-completed issue does not get another version bump"
    );

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .expect("connect admin for at-rest inspection");
    let row = sqlx::query(
        "SELECT title, title_ciphertext, title_nonce, pii_key_ref, reporter, \
                created_by_principal, contains_personal_data \
         FROM issue WHERE tenant_id = $1 AND id = $2::uuid",
    )
    .bind(&tenant_a)
    .bind(&a.id)
    .fetch_one(&admin)
    .await
    .expect("inspect exact test row");
    assert_eq!(row.get::<String, _>("title"), "<encrypted>");
    let ciphertext: Vec<u8> = row.get("title_ciphertext");
    assert!(!ciphertext
        .windows(secret_title.len())
        .any(|window| window == secret_title.as_bytes()));
    assert_eq!(row.get::<Vec<u8>, _>("title_nonce").len(), 12);
    assert_eq!(row.get::<Option<sqlx::types::Uuid>, _>("reporter"), None);
    assert_eq!(
        row.get::<String, _>("created_by_principal"),
        alice.principal_id.0
    );
    assert!(row.get::<bool, _>("contains_personal_data"));
    assert!(
        row.get::<String, _>("pii_key_ref")
            .ends_with(&format!("/subject:{}", alice.principal_id.0)),
        "the title DEK is assigned to the verified creator subject"
    );

    // Exact test-tenant cleanup; no production row or broad collection is touched.
    for tenant in [&tenant_a, &tenant_b] {
        sqlx::query("DELETE FROM issue WHERE tenant_id = $1")
            .bind(tenant)
            .execute(&admin)
            .await
            .expect("clean issue fixture rows");
        sqlx::query("DELETE FROM prefix_counter WHERE tenant_id = $1")
            .bind(tenant)
            .execute(&admin)
            .await
            .expect("clean counter fixture rows");
    }
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos()
}
