#![cfg(feature = "integration")]

use myelin_git::check_status::{
    CheckContext, CheckState, CheckStatus, GitOid, HumanisedRef, Timestamp, TrustTier,
};
use myelin_git::check_status_store::PgCheckStatusProjection;
use myelin_git::merge_gate::{MergeGateOutcome, MergeGatePolicy, UnmetReason};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::collections::BTreeMap;

const TENANT: &str = "acme";
const REGION: &str = "fr-par";
const REPO: &str = "myelin://acme/git/repo/core";
const HEAD: &str = "feedface00";

fn admin_url() -> String {
    let app = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into());
    app.replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn fact(context: &str, attempt: u32, state: CheckState, trust: TrustTier) -> CheckStatus {
    let mut args = BTreeMap::new();
    args.insert("context".to_string(), context.to_string());
    CheckStatus {
        tenant: TenantId(TENANT.into()),
        repo: ArtifactRef(REPO.into()),
        commit_oid: GitOid(HEAD.into()),
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
        .expect("connect to dev Postgres (is the stack up? `run `fed test:backend``)");
    let suffix = unique_suffix();
    let table = format!("check_status_p21_{suffix}");
    let dedup = format!("consumer_dedup_p21_{suffix}");
    PgCheckStatusProjection::connect(pool, &table, &dedup, "git.check_status")
        .await
        .expect("run the check_status migration")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn merge_gate_blocks_until_required_set_complete_over_postgres() {
    let proj = connect().await;
    let head = GitOid(HEAD.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build", "ci/test"]).unwrap();

    proj.apply(
        "p21-build-1",
        REGION,
        &fact("build", 1, CheckState::Success, TrustTier::Trusted),
    )
    .await
    .unwrap();

    match proj
        .merge_gate(TENANT, REGION, REPO, &head, &policy, &[])
        .await
        .unwrap()
    {
        MergeGateOutcome::Blocked { unmet } => {
            assert_eq!(unmet.len(), 1);
            assert_eq!(unmet[0].context, CheckContext::ci("test"));
            assert_eq!(unmet[0].reason, UnmetReason::Missing);
        }
        MergeGateOutcome::Admitted => panic!("a missing required context must block over Postgres"),
    }

    proj.apply(
        "p21-test-1",
        REGION,
        &fact("test", 1, CheckState::Success, TrustTier::Trusted),
    )
    .await
    .unwrap();

    assert_eq!(
        proj.merge_gate(TENANT, REGION, REPO, &head, &policy, &[])
            .await
            .unwrap(),
        MergeGateOutcome::Admitted,
        "a complete green required set admits over Postgres"
    );

    proj.drop_tables().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fork_gate_neutral_until_endorsed_over_postgres() {
    let proj = connect().await;
    let head = GitOid(HEAD.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();

    proj.apply(
        "p21-fork-1",
        REGION,
        &fact("build", 1, CheckState::Success, TrustTier::UntrustedFork),
    )
    .await
    .unwrap();

    match proj
        .merge_gate(TENANT, REGION, REPO, &head, &policy, &[])
        .await
        .unwrap()
    {
        MergeGateOutcome::Blocked { unmet } => {
            assert_eq!(unmet[0].reason, UnmetReason::UntrustedForkNeutral);
        }
        MergeGateOutcome::Admitted => {
            panic!("an un-endorsed fork success must block over Postgres")
        }
    }

    assert_eq!(
        proj.merge_gate(
            TENANT,
            REGION,
            REPO,
            &head,
            &policy,
            &[CheckContext::ci("build")],
        )
        .await
        .unwrap(),
        MergeGateOutcome::Admitted,
        "a maintainer-endorsed fork success admits over Postgres"
    );

    proj.apply(
        "p21-rerun-2",
        REGION,
        &fact("build", 2, CheckState::Success, TrustTier::Trusted),
    )
    .await
    .unwrap();
    assert_eq!(
        proj.merge_gate(TENANT, REGION, REPO, &head, &policy, &[])
            .await
            .unwrap(),
        MergeGateOutcome::Admitted,
        "a trusted re-run supersedes the fork fact and admits with no explicit endorsement"
    );

    proj.drop_tables().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn superseding_failure_reblocks_over_postgres() {
    let proj = connect().await;
    let head = GitOid(HEAD.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();

    proj.apply(
        "p21-ok-1",
        REGION,
        &fact("build", 1, CheckState::Success, TrustTier::Trusted),
    )
    .await
    .unwrap();
    assert_eq!(
        proj.merge_gate(TENANT, REGION, REPO, &head, &policy, &[])
            .await
            .unwrap(),
        MergeGateOutcome::Admitted
    );

    proj.apply(
        "p21-fail-2",
        REGION,
        &fact("build", 2, CheckState::Failure, TrustTier::Trusted),
    )
    .await
    .unwrap();
    match proj
        .merge_gate(TENANT, REGION, REPO, &head, &policy, &[])
        .await
        .unwrap()
    {
        MergeGateOutcome::Blocked { unmet } => {
            assert_eq!(
                unmet[0].reason,
                UnmetReason::NotGreen {
                    state: CheckState::Failure
                }
            );
        }
        MergeGateOutcome::Admitted => panic!("a superseding failure must re-block over Postgres"),
    }

    proj.drop_tables().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn successful_but_unsettled_check_blocks_over_postgres() {
    let proj = connect().await;
    let head = GitOid(HEAD.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();
    let mut unsettled = fact("build", 1, CheckState::Success, TrustTier::Trusted);
    unsettled.cost_settled = false;

    proj.apply("p21-unsettled-1", REGION, &unsettled)
        .await
        .unwrap();

    match proj
        .merge_gate(TENANT, REGION, REPO, &head, &policy, &[])
        .await
        .unwrap()
    {
        MergeGateOutcome::Blocked { unmet } => {
            assert_eq!(unmet[0].reason, UnmetReason::CostUnsettled);
        }
        MergeGateOutcome::Admitted => {
            panic!("a successful check with an unsettled reservation must block")
        }
    }

    proj.drop_tables().await.unwrap();
}
