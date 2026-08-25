#![cfg(feature = "integration")]

use myelin_config::{Mode, MyelinConfig};
use myelin_events::Timestamp;
use myelin_identity::{
    DataRole, ObjectId, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RelName,
    RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::TupleStore;
use myelin_issues::{
    issues_hot_tables, issues_migrations, CreateIssue, IssueAuthorizationBinding, IssueAuthorizer,
    IssuePageRequest, IssuePermission, IssueStoreError, IssueTupleWriter, IssueViewRebuildOutcome,
    PgIssueStore, VisibleIssues,
};
use myelin_storage::{
    all_durable_migrations, DurableTupleBacking, KmsEngine, PgBootstrap, TenantScope,
};
use myelin_substrate::HotTables;
use myelin_tenancy::{Region, TenantId};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
struct FrozenFilterAuthorizer;

impl IssueAuthorizer for FrozenFilterAuthorizer {
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

#[derive(Clone)]
struct TupleWriter(TupleStore);

impl IssueTupleWriter for TupleWriter {
    fn ensure_parent_project<'a>(
        &'a self,
        scope: &'a TenantScope,
        actor: &'a Principal,
        binding: &'a IssueAuthorizationBinding,
    ) -> Pin<Box<dyn Future<Output = Result<Zookie, String>> + Send + 'a>> {
        let tuples = self.0.clone();
        let scope = scope.clone();
        let actor = actor.clone();
        let delta = TupleDelta::Add(RelationTuple {
            object: ObjectId(binding.issue_object.clone()),
            relation: RelName("parent_project".into()),
            subject: PrincipalId(binding.project_userset.clone()),
            caveat: None,
        });
        Box::pin(async move {
            write(&tuples, &scope, &actor, vec![delta])
                .await
                .map_err(|error| error.to_string())
        })
    }
}

fn principal(tenant: &str, region: &str, id: &str) -> Principal {
    Principal::new(
        TenantId::from_token(tenant),
        Region::new(region),
        PrincipalId(id.into()),
        PrincipalKind::Service,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn tuple(op: bool, object: impl Into<String>, relation: &str, subject: &str) -> TupleDelta {
    let edge = RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    };
    if op {
        TupleDelta::Add(edge)
    } else {
        TupleDelta::Remove(edge)
    }
}

async fn write(
    tuples: &TupleStore,
    scope: &TenantScope,
    actor: &Principal,
    deltas: Vec<TupleDelta>,
) -> Result<Zookie, myelin_identity_service::WriteError> {
    write_until(tuples, scope, actor, deltas, None).await
}

async fn write_until(
    tuples: &TupleStore,
    scope: &TenantScope,
    actor: &Principal,
    deltas: Vec<TupleDelta>,
    expires_at: Option<Timestamp>,
) -> Result<Zookie, myelin_identity_service::WriteError> {
    let tuples = tuples.clone();
    let scope = scope.clone();
    let actor = actor.clone();
    tokio::task::spawn_blocking(move || {
        tuples.write_tuples(
            &scope,
            &actor,
            &deltas,
            None,
            expires_at,
            Timestamp("2026-07-18T00:00:00Z".into()),
        )
    })
    .await
    .expect("tuple writer joins")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_effective_filter_survives_restart_revocation_and_rebuild_races() {
    let mut config = MyelinConfig::from_env(Mode::DevDefaults).expect("dev config");
    config.region = "fr-par".into();
    let bootstrap = PgBootstrap::connect(config.clone(), 8)
        .await
        .expect("live PostgreSQL");
    bootstrap.migrate_foundation().await.expect("foundation");
    bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("storage 0067-0069 precedes Issues");
    bootstrap
        .migrate(&issues_migrations(), &issues_hot_tables())
        .await
        .expect("Issues projection table and strict triggers");
    let provider = bootstrap.into_runtime().await.expect("runtime provider");
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(
            &std::env::var("DATABASE_MIGRATION_URL").unwrap_or_else(|_| {
                "postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin".into()
            }),
        )
        .await
        .unwrap();

    let suffix = format!("{}_{}", std::process::id(), now_nanos());
    let tenant = format!("iss_s8_{suffix}");
    let other_tenant = format!("iss_s8_other_{suffix}");
    let alice = principal(&tenant, "fr-par", &format!("p:alice:{suffix}"));
    let bob = principal(&tenant, "fr-par", &format!("p:bob:{suffix}"));
    let outsider = principal(&tenant, "fr-par", &format!("p:outsider:{suffix}"));
    let expired_project_reader = principal(
        &tenant,
        "fr-par",
        &format!("p:expired-project-reader:{suffix}"),
    );
    let expired_inherited_reader = principal(
        &tenant,
        "fr-par",
        &format!("p:expired-inherited-reader:{suffix}"),
    );
    let expired_issue_grantee = principal(
        &tenant,
        "fr-par",
        &format!("p:expired-issue-grantee:{suffix}"),
    );
    let worker = principal(&tenant, "fr-par", &format!("svc:worker:{suffix}"));
    let scope = TenantScope::from_verified_token(&alice, alice.region.clone());
    let tuples = TupleStore::with_pg(
        DurableTupleBacking::new(provider.clone()),
        tokio::runtime::Handle::current(),
    );
    let writer = TupleWriter(tuples.clone());
    let kms = Arc::new(KmsEngine::new());
    let store = PgIssueStore::new(provider.clone(), kms.clone(), FrozenFilterAuthorizer);
    let project = "11111111-1111-1111-1111-111111111111";

    write(
        &tuples,
        &scope,
        &alice,
        vec![
            tuple(
                true,
                format!("project:{project}"),
                "reader",
                &alice.principal_id.0,
            ),
            tuple(
                true,
                format!("project:{project}"),
                "parent_team",
                "team:eng#view",
            ),
            tuple(true, "team:eng", "member", &bob.principal_id.0),
            tuple(
                true,
                format!("project:{project}"),
                "parent_team",
                &outsider.principal_id.0,
            ),
            tuple(
                true,
                format!("project:{project}"),
                "parent_team",
                "team:bad#member",
            ),
            tuple(true, "team:bad", "member", &outsider.principal_id.0),
        ],
    )
    .await
    .expect("seed project viewers");
    write(
        &tuples,
        &scope,
        &alice,
        vec![
            tuple(
                true,
                "team:temporary-project-viewers",
                "member",
                &expired_inherited_reader.principal_id.0,
            ),
            tuple(
                true,
                "team:temporary-issue-grantees",
                "member",
                &expired_issue_grantee.principal_id.0,
            ),
        ],
    )
    .await
    .expect("seed the durable side of temporary userset grants");
    write_until(
        &tuples,
        &scope,
        &alice,
        vec![
            tuple(
                true,
                format!("project:{project}"),
                "reader",
                &expired_project_reader.principal_id.0,
            ),
            tuple(
                true,
                format!("project:{project}"),
                "parent_team",
                "team:temporary-project-viewers#view",
            ),
        ],
        Some(Timestamp("2026-07-18T00:01:00Z".into())),
    )
    .await
    .expect("record temporary direct and inherited project grants");

    let staged = store
        .create(
            &alice,
            CreateIssue {
                project_id: project.into(),
                type_id: "22222222-2222-2222-2222-222222222222".into(),
                prefix: "S8D".into(),
                title: "durable effective visibility".into(),
            },
        )
        .await
        .expect("stage issue");
    store
        .reconcile_authorization(&worker, &staged.id, &writer)
        .await
        .expect("activate normal production issue");
    write_until(
        &tuples,
        &scope,
        &alice,
        vec![tuple(
            true,
            format!("issue:{}", staged.id),
            "confidential_grant",
            "team:temporary-issue-grantees#member",
        )],
        Some(Timestamp("2026-07-18T00:01:00Z".into())),
    )
    .await
    .expect("record a temporary issue-specific userset grant");

    assert!(matches!(
        store
            .list(&alice, IssuePageRequest::new(20, None).unwrap())
            .await,
        Err(IssueStoreError::AuthorizationUnavailable(_))
    ));
    assert!(matches!(
        store
            .view_by_keys(&alice, std::slice::from_ref(&staged.key))
            .await,
        Err(IssueStoreError::AuthorizationUnavailable(_))
    ));
    let built = store
        .rebuild_effective_issue_view(&worker)
        .await
        .expect("worker rebuilds pending projection");
    let IssueViewRebuildOutcome::Published(built) = built else {
        panic!("an uncontended rebuild publishes its staged revision");
    };
    assert!(built.projected_memberships >= 2);
    sqlx::query(
        "UPDATE authz_projection_state SET format_version = 0 \
         WHERE tenant_id = $1 AND region = 'fr-par' AND projection = 'issue:view'",
    )
    .bind(&tenant)
    .execute(&admin)
    .await
    .unwrap();
    assert_eq!(
        store.effective_issue_view_lag(&worker).await.unwrap(),
        Some(1)
    );
    assert!(matches!(
        store
            .list(&alice, IssuePageRequest::new(20, None).unwrap())
            .await,
        Err(IssueStoreError::AuthorizationUnavailable(_))
    ));
    store
        .rebuild_effective_issue_view(&worker)
        .await
        .expect("a current worker replaces a legacy-format projection");
    assert_eq!(
        store
            .list(&alice, IssuePageRequest::new(20, None).unwrap())
            .await
            .unwrap()
            .items[0]
            .id,
        staged.id
    );
    let cards = store
        .view_by_keys(
            &alice,
            &[staged.key.clone(), "S8D-999999".into(), staged.key.clone()],
        )
        .await
        .expect("the card viewport uses the ready effective projection");
    assert_eq!(cards.len(), 1, "missing keys and duplicates add no cards");
    assert_eq!(cards[0].id, staged.id);
    assert_eq!(cards[0].title, "durable effective visibility");
    assert_eq!(
        store
            .list(&bob, IssuePageRequest::new(20, None).unwrap())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert!(store
        .list(&outsider, IssuePageRequest::new(20, None).unwrap())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(store
        .view_by_keys(&outsider, std::slice::from_ref(&staged.key))
        .await
        .unwrap()
        .is_empty());
    for (principal, grant) in [
        (&expired_project_reader, "direct project grant"),
        (&expired_inherited_reader, "inherited project grant"),
        (&expired_issue_grantee, "issue-specific userset grant"),
    ] {
        assert!(
            store
                .list(principal, IssuePageRequest::new(20, None).unwrap())
                .await
                .unwrap()
                .items
                .is_empty(),
            "an expired {grant} cannot leave the issue visible"
        );
    }

    drop(store);
    let restarted_bootstrap = PgBootstrap::connect(config, 4)
        .await
        .expect("restart connection");
    let restarted_provider = restarted_bootstrap
        .into_runtime()
        .await
        .expect("restart runtime");
    let restarted = PgIssueStore::new(restarted_provider, kms, FrozenFilterAuthorizer);
    assert_eq!(
        restarted
            .list(&alice, IssuePageRequest::new(20, None).unwrap())
            .await
            .unwrap()
            .items
            .len(),
        1
    );

    let issue_object = format!("issue:{}", staged.id);
    write(
        &tuples,
        &scope,
        &alice,
        vec![
            tuple(true, &issue_object, "confidential", &alice.principal_id.0),
            tuple(true, &issue_object, "confidential", &bob.principal_id.0),
            tuple(
                true,
                &issue_object,
                "confidential_grant",
                &alice.principal_id.0,
            ),
        ],
    )
    .await
    .unwrap();
    assert!(matches!(
        restarted
            .list(&alice, IssuePageRequest::new(20, None).unwrap())
            .await,
        Err(IssueStoreError::AuthorizationUnavailable(_))
    ));
    restarted
        .rebuild_effective_issue_view(&worker)
        .await
        .unwrap();
    assert_eq!(
        restarted
            .list(&alice, IssuePageRequest::new(20, None).unwrap())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert!(restarted
        .list(&bob, IssuePageRequest::new(20, None).unwrap())
        .await
        .unwrap()
        .items
        .is_empty());

    write(
        &tuples,
        &scope,
        &alice,
        vec![tuple(
            false,
            &issue_object,
            "confidential_grant",
            &alice.principal_id.0,
        )],
    )
    .await
    .unwrap();
    assert!(matches!(
        restarted
            .list(&alice, IssuePageRequest::new(20, None).unwrap())
            .await,
        Err(IssueStoreError::AuthorizationUnavailable(_))
    ));
    restarted
        .rebuild_effective_issue_view(&worker)
        .await
        .unwrap();
    let forbidden = restarted
        .list(&alice, IssuePageRequest::new(20, None).unwrap())
        .await
        .unwrap();
    assert!(forbidden.items.is_empty());
    assert!(forbidden.next_cursor.is_none());

    write(
        &tuples,
        &scope,
        &alice,
        vec![tuple(
            true,
            &issue_object,
            "confidential_grant",
            &alice.principal_id.0,
        )],
    )
    .await
    .unwrap();
    let snapshot_staged = Arc::new(tokio::sync::Notify::new());
    let rebuild_store = restarted.clone();
    let rebuild_worker = worker.clone();
    let rebuild_snapshot = snapshot_staged.clone();
    let rebuilding = tokio::spawn(async move {
        rebuild_store
            .rebuild_effective_issue_view_paused_before_publish_for_test(
                &rebuild_worker,
                Duration::from_secs(2),
                rebuild_snapshot,
            )
            .await
    });
    snapshot_staged.notified().await;
    tokio::time::timeout(
        Duration::from_secs(1),
        write(
            &tuples,
            &scope,
            &alice,
            vec![tuple(
                false,
                &issue_object,
                "confidential_grant",
                &alice.principal_id.0,
            )],
        ),
    )
    .await
    .expect("a visibility change does not wait behind snapshot computation")
    .expect("revoke the issue grant");
    assert!(matches!(
        rebuilding.await.unwrap().unwrap(),
        IssueViewRebuildOutcome::Superseded {
            attempted_revision,
            current_revision,
        } if current_revision > attempted_revision
    ));
    assert!(matches!(
        restarted
            .list(&alice, IssuePageRequest::new(20, None).unwrap())
            .await,
        Err(IssueStoreError::AuthorizationUnavailable(_))
    ));
    restarted
        .rebuild_effective_issue_view(&worker)
        .await
        .unwrap();
    assert!(restarted
        .list(&alice, IssuePageRequest::new(20, None).unwrap())
        .await
        .unwrap()
        .items
        .is_empty());

    let other = principal(&other_tenant, "fr-par", &alice.principal_id.0);
    let other_worker = principal(&other_tenant, "fr-par", "svc:worker:other");
    restarted
        .rebuild_effective_issue_view(&other_worker)
        .await
        .unwrap();
    assert!(restarted
        .list(&other, IssuePageRequest::new(20, None).unwrap())
        .await
        .unwrap()
        .items
        .is_empty());
    let wrong_region = principal(&tenant, "us-east", &alice.principal_id.0);
    assert_eq!(
        restarted
            .list(&wrong_region, IssuePageRequest::new(20, None).unwrap())
            .await,
        Err(IssueStoreError::NotFound)
    );

    for statement in [
        "DELETE FROM issue_view_subject WHERE tenant_id = ANY($1)",
        "DELETE FROM issue_authz_visible WHERE tenant_id = ANY($1)",
        "DELETE FROM issue_authz_binding WHERE tenant_id = ANY($1)",
        "DELETE FROM issue WHERE tenant_id = ANY($1)",
        "DELETE FROM prefix_counter WHERE tenant_id = ANY($1)",
        "DELETE FROM rebac_tuple WHERE tenant_id = ANY($1)",
        "DELETE FROM authz_projection_state WHERE tenant_id = ANY($1)",
        "DELETE FROM outbox WHERE envelope->>'tenant' = ANY($1)",
    ] {
        sqlx::query(statement)
            .bind(vec![tenant.clone(), other_tenant.clone()])
            .execute(&admin)
            .await
            .unwrap();
    }
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos()
}
