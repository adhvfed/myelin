//! Live proof of CT-007 slice 5b.3-4a.2's schema: `ci_job_parent_attempt` is insert-only, immutable,
//! and FK-anchored; `ci_job_prelaunch_usage`'s transition-guard trigger enforces the legal
//! `started -> measured`/`started -> sealed_ceiling` transitions, refuses reverting a terminal state,
//! refuses identity/ceiling tampering, and forbids DELETE outright; and the dedicated region
//! scheduler role can SELECT/seal what the reaper (CT-007 slice 5b.3-4b) needs and nothing more.
#![cfg(feature = "integration")]

use std::time::Duration;

use myelin_ci_controlplane::{ci_region_queue_store_test_support, JobQueueReaper};
use sqlx::postgres::{PgPoolOptions, PgQueryResult};
use sqlx::{Executor, PgPool};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn scheduler_url() -> String {
    std::env::var("MYELIN_CI_SCHEDULER_DATABASE_URL").unwrap_or_else(|_| {
        "postgres://myelin_ci_scheduler_fr_par:myelin_ci_scheduler_dev_pw@localhost:5433/myelin"
            .into()
    })
}

async fn pinned_pool(url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                connection
                    .execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                connection.execute("SET myelin.tenant_id = ''").await.ok();
                connection.execute("SET myelin.region = 'fr-par'").await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .expect("connect to the live development PostgreSQL")
}

async fn app_pinned_pool(url: &str, schema: &str, tenant: &str) -> PgPool {
    let schema = schema.to_owned();
    let tenant = tenant.to_owned();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            let tenant = tenant.clone();
            Box::pin(async move {
                connection
                    .execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                connection
                    .execute(format!("SET myelin.tenant_id = '{tenant}'").as_str())
                    .await?;
                connection.execute("SET myelin.region = 'fr-par'").await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .expect("connect the constrained app role")
}

fn assert_permission_denied(result: Result<PgQueryResult, sqlx::Error>, operation: &str) {
    let error = result.expect_err(operation);
    let code = error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|code| code.into_owned());
    assert_eq!(code.as_deref(), Some("42501"), "{operation}: {error}");
}

fn assert_check_violation(result: Result<PgQueryResult, sqlx::Error>, operation: &str) {
    let error = result.expect_err(operation);
    let code = error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|code| code.into_owned());
    assert_eq!(code.as_deref(), Some("23514"), "{operation}: {error}");
}

/// The transition-guard trigger refuses via `RAISE EXCEPTION` (SQLSTATE `P0001`), distinct from an
/// actual `CHECK` constraint violation (`23514`).
fn assert_trigger_refusal(result: Result<PgQueryResult, sqlx::Error>, operation: &str) {
    let error = result.expect_err(operation);
    let code = error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|code| code.into_owned());
    assert_eq!(code.as_deref(), Some("P0001"), "{operation}: {error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_attempt_and_prelaunch_usage_enforce_the_full_state_machine() {
    let schema = format!("ci_prelaunch_usage_journal_{}", std::process::id());
    let bootstrap = pinned_pool(&admin_url(), "public").await;
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap
        .execute(format!("CREATE SCHEMA {schema} AUTHORIZATION myelin_admin").as_str())
        .await
        .unwrap();
    let admin = pinned_pool(&admin_url(), &schema).await;
    admin
        .execute(
            format!(
                "GRANT USAGE ON SCHEMA {schema} TO myelin_app, myelin_ci_region_scheduler;
                 ALTER DEFAULT PRIVILEGES FOR ROLE myelin_admin IN SCHEMA {schema}
                   GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO myelin_app;"
            )
            .as_str(),
        )
        .await
        .unwrap();
    myelin_storage::PgMigrator::apply_validated(
        &admin,
        &myelin_flow::migrations::migrations(),
        &myelin_storage::HotTables::declare(["workflow_run"]),
    )
    .await
    .expect("apply the myelin-flow prerequisite (workflow_run) into the isolated schema");
    myelin_storage::PgMigrator::apply_validated(
        &admin,
        &myelin_ci_controlplane::ci_controlplane_migrations(),
        &myelin_ci_controlplane::ci_controlplane_hot_tables(),
    )
    .await
    .expect("apply the complete CI control-plane migration set, including the new journal pair");

    let tenant = "prelaunch-usage-tenant";
    let run_id = "50000000-0000-0000-0000-000000000001";
    let wf_run_id = "60000000-0000-0000-0000-000000000001";
    let job_a = "70000000-0000-0000-0000-000000000001";
    let job_b = "70000000-0000-0000-0000-000000000002";
    admin
        .execute(
            sqlx::query(
                "INSERT INTO ci_run (\
                 tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, \
                 definition_snapshot, trigger_kind, trust_tier, state, correlation_id) \
                 VALUES ($1, 'fr-par', $2::uuid, $3::uuid, $3::uuid, $4::uuid, \
                 'snapshot', 'push', 'trusted', 'running', 'corr')",
            )
            .bind(tenant)
            .bind(run_id)
            .bind("80000000-0000-0000-0000-000000000001")
            .bind(wf_run_id),
        )
        .await
        .unwrap();

    let app = app_pinned_pool(&app_url(), &schema, tenant).await;

    // Insert one parent-attempt row per job -- job_a will exercise the checkout-phase journal,
    // job_b exists only to prove a compute job (no phase rows at all) is a legal, independent row.
    for (job_id, lease_epoch, claim_nonce) in [
        (job_a, 1i64, "90000000-0000-0000-0000-000000000001"),
        (job_b, 1i64, "90000000-0000-0000-0000-000000000002"),
    ] {
        let inserted = sqlx::query(
            "INSERT INTO ci_job_parent_attempt (\
             tenant_id, region, job_id, wf_run_id, ci_run_id, reserve_handle, lease_owner, \
             lease_epoch, claim_nonce, claim_started_at_epoch_secs, claim_expires_at_epoch_secs, \
             budget_revision, max_parent_attempts) \
             VALUES ($1, 'fr-par', $2::uuid, $3::uuid, $4::uuid, 'ci-reserve:v2:fixture', \
             'runner-1', $5, $6::uuid, 1000, 2000, 1, 5)",
        )
        .bind(tenant)
        .bind(job_id)
        .bind(wf_run_id)
        .bind(run_id)
        .bind(lease_epoch)
        .bind(claim_nonce)
        .execute(&app)
        .await;
        assert!(inserted.is_ok(), "parent-attempt insert: {inserted:?}");
    }

    // The parent-attempt journal is immutable: no UPDATE, no DELETE, ever.
    assert_permission_denied(
        sqlx::query(
            "UPDATE ci_job_parent_attempt SET lease_owner = 'other' WHERE job_id = $1::uuid",
        )
        .bind(job_a)
        .execute(&app)
        .await,
        "UPDATE ci_job_parent_attempt",
    );
    assert_permission_denied(
        sqlx::query("DELETE FROM ci_job_parent_attempt WHERE job_id = $1::uuid")
            .bind(job_a)
            .execute(&app)
            .await,
        "DELETE FROM ci_job_parent_attempt",
    );

    // A CHECK violation: `started` requires both exact usage columns to be NULL.
    assert_check_violation(
        sqlx::query(
            "INSERT INTO ci_job_prelaunch_usage (\
             tenant_id, region, job_id, lease_epoch, claim_nonce, phase, status, \
             ceiling_cpu_seconds, ceiling_mem_byte_seconds, exact_cpu_seconds, exact_mem_byte_seconds) \
             VALUES ($1, 'fr-par', $2::uuid, 1, '90000000-0000-0000-0000-000000000001'::uuid, \
             'checkout_transport', 'started', 100, 200, 1, 1)",
        )
        .bind(tenant)
        .bind(job_a)
        .execute(&app)
        .await,
        "insert a started row with non-null exact usage",
    );

    // Two phases: transport reaches `measured`; materialization reaches `sealed_ceiling` (the
    // reaper's fallback), proving both terminal states independently.
    for phase in ["checkout_transport", "checkout_materialization"] {
        let inserted = sqlx::query(
            "INSERT INTO ci_job_prelaunch_usage (\
             tenant_id, region, job_id, lease_epoch, claim_nonce, phase, status, \
             ceiling_cpu_seconds, ceiling_mem_byte_seconds) \
             VALUES ($1, 'fr-par', $2::uuid, 1, '90000000-0000-0000-0000-000000000001'::uuid, \
             $3, 'started', 100, 200)",
        )
        .bind(tenant)
        .bind(job_a)
        .bind(phase)
        .execute(&app)
        .await;
        assert!(inserted.is_ok(), "phase insert {phase}: {inserted:?}");
    }

    // Legal transition: started -> measured, with exact usage now present.
    let measured = sqlx::query(
        "UPDATE ci_job_prelaunch_usage \
         SET status = 'measured', exact_cpu_seconds = 7, exact_mem_byte_seconds = 9, resolved_at = now() \
         WHERE tenant_id = $1 AND job_id = $2::uuid AND phase = 'checkout_transport'",
    )
    .bind(tenant)
    .bind(job_a)
    .execute(&app)
    .await;
    assert!(
        measured.is_ok(),
        "legal started->measured transition: {measured:?}"
    );
    assert_eq!(measured.unwrap().rows_affected(), 1);

    // Illegal: reverting measured back to started must refuse.
    assert_trigger_refusal(
        sqlx::query(
            "UPDATE ci_job_prelaunch_usage \
             SET status = 'started', exact_cpu_seconds = NULL, exact_mem_byte_seconds = NULL, resolved_at = NULL \
             WHERE tenant_id = $1 AND job_id = $2::uuid AND phase = 'checkout_transport'",
        )
        .bind(tenant)
        .bind(job_a)
        .execute(&app)
        .await,
        "measured cannot revert to started",
    );

    // Legal transition: started -> sealed_ceiling (the reaper's fallback).
    let sealed = sqlx::query(
        "UPDATE ci_job_prelaunch_usage SET status = 'sealed_ceiling', resolved_at = now() \
         WHERE tenant_id = $1 AND job_id = $2::uuid AND phase = 'checkout_materialization'",
    )
    .bind(tenant)
    .bind(job_a)
    .execute(&app)
    .await;
    assert!(
        sealed.is_ok(),
        "legal started->sealed_ceiling transition: {sealed:?}"
    );

    // A late completion after sealing must refuse -- it can never replace a conservative ceiling
    // with a (possibly lower) measurement.
    assert_trigger_refusal(
        sqlx::query(
            "UPDATE ci_job_prelaunch_usage \
             SET status = 'measured', exact_cpu_seconds = 1, exact_mem_byte_seconds = 1 \
             WHERE tenant_id = $1 AND job_id = $2::uuid AND phase = 'checkout_materialization'",
        )
        .bind(tenant)
        .bind(job_a)
        .execute(&app)
        .await,
        "sealed_ceiling cannot become measured",
    );

    // Identity/ceiling tampering on the one legal transition must also refuse.
    let transport2 = sqlx::query(
        "INSERT INTO ci_job_prelaunch_usage (\
         tenant_id, region, job_id, lease_epoch, claim_nonce, phase, status, \
         ceiling_cpu_seconds, ceiling_mem_byte_seconds, seal_after) \
         VALUES ($1, 'fr-par', $2::uuid, 1, '90000000-0000-0000-0000-000000000002'::uuid, \
         'checkout_transport', 'started', 100, 200, now() + interval '1 day')",
    )
    .bind(tenant)
    .bind(job_b)
    .execute(&app)
    .await;
    assert!(transport2.is_ok());
    assert_trigger_refusal(
        sqlx::query(
            "UPDATE ci_job_prelaunch_usage \
             SET status = 'measured', exact_cpu_seconds = 1, exact_mem_byte_seconds = 1, resolved_at = now(), \
                 ceiling_cpu_seconds = 999 \
             WHERE tenant_id = $1 AND job_id = $2::uuid AND phase = 'checkout_transport'",
        )
        .bind(tenant)
        .bind(job_b)
        .execute(&app)
        .await,
        "the transition cannot also tamper with the recorded ceiling",
    );

    let expired_materialization = sqlx::query(
        "INSERT INTO ci_job_prelaunch_usage (\
         tenant_id, region, job_id, lease_epoch, claim_nonce, phase, status, \
         ceiling_cpu_seconds, ceiling_mem_byte_seconds, started_at, seal_after) \
         VALUES ($1, 'fr-par', $2::uuid, 1, '90000000-0000-0000-0000-000000000002'::uuid, \
         'checkout_materialization', 'started', 300, 400, now() - interval '2 minutes', \
         now() - interval '1 minute')",
    )
    .bind(tenant)
    .bind(job_b)
    .execute(&app)
    .await;
    assert!(expired_materialization.is_ok());

    // DELETE is forbidden outright, resolved or not.
    assert_permission_denied(
        sqlx::query("DELETE FROM ci_job_prelaunch_usage WHERE job_id = $1::uuid")
            .bind(job_a)
            .execute(&app)
            .await,
        "DELETE FROM ci_job_prelaunch_usage",
    );

    // The dedicated region scheduler role (the reaper's real production role) can SELECT both
    // journal tables and seal an unresolved row through the exact column-scoped grant -- and
    // nothing more.
    let scheduler = pinned_pool(&scheduler_url(), &schema).await;
    let visible_parent: i64 = sqlx::query_scalar("SELECT count(*) FROM ci_job_parent_attempt")
        .fetch_one(&scheduler)
        .await
        .expect("scheduler SELECT on ci_job_parent_attempt");
    assert_eq!(visible_parent, 2);
    let visible_usage: i64 = sqlx::query_scalar("SELECT count(*) FROM ci_job_prelaunch_usage")
        .fetch_one(&scheduler)
        .await
        .expect("scheduler SELECT on ci_job_prelaunch_usage");
    assert_eq!(visible_usage, 4);

    let region_store = ci_region_queue_store_test_support(scheduler.clone());
    let reaper = JobQueueReaper::new(region_store, "fr-par", Duration::from_secs(15));
    assert_eq!(
        reaper
            .reap_once()
            .await
            .expect("topology-aware full regional reaper sweep"),
        1,
        "only the phase whose own immutable deadline elapsed is sealed"
    );
    let reaped_statuses: (String, String) = sqlx::query_as(
        "SELECT
           max(status) FILTER (WHERE phase = 'checkout_transport'),
           max(status) FILTER (WHERE phase = 'checkout_materialization')
         FROM ci_job_prelaunch_usage
         WHERE tenant_id = $1 AND job_id = $2::uuid",
    )
    .bind(tenant)
    .bind(job_b)
    .fetch_one(&scheduler)
    .await
    .unwrap();
    assert_eq!(
        reaped_statuses,
        ("started".into(), "sealed_ceiling".into()),
        "the future-deadline phase remains live even though the parent claim timestamps are old"
    );

    let sealed_by_reaper = sqlx::query(
        "UPDATE ci_job_prelaunch_usage SET status = 'sealed_ceiling', resolved_at = now() \
         WHERE tenant_id = $1 AND job_id = $2::uuid AND phase = 'checkout_transport'",
    )
    .bind(tenant)
    .bind(job_b)
    .execute(&scheduler)
    .await;
    assert!(
        sealed_by_reaper.is_ok(),
        "the scheduler's column-scoped grant permits sealing a started row: {sealed_by_reaper:?}"
    );

    assert_permission_denied(
        sqlx::query(
            "UPDATE ci_job_prelaunch_usage SET ceiling_cpu_seconds = 1 WHERE job_id = $1::uuid",
        )
        .bind(job_a)
        .execute(&scheduler)
        .await,
        "the scheduler's grant is column-scoped to (status, resolved_at) only",
    );
    assert_permission_denied(
        sqlx::query(
            "UPDATE ci_job_parent_attempt SET lease_owner = 'reaper' WHERE job_id = $1::uuid",
        )
        .bind(job_a)
        .execute(&scheduler)
        .await,
        "the scheduler never gets write access to the immutable parent-attempt journal",
    );

    scheduler.close().await;
    app.close().await;
    admin.close().await;
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap.close().await;
}
