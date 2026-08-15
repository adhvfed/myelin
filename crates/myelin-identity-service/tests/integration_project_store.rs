#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::{EventEnvelope, IdMinter, Timestamp, Ulid, UlidMinter};
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

struct FixedMinter(String);

impl IdMinter for FixedMinter {
    fn mint(&self) -> Ulid {
        Ulid(self.0.clone())
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
    let event_id = UlidMinter::new().mint().0;
    let tuples = TupleStore::with_pg(
        DurableTupleBacking::new(app.clone()),
        tokio::runtime::Handle::current(),
    );
    let store = PgProjectStore::with_minter(app, Arc::new(FixedMinter(event_id.clone())));

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
    let temporary_team_member = Principal {
        principal_id: PrincipalId("p:temporary-team-member".into()),
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

    tuples
        .write_tuples(
            &scope,
            &founder,
            &[add(
                "team:temporary-project-access",
                "member",
                &temporary_team_member.principal_id.0,
            )],
            None,
            None,
            Timestamp("2020-01-01T00:00:00Z".into()),
        )
        .expect("seed the durable side of a temporary inherited grant");
    tuples
        .write_tuples(
            &scope,
            &founder,
            &[
                add(
                    &format!("project:{}", first.project.id),
                    "reader",
                    &outsider.principal_id.0,
                ),
                add(
                    &format!("project:{}", first.project.id),
                    "parent_team",
                    "team:temporary-project-access#view",
                ),
            ],
            None,
            Some(Timestamp("2020-01-01T00:01:00Z".into())),
            Timestamp("2020-01-01T00:00:00Z".into()),
        )
        .expect("record temporary direct and inherited project grants");
    assert!(
        store
            .list_visible(&outsider, None, 100)
            .await
            .unwrap()
            .is_empty(),
        "a direct project grant disappears from the user's project list at its deadline"
    );
    assert!(
        store
            .list_visible(&temporary_team_member, None, 100)
            .await
            .unwrap()
            .is_empty(),
        "an expired parent edge cannot keep inherited project visibility alive"
    );
    let expired_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM rebac_tuple \
          WHERE tenant_id = $1 AND region = $2 AND expires_at < CURRENT_TIMESTAMP",
    )
    .bind(&tenant)
    .bind(&region)
    .fetch_one(admin.db_pool())
    .await
    .unwrap();
    assert_eq!(
        expired_rows, 2,
        "expiry revokes authority without erasing the durable relationship history"
    );

    let tuple: (String, String, String) = sqlx::query(
        "SELECT object_id, relation, subject FROM rebac_tuple \
          WHERE tenant_id = $1 AND region = $2 AND object_id = $3 \
            AND relation = 'writer' AND subject = $4",
    )
    .bind(&tenant)
    .bind(&region)
    .bind(format!("project:{}", first.project.id))
    .bind(&founder.principal_id.0)
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
    .bind(&event_id)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_listing_keeps_the_newest_work_in_reach_across_pages() {
    let config = test_config();
    let admin = SubstrateProvider::connect(admin_config(&config), 4)
        .await
        .expect("connect to the Postgres required by project discovery");
    admin.migrate_foundation().await.unwrap();
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap();
    let app = SubstrateProvider::connect(config, 6)
        .await
        .expect("connect constrained runtime role");
    let region = app.config().region.clone();
    let tenant = format!("project-listing-{}", unique());
    let founder = actor(&tenant, &region);
    let store = PgProjectStore::new(app);

    let oldest = store
        .create(&founder, proposal("Oldest work", "OLD", "create-oldest"))
        .await
        .expect("create the oldest project")
        .project;
    let middle = store
        .create(&founder, proposal("Middle work", "MID", "create-middle"))
        .await
        .expect("create the middle project")
        .project;
    let newest = store
        .create(&founder, proposal("Newest work", "NEW", "create-newest"))
        .await
        .expect("create the newest project")
        .project;

    sqlx::query(
        "UPDATE identity_project \
            SET created_at = CASE issue_prefix \
                WHEN 'OLD' THEN TIMESTAMPTZ '2026-01-01T00:00:00Z' \
                WHEN 'MID' THEN TIMESTAMPTZ '2026-01-02T00:00:00Z' \
                WHEN 'NEW' THEN TIMESTAMPTZ '2026-01-03T00:00:00Z' \
                ELSE created_at \
            END \
          WHERE tenant_id = $1 AND region = $2",
    )
    .bind(&tenant)
    .bind(&region)
    .execute(admin.db_pool())
    .await
    .expect("make recency deterministic without coupling it to random project ids");

    let first_page = store
        .list_visible(&founder, None, 2)
        .await
        .expect("list the first page of visible projects");
    assert_eq!(
        first_page
            .iter()
            .map(|project| &project.id)
            .collect::<Vec<_>>(),
        vec![&newest.id, &middle.id],
        "new work belongs at the front of a developer's project list"
    );

    let second_page = store
        .list_visible(&founder, Some(&middle.id), 2)
        .await
        .expect("continue from the last project on the first page");
    assert_eq!(
        second_page
            .iter()
            .map(|project| &project.id)
            .collect::<Vec<_>>(),
        vec![&oldest.id],
        "the cursor continues by stable creation order without gaps or repeats"
    );
    assert!(
        store
            .list_visible(&founder, Some("00000000-0000-0000-0000-000000000000"), 2,)
            .await
            .expect("an unknown cursor fails closed")
            .is_empty(),
        "a cursor must name a project visible to its caller"
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operator_bootstrap_registers_existing_project_metadata_once() {
    let config = test_config();
    let admin = SubstrateProvider::connect(admin_config(&config), 4)
        .await
        .expect("connect to the Postgres required by operator bootstrap");
    admin.migrate_foundation().await.unwrap();
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap();
    let app = SubstrateProvider::connect(config, 6)
        .await
        .expect("connect constrained runtime role");
    let region = app.config().region.clone();
    let tenant = format!("project-bootstrap-{}", unique());
    let founder = actor(&tenant, &region);
    let store = PgProjectStore::new(app);
    let project_id = "11111111-1111-4111-8111-111111111111";
    let issue_type_id = "22222222-2222-4222-8222-222222222222";

    let first = store
        .ensure_existing_project_metadata(
            &founder,
            project_id,
            "Imported engineering",
            "BOOT",
            issue_type_id,
        )
        .await
        .expect("bootstrap makes a reference-only project usable by current issue tools");
    assert!(first.registered);
    assert_eq!(first.project.id, project_id);
    assert_eq!(first.project.default_issue_type_id, issue_type_id);

    let replay = store
        .ensure_existing_project_metadata(
            &founder,
            project_id,
            "Imported engineering",
            "BOOT",
            issue_type_id,
        )
        .await
        .expect("repeating operator bootstrap preserves the original project");
    assert!(!replay.registered);
    assert_eq!(replay.project, first.project);

    let changed_contract = store
        .ensure_existing_project_metadata(
            &founder,
            project_id,
            "Imported engineering",
            "BOOT",
            "33333333-3333-4333-8333-333333333333",
        )
        .await;
    assert!(matches!(changed_contract, Err(ProjectError::Conflict(_))));
}
