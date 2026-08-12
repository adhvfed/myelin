#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::{EventEnvelope, IdMinter, Timestamp, Ulid};
use myelin_identity::{
    DataRole, ObjectId, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RelName,
    RelationTuple, TupleDelta, IDENTITY_PROJECT_CREATED,
};
use myelin_identity_service::{NewProject, PgProjectStore, ProjectError, TupleStore};
use myelin_storage::{all_durable_migrations, DurableTupleBacking, SubstrateProvider, TenantScope};
use myelin_substrate::HotTables;
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;
use std::sync::Arc;

fn test_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            config.database_url = database_url;
        }
    }
    config
}

fn admin_config(config: &MyelinConfig) -> MyelinConfig {
    let mut admin = config.clone();
    admin.database_url = admin
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    admin
}

fn unique() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn actor(tenant: &str, region: &str) -> Principal {
    Principal::new(
        TenantId::from_token(tenant),
        Region::new(region),
        PrincipalId("p:project-founder".into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn proposal(name: &str, prefix: &str, nonce: &str) -> NewProject {
    NewProject {
        name: name.into(),
        issue_prefix: prefix.into(),
        client_nonce: nonce.into(),
    }
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

struct FixedMinter(&'static str);

impl IdMinter for FixedMinter {
    fn mint(&self) -> Ulid {
        Ulid(self.0.into())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_creation_co_commits_its_creator_grant_and_retry_identity() {
    let config = test_config();
    let admin = SubstrateProvider::connect(admin_config(&config), 4)
        .await
        .expect("connect to the Postgres required by the project onboarding story");
    admin.migrate_foundation().await.unwrap();
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap();
    let app = SubstrateProvider::connect(config, 6)
        .await
        .expect("connect constrained runtime role");
    let region = app.config().region.clone();
    let tenant = format!("project-onboarding-{}", unique());
    let founder = actor(&tenant, &region);
    let event_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let tuples = TupleStore::with_pg(
        DurableTupleBacking::new(app.clone()),
        tokio::runtime::Handle::current(),
    );
    let store = PgProjectStore::with_minter(app, Arc::new(FixedMinter(event_id)));

    let first = store
        .create(
            &founder,
            proposal("Developer experience", "DX", "request-v1-create-dx"),
        )
        .await
        .expect("a founder creates the first project without an operator grant");
    assert!(first.created);
    assert_eq!(first.project.name, "Developer experience");
    assert_eq!(first.project.issue_prefix, "DX");

    let retry = store
        .create(
            &founder,
            proposal("Developer experience", "DX", "request-v1-create-dx"),
        )
        .await
        .expect("the same request is replay-safe");
    assert!(!retry.created);
    assert_eq!(retry.project, first.project);

    assert!(matches!(
        store
            .create(
                &founder,
                proposal("A different project", "OTHER", "request-v1-create-dx"),
            )
            .await,
        Err(ProjectError::Conflict(_))
    ));
    assert!(matches!(
        store
            .create(
                &founder,
                proposal("Another DX project", "DX", "request-v1-another-dx"),
            )
            .await,
        Err(ProjectError::Conflict(_))
    ));

    let fetched = store
        .get(&founder, &first.project.id)
        .await
        .expect("project metadata is durable");
    assert_eq!(fetched, first.project);
    assert_eq!(
        store.list_visible(&founder, None, 100).await.unwrap(),
        vec![first.project.clone()]
    );

    let team_member = actor(&tenant, &region);
    let team_member = Principal {
        principal_id: PrincipalId("p:team-member".into()),
        ..team_member
    };
    let org_member = Principal {
        principal_id: PrincipalId("p:org-member".into()),
        ..actor(&tenant, &region)
    };
    let outsider = Principal {
        principal_id: PrincipalId("p:outsider".into()),
        ..actor(&tenant, &region)
    };
    let scope = TenantScope::from_verified_token(&founder, founder.region.clone());
    tuples
        .write_tuples(
            &scope,
            &founder,
            &[
                add(
                    &format!("project:{}", first.project.id),
                    "parent_team",
                    "team:developer-experience#view",
                ),
                add("team:developer-experience", "member", "p:team-member"),
                add(
                    "team:developer-experience",
                    "parent_org",
                    "org:engineering#view",
                ),
                add("org:engineering", "member", "p:org-member"),
            ],
            None,
            None,
            Timestamp("2026-08-09T18:00:00Z".into()),
        )
        .expect("seed the core org-to-team-to-project hierarchy");
    assert_eq!(
        store.list_visible(&team_member, None, 100).await.unwrap(),
        vec![first.project.clone()],
        "a team member inherits the project's view permission"
    );
    assert_eq!(
        store.list_visible(&org_member, None, 100).await.unwrap(),
        vec![first.project.clone()],
        "an org member inherits project view through the parent team"
    );
    assert!(
        store
            .list_visible(&outsider, None, 100)
            .await
            .unwrap()
            .is_empty(),
        "an unrelated principal learns no project metadata"
    );

    let tuple: (String, String, String) = sqlx::query(
        "SELECT object_id, relation, subject FROM rebac_tuple \
          WHERE tenant_id = $1 AND region = $2 AND object_id = $3",
    )
    .bind(&tenant)
    .bind(&region)
    .bind(format!("project:{}", first.project.id))
    .fetch_one(admin.db_pool())
    .await
    .map(|row| {
        (
            row.get("object_id"),
            row.get("relation"),
            row.get("subject"),
        )
    })
    .expect("the creator grant committed with the project");
    assert_eq!(
        tuple,
        (
            format!("project:{}", first.project.id),
            "writer".into(),
            founder.principal_id.0.clone(),
        )
    );

    let envelope: EventEnvelope = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT envelope FROM outbox WHERE event_id = $1",
    )
    .bind(event_id)
    .fetch_one(admin.db_pool())
    .await
    .and_then(|value| {
        serde_json::from_value(value).map_err(|error| sqlx::Error::Decode(Box::new(error)))
    })
    .expect("the project event committed with the metadata and grant");
    assert_eq!(envelope.type_.0, IDENTITY_PROJECT_CREATED);
    assert_eq!(envelope.payload["project_id"], first.project.id);
    assert_eq!(envelope.payload["creator_grant"]["relation"], "writer");
    assert_eq!(
        envelope.subject.0,
        format!("myelin://{tenant}/identity/project/{}", first.project.id)
    );

    let failed = store
        .create(
            &founder,
            proposal("Must roll back", "ROLLBACK", "request-v1-event-collision"),
        )
        .await
        .expect_err("a divergent event-id collision aborts the whole transaction");
    assert!(matches!(failed, ProjectError::Storage(_)));
    let rolled_back_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM identity_project WHERE tenant_id = $1 AND issue_prefix = 'ROLLBACK'",
    )
    .bind(&tenant)
    .fetch_one(admin.db_pool())
    .await
    .unwrap();
    let creator_grants: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM rebac_tuple \
          WHERE tenant_id = $1 AND region = $2 AND relation = 'writer' AND subject = $3",
    )
    .bind(&tenant)
    .bind(&region)
    .bind(&founder.principal_id.0)
    .fetch_one(admin.db_pool())
    .await
    .unwrap();
    assert_eq!(rolled_back_rows, 0);
    assert_eq!(
        creator_grants, 1,
        "the failed project added no creator grant"
    );

    sqlx::query("DELETE FROM rebac_tuple WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM identity_project WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox WHERE envelope->>'tenant' = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
}
