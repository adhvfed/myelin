#![cfg(feature = "integration")]

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
use myelin_storage::{identity_durable_migrations, DurableTupleBacking};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

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

fn latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_grant_on_issue_x_does_not_authorize_repo_x() {
    let admin = common::admin_provider(4).await;
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("identity durable migrations execute against the live DB");
    admin
        .migrate(
            &myelin_storage::identity_tuple_revision_migrations(),
            &HotTables::none(),
        )
        .await
        .expect("durable relationship revisions migrate");
    let app = common::app_provider(6).await;
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("r22-key-{suffix}");

    let alice = Principal::new(
        TenantId(tenant.clone()),
        Region(region.clone()),
        PrincipalId("p:alice".into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let scope = myelin_storage::TenantScope::from_verified_token(&alice, alice.region.clone());

    let tstore1 = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(format!("{suffix}k1"))),
        DurableTupleBacking::new(app.clone()),
        handle.clone(),
    );
    tstore1
        .write_tuples(
            &scope,
            &alice,
            &[
                TupleDelta::Add(tuple("issue:X-1", "reader", "p:alice")),
                TupleDelta::Add(tuple("repo:team/app", "reader", "p:alice")),
            ],
            None,
            None,
            Timestamp("2026-07-15T00:00:00Z".into()),
        )
        .expect("durable grants write");

    let tstore2 = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(format!("{suffix}k2"))),
        DurableTupleBacking::new(app.clone()),
        handle.clone(),
    );
    let check = StoreBackedCheck::new(tstore2);
    let reader = Permission("reader".into());
    let d = |object: &str| {
        check
            .check(
                &alice,
                &reader,
                &ArtifactRef(object.into()),
                &latest(),
                None,
            )
            .expect("check evaluates")
    };

    assert_eq!(
        d("issue:X-1"),
        Decision::Allow,
        "bare spelling matches the durable grant"
    );
    assert_eq!(
        d(&format!("myelin://{tenant}/issues/issue/X-1")),
        Decision::Allow,
        "URN spelling matches the SAME durable grant (one canonical key)"
    );

    assert_eq!(
        d("repo:X-1"),
        Decision::Deny,
        "durable issue:X grant must not authorize repo:X"
    );
    assert_eq!(
        d(&format!("myelin://{tenant}/git/repo/X-1")),
        Decision::Deny,
        "…nor the URN spelling of repo:X"
    );

    assert_eq!(
        d("repo:team/app"),
        Decision::Allow,
        "the durable namespaced-slug grant matches its own check"
    );
    assert_eq!(
        d("repo:app"),
        Decision::Deny,
        "no aliasing onto the collapsed slug"
    );
}
