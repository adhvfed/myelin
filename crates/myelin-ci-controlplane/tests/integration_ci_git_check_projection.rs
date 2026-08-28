#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    assemble_check_status, CheckEmitContext, CheckProvider, CheckState, CheckStatusUpdate,
    CostPosture, TrustTier,
};
use myelin_events::{
    Actor, CorrelationId, DedupLedger, Delivered, EventDraft, EventEnvelope, EventId, Message,
    Timestamp, CONSUMER_DEAD_LETTER_MIGRATION, CONSUMER_DEDUP_MIGRATION,
};
use myelin_git::check_status::{CheckState as GitCheckState, GitOid};
use myelin_git::check_status_store::{
    build_durable_check_consumer, check_status_hot_tables, check_status_migrations, projection_ddl,
    PgCheckStatusProjection, CHECK_STATUS_CONSUMER,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::events_durable::{DurableDeadLetterBacking, DurableDedupBacking};
use myelin_storage::{foundation_migrations, PgMigrator};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool};
use std::sync::Arc;

const TENANT: &str = "acme";
const REGION: &str = "fr-par";
const REPO: &str = "myelin://acme/git/repo/team/core";
const COMMIT: &str = "feedface00000000000000000000000000000000";

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

async fn isolated_pool(schema: &str) -> PgPool {
    let search_path = schema.to_string();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |connection, _| {
            let search_path = search_path.clone();
            Box::pin(async move {
                connection
                    .execute(format!("SET search_path TO {search_path}").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&admin_url())
        .await
        .expect("connect to live dev Postgres");
    pool.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    pool.execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .unwrap();
    pool.execute(CONSUMER_DEDUP_MIGRATION).await.unwrap();
    pool.execute(CONSUMER_DEAD_LETTER_MIGRATION).await.unwrap();
    pool
}

fn producer_context(attempt: u32) -> CheckEmitContext {
    CheckEmitContext {
        tenant: TENANT.into(),
        repo: REPO.into(),
        commit_oid: COMMIT.into(),
        run_ref: format!("myelin://{TENANT}/ci/run/run-{attempt}"),
        run_attempt: attempt,
        trust_tier: TrustTier::Trusted,
        started_at: "2026-07-23T00:00:00Z".into(),
        completed_at: Some("2026-07-23T00:01:00Z".into()),
    }
}

fn envelope(event_id: &str, draft: EventDraft) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: draft.type_,
        schema_ver: 1,
        tenant: TenantId(TENANT.into()),
        region: Region(REGION.into()),
        actor: Actor(Principal::stub(
            PrincipalId("ci-controlplane".into()),
            PrincipalKind::Service,
            TenantId(TENANT.into()),
        )),
        subject: draft.subject,
        aggregate: draft.aggregate,
        causation_id: None,
        correlation_id: CorrelationId("ci-git-check-projection-proof".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: draft.contains_personal_data,
        data_role: draft.data_role,
        visibility: draft.visibility,
        pii_key_ref: draft.pii_key_ref,
        occurred_at: Timestamp("2026-07-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-23T00:00:01Z".into()),
        payload: draft.payload,
    }
}

fn check_event(
    event_id: &str,
    attempt: u32,
    state: CheckState,
    cost: CostPosture,
) -> EventEnvelope {
    let status = CheckStatusUpdate::required(CheckProvider::Ci, "build", state).with_cost(cost);
    envelope(
        event_id,
        assemble_check_status(&producer_context(attempt), &status),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_ci_draft_cocommits_into_git_projection_and_redelivers_without_loss() {
    let nonce = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let schema = format!("ci_git_check_{nonce}");
    let pool = isolated_pool(&schema).await;
    let runtime = tokio::runtime::Handle::current();
    let dedup = DedupLedger::durable(Arc::new(DurableDedupBacking::new(
        pool.clone(),
        runtime.clone(),
    )));
    let dead_letters = Arc::new(DurableDeadLetterBacking::new(pool.clone(), runtime.clone()));
    let consumer = build_durable_check_consumer(
        runtime,
        REGION,
        dedup,
        dead_letters,
        myelin_events::DurableWorkerAdmission::new(64, 32, 16).unwrap(),
    )
    .expect("bind consumer");

    let first = check_event(
        "ci-git-check-failure-1",
        1,
        CheckState::Failure,
        CostPosture::Settled,
    );
    let first_message = Message {
        subject: first.subject.0.clone(),
        envelope: first.clone(),
    };

    assert_eq!(
        tokio::task::block_in_place(|| consumer.deliver(&first_message)),
        Delivered::Retried(2)
    );
    let marked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM consumer_dedup WHERE consumer=$1 AND event_id=$2)",
    )
    .bind(CHECK_STATUS_CONSUMER)
    .bind(&first.event_id.0)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!marked, "retry rolled back the dedup mark");

    pool.execute(
        projection_ddl("check_status", "check_projection_test_dedup")
            .unwrap()
            .as_str(),
    )
    .await
    .unwrap();
    let projection = PgCheckStatusProjection::connect(
        pool.clone(),
        "check_status",
        "check_projection_test_dedup",
        CHECK_STATUS_CONSUMER,
    )
    .await
    .unwrap();

    assert_eq!(
        tokio::task::block_in_place(|| consumer.deliver(&first_message)),
        Delivered::Acked,
        "redelivery co-commits the mark and projection"
    );
    assert_eq!(
        tokio::task::block_in_place(|| consumer.deliver(&first_message)),
        Delivered::Deduplicated
    );

    let success = check_event(
        "ci-git-check-success-2",
        2,
        CheckState::Success,
        CostPosture::Settled,
    );
    assert_eq!(
        tokio::task::block_in_place(|| consumer.deliver(&Message {
            subject: success.subject.0.clone(),
            envelope: success.clone(),
        })),
        Delivered::Acked
    );
    let mut late = check_event(
        "ci-git-check-late-1",
        1,
        CheckState::Failure,
        CostPosture::Settled,
    );
    late.recorded_at = Timestamp("2026-07-23T00:02:00Z".into());
    assert_eq!(
        tokio::task::block_in_place(|| consumer.deliver(&Message {
            subject: late.subject.0.clone(),
            envelope: late,
        })),
        Delivered::Acked,
        "a distinct late event is acknowledged after its stale attempt is dropped"
    );

    let current = projection
        .current(TENANT, REGION, REPO, &GitOid(COMMIT.into()), "ci", "build")
        .await
        .unwrap()
        .expect("Git projected CI's check");
    assert_eq!(current.run_attempt, 2);
    assert_eq!(current.state, GitCheckState::Success);
    assert!(current.cost_settled);
    assert_eq!(
        projection
            .row_count_for_commit(TENANT, REGION, REPO, &GitOid(COMMIT.into()))
            .await
            .unwrap(),
        1,
        "supersession retained one current row"
    );

    let mut forged = check_event(
        "ci-git-check-forged-provenance",
        3,
        CheckState::Success,
        CostPosture::Settled,
    );
    forged.aggregate.0.push_str("-forged");
    assert!(matches!(
        tokio::task::block_in_place(|| consumer.deliver(&Message {
            subject: forged.subject.0.clone(),
            envelope: forged,
        })),
        Delivered::DeadLettered(_)
    ));
    for (event_id, mutate) in [
        (
            "ci-git-check-forged-provider",
            ("context.provider", "external"),
        ),
        (
            "ci-git-check-cross-tenant-run",
            ("run", "myelin://other/ci/run/run-3"),
        ),
        (
            "ci-git-check-noncanonical-details",
            ("details_ref", "myelin://acme/ci/run/run-3#summary"),
        ),
        (
            "ci-git-check-normalized-details",
            ("details_ref", "myelin://acme/ci/run/run-3#step-003"),
        ),
    ] {
        let mut forged = check_event(event_id, 3, CheckState::Success, CostPosture::Settled);
        match mutate.0 {
            "context.provider" => forged.payload["context"]["provider"] = mutate.1.into(),
            field => forged.payload[field] = mutate.1.into(),
        }
        assert!(matches!(
            tokio::task::block_in_place(|| consumer.deliver(&Message {
                subject: forged.subject.0.clone(),
                envelope: forged,
            })),
            Delivered::DeadLettered(_)
        ));
    }

    projection.drop_tables().await.unwrap();
    pool.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_migration_and_app_role_consumer_enforce_tenant_region_rls() {
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect migration role");
    PgMigrator::apply(&admin, &foundation_migrations())
        .await
        .expect("apply durable consumer foundation");
    PgMigrator::apply_validated(
        &admin,
        &check_status_migrations(),
        &check_status_hot_tables(),
    )
    .await
    .expect("apply production Git check migrations");

    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&app_url())
        .await
        .expect("connect constrained runtime role");
    let runtime = tokio::runtime::Handle::current();
    let consumer = build_durable_check_consumer(
        runtime.clone(),
        REGION,
        DedupLedger::durable(Arc::new(DurableDedupBacking::new(
            app.clone(),
            runtime.clone(),
        ))),
        Arc::new(DurableDeadLetterBacking::new(app.clone(), runtime)),
        myelin_events::DurableWorkerAdmission::new(64, 32, 16).unwrap(),
    )
    .unwrap();
    let event = check_event(
        &format!("ci-git-check-production-rls-{nonce}"),
        41,
        CheckState::Success,
        CostPosture::Settled,
    );
    assert_eq!(
        tokio::task::block_in_place(|| consumer.deliver(&Message {
            subject: event.subject.0.clone(),
            envelope: event,
        })),
        Delivered::Acked,
        "the constrained runtime role writes through the consumer's scoped co-commit transaction"
    );

    async fn visible_rows(pool: &PgPool, tenant: &str, region: &str) -> i64 {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(
            "SELECT set_config('myelin.tenant_id',$1,true), \
                    set_config('myelin.region',$2,true)",
        )
        .bind(tenant)
        .bind(region)
        .execute(&mut *tx)
        .await
        .unwrap();
        let count = sqlx::query_scalar(
            "SELECT count(*) FROM check_status \
             WHERE repo_ref=$1 AND commit_oid=$2 AND context_provider='ci' AND context_name='build'",
        )
        .bind(REPO)
        .bind(COMMIT)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        count
    }

    assert_eq!(visible_rows(&app, TENANT, REGION).await, 1);
    assert_eq!(
        visible_rows(&app, "globex", REGION).await,
        0,
        "a foreign tenant cannot observe the projected check"
    );
    assert_eq!(
        visible_rows(&app, TENANT, "eu-west").await,
        0,
        "a foreign region cannot observe the projected check"
    );

    let mut cleanup = admin.begin().await.unwrap();
    sqlx::query(
        "SELECT set_config('myelin.tenant_id',$1,true), \
                set_config('myelin.region',$2,true)",
    )
    .bind(TENANT)
    .bind(REGION)
    .execute(&mut *cleanup)
    .await
    .unwrap();
    sqlx::query(
        "DELETE FROM check_status \
         WHERE repo_ref=$1 AND commit_oid=$2 AND context_provider='ci' AND context_name='build'",
    )
    .bind(REPO)
    .bind(COMMIT)
    .execute(&mut *cleanup)
    .await
    .unwrap();
    cleanup.commit().await.unwrap();
}
