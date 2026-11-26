#![cfg(feature = "integration")]

//! Durable checks read only the relationships the question can reach, and still see every
//! committed change on the next request.

mod common;

use std::sync::Arc;

use myelin_events::Timestamp;
use myelin_identity::{
    Consistency, ConsistencyMode, DataRole, Decision, IdentityService, ObjectId, Permission,
    Principal, PrincipalId, PrincipalKind, PrincipalStatus, RelName, RelationTuple, TupleDelta,
    Zookie,
};
use myelin_identity_service::tuple_store::TupleStore;
use myelin_identity_service::StoreBackedCheck;
use myelin_storage::migration::HotTables;
use myelin_storage::{
    identity_durable_migrations, identity_tuple_revision_migrations,
    rebac_tuple_subgraph_migrations, relationship_object_key_migrations, DurableTupleBacking,
    SubgraphRoots, SubgraphSubjects, SubstrateProvider, TenantScope,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

struct UniqueMinter {
    base: String,
    n: std::sync::atomic::AtomicU64,
}

impl myelin_events::IdMinter for UniqueMinter {
    fn mint(&self) -> myelin_events::Ulid {
        let n = self.n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        myelin_events::Ulid(format!("01J{}{n:012}", self.base))
    }
}

fn suffix() -> String {
    format!(
        "{}{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn tuple(object: &str, relation: &str, subject: &str) -> RelationTuple {
    RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    }
}

fn strong() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

struct Fixture {
    app: SubstrateProvider,
    tuples: TupleStore,
    alice: Principal,
    scope: TenantScope,
}

impl Fixture {
    async fn new() -> Fixture {
        let admin = common::admin_provider(4).await;
        for migrations in [
            identity_durable_migrations(),
            identity_tuple_revision_migrations(),
            rebac_tuple_subgraph_migrations(),
            relationship_object_key_migrations(),
        ] {
            admin
                .migrate(&migrations, &HotTables::none())
                .await
                .expect("relationship migrations apply");
        }
        let app = common::app_provider(6).await;
        let suffix = suffix();
        let alice = Principal::new(
            TenantId(format!("subgraph-{suffix}")),
            Region(app.config().region.clone()),
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let scope = TenantScope::from_verified_token(&alice, alice.region.clone());
        let tuples = TupleStore::with_pg_minter(
            Arc::new(UniqueMinter {
                base: suffix,
                n: Default::default(),
            }),
            DurableTupleBacking::new(app.clone()),
            tokio::runtime::Handle::current(),
        );
        Fixture {
            app,
            tuples,
            alice,
            scope,
        }
    }

    fn write(&self, deltas: &[TupleDelta]) {
        self.tuples
            .write_tuples(
                &self.scope,
                &self.alice,
                deltas,
                None,
                None,
                Timestamp("2026-11-26T00:00:00Z".into()),
            )
            .expect("relationship write commits");
    }

    /// A fresh service per request, as each Edge request reaches the store on its own.
    fn check(&self, object: &str) -> Decision {
        StoreBackedCheck::new(self.tuples.clone())
            .check(
                &self.alice,
                &Permission("reader".into()),
                &ArtifactRef(object.into()),
                &strong(),
                None,
            )
            .expect("check evaluates")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revoked_inherited_grant_denies_on_the_next_check() {
    let f = Fixture::new().await;
    f.write(&[
        TupleDelta::Add(tuple("repo:core", "reader", "team:eng#member")),
        TupleDelta::Add(tuple("team:eng", "member", "group:staff#member")),
        TupleDelta::Add(tuple("group:staff", "member", "p:alice")),
    ]);
    assert_eq!(
        f.check("repo:core"),
        Decision::Allow,
        "two userset hops grant"
    );

    f.write(&[TupleDelta::Remove(tuple(
        "group:staff",
        "member",
        "p:alice",
    ))]);
    assert_eq!(
        f.check("repo:core"),
        Decision::Deny,
        "removing the innermost membership denies on the very next check"
    );

    f.write(&[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))]);
    let batch = StoreBackedCheck::new(f.tuples.clone())
        .check_each(
            &f.alice,
            &[
                (Permission("reader".into()), ArtifactRef("repo:core".into())),
                (
                    Permission("reader".into()),
                    ArtifactRef("repo:other".into()),
                ),
            ],
            &strong(),
        )
        .expect("batch evaluates");
    assert_eq!(batch, vec![Decision::Allow, Decision::Deny]);

    f.write(&[TupleDelta::Remove(tuple("repo:core", "reader", "p:alice"))]);
    let batch = StoreBackedCheck::new(f.tuples.clone())
        .check_each(
            &f.alice,
            &[(Permission("reader".into()), ArtifactRef("repo:core".into()))],
            &strong(),
        )
        .expect("batch evaluates");
    assert_eq!(
        batch,
        vec![Decision::Deny],
        "a batch sees a revoked direct grant on the next request"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_check_reads_only_the_relationships_it_can_reach() {
    let f = Fixture::new().await;
    let mut unrelated = Vec::new();
    for n in 0..300 {
        unrelated.push(TupleDelta::Add(tuple(
            "repo:core",
            "reader",
            &format!("p:{n}"),
        )));
        unrelated.push(TupleDelta::Add(tuple(
            &format!("repo:other-{n}"),
            "reader",
            "p:alice",
        )));
    }
    f.write(&unrelated);
    f.write(&[
        TupleDelta::Add(tuple("repo:core", "reader", "team:eng#member")),
        TupleDelta::Add(tuple("team:eng", "member", "p:alice")),
        TupleDelta::Add(tuple("team:eng", "member", "p:bob")),
    ]);

    let graph = DurableTupleBacking::new(f.app.clone())
        .subgraph_in(
            &f.alice.tenant.0,
            &f.alice.region.0,
            SubgraphRoots::Objects(vec!["repo:core".into()]),
            SubgraphSubjects::Only("p:alice".into()),
            32,
        )
        .await
        .expect("subgraph reads");
    let mut edges = graph
        .edges
        .iter()
        .map(|edge| format!("{}#{}@{}", edge.object, edge.relation, edge.subject))
        .collect::<Vec<_>>();
    edges.sort();
    assert_eq!(
        edges,
        vec![
            "repo:core#reader@team:eng#member",
            "team:eng#member@p:alice"
        ],
        "neither other people's grants nor alice's grants elsewhere are read"
    );
    assert_eq!(f.check("repo:core"), Decision::Allow);

    let whole_tenant = f.tuples.tuples_in(&f.scope).expect("whole tenant reads");
    assert_eq!(
        whole_tenant.len(),
        603,
        "the whole-tenant read a check used to make holds every tuple"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_artifact_reference_write_is_stored_under_its_object_key() {
    let f = Fixture::new().await;
    let urn = format!("myelin://{}/git/repo/core", f.alice.tenant.0);
    f.write(&[TupleDelta::Add(tuple(&urn, "reader", "p:alice"))]);

    assert_eq!(f.check("repo:core"), Decision::Allow);
    assert_eq!(f.check(&urn), Decision::Allow);
    let stored = f.tuples.tuples_in(&f.scope).expect("whole tenant reads");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].tuple.object.0, "repo:core");

    f.write(&[TupleDelta::Remove(tuple(&urn, "reader", "p:alice"))]);
    assert_eq!(
        f.check("repo:core"),
        Decision::Deny,
        "removing by reference removes the keyed tuple"
    );
}

/// A provider whose every connection works in `schema` (then `public`, for the RLS helpers).
async fn schema_provider(config: myelin_config::MyelinConfig, schema: &str) -> SubstrateProvider {
    let mut config = config;
    let separator = if config.database_url.contains('?') {
        '&'
    } else {
        '?'
    };
    config.database_url = format!(
        "{}{separator}options=-c%20search_path%3D{schema}%2Cpublic",
        config.database_url
    );
    SubstrateProvider::connect(config, 4)
        .await
        .expect("connect inside the test schema")
}

/// An empty schema holding the relationship tables as they stood before object keys were
/// enforced, so legacy spellings can be written the way older code wrote them.
async fn legacy_relationship_schema() -> (sqlx::PgPool, String, SubstrateProvider, SubstrateProvider)
{
    let bare = sqlx::PgPool::connect(&common::admin_database_config().database_url)
        .await
        .expect("connect as the migration role");
    let schema = format!("object_keys_{}", suffix());
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema} AUTHORIZATION myelin_admin;
         GRANT USAGE ON SCHEMA {schema} TO myelin_app;
         ALTER DEFAULT PRIVILEGES FOR ROLE myelin_admin IN SCHEMA {schema}
           GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO myelin_app;"
    ))
    .execute(&bare)
    .await
    .expect("create the test schema");
    let admin = schema_provider(common::admin_database_config(), &schema).await;
    for migrations in [
        identity_durable_migrations(),
        identity_tuple_revision_migrations(),
        rebac_tuple_subgraph_migrations(),
    ] {
        admin
            .migrate(&migrations, &HotTables::none())
            .await
            .expect("pre-key relationship schema");
    }
    let app = schema_provider(common::app_database_config(), &schema).await;
    (bare, schema, admin, app)
}

async fn insert_legacy(
    admin: &SubstrateProvider,
    tenant: &str,
    region: &str,
    rows: &[(&str, &str, &str, Option<&str>)],
    revisions: &[(&str, i64)],
) {
    let pool = admin.db_pool();
    let mut conn = pool.acquire().await.expect("admin connection");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, false), set_config('myelin.region', $2, false)",
    )
    .bind(tenant)
    .bind(region)
    .execute(&mut *conn)
    .await
    .expect("scope the legacy writes");
    for (object, relation, subject, expires_at) in rows {
        sqlx::query(
            "INSERT INTO rebac_tuple (tenant_id, region, object_id, relation, subject, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6::timestamptz)",
        )
        .bind(tenant)
        .bind(region)
        .bind(object)
        .bind(relation)
        .bind(subject)
        .bind(expires_at)
        .execute(&mut *conn)
        .await
        .expect("write a legacy tuple");
    }
    for (object, revision) in revisions {
        sqlx::query(
            "INSERT INTO rebac_object_revision (tenant_id, region, object_id, revision) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(tenant)
        .bind(region)
        .bind(object)
        .bind(revision)
        .execute(&mut *conn)
        .await
        .expect("write a legacy revision");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_legacy_reference_spelled_exclusion_is_migrated_and_denies() {
    let (bare, schema, admin, app) = legacy_relationship_schema().await;
    let region = app.config().region.clone();
    let tenant = format!("legacy-{}", suffix());
    let alice = Principal::new(
        TenantId(tenant.clone()),
        Region(region.clone()),
        PrincipalId("p:alice".into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let secret = format!("myelin://{tenant}/issue/issue/secret");
    let dup = format!("myelin://{tenant}/issue/issue/dup");
    insert_legacy(
        &admin,
        &tenant,
        &region,
        &[
            ("project:proj", "reader", "p:alice", None),
            ("issue:secret", "parent_project", "project:proj#view", None),
            (&secret, "confidential", "p:alice", None),
            (
                "issue:dup",
                "watcher",
                "p:alice",
                Some("2030-01-01T00:00:00Z"),
            ),
            (&dup, "watcher", "p:alice", None),
        ],
        &[("issue:secret", 3), (&secret, 7)],
    )
    .await;

    let check = StoreBackedCheck::new(TupleStore::with_pg(
        DurableTupleBacking::new(app.clone()),
        tokio::runtime::Handle::current(),
    ));
    for admit in check.admit_issue_fragment() {
        assert!(matches!(
            admit,
            myelin_identity::FragmentAdmit::Admitted { .. }
        ));
    }
    let view = |check: &StoreBackedCheck| {
        check
            .check(
                &alice,
                &Permission("view".into()),
                &ArtifactRef("issue:secret".into()),
                &strong(),
                None,
            )
            .expect("check evaluates")
    };
    assert_eq!(
        view(&check),
        Decision::Allow,
        "before the migration the reference-spelled `confidential` row is not read, so the \
         exclusion fails open - the gap the migration closes"
    );

    admin
        .migrate(&relationship_object_key_migrations(), &HotTables::none())
        .await
        .expect("legacy spellings are rewritten and the key constraints validate");
    assert_eq!(
        view(&check),
        Decision::Deny,
        "the migrated `confidential` row subtracts the inherited view"
    );

    let mut conn = admin.db_pool().acquire().await.unwrap();
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, false), set_config('myelin.region', $2, false)",
    )
    .bind(&tenant)
    .bind(&region)
    .execute(&mut *conn)
    .await
    .unwrap();
    let objects: Vec<(String, String, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT object_id, relation, expires_at FROM rebac_tuple ORDER BY object_id, relation",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        objects
            .iter()
            .map(|(object, relation, expires)| (
                object.as_str(),
                relation.as_str(),
                expires.is_some()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("issue:dup", "watcher", false),
            ("issue:secret", "confidential", false),
            ("issue:secret", "parent_project", false),
            ("project:proj", "reader", false),
        ],
        "colliding spellings merge, keeping the longer (never-expiring) lifetime"
    );
    let revisions: Vec<(String, i64)> =
        sqlx::query_as("SELECT object_id, revision FROM rebac_object_revision")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(revisions, vec![("issue:secret".to_string(), 7)]);
    let validated: Vec<bool> = sqlx::query_scalar(
        "SELECT convalidated FROM pg_constraint \
         WHERE conname IN ('rebac_tuple_object_is_key', 'rebac_object_revision_object_is_key') \
           AND connamespace = (SELECT oid FROM pg_namespace WHERE nspname = $1)",
    )
    .bind(&schema)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(validated, vec![true, true]);

    // The constraint and the Rust rule agree on every shape, whitespace included.
    for object in [
        "repo:core",
        "team/app",
        "foo://bar",
        "\u{200b}x",
        "\u{180e}x",
        "x y",
        " repo:core",
        "repo:core ",
        "repo:core\u{3000}",
        "\u{a0}x",
        "x\u{85}",
        "\tx",
        "a#b",
        "myelin://t/git/repo/x",
        "",
    ] {
        let stored = sqlx::query(
            "INSERT INTO rebac_tuple (tenant_id, region, object_id, relation, subject) \
             VALUES ($1, $2, $3, 'probe', 'p:probe')",
        )
        .bind(&tenant)
        .bind(&region)
        .bind(object)
        .execute(&mut *conn)
        .await
        .is_ok();
        assert_eq!(
            stored,
            myelin_storage::tuple_subgraph::is_canonical_tuple_object(object),
            "{object:?}"
        );
    }
    drop(conn);
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&bare)
        .await
        .expect("remove the test schema");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relationship_object_without_a_key_stops_the_migration() {
    let (bare, schema, admin, app) = legacy_relationship_schema().await;
    let tenant = format!("keyless-{}", suffix());
    insert_legacy(
        &admin,
        &tenant,
        &app.config().region.clone(),
        &[("#anchor-only", "reader", "p:alice", None)],
        &[],
    )
    .await;
    let refused = admin
        .migrate(&relationship_object_key_migrations(), &HotTables::none())
        .await
        .expect_err("a row no check can find is reported, not guessed at");
    assert!(refused.to_string().contains("no key"), "{refused}");
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&bare)
        .await
        .expect("remove the test schema");
}
