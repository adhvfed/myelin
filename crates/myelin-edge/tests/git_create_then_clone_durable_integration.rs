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

fn threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN - LEGAL".into(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_bootstrap_grant_admits_creator_via_a_separate_check_store() {
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

    let check = StoreBackedCheck::with_pg(app.clone(), kms.clone(), Arc::new(cell), handle.clone());
    for admit in check.admit_git_fragment() {
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "the Git fragment admits over the durable engine: {admit:?}"
        );
    }
    let authz = CheckBackedRepoAuthorizer::try_new(check, 300, &threshold())
        .expect("the wire authorizer constructs over the durable engine");

    let bootstrap_tuples = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(format!("{suffix}b"))),
        DurableTupleBacking::new(app.clone()),
        handle.clone(),
    );
    let bootstrap = TupleRepoBootstrap::new(bootstrap_tuples);

    let creator = principal("svc:creator", &tenant, &region);
    let repo = RepoLoc::new(&tenant, &region, "widgets");

    assert!(
        !authz.authorize_repo(&creator, &repo, RepoAccess::Read),
        "no tuple yet → the durable check denies (deny-by-default)"
    );

    bootstrap
        .grant_creator(&creator, &repo)
        .expect("the durable creator→admin bootstrap grant writes into rebac_tuple");

    assert!(
        authz.authorize_repo(&creator, &repo, RepoAccess::Read),
        "the creator's wire Read authorization admits via the durable bootstrap grant (cross-store)"
    );
    assert!(
        authz.authorize_repo(&creator, &repo, RepoAccess::Write),
        "the creator's wire Write authorization admits via the durable bootstrap grant (cross-store)"
    );

    let mallory = principal("svc:mallory", &tenant, &region);
    assert!(
        !authz.authorize_repo(&mallory, &repo, RepoAccess::Read),
        "an ungranted principal is denied Read over the durable path"
    );
    assert!(
        !authz.authorize_repo(&mallory, &repo, RepoAccess::Write),
        "an ungranted principal is denied Write over the durable path"
    );

    let _ = sqlx::query("DELETE FROM rebac_tuple WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await;
    let _ = sqlx::query("DELETE FROM outbox WHERE aggregate LIKE $1")
        .bind(format!("identity:tuple:{tenant}:%"))
        .execute(admin.db_pool())
        .await;
    println!(
        "OK: durable bootstrap grant (one TupleStore) admits the creator's Read+Write through a \
         SEPARATE StoreBackedCheck over the same rebac_tuple; an ungranted principal is denied."
    );
}
