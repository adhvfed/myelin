#![cfg(feature = "integration")]

use myelin_config::{Mode, MyelinConfig};
use myelin_events::{EventEnvelope, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, DataRole, Decision, FragmentAdmit, IdentityService, ObjectId,
    Permission, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RelName, RelationTuple,
    TupleDelta, Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_issues::events::{
    ISSUE_AUTHORIZATION_REQUESTED, ISSUE_CREATED, RELATION_CREATED, RELATION_REMOVED,
};
use myelin_issues::{
    issues_hot_tables, issues_migrations, CreateIssue, CreateIssueIntent, ImportIssue,
    IssueActorKind, IssueAuthorizationBinding, IssueAuthorizer, IssueLifecycleRel,
    IssuePageRequest, IssuePermission, IssueStoreError, IssueTupleWriter, PgIssueStore,
    SourceSystem, VisibleIssues,
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
            IssuePermission::Close | IssuePermission::ManageRelations => "manage",
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

    sqlx::query(
        "INSERT INTO import_map (tenant_id, region, import_job, source, source_id, \
           myelin_kind, status) \
         VALUES ($1, 'fr-par', $2::uuid, 'github', 'github:acme/platform#41', \
           'cycle', 'pending')",
    )
    .bind(&tenant)
    .bind(&import_job_id)
    .execute(&admin)
    .await
    .expect("another artifact kind may own the same external source identity");

    let first_import = store
        .import_issue(&creator, imported.clone())
        .await
        .expect("the import creates its first source record");
    assert!(first_import.created);
    let resumed_import = store
        .import_issue(&creator, imported.clone())
        .await
        .expect("resuming the same source record returns its original issue");
    assert!(!resumed_import.created);
    assert_eq!(resumed_import.issue, first_import.issue);
    let mut corrected_import = imported;
    corrected_import.issue.title = "corrected source title".into();
    assert!(matches!(
        store.import_issue(&creator, corrected_import).await,
        Err(IssueStoreError::Conflict(_))
    ));

    let map = sqlx::query(
        "SELECT status, source, source_id, request_hash, myelin_id::text AS myelin_id \
         FROM import_map WHERE tenant_id = $1 AND import_job = $2::uuid \
           AND myelin_kind = 'issue'",
    )
    .bind(&tenant)
    .bind(&import_job_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(map.get::<String, _>("status"), "created");
    assert_eq!(map.get::<String, _>("source"), "github");
    assert_eq!(map.get::<String, _>("source_id"), "github:acme/platform#41");
    assert!(map.get::<String, _>("request_hash").starts_with("blake3:"));
    assert_eq!(map.get::<String, _>("myelin_id"), first_import.issue.id);
    let mapped_kinds: Vec<String> = sqlx::query_scalar(
        "SELECT myelin_kind FROM import_map \
         WHERE tenant_id = $1 AND import_job = $2::uuid \
           AND source = 'github' AND source_id = 'github:acme/platform#41' \
         ORDER BY myelin_kind",
    )
    .bind(&tenant)
    .bind(&import_job_id)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(mapped_kinds, vec!["cycle".to_string(), "issue".to_string()]);
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

    let retry_key = "interactive-create-retry";
    assert!(race_store
        .create_idempotent_then_abort_for_test(
            &creator,
            &creator,
            proposal("IDM", "retry-safe interactive issue"),
            retry_key,
        )
        .await
        .is_err());
    let aborted_ledger_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM issue_create_idempotency \
         WHERE tenant_id = $1 AND request_hash ~ '^blake3:'",
    )
    .bind(&tenant)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        aborted_ledger_count, 0,
        "the failed transaction left no claim"
    );

    let first_create = race_store
        .create_idempotent(
            &creator,
            &creator,
            proposal("IDM", "retry-safe interactive issue"),
            retry_key,
        )
        .await
        .expect("the first interactive request creates an issue");
    assert!(first_create.created);
    let replayed_create = race_store
        .create_idempotent(
            &creator,
            &creator,
            proposal("IDM", "retry-safe interactive issue"),
            retry_key,
        )
        .await
        .expect("the retry returns its original issue");
    assert!(!replayed_create.created);
    assert_eq!(replayed_create.receipt, first_create.receipt);

    let original_defaults = proposal("DFT", "retry across changed project defaults");
    let defaulted_intent = CreateIssueIntent {
        project_id: original_defaults.project_id.clone(),
        type_id: None,
        prefix: None,
        title: original_defaults.title.clone(),
    };
    let first_defaulted = race_store
        .create_idempotent_from_intent(
            &creator,
            &creator,
            original_defaults.clone(),
            defaulted_intent.clone(),
            "create-with-project-defaults",
        )
        .await
        .expect("the original project defaults create one issue");
    let mut changed_defaults = original_defaults;
    changed_defaults.type_id = "33333333-3333-3333-3333-333333333333".into();
    let replayed_after_default_change = race_store
        .create_idempotent_from_intent(
            &creator,
            &creator,
            changed_defaults,
            defaulted_intent,
            "create-with-project-defaults",
        )
        .await
        .expect("a changed project default cannot strand the caller's receipt");
    assert!(first_defaulted.created);
    assert!(!replayed_after_default_change.created);
    assert_eq!(
        replayed_after_default_change.receipt, first_defaulted.receipt,
        "the durable caller intent, not today's defaults, identifies the request"
    );
    store
        .reconcile_authorization(&worker, &first_defaulted.receipt.id, &writer)
        .await
        .expect("the default-backed issue follows the ordinary authorization saga");

    assert!(matches!(
        race_store
            .create_idempotent(
                &creator,
                &creator,
                proposal("IDM", "same key but different work"),
                retry_key,
            )
            .await,
        Err(IssueStoreError::Conflict(_))
    ));
    let retry_safe_issue_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM issue WHERE tenant_id = $1 AND prefix = 'IDM'")
            .bind(&tenant)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(retry_safe_issue_count, 1);
    store
        .reconcile_authorization(&worker, &first_create.receipt.id, &writer)
        .await
        .expect("the retry-safe issue follows the ordinary authorization saga");

    let mut create_joins = Vec::new();
    for _ in 0..8 {
        let store = race_store.clone();
        let creator = creator.clone();
        create_joins.push(tokio::spawn(async move {
            store
                .create_idempotent(
                    &creator,
                    &creator,
                    proposal("IDC", "concurrent interactive retry"),
                    "concurrent-interactive-create",
                )
                .await
        }));
    }
    let mut create_receipts = Vec::new();
    let mut create_winners = 0;
    for join in create_joins {
        let outcome = join.await.unwrap().expect("concurrent interactive retry");
        create_winners += usize::from(outcome.created);
        create_receipts.push(outcome.receipt);
    }
    assert_eq!(create_winners, 1, "one caller intent creates one issue");
    assert!(
        create_receipts
            .iter()
            .all(|receipt| receipt == &create_receipts[0]),
        "every concurrent retry receives the original creation receipt"
    );
    let raced_create_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM issue WHERE tenant_id = $1 AND prefix = 'IDC'")
            .bind(&tenant)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(raced_create_count, 1);
    store
        .reconcile_authorization(&worker, &create_receipts[0].id, &writer)
        .await
        .expect("the concurrent retry winner follows the ordinary authorization saga");

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
    assert_eq!(activated.issue.creator_kind, IssueActorKind::Service);
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

    let target_ref = myelin_issues::issue_root_ref(&tenant, &raced.key).0;
    let relation = restarted
        .create_relation(&creator, &staged.id, &target_ref, IssueLifecycleRel::Blocks)
        .await
        .expect("a manager relates two visible issues");
    assert!(relation.created);
    assert_eq!(
        relation.relation.source_ref,
        myelin_issues::issue_root_ref(&tenant, &staged.key).0
    );
    assert_eq!(relation.relation.target_ref, target_ref);
    assert_eq!(relation.relation.relation, "blocks");
    assert_eq!(relation.relation.created_by, creator.principal_id.0);
    assert_eq!(relation.relation.creator_kind, IssueActorKind::Service);

    let retry = restarted
        .create_relation(&creator, &staged.id, &target_ref, IssueLifecycleRel::Blocks)
        .await
        .expect("retrying the same relation returns its durable identity");
    assert!(!retry.created);
    assert_eq!(retry.relation, relation.relation);
    assert_eq!(
        restarted
            .list_relations(&creator, &staged.id)
            .await
            .unwrap(),
        vec![relation.relation.clone()]
    );

    for event_type in [RELATION_CREATED, "refs.edge.created"] {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM outbox
              WHERE envelope->>'tenant' = $1 AND envelope->>'type_' = $2
                AND envelope->'payload'->>'relation_id' = $3",
        )
        .bind(&tenant)
        .bind(event_type)
        .bind(&relation.relation.id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            count, 1,
            "the idempotent source-of-truth write emits `{event_type}` once"
        );
    }

    let removed = restarted
        .remove_relation(&creator, &staged.id, &relation.relation.id)
        .await
        .expect("remove the dependency")
        .expect("the dependency existed");
    assert_eq!(removed, relation.relation);
    assert!(restarted
        .list_relations(&creator, &staged.id)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        restarted
            .remove_relation(&creator, &staged.id, &relation.relation.id)
            .await
            .expect("removal retries are safe"),
        None
    );
    for event_type in [RELATION_REMOVED, "refs.edge.removed"] {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM outbox
              WHERE envelope->>'tenant' = $1 AND envelope->>'type_' = $2
                AND envelope->'payload'->>'relation_id' = $3",
        )
        .bind(&tenant)
        .bind(event_type)
        .bind(&relation.relation.id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            count, 1,
            "the idempotent source-of-truth removal emits `{event_type}` once"
        );
    }

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
        "DELETE FROM issue_view_subject WHERE tenant_id = $1",
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
