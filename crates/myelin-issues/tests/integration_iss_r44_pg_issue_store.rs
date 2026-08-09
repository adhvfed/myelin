#![cfg(feature = "integration")]

use myelin_config::{Mode, MyelinConfig};
use myelin_events::{EventEnvelope, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, DataRole, Decision, FragmentAdmit, IdentityService, ObjectId,
    Permission, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RelName, RelationTuple,
    TupleDelta, Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_issues::events::{ISSUE_AUTHORIZATION_REQUESTED, ISSUE_CREATED};
use myelin_issues::{
    issues_hot_tables, issues_migrations, CreateIssue, ImportIssue, IssueAuthorizationBinding,
    IssueAuthorizer, IssuePageRequest, IssuePermission, IssueStoreError, IssueTupleWriter,
    PgIssueStore, SourceSystem, VisibleIssues,
};
use myelin_storage::{
    all_durable_migrations, DurableTupleBacking, KmsEngine, PgBootstrap, TenantScope,
};
use myelin_substrate::HotTables;
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use sqlx::Row;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

fn admin_url() -> String {
    std::env::var("DATABASE_MIGRATION_URL")
        .unwrap_or_else(|_| "postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin".into())
}

#[derive(Clone)]
struct RebacAuthorizer {
    identity: Arc<StoreBackedCheck>,
}

#[derive(Clone)]
struct AllowAuthorizer;

impl IssueAuthorizer for AllowAuthorizer {
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
        Ok(VisibleIssues::effective_issue_view_filter())
    }
}

impl RebacAuthorizer {
    fn allows(&self, principal: &Principal, permission: &str, object: String) -> bool {
        matches!(
            self.identity.check(
                principal,
                &Permission(permission.into()),
                &ArtifactRef(object),
                &Consistency {
                    at_least: Zookie(String::new()),
                    mode: ConsistencyMode::Strong,
                },
                None,
            ),
            Ok(Decision::Allow)
        )
    }
}

impl IssueAuthorizer for RebacAuthorizer {
    fn may_create(&self, principal: &Principal, project_id: &str) -> bool {
        self.allows(principal, "view", format!("project:{project_id}"))
    }

    fn may_access(
        &self,
        principal: &Principal,
        issue_id: &str,
        permission: IssuePermission,
    ) -> bool {
        let permission = match permission {
            IssuePermission::View => "view",
            IssuePermission::Close => "manage",
        };
        self.allows(principal, permission, format!("issue:{issue_id}"))
    }

    fn visible_issues(&self, _principal: &Principal) -> Result<VisibleIssues, String> {
        Ok(VisibleIssues::effective_issue_view_filter())
    }
}

#[derive(Clone)]
struct DurableIdentityWriter {
    tuples: TupleStore,
}

impl IssueTupleWriter for DurableIdentityWriter {
    fn ensure_parent_project<'a>(
        &'a self,
        scope: &'a TenantScope,
        actor: &'a Principal,
        binding: &'a IssueAuthorizationBinding,
    ) -> Pin<Box<dyn Future<Output = Result<Zookie, String>> + Send + 'a>> {
        let tuples = self.tuples.clone();
        let scope = scope.clone();
        let actor = actor.clone();
        let delta = TupleDelta::Add(RelationTuple {
            object: ObjectId(binding.issue_object.clone()),
            relation: RelName(binding.relation.clone()),
            subject: PrincipalId(binding.project_userset.clone()),
            caveat: None,
        });
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                tuples.write_tuples(
                    &scope,
                    &actor,
                    &[delta],
                    None,
                    None,
                    Timestamp("2026-07-18T00:00:00Z".into()),
                )
            })
            .await
            .map_err(|_| "join failed".to_string())?
            .map_err(|error| error.to_string())
        })
    }
}

struct SecretFailureWriter;

impl IssueTupleWriter for SecretFailureWriter {
    fn ensure_parent_project<'a>(
        &'a self,
        _scope: &'a TenantScope,
        _actor: &'a Principal,
        _binding: &'a IssueAuthorizationBinding,
    ) -> Pin<Box<dyn Future<Output = Result<Zookie, String>> + Send + 'a>> {
        Box::pin(async { Err("rpc bearer=top-secret customer@example.test".into()) })
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

fn proposal(prefix: &str, title: &str) -> CreateIssue {
    CreateIssue {
        project_id: "11111111-1111-1111-1111-111111111111".into(),
        type_id: "22222222-2222-2222-2222-222222222222".into(),
        prefix: prefix.into(),
        title: title.into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saga_is_fail_closed_rollback_safe_restartable_idempotent_and_concurrent() {
    let mut config = MyelinConfig::from_env(Mode::DevDefaults).expect("dev config");
    config.region = "fr-par".into();
    let bootstrap = PgBootstrap::connect(config, 8)
        .await
        .expect("validate split database roles (have you run `fed test:backend`?)");
    bootstrap
        .migrate_foundation()
        .await
        .expect("apply shared outbox and durable Identity tuple foundation");
    bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("apply durable projection substrate");
    bootstrap
        .migrate(&issues_migrations(), &issues_hot_tables())
        .await
        .expect("apply Issues saga schema");
    let provider = bootstrap.into_runtime().await.expect("RLS runtime handoff");

    let tuples = TupleStore::with_pg(
        DurableTupleBacking::new(provider.clone()),
        tokio::runtime::Handle::current(),
    );
    let identity = Arc::new(StoreBackedCheck::new(tuples.clone()));
    for verdict in identity.admit_issue_fragment() {
        assert!(matches!(verdict, FragmentAdmit::Admitted { .. }));
    }

    let suffix = format!("{}_{}", std::process::id(), now_nanos());
    let tenant = format!("r44_saga_{suffix}");
    let creator = principal(&tenant, &format!("svc:creator:{suffix}"));
    let worker = principal(&tenant, &format!("svc:reconciler:{suffix}"));
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
    .expect("seed creator as project reader");

    let authorizer = RebacAuthorizer {
        identity: identity.clone(),
    };
    let kms = Arc::new(KmsEngine::new());
    let store = PgIssueStore::new(provider.clone(), kms.clone(), authorizer.clone());
    let writer = DurableIdentityWriter {
        tuples: tuples.clone(),
    };
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("admin inspection connection");

    let import_job_id = sqlx::types::Uuid::new_v4().to_string();
    let imported = ImportIssue {
        import_job_id: import_job_id.clone(),
        source: SourceSystem::GitHub,
        source_id: "github:acme/platform#41".into(),
        issue: proposal("IMP", "imported exactly once"),
    };
    assert!(store
        .import_issue_then_abort_for_test(&creator, imported.clone())
        .await
        .is_err());
    for (table, count) in [
        (
            "issue",
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM issue WHERE tenant_id = $1 AND prefix = 'IMP'",
            )
            .bind(&tenant)
            .fetch_one(&admin)
            .await
            .unwrap(),
        ),
        (
            "import map",
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM import_map WHERE tenant_id = $1 AND import_job = $2::uuid",
            )
            .bind(&tenant)
            .bind(&import_job_id)
            .fetch_one(&admin)
            .await
            .unwrap(),
        ),
    ] {
        assert_eq!(count, 0, "an aborted import left no {table} state");
    }

    let first_import = store
        .import_issue(&creator, imported.clone())
        .await
        .expect("the import creates its first source record");
    assert!(first_import.created);
    let resumed_import = store
        .import_issue(&creator, imported)
        .await
        .expect("resuming the same source record returns its original issue");
    assert!(!resumed_import.created);
    assert_eq!(resumed_import.issue, first_import.issue);

    let map = sqlx::query(
        "SELECT status, source, source_id, myelin_id::text AS myelin_id \
         FROM import_map WHERE tenant_id = $1 AND import_job = $2::uuid",
    )
    .bind(&tenant)
    .bind(&import_job_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(map.get::<String, _>("status"), "created");
    assert_eq!(map.get::<String, _>("source"), "github");
    assert_eq!(map.get::<String, _>("source_id"), "github:acme/platform#41");
    assert_eq!(map.get::<String, _>("myelin_id"), first_import.issue.id);
    let imported_issue_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM issue WHERE tenant_id = $1 AND prefix = 'IMP'")
            .bind(&tenant)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(imported_issue_count, 1);
    let imported_request_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE envelope->>'tenant' = $1 \
         AND envelope->>'type_' = $2 AND aggregate = $3",
    )
    .bind(&tenant)
    .bind(ISSUE_AUTHORIZATION_REQUESTED)
    .bind(format!("issue:{}", first_import.issue.id))
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(imported_request_count, 1);
    store
        .reconcile_authorization(&worker, &first_import.issue.id, &writer)
        .await
        .expect("the ordinary authorization saga activates an imported issue");

    let raced_import = ImportIssue {
        import_job_id: import_job_id.clone(),
        source: SourceSystem::GitHub,
        source_id: "github:acme/platform#42".into(),
        issue: proposal("IRC", "concurrent import converges"),
    };
    let race_store = PgIssueStore::new(provider.clone(), kms.clone(), AllowAuthorizer);
    let mut import_joins = Vec::new();
    for _ in 0..8 {
        let store = race_store.clone();
        let creator = creator.clone();
        let raced_import = raced_import.clone();
        import_joins.push(tokio::spawn(async move {
            store.import_issue(&creator, raced_import).await
        }));
    }
    let mut imported_receipts = Vec::new();
    let mut import_winners = 0;
    for join in import_joins {
        let outcome = join.await.unwrap().expect("concurrent import retry");
        import_winners += usize::from(outcome.created);
        imported_receipts.push(outcome.issue);
    }
    assert_eq!(import_winners, 1, "one source record creates one issue");
    assert!(
        imported_receipts
            .iter()
            .all(|receipt| receipt == &imported_receipts[0]),
        "every concurrent retry receives the original creation receipt"
    );
    store
        .reconcile_authorization(&worker, &imported_receipts[0].id, &writer)
        .await
        .expect("the concurrent import winner follows the ordinary authorization saga");
    let raced_import_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM issue WHERE tenant_id = $1 AND prefix = 'IRC'")
            .bind(&tenant)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(raced_import_count, 1);

    assert!(store
        .create_then_abort_for_test(&creator, proposal("RBK", "must roll back"))
        .await
        .is_err());
    for (table, count) in [
        (
            "issue",
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM issue WHERE tenant_id = $1 AND prefix = 'RBK'",
            )
            .bind(&tenant)
            .fetch_one(&admin)
            .await
            .unwrap(),
        ),
        (
            "binding",
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM issue_authz_binding b JOIN issue i \
                 ON i.tenant_id = b.tenant_id AND i.id = b.issue_id \
                 WHERE b.tenant_id = $1 AND i.prefix = 'RBK'",
            )
            .bind(&tenant)
            .fetch_one(&admin)
            .await
            .unwrap(),
        ),
        (
            "prefix",
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM prefix_counter WHERE tenant_id = $1 AND prefix = 'RBK'",
            )
            .bind(&tenant)
            .fetch_one(&admin)
            .await
            .unwrap(),
        ),
    ] {
        assert_eq!(count, 0, "rollback left no {table} state");
    }

    let staged = store
        .create(&creator, proposal("ENG", "private staged title"))
        .await
        .expect("stage invisible issue");
    assert_eq!(
        store.view(&creator, &staged.id).await,
        Err(IssueStoreError::NotFound),
        "no tuple means ReBAC denies the pending object"
    );
    assert!(matches!(
        store
            .list(&creator, IssuePageRequest::new(20, None).unwrap())
            .await,
        Err(IssueStoreError::AuthorizationUnavailable(_))
    ));
    assert_eq!(
        store.pending_authorization_ids(&worker, 10).await.unwrap(),
        vec![staged.id.clone()],
        "a restart scanner can recover every staged pending binding"
    );

    assert!(matches!(
        store
            .reconcile_authorization(&worker, &staged.id, &SecretFailureWriter)
            .await,
        Err(IssueStoreError::AuthorizationUnavailable(_))
    ));
    let last_error: String = sqlx::query_scalar(
        "SELECT last_error FROM issue_authz_binding WHERE tenant_id = $1 AND region = 'fr-par' \
         AND issue_id = $2::uuid",
    )
    .bind(&tenant)
    .bind(&staged.id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(last_error, "identity_tuple_write_failed");

    assert!(store
        .reconcile_then_crash_for_test(&worker, &staged.id, &writer)
        .await
        .is_err());
    assert!(authorizer.allows(&creator, "view", format!("issue:{}", staged.id)));
    assert_eq!(
        store.view(&creator, &staged.id).await,
        Err(IssueStoreError::NotFound)
    );
    assert!(matches!(
        store
            .list(&creator, IssuePageRequest::new(20, None).unwrap())
            .await,
        Err(IssueStoreError::AuthorizationUnavailable(_))
    ));

    let restarted = PgIssueStore::new(provider.clone(), kms, authorizer.clone());
    let activated = restarted
        .reconcile_authorization(&worker, &staged.id, &writer)
        .await
        .expect("retry activates after idempotent tuple add");
    assert!(activated.newly_activated);
    assert_eq!(activated.issue.title, "private staged title");
    assert!(
        !restarted
            .reconcile_authorization(&worker, &staged.id, &writer)
            .await
            .unwrap()
            .newly_activated
    );
    restarted
        .rebuild_effective_issue_view(&worker)
        .await
        .expect("publish effective list projection after activation");
    assert_eq!(
        restarted.view(&creator, &staged.id).await.unwrap().id,
        staged.id
    );

    let binding = sqlx::query(
        "SELECT request_event_id, created_event_id FROM issue_authz_binding \
         WHERE tenant_id = $1 AND region = 'fr-par' AND issue_id = $2::uuid",
    )
    .bind(&tenant)
    .bind(&staged.id)
    .fetch_one(&admin)
    .await
    .unwrap();
    let request_id: String = binding.get("request_event_id");
    let created_id: String = binding.get("created_event_id");
    assert!(is_canonical_ulid(&request_id));
    assert!(is_canonical_ulid(&created_id));
    assert_eq!(request_id, staged.authorization_request_event_id);
    let request: EventEnvelope = envelope(&admin, &request_id).await;
    let created: EventEnvelope = envelope(&admin, &created_id).await;
    assert_eq!(request.type_.0, ISSUE_AUTHORIZATION_REQUESTED);
    assert_eq!(created.type_.0, ISSUE_CREATED);
    assert_eq!(created.actor.0.principal_id, creator.principal_id);
    assert!(!created.contains_personal_data);
    assert!(created.pii_key_ref.is_none());
    assert_eq!(created.causation_id, Some(request.event_id));
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
    assert_eq!(created_count, 1);

    let raced = restarted
        .create(&creator, proposal("RCE", "concurrent bootstrap"))
        .await
        .unwrap();
    let mut joins = Vec::new();
    for _ in 0..8 {
        let store = restarted.clone();
        let writer = writer.clone();
        let worker = worker.clone();
        let id = raced.id.clone();
        joins.push(tokio::spawn(async move {
            store.reconcile_authorization(&worker, &id, &writer).await
        }));
    }
    let mut winners = 0;
    for join in joins {
        if let Ok(outcome) = join.await.unwrap() {
            winners += usize::from(outcome.newly_activated);
        }
    }
    assert_eq!(winners, 1);
    assert!(
        !restarted
            .reconcile_authorization(&worker, &raced.id, &writer)
            .await
            .unwrap()
            .newly_activated,
        "a scanner retry converges any transient concurrent Identity-write loser"
    );
    let tuple_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rebac_tuple WHERE tenant_id = $1 AND region = 'fr-par' \
         AND object_id = $2 AND relation = 'parent_project'",
    )
    .bind(&tenant)
    .bind(format!("issue:{}", raced.id))
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(tuple_count, 1);
    let race_created: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE envelope->>'tenant' = $1 \
         AND envelope->>'type_' = $2 AND aggregate = $3",
    )
    .bind(&tenant)
    .bind(ISSUE_CREATED)
    .bind(format!("issue:{}", raced.id))
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(race_created, 1);

    let legacy_id = sqlx::types::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO issue (tenant_id, region, id, key, prefix, type_id, type_rank, state, \
           state_category, reporter, project_id, rank, title, title_nonce, title_ciphertext, \
           created_by_principal, pii_key_ref, contains_personal_data, version) \
         SELECT tenant_id, region, $3, $4, 'LEG', type_id, type_rank, state, state_category, \
           reporter, project_id, $5, title, title_nonce, title_ciphertext, created_by_principal, \
           pii_key_ref, contains_personal_data, version \
         FROM issue WHERE tenant_id = $1 AND region = $2 AND id = $6::uuid",
    )
    .bind(&tenant)
    .bind("fr-par")
    .bind(legacy_id)
    .bind(format!("LEG-{suffix}"))
    .bind(format!("legacy|{suffix}"))
    .bind(&staged.id)
    .execute(&admin)
    .await
    .unwrap();
    restarted
        .rebuild_effective_issue_view(&worker)
        .await
        .expect("rebuild after issue-domain mutations");
    let listed = restarted
        .list(&creator, IssuePageRequest::new(100, None).unwrap())
        .await
        .unwrap();
    assert!(listed
        .items
        .iter()
        .all(|item| item.id != legacy_id.to_string()));
    assert!(!restarted
        .pending_authorization_ids(&worker, 100)
        .await
        .unwrap()
        .contains(&legacy_id.to_string()));

    for statement in [
        "DELETE FROM import_map WHERE tenant_id = $1",
        "DELETE FROM issue_authz_binding WHERE tenant_id = $1",
        "DELETE FROM issue WHERE tenant_id = $1",
        "DELETE FROM prefix_counter WHERE tenant_id = $1",
        "DELETE FROM rebac_tuple WHERE tenant_id = $1",
        "DELETE FROM issue_authz_visible WHERE tenant_id = $1",
        "DELETE FROM authz_projection_state WHERE tenant_id = $1",
        "DELETE FROM outbox_quarantine WHERE event_id IN (SELECT event_id FROM outbox WHERE envelope->>'tenant' = $1)",
        "DELETE FROM outbox WHERE envelope->>'tenant' = $1",
    ] {
        sqlx::query(statement)
            .bind(&tenant)
            .execute(&admin)
            .await
            .unwrap();
    }
}

async fn envelope(pool: &sqlx::PgPool, event_id: &str) -> EventEnvelope {
    let value: serde_json::Value =
        sqlx::query_scalar("SELECT envelope FROM outbox WHERE event_id = $1")
            .bind(event_id)
            .fetch_one(pool)
            .await
            .unwrap();
    serde_json::from_value(value).unwrap()
}

fn is_canonical_ulid(value: &str) -> bool {
    const CROCKFORD: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    value.len() == 26 && value.chars().all(|ch| CROCKFORD.contains(ch))
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos()
}
