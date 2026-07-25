//! # R2.2 — the type-qualified object key over the LIVE durable (PG) tuple store.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo test` stays DB-free. Runs
//! against the docker-compose dev stack:
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     cargo test -p myelin-identity-service --features integration \
//!       --test integration_r22_typed_object_key_durable -- --nocapture
//!
//! The DB-free unit tests already pin the R2.2 keying on the in-memory double; this proves the SAME
//! properties through the PRODUCTION `with_pg` path (the durable `rebac_tuple` edge set read back by
//! a FRESH store instance — i.e. the canonicalisation is read-side and store-agnostic, so a durable
//! grant behaves identically across restarts):
//!   1. a DURABLE grant on `issue:<X>` does NOT authorize `repo:<X>` (cross-type confusion dead on
//!      the durable path, in both the bare and the URN check spelling);
//!   2. the bare (`issue:<X>`) and URN (`myelin://<t>/issues/issue/<X>`) spellings of the SAME
//!      object both authorize off the ONE durable grant (write==read keying, zero migration);
//!   3. a NAMESPACED slug grant (`repo:team/app` — the R2.1a git grammar) authorizes its own check
//!      and does NOT alias onto `repo:app` (the R2.1a carry-forward, durable leg).
#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_events::Timestamp;
use myelin_identity::{
    Consistency, ConsistencyMode, DataRole, Decision, IdentityService, ObjectId, Permission,
    Principal, PrincipalId, PrincipalKind, PrincipalStatus, RelName, RelationTuple, TupleDelta,
    Zookie,
};
use myelin_identity_service::tuple_store::TupleStore;
use myelin_identity_service::StoreBackedCheck;
use myelin_storage::migration::HotTables;
use myelin_storage::{identity_durable_migrations, DurableTupleBacking, SubstrateProvider};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

/// DDL runs as the migration/owner role (PG16 revokes `CREATE` on `public` for the app role).
fn admin_config(cfg: &MyelinConfig) -> MyelinConfig {
    let mut c = cfg.clone();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

/// A per-run unique suffix so a fresh run uses fresh `(tenant)` partitions.
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

/// A per-store-unique minter (mirrors `integration_mr007_identity_durable.rs`): the co-committed
/// `identity.tuple.written` rows need globally-unique event ids across suites sharing the live DB.
struct UniqueMinter {
    base: String,
    n: std::sync::atomic::AtomicU64,
}

impl UniqueMinter {
    fn new(base: impl Into<String>) -> Self {
        UniqueMinter { base: base.into(), n: std::sync::atomic::AtomicU64::new(0) }
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
    Consistency { at_least: Zookie(String::new()), mode: ConsistencyMode::Strong }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_grant_on_issue_x_does_not_authorize_repo_x() {
    // Migrate (admin role), then run through the NOBYPASSRLS app role.
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
    let app = match SubstrateProvider::connect(MyelinConfig::dev(), 6).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("r22-key-{suffix}");

    // The verified subject (tenant-from-token: the check scope derives from alice's own
    // tenant/region — the same partition the grant is written into).
    let alice = Principal::new(
        TenantId(tenant.clone()),
        Region(region.clone()),
        PrincipalId("p:alice".into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let scope = myelin_storage::TenantScope::from_verified_token(&alice, alice.region.clone());

    // ---- Write the DURABLE grants through store instance #1 (the production with_pg path). ----
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
                // The grant under test: reader on the ISSUE X-1 (never the repo).
                TupleDelta::Add(tuple("issue:X-1", "reader", "p:alice")),
                // The R2.1a namespaced-slug grant.
                TupleDelta::Add(tuple("repo:team/app", "reader", "p:alice")),
            ],
            None,
            None,
            Timestamp("2026-07-15T00:00:00Z".into()),
        )
        .expect("durable grants write");

    // ---- Check through a FRESH store instance over the SAME live pool (restart-shaped). ----
    let tstore2 = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(format!("{suffix}k2"))),
        DurableTupleBacking::new(app.clone()),
        handle.clone(),
    );
    let check = StoreBackedCheck::new(tstore2);
    let reader = Permission("reader".into());
    let d = |object: &str| {
        check
            .check(&alice, &reader, &ArtifactRef(object.into()), &latest(), None)
            .expect("check evaluates")
    };

    // (1) The durable issue grant authorizes the ISSUE — in BOTH spellings (write==read keying).
    assert_eq!(d("issue:X-1"), Decision::Allow, "bare spelling matches the durable grant");
    assert_eq!(
        d(&format!("myelin://{tenant}/issues/issue/X-1")),
        Decision::Allow,
        "URN spelling matches the SAME durable grant (one canonical key)"
    );

    // (2) The SAME trailing id on the REPO type is DENIED — cross-type confusion dead durably.
    assert_eq!(d("repo:X-1"), Decision::Deny, "durable issue:X grant must not authorize repo:X");
    assert_eq!(
        d(&format!("myelin://{tenant}/git/repo/X-1")),
        Decision::Deny,
        "…nor the URN spelling of repo:X"
    );

    // (3) The namespaced slug keys whole on the durable path: its own check admits; the collapse
    //     target `repo:app` does not alias.
    assert_eq!(
        d("repo:team/app"),
        Decision::Allow,
        "the durable namespaced-slug grant matches its own check"
    );
    assert_eq!(d("repo:app"), Decision::Deny, "no aliasing onto the collapsed slug");
}
