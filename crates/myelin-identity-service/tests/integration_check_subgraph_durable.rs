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
    rebac_tuple_subgraph_migrations, DurableTupleBacking, PgStore, SubgraphRoots, SubgraphSubjects,
    SubstrateProvider, TenantScope,
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

    // A tuple stored under a reference spelling before writes were keyed stays revocable by
    // that spelling.
    let (tenant, region, legacy) = (
        f.alice.tenant.0.clone(),
        f.alice.region.0.clone(),
        urn.clone(),
    );
    f.app
        .with_tenant_tx(&tenant.clone(), move |conn| {
            Box::pin(async move {
                PgStore::insert_tuple_on_conn(conn, &tenant, &region, &legacy, "reader", "p:alice")
                    .await
            })
        })
        .await
        .expect("seed a reference-spelled tuple");
    assert_eq!(f.check(&urn), Decision::Allow);
    f.write(&[TupleDelta::Remove(tuple(&urn, "reader", "p:alice"))]);
    assert_eq!(
        f.check(&urn),
        Decision::Deny,
        "removing by the stored spelling revokes it"
    );
    assert!(f
        .tuples
        .tuples_in(&f.scope)
        .expect("whole tenant reads")
        .is_empty());
}
