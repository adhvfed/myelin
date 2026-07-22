//! Live PostgreSQL proof that the deployable Issues authorization worker boots, reconciles, drains,
//! and restarts without a duplicate activation/event.
#![cfg(feature = "integration")]

use myelin_config::{Mode, MyelinConfig};
use myelin_edge::{
    spawn_issue_authorization_reconciler, IssueReconciliationConfig, StoreBackedIssueAuthorizer,
};
use myelin_events::Timestamp;
use myelin_identity::{
    DataRole, FragmentAdmit, ObjectId, Principal, PrincipalId, PrincipalKind, PrincipalStatus,
    RelName, RelationTuple, TupleDelta,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_issues::events::ISSUE_CREATED;
use myelin_issues::{
    issues_hot_tables, issues_migrations, CreateIssue, IssueStoreError, PgIssueStore,
};
use myelin_storage::{DurableTupleBacking, KmsEngine, PgBootstrap, TenantScope};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;
use std::time::Duration;

fn admin_url() -> String {
    std::env::var("DATABASE_MIGRATION_URL")
        .expect("the caller checked DATABASE_MIGRATION_URL before live setup")
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

fn proposal() -> CreateIssue {
    CreateIssue {
        project_id: "11111111-1111-1111-1111-111111111111".into(),
        type_id: "22222222-2222-2222-2222-222222222222".into(),
        prefix: "OPS".into(),
        title: "restart-safe authorization".into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_worker_boots_reconciles_and_restarts_idempotently() {
    if std::env::var("DATABASE_URL").is_err() || std::env::var("DATABASE_MIGRATION_URL").is_err() {
        eprintln!(
            "SKIP production_worker_boots_reconciles_and_restarts_idempotently: split database URLs unset"
        );
        return;
    }

    let mut config = MyelinConfig::from_env(Mode::DevDefaults).expect("valid live config");
    config.region = "fr-par".into();
    let bootstrap = PgBootstrap::connect(config, 8)
        .await
        .expect("connect split migration/runtime roles");
    bootstrap
        .migrate_foundation()
        .await
        .expect("migrate outbox and Identity tuple foundation");
    bootstrap
        .migrate(&issues_migrations(), &issues_hot_tables())
        .await
        .expect("migrate the Issues authorization saga");
    let provider = bootstrap.into_runtime().await.expect("runtime handoff");

    let tuples = TupleStore::with_pg(
        DurableTupleBacking::new(provider.clone()),
        tokio::runtime::Handle::current(),
    );
    let identity = StoreBackedCheck::new(tuples.clone());
    for verdict in identity.admit_issue_fragment() {
        assert!(matches!(verdict, FragmentAdmit::Admitted { .. }));
    }

    let suffix = format!("{}_{}", std::process::id(), now_nanos());
    let tenant = format!("edge_issue_reconcile_{suffix}");
    let creator = principal(&tenant, &format!("service:creator:{suffix}"));
    let scope = TenantScope::from_verified_token(&creator, creator.region.clone());
    let project_reader = TupleDelta::Add(RelationTuple {
        object: ObjectId("project:11111111-1111-1111-1111-111111111111".into()),
        relation: RelName("reader".into()),
        subject: creator.principal_id.clone(),
        caveat: None,
    });
    let tuples_for_seed = tuples.clone();
    let scope_for_seed = scope.clone();
    let creator_for_seed = creator.clone();
    tokio::task::spawn_blocking(move || {
        tuples_for_seed.write_tuples(
            &scope_for_seed,
            &creator_for_seed,
            &[project_reader],
            None,
            None,
            Timestamp("2026-07-18T00:00:00Z".into()),
        )
    })
    .await
    .unwrap()
    .expect("seed project reader");

    let kms = Arc::new(KmsEngine::new());
    let authorizer = StoreBackedIssueAuthorizer::new(identity.clone());
    let store = Arc::new(PgIssueStore::new(provider.clone(), kms.clone(), authorizer));
    let staged = store
        .create(&creator, proposal())
        .await
        .expect("stage issue");
    assert_eq!(
        store.view(&creator, &staged.id).await,
        Err(IssueStoreError::NotFound),
        "pending issue is invisible before the worker runs"
    );

    let worker_config = IssueReconciliationConfig::new(
        vec![TenantId::from_token(tenant.clone())],
        Region::new("fr-par"),
        10,
        Duration::from_millis(20),
        Duration::from_millis(100),
    )
    .unwrap();
    let first = spawn_issue_authorization_reconciler(
        store.clone(),
        identity.clone(),
        worker_config.clone(),
    );
    wait_until_visible(&store, &creator, &staged.id).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while first.metrics().snapshot().newly_activated == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first sweep publishes its activation metric");
    assert_eq!(first.metrics().snapshot().newly_activated, 1);
    first.shutdown().await.expect("first worker drains");

    // A fresh store + worker models process restart. The durable pending/active state is the oracle;
    // the second immediate scan emits no second `issue.created` event.
    let restarted = Arc::new(PgIssueStore::new(
        provider.clone(),
        kms,
        StoreBackedIssueAuthorizer::new(identity.clone()),
    ));
    let second = spawn_issue_authorization_reconciler(restarted, identity, worker_config);
    tokio::time::timeout(Duration::from_secs(2), async {
        while second.metrics().snapshot().sweeps == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("restart immediate sweep");
    assert_eq!(second.metrics().snapshot().newly_activated, 0);
    second.shutdown().await.expect("restarted worker drains");

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("admin inspection connection");
    let created_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE envelope->>'tenant' = $1 \
         AND envelope->>'type_' = $2 AND aggregate = $3",
    )
    .bind(&tenant)
    .bind(ISSUE_CREATED)
    .bind(format!("issue:{}", staged.id))
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(created_count, 1, "restart did not duplicate activation");

    for statement in [
        "DELETE FROM issue_authz_binding WHERE tenant_id = $1",
        "DELETE FROM issue WHERE tenant_id = $1",
        "DELETE FROM prefix_counter WHERE tenant_id = $1",
        "DELETE FROM rebac_tuple WHERE tenant_id = $1",
        "DELETE FROM outbox WHERE envelope->>'tenant' = $1",
    ] {
        sqlx::query(statement)
            .bind(&tenant)
            .execute(&admin)
            .await
            .unwrap();
    }
}

async fn wait_until_visible(
    store: &PgIssueStore<StoreBackedIssueAuthorizer>,
    creator: &Principal,
    issue_id: &str,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if store.view(creator, issue_id).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("worker activates the pending issue");
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos()
}
