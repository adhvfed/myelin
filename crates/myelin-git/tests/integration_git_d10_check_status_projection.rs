//! # GIT-D10 part (a) — the STORE-BACKED `check_status` projection over LIVE Postgres (P-281, M3)
//!
//! **Contract:** `contract-index.md` row 5.9 (the Git↔CI `CheckStatus` seam — Git is the consumer +
//! gate; the projection is keyed `(commit_oid, context)`, last-writer-wins by monotonic `run_attempt`,
//! idempotent on `event_id`). Owning architecture:
//! `git-hosting/architecture/02-internals-and-algorithms.md` §6.1 (the `check_status` consumer + the
//! supersession algorithm). **Reconciliation:** X-1 (the bus is at-least-once → the stale-lower-attempt
//! drop is MANDATORY). **Drill:** GIT-D10 part (a) — out-of-order/dup `ci.check.updated` →
//! `run_attempt`-monotonic supersession holds the correct current row, dropping stale lower attempts
//! (EXACTLY 1 current row per key; idempotent on `event_id`).
//!
//! This is the DEV-REAL data-layer proof the seam-floor named (`check_status.rs` §"what is still a
//! FLOOR", leg 2): the real `check_status` table + the migration + the same-tx `consumer_dedup` write,
//! against the docker-compose dev Postgres (NOT a mock — the binding policy floor is over for anything
//! Docker can run). The PRODUCER is SYNTHETIC here (CI's real emit is EB-27/M4 — the M4 co-gate
//! GIT-D10 / CI-D8 re-confirms this end-to-end).
//!
//! Run against the dev stack:
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-git --features integration \
//!     --test integration_git_d10_check_status_projection -- --nocapture
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

/// The dev default mirrors the myelin-config dev DATABASE_URL (the admin role so the test owns its
/// scratch tables — same convention as the identity ReBAC integration test).
fn admin_url() -> String {
    let app = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into());
    app.replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// The SYNTHETIC `ci.check.updated` fact (CI's real producer is EB-27/M4). One commit; vary the
/// context, attempt, state, trust to drive the supersession + the gate.
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

/// A stable `event_id` per (context, attempt) so a re-delivery carries the SAME id (the dedup key) —
/// the same convention the Bus carriage drill uses.
fn event_id(context: &str, attempt: u32) -> String {
    format!("gitp20-{context}-a{attempt}")
}

/// A per-TEST unique table suffix — the pid plus a monotonic counter so the three concurrent tests
/// never race on the same `CREATE TABLE` (each owns its own scratch projection).
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

/// Holding the protected-push admission transaction must not consume the only connection needed by
/// the nested ref/outbox mutation. With both lanes bounded to one connection, acquiring the ordinary
/// pool inside the admission callback succeeds only when the composition uses distinct pools.
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

/// **GIT-D10 part (a) — out-of-order + dup `ci.check.updated` → exactly 1 current row per key.**
///
/// The scenario the X-1 seam must survive (the bus is at-least-once):
/// - `build` attempt 1 (failure) lands, then attempt 2 (a re-run, success) supersedes it;
/// - a DISTINCT context `test` attempt 1 (success) coexists (a second row, same commit);
/// - the at-least-once transport RE-DELIVERS the stale `build` attempt 1 (a LATE LOWER attempt) — it
///   is DROPPED in SQL (the supersession `WHERE`), the current row stays attempt 2;
/// - a DUPLICATE of `build` attempt 2 (SAME `event_id`) — the `consumer_dedup` guard absorbs it.
///
/// The green artifact: EXACTLY ONE current row per `(commit_oid, context)` (here 2 keys → 2 rows), the
/// `build` row at the highest attempt (2, success), and the stale/dup re-deliveries observably
/// no-op'd.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn check_status_supersession_holds_one_current_row_per_key() {
    let proj = connect().await;
    let commit = GitOid(COMMIT.into());

    // build#1 (failure) — seeds the row.
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
    // test#1 (success) — a DISTINCT context → a second row on the same commit.
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
    // build#2 (re-run, success) — supersedes build#1 IN PLACE (the >= rule).
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

    // The at-least-once transport RE-DELIVERS the stale build#1 (a NEW event_id so the dedup guard does
    // NOT absorb it — this isolates the SUPERSESSION drop). The supersession `WHERE` drops it in SQL.
    assert_eq!(
        proj.apply(
            "gitp20-build-a1-redelivered",
            REGION,
            &fact("build", 1, CheckState::Failure, TrustTier::Trusted)
        )
        .await
        .unwrap(),
        StoreApplyOutcome::DroppedStale,
        "a late LOWER attempt is dropped in SQL — the newer row is not clobbered"
    );

    // A DUPLICATE of build#2 (the SAME event_id) — the consumer_dedup guard absorbs it (idempotent).
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

    // THE GREEN ARTIFACT: exactly ONE current row per key. 2 keys (build, test) → exactly 2 rows.
    assert_eq!(
        proj.row_count_for_commit(TENANT, REGION, REPO, &commit)
            .await
            .unwrap(),
        2,
        "exactly one current row per (commit_oid, context) — no duplicate/ghost rows"
    );

    // The current build row is the highest attempt (2, the re-run success), NOT the stale failure.
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

    // The test row is its own attempt-1 success (independent key, untouched by the build supersession).
    let test_row = proj
        .current(TENANT, REGION, REPO, &commit, "ci", "test")
        .await
        .unwrap()
        .expect("the test row is present");
    assert_eq!(test_row.run_attempt, 1);
    assert_eq!(test_row.state, CheckState::Success);

    proj.drop_tables().await.unwrap();
}

/// **GIT-D10 part (a) — the supersession is monotonic on the COUNTER regardless of arrival order.**
/// Apply attempts in SCRAMBLED order (3, 1, 2); the current row is always the highest attempt (3),
/// proving the `>=`-in-SQL rule is order-independent (clocks are not authority; the counter is — X-1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supersession_is_order_independent_highest_attempt_wins() {
    let proj = connect().await;
    let commit = GitOid(COMMIT.into());

    // Scrambled arrival: attempt 3 (success) first, then 1 (failure), then 2 (error). Each a distinct
    // event_id so the dedup guard never fires — this isolates the supersession ordering.
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

    // Exactly one row, at attempt 3 (the highest), success (the attempt-3 state).
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

/// **GIT-D10 part (a) — idempotent on `event_id` (the same fact re-applied is a no-op, 0 dup).** A
/// re-delivery of the SAME `event_id` is absorbed by the `consumer_dedup` guard; the row is applied
/// exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idempotent_on_event_id_zero_dup() {
    let proj = connect().await;
    let commit = GitOid(COMMIT.into());
    let f = fact("build", 5, CheckState::Success, TrustTier::Trusted);

    assert_eq!(
        proj.apply("evt-once", REGION, &f).await.unwrap(),
        StoreApplyOutcome::Superseded
    );
    // Re-deliver the SAME event_id twice — both are the effectively-once no-op.
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

/// The repository is part of the projection key. Two repositories may legitimately report the
/// same commit OID and context; neither fact may overwrite or satisfy the other repository's gate.
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
