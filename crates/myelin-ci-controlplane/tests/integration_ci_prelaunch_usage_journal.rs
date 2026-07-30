//! Live proof of CT-007 slice 5b.3-4a.2's schema: `ci_job_parent_attempt` is insert-only, immutable,
//! and FK-anchored; `ci_job_prelaunch_usage`'s transition-guard trigger enforces the legal
//! `started -> measured`/`started -> sealed_ceiling` transitions, refuses reverting a terminal state,
//! refuses identity/ceiling tampering, and forbids DELETE outright; and the dedicated region
//! scheduler role can SELECT/seal what the reaper (CT-007 slice 5b.3-4b) needs and nothing more.
#![cfg(feature = "integration")]

mod common;

use std::time::Duration;

use myelin_ci_controlplane::{
    ci_region_queue_store_test_support, CiJobAccountingRecord, CiJobAccountingStore,
    CiJobAccountingWrite, CiJobAccountingWriteVersion, CiJobTerminalDisposition, JobQueueReaper,
};
use myelin_ci_sandbox::ResourceUsage;
use myelin_flow::MinorUnits;
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};
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
    // Unlike the other schema fixtures, this suite DELIBERATELY exercises the real
    // `myelin_ci_region_scheduler` role against its isolated schema — it asserts exactly which
    // journal columns that role may read and seal. Revoking those grants after migration (what the
    // other fixtures do) would delete the thing under test, so this one takes the other branch:
    // hold CI_PRIVILEGE_FIXTURE_LOCK across the WHOLE body, so the grants exist but no concurrent
    // scheduler-boundary or production-boot probe can observe them.
    //
    // The sweep prefix and the inner `with_schema_cleanup` are what make that hold panic-safe. With
    // only the happy-path DROP at the end, a panic anywhere in the body released the lock while a
    // fully scheduler-granted schema was still standing — the exact state the lock exists to hide —
    // and it stayed there for every later run. Cleanup now happens INSIDE the lock, and the prefix
    // sweep collects whatever an earlier crashed run left behind.
    common::with_privilege_fixture_lock(&admin_url(), &["ci_prelaunch_usage_journal_"], || async {
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
    let cleanup_bootstrap = bootstrap.clone();
    let schema_for_cleanup = schema.clone();
    common::with_schema_cleanup(&cleanup_bootstrap, &schema_for_cleanup, move || async move {
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

    // **CT-007 phase-credential generations: the scheduler role holds NO privilege at all on the
    // credential log.** Neither reaping nor renewal reads it, and it is the durable authority every
    // phase execution gate consults — so unlike the two journal tables above, even SELECT is denied.
    for statement in [
        "SELECT count(*) FROM ci_job_credential_generation",
        "SELECT generation_id FROM ci_job_credential_generation LIMIT 1",
    ] {
        assert_permission_denied(
            sqlx::query(statement).execute(&scheduler).await,
            "the scheduler role may not READ the credential log",
        );
    }
    assert_permission_denied(
        sqlx::query(
            "INSERT INTO ci_job_credential_generation (\
             tenant_id, region, job_id, wf_run_id, ci_run_id, token_authority_handle, idem_token, \
             lease_owner, lease_epoch, claim_nonce, claim_started_at_epoch_secs, \
             claim_expires_at_epoch_secs, binding_version, purpose, phase_ordinal, \
             issued_at_epoch_secs, expires_at_epoch_secs, generation_id, jti) \
             VALUES ($1, 'fr-par', $2::uuid, $3::uuid, $4::uuid, 'h', 'i', 'o', 1, $5::uuid, \
             1, 2, 1, 'workload', 4, 1, 2, 'g', 'j')",
        )
        .bind(tenant)
        .bind(job_a)
        .bind(wf_run_id)
        .bind(run_id)
        .bind("90000000-0000-0000-0000-000000000001")
        .execute(&scheduler)
        .await,
        "the scheduler role may not forge a credential generation",
    );
    assert_permission_denied(
        sqlx::query("UPDATE ci_job_credential_generation SET jti = 'x'")
            .execute(&scheduler)
            .await,
        "the scheduler role may not rewrite a credential generation",
    );
    assert_permission_denied(
        sqlx::query("DELETE FROM ci_job_credential_generation")
            .execute(&scheduler)
            .await,
        "the scheduler role may not delete a credential generation",
    );

    // 5b.3-5a compatibility proof: after the additive disposition/v4 migration, an immutable
    // historical v3-only accounting row still inserts and replays byte-for-byte unchanged.
    let accounting = CiJobAccountingStore::with_pg(app.clone(), Region("fr-par".into()));
    let accounting_tenant = TenantId::from_token(tenant);
    let accounting_region = Region("fr-par".into());
    let principal = Principal::new(
        accounting_tenant.clone(),
        accounting_region.clone(),
        PrincipalId("prelaunch-accounting-test".into()),
        PrincipalKind::Service,
        DataRole::Processor,
        PrincipalStatus::Active,
    );
    let scope = TenantScope::from_verified_token(&principal, accounting_region);
    let legacy = CiJobAccountingRecord {
        tenant: TenantId::from_token(tenant),
        job_id: "70000000-0000-0000-0000-000000000003".into(),
        wf_run_id: wf_run_id.into(),
        ci_run_id: run_id.into(),
        reserve_handle: "ci-reserve:v1:legacy".into(),
        passed: false,
        timed_out: false,
        skipped: true,
        usage: ResourceUsage {
            cpu_seconds: 0,
            mem_byte_seconds: 0,
        },
        pricing_revision: "ci-skipped:v1".into(),
        billed: MinorUnits::ZERO,
        refunded: MinorUnits(1),
        disposition: None,
        completion_receipt: format!("v3:{}", "a".repeat(64)),
        legacy_completion_receipt_v3: None,
    };
    assert_eq!(
        accounting.record(&scope, &legacy).await.unwrap(),
        CiJobAccountingWrite::Inserted
    );
    assert_eq!(
        accounting.record(&scope, &legacy).await.unwrap(),
        CiJobAccountingWrite::ExactReplay,
        "the additive v4 columns never force or reinterpret an historical v3 receipt"
    );
    let mut accounting_conn = app.acquire().await.unwrap();
    let loaded = accounting
        .load_in_tx(&mut accounting_conn, &scope, &legacy.job_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded, legacy);
    drop(accounting_conn);

    let v4_accounting = CiJobAccountingStore::with_pg_and_write_version(
        app.clone(),
        Region("fr-par".into()),
        CiJobAccountingWriteVersion::V4,
    );
    let v4 = CiJobAccountingRecord {
        tenant: TenantId::from_token(tenant),
        job_id: "70000000-0000-0000-0000-000000000004".into(),
        wf_run_id: wf_run_id.into(),
        ci_run_id: run_id.into(),
        reserve_handle: "ci-reserve:v1:v4-explicit".into(),
        passed: false,
        timed_out: false,
        skipped: false,
        usage: ResourceUsage {
            cpu_seconds: 1,
            mem_byte_seconds: 2,
        },
        pricing_revision: "ci-test:v1".into(),
        billed: MinorUnits(1),
        refunded: MinorUnits::ZERO,
        disposition: Some(CiJobTerminalDisposition::WorkloadFailed),
        completion_receipt: format!("v4:{}", "b".repeat(64)),
        legacy_completion_receipt_v3: Some(format!("v3:{}", "c".repeat(64))),
    };
    assert_eq!(
        v4_accounting.record(&scope, &v4).await.unwrap(),
        CiJobAccountingWrite::Inserted,
        "v4 remains explicitly activatable after the default writer is pinned to v3"
    );
    assert_eq!(
        v4_accounting.record(&scope, &v4).await.unwrap(),
        CiJobAccountingWrite::ExactReplay
    );

    scheduler.close().await;
    app.close().await;
    admin.close().await;
    // The schema drop belongs to `with_schema_cleanup` above, which runs it on the panicking path
    // too — and, critically, still inside CI_PRIVILEGE_FIXTURE_LOCK. `bootstrap` is deliberately NOT
    // closed here: `cleanup_bootstrap` is a `.clone()` of the SAME underlying pool, and
    // `PgPool::close()` shuts it down for every clone, which would break that drop silently.
    })
    .await;
    })
    .await;
}
