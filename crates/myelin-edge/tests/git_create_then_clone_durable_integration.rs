//! # R2.1a-followup, Defect #1 — durable-PG create-then-clone authorization, over a LIVE Postgres.
//!
//! The R2.1a wire-authz correctness rests on ONE cross-store property: a creator→admin grant written
//! through the bootstrap writer's `TupleStore` is visible to the wire authorizer's SEPARATE
//! `StoreBackedCheck`, because both read/write the SAME durable `rebac_tuple` table. R2.1a proved this
//! from code (`tuples_in` reads live PG per check) but shipped only an IN-MEMORY unit test
//! (`bootstrap_grant_then_authorizer_admits_creator`, a single shared store). This is the DURABLE
//! analogue: it wires the PRODUCTION shape — TWO separate durable stores over ONE PG table — and
//! asserts the creator's wire authorization ADMITS (Read AND Write) via the bootstrap grant while an
//! ungranted principal is DENIED.
//!
//! Gated behind `--features integration` so the default `cargo test` stays DB-free. Runs ONLY against
//! the docker-compose dev stack:
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     cargo test -p myelin-edge --features integration \
//!       --test git_create_then_clone_durable_integration -- --nocapture
//!
//! Migrations run as the `myelin_admin` role (PG16 revokes `CREATE` on `public` for the app role);
//! the RLS-scoped runtime reads/writes go through the `myelin_app` (NOBYPASSRLS) role — matching how
//! the existing identity/edge integration tests split the two roles.
#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_edge::{
    repo_authz::{RepoAccess, RepoAuthorizer},
    CheckBackedRepoAuthorizer, RepoBootstrapGrants, TupleRepoBootstrap,
};
use myelin_git::core::RepoLoc;
use myelin_identity::{
    DataRole, FragmentAdmit, Principal, PrincipalId, PrincipalKind, PrincipalStatus,
};
use myelin_identity_service::{CellTokenAuthority, StoreBackedCheck, TupleStore};
use myelin_storage::migration::HotTables;
use myelin_storage::{
    identity_durable_migrations, DurableTupleBacking, KmsEngine, SubstrateProvider,
};
use myelin_substrate::FailStaticThreshold;
use myelin_tenancy::{Region, TenantId};

/// DDL runs as the migration/owner role (PG16 revokes `CREATE` on `public` for the app role).
fn admin_config(cfg: &MyelinConfig) -> MyelinConfig {
    let mut c = cfg.clone();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

/// A per-run unique suffix so a fresh run uses fresh `(tenant)` partitions (no cross-run collision).
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

/// A per-store-UNIQUE, lexically-monotonic id minter for the durable bootstrap tuple store. The
/// default `MonotonicMinter` resets to `0` per store, so a co-committed `iam.tuple_written` could mint
/// an `event_id` the global `outbox` `UNIQUE(event_id)` collapses when suites share the live DB. This
/// double reproduces the production ULID uniqueness via a per-store `base`.
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

/// The thresholds-file `[fail_static]` fixture (mirrors the repo_authz_live unit fixtures).
fn threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN — LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    }
}

fn principal(id: &str, tenant: &str, region: &str) -> Principal {
    Principal::new(
        TenantId(tenant.into()),
        Region(region.into()),
        PrincipalId(id.into()),
        PrincipalKind::Service,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

/// **Defect #1 — the durable cross-store proof.** Over the LIVE pool, a creator→admin grant written
/// through a durable `TupleRepoBootstrap` (one `TupleStore::with_pg`) is admitted by the wire
/// `CheckBackedRepoAuthorizer` backed by a SEPARATE durable `StoreBackedCheck::with_pg` — two stores
/// over the SAME `rebac_tuple` table, exactly as production wires them. An ungranted principal is
/// denied through the same authorizer (deny-by-default holds over the durable path too).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_bootstrap_grant_admits_creator_via_a_separate_check_store() {
    // ── Migrate as the admin role: the reused `rebac_tuple` edge set + RLS, plus the foundation
    //    `outbox` the tuple write co-commits its `iam.tuple_written` into. ──
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
        .migrate_foundation()
        .await
        .expect("foundation (outbox) migration");

    // ── The RLS-scoped runtime pool (myelin_app, NOBYPASSRLS, reset-on-release). ──
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
    let tenant = format!("r21a-fu-{suffix}");

    let kms = Arc::new(KmsEngine::new());
    let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell authority");

    // ── The WIRE AUTHORIZER's engine: a durable StoreBackedCheck over the app pool, with the Git
    //    ReBAC fragment admitted (else every compiled pull/push denies). This store is SEPARATE from
    //    the bootstrap writer's — it holds its OWN internal TupleStore over the SAME rebac_tuple. ──
    let check = StoreBackedCheck::with_pg(app.clone(), kms.clone(), Arc::new(cell), handle.clone());
    for admit in check.admit_git_fragment() {
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "the Git fragment admits over the durable engine: {admit:?}"
        );
    }
    let authz = CheckBackedRepoAuthorizer::try_new(check, 300, &threshold())
        .expect("the wire authorizer constructs over the durable engine");

    // ── The BOOTSTRAP WRITER over a SEPARATE durable TupleStore (its own with_pg instance) — the
    //    production two-stores-over-one-table shape (NOT check.tuples().clone()). ──
    let bootstrap_tuples = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(format!("{suffix}b"))),
        DurableTupleBacking::new(app.clone()),
        handle.clone(),
    );
    let bootstrap = TupleRepoBootstrap::new(bootstrap_tuples);

    let creator = principal("svc:creator", &tenant, &region);
    let repo = RepoLoc::new(&tenant, &region, "widgets");

    // Before the grant: deny-by-default (proves the admit below is the GRANT's doing, over live PG).
    assert!(
        !authz.authorize_repo(&creator, &repo, RepoAccess::Read),
        "no tuple yet → the durable check denies (deny-by-default)"
    );

    // Write the creator→admin grant through the SEPARATE bootstrap store.
    bootstrap
        .grant_creator(&creator, &repo)
        .expect("the durable creator→admin bootstrap grant writes into rebac_tuple");

    // THE CROSS-STORE ASSERTION: the wire authorizer's SEPARATE StoreBackedCheck resolves the grant
    // the bootstrap writer committed — Read AND Write both ADMIT (admin ⊆ pull and ⊆ push).
    assert!(
        authz.authorize_repo(&creator, &repo, RepoAccess::Read),
        "the creator's wire Read authorization admits via the durable bootstrap grant (cross-store)"
    );
    assert!(
        authz.authorize_repo(&creator, &repo, RepoAccess::Write),
        "the creator's wire Write authorization admits via the durable bootstrap grant (cross-store)"
    );

    // An UNGRANTED in-tenant principal is DENIED for both accesses (per-object tuples, no wildcard).
    let mallory = principal("svc:mallory", &tenant, &region);
    assert!(
        !authz.authorize_repo(&mallory, &repo, RepoAccess::Read),
        "an ungranted principal is denied Read over the durable path"
    );
    assert!(
        !authz.authorize_repo(&mallory, &repo, RepoAccess::Write),
        "an ungranted principal is denied Write over the durable path"
    );

    // ── Cleanup (admin role — RLS-bypassing owner). ──
    let _ = sqlx::query("DELETE FROM rebac_tuple WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await;
    let _ = sqlx::query("DELETE FROM outbox WHERE aggregate LIKE $1")
        .bind(format!("iam:tuple:{tenant}:%"))
        .execute(admin.db_pool())
        .await;
    println!(
        "OK: durable bootstrap grant (one TupleStore) admits the creator's Read+Write through a \
         SEPARATE StoreBackedCheck over the same rebac_tuple; an ungranted principal is denied."
    );
}
