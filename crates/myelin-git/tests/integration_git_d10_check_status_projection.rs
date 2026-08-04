#![cfg(feature = "integration")]

use myelin_git::check_status::{
    CheckContext, CheckState, CheckStatus, GitOid, HumanisedRef, Timestamp, TrustTier,
};
use myelin_git::check_status_store::{PgCheckStatusProjection, StoreApplyOutcome};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{SubstrateProvider, TenantScope};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use std::collections::BTreeMap;

const TENANT: &str = "acme";
const REGION: &str = "fr-par";
const REPO: &str = "myelin://acme/git/repo/core";
const COMMIT: &str = "abc123def";

fn admin_url() -> String {
    let app = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into());
    app.replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn fact_for_repo(
    repo: &str,
    context: &str,
    attempt: u32,
    state: CheckState,
    trust: TrustTier,
) -> CheckStatus {
    let mut args = BTreeMap::new();
    args.insert("context".to_string(), context.to_string());
    CheckStatus {
        tenant: TenantId(TENANT.into()),
        repo: ArtifactRef(repo.into()),
        commit_oid: GitOid(COMMIT.into()),
        context: CheckContext::ci(context),
        state,
        required: true,
        run: ArtifactRef(format!("myelin://{TENANT}/ci/run/{attempt}")),
        run_attempt: attempt,
        trust_tier: trust,
        details_ref: ArtifactRef(format!("myelin://{TENANT}/ci/run/{attempt}#step-2")),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args,
        },
        started_at: Timestamp("2026-06-22T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-22T00:01:00Z".into())),
        cost_settled: true,
    }
}

fn fact(context: &str, attempt: u32, state: CheckState, trust: TrustTier) -> CheckStatus {
    fact_for_repo(REPO, context, attempt, state, trust)
}

fn event_id(context: &str, attempt: u32) -> String {
    format!("gitp20-{context}-a{attempt}")
}

fn unique_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    (u64::from(std::process::id()) << 16) | n
}

async fn connect() -> PgCheckStatusProjection {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres (is the stack up? `docker compose -f docker-compose.dev.yml up -d --wait`)");
    let suffix = unique_suffix();
    let table = format!("check_status_p20_{suffix}");
    let dedup = format!("consumer_dedup_p20_{suffix}");
    PgCheckStatusProjection::connect(pool, &table, &dedup, "git.check_status")
        .await
        .expect("run the check_status migration")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_push_admission_has_a_reserved_pool_lane_at_max_capacity() {
    let mut config = myelin_config::MyelinConfig::dev();
    config.database_url = admin_url();
    config.region = REGION.into();
    let ordinary = SubstrateProvider::connect(config.clone(), 1)
        .await
        .expect("one-connection ordinary lane");
    let admission = SubstrateProvider::connect(config, 1)
        .await
        .expect("one-connection admission lane");
    let projection = PgCheckStatusProjection::production(
        ordinary.clone(),
        admission,
        tokio::runtime::Handle::current(),
    );
    let principal = Principal::new(
        TenantId(TENANT.into()),
        Region(REGION.into()),
        PrincipalId("git-admission-proof".into()),
        PrincipalKind::Service,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let scope = TenantScope::from_verified_token(&principal, Region(REGION.into()));
    let ordinary_pool = ordinary.db_pool().clone();
    let nested_lane_available = projection
        .with_admission_snapshot(&scope, REPO, &[], move |_| {
            ordinary_pool.try_acquire().is_some()
        })
        .expect("admission transaction");
    assert!(
        nested_lane_available,
        "the admission lock must not pin the ordinary pool's sole connection"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn check_status_supersession_holds_one_current_row_per_key() {
    let proj = connect().await;
    let commit = GitOid(COMMIT.into());

    assert_eq!(
        proj.apply(
            &event_id("build", 1),
            REGION,
            &fact("build", 1, CheckState::Failure, TrustTier::Trusted)
        )
        .await
        .unwrap(),
        StoreApplyOutcome::Superseded
    );
    assert_eq!(
        proj.apply(
            &event_id("test", 1),
            REGION,
            &fact("test", 1, CheckState::Success, TrustTier::Trusted)
        )
        .await
        .unwrap(),
        StoreApplyOutcome::Superseded
    );
    assert_eq!(
        proj.apply(
            &event_id("build", 2),
            REGION,
            &fact("build", 2, CheckState::Success, TrustTier::Trusted)
        )
        .await
        .unwrap(),
        StoreApplyOutcome::Superseded
    );

    assert_eq!(
        proj.apply(
            "gitp20-build-a1-redelivered",
            REGION,
            &fact("build", 1, CheckState::Failure, TrustTier::Trusted)
        )
        .await
        .unwrap(),
        StoreApplyOutcome::DroppedStale,
        "a late LOWER attempt is dropped in SQL - the newer row is not clobbered"
    );

    assert_eq!(
        proj.apply(
            &event_id("build", 2),
            REGION,
            &fact("build", 2, CheckState::Success, TrustTier::Trusted)
        )
        .await
        .unwrap(),
        StoreApplyOutcome::DuplicateEvent,
        "a re-delivered event_id is the effectively-once no-op"
    );

    assert_eq!(
        proj.row_count_for_commit(TENANT, REGION, REPO, &commit)
            .await
            .unwrap(),
        2,
        "exactly one current row per (commit_oid, context) - no duplicate/ghost rows"
    );

    let build_row = proj
        .current(TENANT, REGION, REPO, &commit, "ci", "build")
        .await
        .unwrap()
        .expect("the build row is present");
    assert_eq!(
        build_row.run_attempt, 2,
        "the current build row is the highest attempt"
    );
    assert_eq!(
        build_row.state,
        CheckState::Success,
        "the re-run success is current, not the stale failure"
    );

    let test_row = proj
        .current(TENANT, REGION, REPO, &commit, "ci", "test")
        .await
        .unwrap()
        .expect("the test row is present");
    assert_eq!(test_row.run_attempt, 1);
    assert_eq!(test_row.state, CheckState::Success);

    proj.drop_tables().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supersession_is_order_independent_highest_attempt_wins() {
    let proj = connect().await;
    let commit = GitOid(COMMIT.into());

    assert_eq!(
        proj.apply(
            "scramble-a3",
            REGION,
            &fact("lint", 3, CheckState::Success, TrustTier::Trusted)
        )
        .await
        .unwrap(),
        StoreApplyOutcome::Superseded
    );
    assert_eq!(
        proj.apply(
            "scramble-a1",
            REGION,
            &fact("lint", 1, CheckState::Failure, TrustTier::Trusted)
        )
        .await
        .unwrap(),
        StoreApplyOutcome::DroppedStale,
        "attempt 1 < the stored 3 → dropped"
    );
    assert_eq!(
        proj.apply(
            "scramble-a2",
            REGION,
            &fact("lint", 2, CheckState::Error, TrustTier::Trusted)
        )
        .await
        .unwrap(),
        StoreApplyOutcome::DroppedStale,
        "attempt 2 < the stored 3 → dropped"
    );

    assert_eq!(
        proj.row_count_for_commit(TENANT, REGION, REPO, &commit)
            .await
            .unwrap(),
        1
    );
    let row = proj
        .current(TENANT, REGION, REPO, &commit, "ci", "lint")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.run_attempt, 3,
        "the highest attempt is current regardless of arrival order"
    );
    assert_eq!(row.state, CheckState::Success);

    proj.drop_tables().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idempotent_on_event_id_zero_dup() {
    let proj = connect().await;
    let commit = GitOid(COMMIT.into());
    let f = fact("build", 5, CheckState::Success, TrustTier::Trusted);

    assert_eq!(
        proj.apply("evt-once", REGION, &f).await.unwrap(),
        StoreApplyOutcome::Superseded
    );
    assert_eq!(
        proj.apply("evt-once", REGION, &f).await.unwrap(),
        StoreApplyOutcome::DuplicateEvent
    );
    assert_eq!(
        proj.apply("evt-once", REGION, &f).await.unwrap(),
        StoreApplyOutcome::DuplicateEvent
    );

    assert_eq!(
        proj.row_count_for_commit(TENANT, REGION, REPO, &commit)
            .await
            .unwrap(),
        1,
        "applied exactly once"
    );
    let row = proj
        .current(TENANT, REGION, REPO, &commit, "ci", "build")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.run_attempt, 5);

    proj.drop_tables().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identical_commit_and_context_are_isolated_by_repository() {
    const OTHER_REPO: &str = "myelin://acme/git/repo/other";

    let proj = connect().await;
    let commit = GitOid(COMMIT.into());
    proj.apply(
        "repo-core-build",
        REGION,
        &fact_for_repo(REPO, "build", 1, CheckState::Success, TrustTier::Trusted),
    )
    .await
    .unwrap();
    proj.apply(
        "repo-other-build",
        REGION,
        &fact_for_repo(
            OTHER_REPO,
            "build",
            7,
            CheckState::Failure,
            TrustTier::Trusted,
        ),
    )
    .await
    .unwrap();

    assert_eq!(
        proj.row_count_for_commit(TENANT, REGION, REPO, &commit)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        proj.row_count_for_commit(TENANT, REGION, OTHER_REPO, &commit)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        proj.current(TENANT, REGION, REPO, &commit, "ci", "build")
            .await
            .unwrap()
            .unwrap()
            .state,
        CheckState::Success
    );
    assert_eq!(
        proj.current(TENANT, REGION, OTHER_REPO, &commit, "ci", "build")
            .await
            .unwrap()
            .unwrap()
            .state,
        CheckState::Failure
    );

    proj.drop_tables().await.unwrap();
}
