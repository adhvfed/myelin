#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_events::{EventEnvelope, Timestamp};
use myelin_identity::iam_events::IDENTITY_TUPLE_WRITTEN;
use myelin_identity::{
    Consistency, ConsistencyMode, DataRole, FragmentAdmit, ListObjectsResult, ObjectId, ObjectType,
    Permission, Precondition, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RelName,
    RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::expand::Expand;
use myelin_identity_service::list_objects::ListObjects;
use myelin_identity_service::namespace::{FragmentDef, NamespaceEngine, PermissionRule, Userset};
use myelin_identity_service::principal_store::{PrincipalProfile, PrincipalStore};
use myelin_identity_service::reverse_index::ReverseIndex;
use myelin_identity_service::tuple_store::TupleStore;
use myelin_storage::migration::HotTables;
use myelin_storage::{
    identity_durable_migrations, identity_tuple_revision_migrations, DurablePrincipalBacking,
    DurableTupleBacking, KmsEngine, SubstrateProvider,
};
use myelin_tenancy::{Region, TenantId};

fn admin_config(cfg: &MyelinConfig) -> MyelinConfig {
    let mut c = cfg.clone();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

fn uniq() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn scope(tenant: &str, region: &str) -> myelin_storage::TenantScope {
    let p = Principal::stub(
        PrincipalId("p:admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    myelin_storage::TenantScope::from_verified_token(&p, Region(region.into()))
}

fn actor(tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId("p:writer".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

struct UniqueMinter {
    base: String,
    n: std::sync::atomic::AtomicU64,
}

impl UniqueMinter {
    fn new(base: impl Into<String>) -> Self {
        UniqueMinter {
            base: base.into(),
            n: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl myelin_events::IdMinter for UniqueMinter {
    fn mint(&self) -> myelin_events::Ulid {
        let n = self.n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        myelin_events::Ulid(format!("01J{}{n:012}", self.base))
    }
}

fn tuple(object: &str, relation: &str, subject: &str) -> RelationTuple {
    RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    }
}

fn profile(email_addr: &str, name: &str) -> PrincipalProfile {
    let email = email_addr.to_string();
    let display_name = name.to_string();
    PrincipalProfile {
        email,
        display_name,
    }
}

async fn app_provider() -> Option<SubstrateProvider> {
    match SubstrateProvider::connect(MyelinConfig::dev(), 6).await {
        Ok(p) => Some(p),
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            None
        }
    }
}

async fn residual_guc(pool: &sqlx::PgPool) -> String {
    let mut conn = pool.acquire().await.expect("acquire");
    let v: Option<String> = sqlx::query_scalar("SELECT current_setting('myelin.tenant_id', true)")
        .fetch_one(&mut *conn)
        .await
        .expect("read GUC");
    v.unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_principal_and_tuple_round_trip_across_a_fresh_store_instance() {
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("identity durable migrations execute against the live DB");
    admin
        .migrate(&identity_tuple_revision_migrations(), &HotTables::none())
        .await
        .expect("durable relationship revisions migrate");

    let Some(app) = app_provider().await else {
        return;
    };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr007-dur-{suffix}");
    let s = scope(&tenant, &region);

    let kms = Arc::new(KmsEngine::new());

    let pstore1 = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(app.clone()),
        handle.clone(),
    );
    let tstore1 = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(format!("{suffix}w1"))),
        DurableTupleBacking::new(app.clone()),
        handle.clone(),
    );

    let alice = PrincipalId("p:alice".into());
    let written = pstore1
        .put_principal(
            &s,
            alice.clone(),
            PrincipalKind::Human,
            DataRole::Processor,
            PrincipalStatus::Active,
            Some(&profile("alice@acme.test", "Alice")),
        )
        .expect("durable principal write");
    assert!(
        written.profile_ref.is_some(),
        "a profiled principal has a profile_ref"
    );
    pstore1
        .put_principal(
            &s,
            PrincipalId("svc:deploy".into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .expect("durable service principal write");

    let z = tstore1
        .write_tuples(
            &s,
            &actor(&tenant),
            &[
                TupleDelta::Add(tuple("repo:core", "reader", "p:alice")),
                TupleDelta::Add(tuple("repo:core", "writer", "p:bob")),
            ],
            None,
            None,
            Timestamp("2026-06-26T00:00:00Z".into()),
        )
        .expect("durable tuple write");

    let pstore2 = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(app.clone()),
        handle.clone(),
    );
    let tstore2 = TupleStore::with_pg(DurableTupleBacking::new(app.clone()), handle.clone());

    let read = pstore2
        .get_principal(&s, &alice)
        .expect("the principal row is durable across a fresh instance");
    assert_eq!(read.principal_id, alice);
    assert_eq!(read.kind, PrincipalKind::Human);
    assert_eq!(read.data_role, DataRole::Processor);
    assert_eq!(read.status, PrincipalStatus::Active);
    assert!(
        read.profile_ref.is_some(),
        "the erasable profile_ref persists (ciphertext durable)"
    );
    assert_eq!(
        pstore2.principals_in(&s).len(),
        2,
        "both the human + service principals are durable"
    );
    let prof = pstore2
        .get_profile(&s, &alice)
        .expect("profile read succeeds")
        .expect("the profile ciphertext is durable + decrypts under the shared KMS");
    assert_eq!(
        prof,
        profile("alice@acme.test", "Alice"),
        "the profile round-trips"
    );

    let durable_edges = tstore2
        .tuples_in(&s)
        .expect("read durable tuples through the fresh store");
    assert!(
        durable_edges.iter().all(|tuple| tuple.zookie == z),
        "a fresh store reads each edge at the revision committed by the original writer"
    );
    assert_eq!(
        tstore2
            .object_zookie(&s, "repo:core")
            .expect("read the durable object revision"),
        z,
        "the object watermark survives a fresh TupleStore"
    );
    let mut edges: Vec<(String, String, String)> = durable_edges
        .into_iter()
        .map(|t| (t.tuple.object.0, t.tuple.relation.0, t.tuple.subject.0))
        .collect();
    edges.sort();
    assert_eq!(
        edges,
        vec![
            ("repo:core".into(), "reader".into(), "p:alice".into()),
            ("repo:core".into(), "writer".into(), "p:bob".into()),
        ],
        "both durable edges round-trip across a fresh store instance"
    );
    assert!(!z.0.is_empty(), "the write returned a monotonic zookie");

    let next = tstore2
        .write_tuples(
            &s,
            &actor(&tenant),
            &[TupleDelta::Add(tuple("repo:web", "reader", "p:alice"))],
            None,
            None,
            Timestamp("2026-06-26T00:01:00Z".into()),
        )
        .expect("a fresh writer advances the durable revision");
    assert!(
        next.0 > z.0,
        "a restarted writer advances instead of minting revision one again"
    );

    for sql in [
        "DELETE FROM rebac_tuple WHERE tenant_id = $1",
        "DELETE FROM rebac_object_revision WHERE tenant_id = $1",
        "DELETE FROM rebac_revision WHERE tenant_id = $1",
        "DELETE FROM principal WHERE tenant_id = $1",
        "DELETE FROM credential_link WHERE tenant_id = $1",
    ] {
        let _ = sqlx::query(sql)
            .bind(&tenant)
            .execute(admin.db_pool())
            .await;
    }
    let _ = sqlx::query("DELETE FROM outbox WHERE aggregate LIKE $1")
        .bind(format!("identity:tuple:{tenant}:%"))
        .execute(admin.db_pool())
        .await;
    println!(
        "OK [1]: principal row + profile ciphertext + tuple edges durable across a fresh instance."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tenant_a_writes_are_invisible_to_tenant_b_and_no_guc_bleeds() {
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("identity durable migrations");
    admin
        .migrate(&identity_tuple_revision_migrations(), &HotTables::none())
        .await
        .expect("durable relationship revisions migrate");

    let Some(app) = app_provider().await else {
        return;
    };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let kms = Arc::new(KmsEngine::new());
    let suffix = uniq();
    let tenant_a = format!("mr007A-{suffix}");
    let tenant_b = format!("mr007B-{suffix}");
    let sa = scope(&tenant_a, &region);
    let sb = scope(&tenant_b, &region);

    let pstore = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(app.clone()),
        handle.clone(),
    );
    let tstore = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(format!("{suffix}w2"))),
        DurableTupleBacking::new(app.clone()),
        handle.clone(),
    );

    let alice = PrincipalId("p:alice".into());
    pstore
        .put_principal(
            &sa,
            alice.clone(),
            PrincipalKind::Human,
            DataRole::Processor,
            PrincipalStatus::Active,
            Some(&profile("alice@a.test", "Alice")),
        )
        .expect("tenant A principal write");
    tstore
        .write_tuples(
            &sa,
            &actor(&tenant_a),
            &[TupleDelta::Add(tuple("repo:secret", "reader", "p:alice"))],
            None,
            None,
            Timestamp("2026-06-26T00:00:00Z".into()),
        )
        .expect("tenant A tuple write");

    assert_eq!(
        pstore.principals_in(&sa).len(),
        1,
        "tenant A sees its principal"
    );
    assert_eq!(
        tstore
            .tuples_in(&sa)
            .expect("read tenant A's tuple partition")
            .len(),
        1,
        "tenant A sees its tuple"
    );

    assert!(
        pstore.get_principal(&sb, &alice).is_none(),
        "tenant B cannot see tenant A's principal (RLS via with_tenant_tx)"
    );
    assert!(
        pstore.principals_in(&sb).is_empty(),
        "tenant B's principal partition is empty"
    );
    assert!(
        tstore
            .tuples_in(&sb)
            .expect("read tenant B's tuple partition")
            .is_empty(),
        "tenant B cannot see tenant A's tuple (RLS)"
    );

    assert!(
        residual_guc(app.db_pool()).await.is_empty(),
        "no residual myelin.tenant_id GUC after the tenant-scoped ops (no bleed)"
    );

    let unknown =
        pstore.link_credential(&sa, "oidc", "sub-unknown", &PrincipalId("p:ghost".into()));
    assert!(
        matches!(
            unknown,
            Err(myelin_identity_service::principal_store::PrincipalError::UnknownPrincipal { .. })
        ),
        "a link to a non-existent principal is refused"
    );
    pstore
        .link_credential(&sa, "oidc", "sub-alice", &alice)
        .expect("link a verified credential to an existing principal");
    let resolved = pstore
        .resolve_credential(&sa, "oidc", "sub-alice")
        .expect("the credential resolves to its principal");
    assert_eq!(resolved.principal_id, alice);
    assert!(
        pstore
            .resolve_credential(&sb, "oidc", "sub-alice")
            .is_none(),
        "a credential verified for tenant A never resolves into tenant B's directory"
    );

    for sql in [
        "DELETE FROM rebac_tuple WHERE tenant_id = $1 OR tenant_id = $2",
        "DELETE FROM rebac_object_revision WHERE tenant_id = $1 OR tenant_id = $2",
        "DELETE FROM rebac_revision WHERE tenant_id = $1 OR tenant_id = $2",
        "DELETE FROM principal WHERE tenant_id = $1 OR tenant_id = $2",
        "DELETE FROM credential_link WHERE tenant_id = $1 OR tenant_id = $2",
        "DELETE FROM outbox WHERE aggregate LIKE 'identity:tuple:' || $1 || ':%' \
         OR aggregate LIKE 'identity:tuple:' || $2 || ':%'",
    ] {
        let _ = sqlx::query(sql)
            .bind(&tenant_a)
            .bind(&tenant_b)
            .execute(admin.db_pool())
            .await;
    }
    println!("OK [2]: tenant A invisible to tenant B (RLS via with_tenant_tx); no GUC bleed; credential isolation.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_tuple_write_co_commits_exactly_one_outbox_event() {
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("identity durable migrations");
    admin
        .migrate(&identity_tuple_revision_migrations(), &HotTables::none())
        .await
        .expect("durable relationship revisions migrate");
    admin
        .migrate_foundation()
        .await
        .expect("foundation (outbox) migration");
    let Some(app) = app_provider().await else {
        return;
    };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr007-ob-{suffix}");
    let s = scope(&tenant, &region);

    let aggregate = format!("identity:tuple:{tenant}:repo:core");

    async fn outbox_count(pool: &sqlx::PgPool, aggregate: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE aggregate = $1")
            .bind(aggregate)
            .fetch_one(pool)
            .await
            .expect("count outbox rows")
    }

    let tstore = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(suffix.clone())),
        DurableTupleBacking::new(app.clone()),
        handle,
    );

    let _ = sqlx::query("DELETE FROM outbox WHERE aggregate = $1")
        .bind(&aggregate)
        .execute(admin.db_pool())
        .await;
    assert_eq!(
        outbox_count(admin.db_pool(), &aggregate).await,
        0,
        "no outbox row for this aggregate before the write"
    );

    let z = tstore
        .write_tuples(
            &s,
            &actor(&tenant),
            &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
            None,
            None,
            Timestamp("2026-06-26T00:00:00Z".into()),
        )
        .expect("durable tuple write");
    assert_eq!(
        outbox_count(admin.db_pool(), &aggregate).await,
        1,
        "a committed durable write co-committed EXACTLY one identity.tuple.written row (BUS-2 exact)"
    );

    let env_json: serde_json::Value =
        sqlx::query_scalar("SELECT envelope FROM outbox WHERE aggregate = $1")
            .bind(&aggregate)
            .fetch_one(admin.db_pool())
            .await
            .expect("read the co-committed outbox envelope");
    let env: EventEnvelope =
        serde_json::from_value(env_json).expect("the outbox row is a canonical EventEnvelope");
    assert_eq!(
        env.type_.0, IDENTITY_TUPLE_WRITTEN,
        "the co-committed event is identity.tuple.written"
    );
    assert_eq!(
        env.payload["zookie"],
        serde_json::json!(z.0),
        "it carries the write's zookie"
    );
    assert!(
        !env.contains_personal_data,
        "the identity.* event carries no inline PII"
    );
    assert_eq!(
        env.actor.0.principal_id.0, "p:writer",
        "attribution by opaque principal_id only"
    );

    let stale = Zookie("zk-00000000000000000000".into());
    let err = tstore
        .write_tuples(
            &s,
            &actor(&tenant),
            &[TupleDelta::Add(tuple("repo:core", "writer", "p:bob"))],
            Some(&Precondition {
                expected_zookie: Some(stale),
            }),
            None,
            Timestamp("2026-06-26T00:00:01Z".into()),
        )
        .expect_err("a stale precondition aborts the write");
    assert!(
        matches!(
            err,
            myelin_identity_service::tuple_store::WriteError::PreconditionFailed { .. }
        ),
        "the aborted write is a precondition failure"
    );
    assert_eq!(
        outbox_count(admin.db_pool(), &aggregate).await,
        1,
        "the aborted write co-committed NO outbox row (0 ghost - commit/abort together)"
    );

    for sql in [
        "DELETE FROM rebac_tuple WHERE tenant_id = $1",
        "DELETE FROM rebac_object_revision WHERE tenant_id = $1",
        "DELETE FROM rebac_revision WHERE tenant_id = $1",
        "DELETE FROM outbox WHERE aggregate = $1",
    ] {
        let bind = if sql.contains("outbox") {
            &aggregate
        } else {
            &tenant
        };
        let _ = sqlx::query(sql).bind(bind).execute(admin.db_pool()).await;
    }
    println!(
        "OK [3]: a committed durable tuple write co-commits EXACTLY one identity.tuple.written row into \
         the SAME-DB outbox; an aborted write co-commits none (0 ghost)."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_identity_instances_share_one_monotonic_relationship_clock() {
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(provider) => provider,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("identity durable migrations");
    admin
        .migrate(&identity_tuple_revision_migrations(), &HotTables::none())
        .await
        .expect("durable relationship revisions migrate");
    admin
        .migrate_foundation()
        .await
        .expect("foundation (outbox) migration");
    let Some(app) = app_provider().await else {
        return;
    };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr007-clock-{suffix}");
    let tenant_scope = scope(&tenant, &region);

    let first = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(format!("{suffix}a"))),
        DurableTupleBacking::new(app.clone()),
        handle.clone(),
    );
    let second = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(format!("{suffix}b"))),
        DurableTupleBacking::new(app.clone()),
        handle.clone(),
    );
    let first_scope = tenant_scope.clone();
    let second_scope = tenant_scope.clone();
    let first_actor = actor(&tenant);
    let second_actor = actor(&tenant);

    let first_write = tokio::spawn(async move {
        first.write_tuples(
            &first_scope,
            &first_actor,
            &[TupleDelta::Add(tuple("repo:first", "reader", "p:alice"))],
            None,
            None,
            Timestamp("2026-06-26T00:02:00Z".into()),
        )
    });
    let second_write = tokio::spawn(async move {
        second.write_tuples(
            &second_scope,
            &second_actor,
            &[TupleDelta::Add(tuple("repo:second", "reader", "p:bob"))],
            None,
            None,
            Timestamp("2026-06-26T00:02:00Z".into()),
        )
    });
    let (first_revision, second_revision) = tokio::join!(first_write, second_write);
    let first_revision = first_revision
        .expect("the first writer task completes")
        .expect("the first writer commits");
    let second_revision = second_revision
        .expect("the second writer task completes")
        .expect("the second writer commits");
    assert_ne!(
        first_revision, second_revision,
        "two live instances never mint the same relationship revision"
    );

    let restarted = TupleStore::with_pg(DurableTupleBacking::new(app), handle);
    let tuples = restarted
        .tuples_in(&tenant_scope)
        .expect("a restarted reader sees both concurrent writes");
    assert_eq!(
        tuples.len(),
        2,
        "both independently committed grants survive"
    );
    let latest = [first_revision.clone(), second_revision.clone()]
        .into_iter()
        .max_by(|left, right| left.0.cmp(&right.0))
        .expect("two revisions have a maximum");
    assert_eq!(
        restarted.current_zookie(),
        latest,
        "reading durable state restores the process-local high-water mark"
    );

    for sql in [
        "DELETE FROM rebac_tuple WHERE tenant_id = $1",
        "DELETE FROM rebac_object_revision WHERE tenant_id = $1",
        "DELETE FROM rebac_revision WHERE tenant_id = $1",
    ] {
        let _ = sqlx::query(sql)
            .bind(&tenant)
            .execute(admin.db_pool())
            .await;
    }
    let _ = sqlx::query("DELETE FROM outbox WHERE aggregate LIKE $1")
        .bind(format!("identity:tuple:{tenant}:%"))
        .execute(admin.db_pool())
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restarted_identity_serves_strong_relationship_reads_before_its_projection_is_warm() {
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(provider) => provider,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("identity durable migrations");
    admin
        .migrate(&identity_tuple_revision_migrations(), &HotTables::none())
        .await
        .expect("durable relationship revisions migrate");
    admin
        .migrate_foundation()
        .await
        .expect("foundation (outbox) migration");
    let Some(app) = app_provider().await else {
        return;
    };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr007-cold-projection-{suffix}");
    let tenant_scope = scope(&tenant, &region);
    let writer = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(format!("{suffix}cold"))),
        DurableTupleBacking::new(app.clone()),
        handle.clone(),
    );
    let committed = writer
        .write_tuples(
            &tenant_scope,
            &actor(&tenant),
            &[
                TupleDelta::Add(tuple("repo:core", "reader", "p:alice")),
                TupleDelta::Add(tuple("channel:eng", "watcher", "p:alice")),
            ],
            None,
            None,
            Timestamp("2026-06-26T00:03:00Z".into()),
        )
        .expect("commit repository and channel relationships");
    drop(writer);

    let restarted = TupleStore::with_pg(DurableTupleBacking::new(app), handle);
    let alice = Principal::stub(
        PrincipalId("p:alice".into()),
        PrincipalKind::Human,
        TenantId(tenant.clone()),
    );
    let strong = Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    };
    let mut namespace = NamespaceEngine::with_core_hierarchy();
    assert!(matches!(
        namespace.admit(&FragmentDef {
            object_type: ObjectType("repo".into()),
            relations: vec![RelName("reader".into())],
            permissions: vec![PermissionRule {
                permission: Permission("read".into()),
                rewrite: Userset::Relation(RelName("reader".into())),
            }],
        }),
        FragmentAdmit::Admitted { .. }
    ));
    let repositories = ListObjects::new(restarted.clone(), namespace, ReverseIndex::new())
        .list_objects(
            &tenant_scope,
            &alice,
            &Permission("read".into()),
            &ObjectType("repo".into()),
            &strong,
        )
        .expect("a strong list falls back to the durable graph while the projection is cold");
    assert_eq!(
        repositories,
        ListObjectsResult::Ids {
            ids: vec![ObjectId("repo:core".into())],
            zookie: committed.clone(),
        },
        "restart does not turn a durable repository grant into an empty list"
    );

    let watchers = Expand::new(restarted.clone(), NamespaceEngine::with_core_hierarchy())
        .list_subjects(
            &tenant_scope,
            &ObjectId("channel:eng".into()),
            &ObjectType("channel".into()),
            &Permission("watcher".into()),
            &strong,
        )
        .expect("subject expansion reads the same durable snapshot as checks");
    assert_eq!(watchers.members, vec![PrincipalId("p:alice".into())]);
    assert_eq!(
        watchers.zookie, committed,
        "the response reports the authoritative revision it evaluated"
    );

    let future = Consistency {
        at_least: Zookie("zk-99999999999999999999".into()),
        mode: ConsistencyMode::Strong,
    };
    assert!(
        matches!(
            ListObjects::new(
                restarted,
                NamespaceEngine::with_core_hierarchy(),
                ReverseIndex::new(),
            )
            .list_objects(
                &tenant_scope,
                &alice,
                &Permission("reader".into()),
                &ObjectType("repo".into()),
                &future,
            ),
            Err(myelin_identity::AuthzError::Unavailable(_))
        ),
        "the restarted service never claims to have reached an uncommitted future revision"
    );

    for sql in [
        "DELETE FROM rebac_tuple WHERE tenant_id = $1",
        "DELETE FROM rebac_object_revision WHERE tenant_id = $1",
        "DELETE FROM rebac_revision WHERE tenant_id = $1",
    ] {
        let _ = sqlx::query(sql)
            .bind(&tenant)
            .execute(admin.db_pool())
            .await;
    }
    let _ = sqlx::query("DELETE FROM outbox WHERE aggregate LIKE $1")
        .bind(format!("identity:tuple:{tenant}:%"))
        .execute(admin.db_pool())
        .await;
}
