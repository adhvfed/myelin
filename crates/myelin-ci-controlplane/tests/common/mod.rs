#![cfg(feature = "integration")]
#![allow(dead_code)]

use futures::FutureExt;
use sqlx::{Executor, PgPool};
use std::sync::Arc;

use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    CompletionSettlementOwner, RunnerHooks, SandboxBackend, SandboxCancellation, SandboxHandle,
    SandboxLaunch, SandboxLaunchError, SandboxOutputSink,
};

pub struct LegacyStreamingGvisor<'a>(pub &'a GvisorBackend);

impl SandboxBackend for LegacyStreamingGvisor<'_> {
    type Error = <GvisorBackend as SandboxBackend>::Error;

    fn launch(
        &self,
        spec: &myelin_ci_sandbox::JobSpec,
        hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
        let mut workload_only = spec.clone();
        workload_only.workspace.repo_ref = None;
        workload_only.workspace.commit = None;
        self.0.launch(&workload_only, hooks)
    }

    fn launch_streaming(
        &self,
        spec: &myelin_ci_sandbox::JobSpec,
        hooks: &RunnerHooks,
        output: Arc<dyn SandboxOutputSink>,
        cancellation: SandboxCancellation,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
        let mut workload_only = spec.clone();
        workload_only.workspace.repo_ref = None;
        workload_only.workspace.commit = None;
        self.0
            .launch_streaming(&workload_only, hooks, output, cancellation)
    }

    fn kill(&self, handle: &SandboxHandle) -> Result<(), Self::Error> {
        self.0.kill(handle)
    }
}

pub fn legacy_streaming_hooks(
    real: RunnerHooks,
    repo_ref: String,
    commit: String,
) -> RunnerHooks {
    assert_eq!(
        real.completion_settlement_owner(),
        CompletionSettlementOwner::TerminalReporter
    );
    let real = Arc::new(real);
    let restore = move |spec: &myelin_ci_sandbox::JobSpec| {
        let mut restored = spec.clone();
        restored.workspace.repo_ref = Some(repo_ref.clone());
        restored.workspace.commit = Some(commit.clone());
        restored
    };
    let restore = Arc::new(restore);

    let reserve_real = real.clone();
    let reserve_restore = restore.clone();
    let release_real = real.clone();
    let release_restore = restore.clone();
    let launch_real = real.clone();
    let launch_restore = restore.clone();
    let isolation_real = real;
    RunnerHooks::new_with_launch_fence(
        CompletionSettlementOwner::TerminalReporter,
        Box::new(move |spec| reserve_real.reserve(&reserve_restore(spec))),
        Box::new(move |spec, handle, _usage| {
            release_real.release_unused(&release_restore(spec), handle)
        }),
        Box::new(move |spec| launch_real.acquire_launch_permit(&launch_restore(spec))),
        Box::new(move |spec| isolation_real.enforce_isolation_floor(&restore(spec))),
    )
}

struct LegacyRunscComputeAttemptAuthority;

impl myelin_ci_sandbox::checkout_orchestration::AttemptAuthority
    for LegacyRunscComputeAttemptAuthority
{
    fn begin_phase(
        &self,
        _phase: myelin_ci_sandbox::PreparationPhase,
    ) -> Result<(), myelin_ci_sandbox::checkout_orchestration::AttemptAuthorityError> {
        panic!("compute must not open a checkout preparation phase")
    }

    fn complete_phase(
        &self,
        _phase: myelin_ci_sandbox::PreparationPhase,
        _usage: myelin_ci_sandbox::ResourceUsage,
    ) -> Result<(), myelin_ci_sandbox::checkout_orchestration::AttemptAuthorityError> {
        panic!("compute must not complete a checkout preparation phase")
    }

    fn seal_phase(
        &self,
        _phase: myelin_ci_sandbox::PreparationPhase,
    ) -> Result<(), myelin_ci_sandbox::checkout_orchestration::AttemptAuthorityError> {
        panic!("compute must not seal a checkout preparation phase")
    }

    fn renew_preparation_lease(
        &self,
    ) -> Result<(), myelin_ci_sandbox::PreparationLeaseLost> {
        panic!("compute must not renew a checkout preparation lease")
    }

    fn mint_phase_credential(
        &self,
        _phase: myelin_ci_sandbox::CheckoutPhase,
    ) -> Result<
        myelin_ci_sandbox::checkout_orchestration::PhaseCredentialCarrier,
        myelin_ci_sandbox::checkout_orchestration::AttemptAuthorityError,
    > {
        panic!("compute must not mint a checkout phase credential")
    }

    fn mint_workload_credential(
        &self,
    ) -> Result<
        myelin_ci_sandbox::checkout_orchestration::WorkloadCredentialCarrier,
        myelin_ci_sandbox::checkout_orchestration::AttemptAuthorityError,
    > {
        panic!("compute discards the unused attempt authority before workload launch")
    }

    fn should_requeue(&self) -> bool {
        false
    }
}

pub fn stage_b_compute_hooks_for_legacy_runsc_test() -> myelin_ci_sandbox::RunnerHooks {
    with_stage_b_compute_admission_for_legacy_runsc_test(myelin_ci_sandbox::RunnerHooks::new(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(|spec| {
            Ok(myelin_ci_sandbox::ReserveHandle(
                spec.meter_to.reserve_id.clone(),
            ))
        }),
        Box::new(|_spec, _handle, _usage| Ok(())),
        Box::new(|_token| Ok(())),
        Box::new(|_spec| Ok(())),
    ))
}

pub fn with_stage_b_compute_admission_for_legacy_runsc_test(
    hooks: myelin_ci_sandbox::RunnerHooks,
) -> myelin_ci_sandbox::RunnerHooks {
    hooks.with_parent_attempt_reservation(Box::new(|spec| {
        Ok(
            myelin_ci_sandbox::checkout_orchestration::ParentAttemptAdmission::Admitted {
                claim: myelin_ci_sandbox::PreparationReportClaim {
                    tenant_id: "legacy-real-runsc".into(),
                    region: "fr-par".into(),
                    project_id: "55555555-5555-4555-8555-555555555555".into(),
                    wf_run_id: "11111111-1111-1111-1111-111111111111".into(),
                    ci_run_id: "22222222-2222-2222-2222-222222222222".into(),
                    job_id: "33333333-3333-3333-3333-333333333333".into(),
                    token_authority_handle: "legacy-real-runsc-compute".into(),
                    idem_token: spec.idem_token.0.clone(),
                    lease_owner: "legacy-real-runsc".into(),
                    lease_epoch: 1,
                    claim_nonce: "44444444-4444-4444-4444-444444444444".into(),
                    claim_started_at_epoch_secs: 1,
                    claim_expires_at_epoch_secs: i64::MAX,
                },
                reserve: myelin_ci_sandbox::ReserveHandle(spec.meter_to.reserve_id.clone()),
                attempt_authority: Box::new(LegacyRunscComputeAttemptAuthority),
            },
        )
    }))
}

pub async fn with_schema_cleanup<Fut>(pool: &PgPool, schema: &str, body: impl FnOnce() -> Fut)
where
    Fut: std::future::Future<Output = ()>,
{
    let result = std::panic::AssertUnwindSafe(body()).catch_unwind().await;
    if let Err(error) = pool
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
    {
        eprintln!(
            "with_schema_cleanup: DROP SCHEMA IF EXISTS {schema} CASCADE failed (schema may have \
             leaked): {error}"
        );
    }
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

pub const CI_PRIVILEGE_FIXTURE_LOCK: i64 = 0x4349_4649_5854_5552;

pub async fn with_privilege_fixture_lock<Fut>(
    admin_url: &str,
    sweep_prefixes: &[&str],
    body: impl FnOnce() -> Fut,
) where
    Fut: std::future::Future<Output = ()>,
{
    use sqlx::Row;
    let lock_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(admin_url)
        .await
        .expect("connect the fixture-lock session");
    let mut lock_conn = lock_pool
        .acquire()
        .await
        .expect("acquire the dedicated fixture-lock connection");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(CI_PRIVILEGE_FIXTURE_LOCK)
        .execute(&mut *lock_conn)
        .await
        .expect("take the privilege-fixture advisory lock");

    let sweep = async {
        for prefix in sweep_prefixes {
            let leaked = sqlx::query("SELECT nspname FROM pg_namespace WHERE nspname LIKE $1")
                .bind(format!("{prefix}%"))
                .fetch_all(&mut *lock_conn)
                .await
                .unwrap_or_else(|error| {
                    panic!("enumerate leaked `{prefix}` fixture schemas: {error}")
                });
            for row in leaked {
                let schema: String = row.get("nspname");
                let quoted: String = sqlx::query_scalar("SELECT quote_ident($1)")
                    .bind(&schema)
                    .fetch_one(&mut *lock_conn)
                    .await
                    .unwrap_or_else(|error| panic!("quote leaked schema `{schema}`: {error}"));
                sqlx::query(&format!("DROP SCHEMA IF EXISTS {quoted} CASCADE"))
                    .execute(&mut *lock_conn)
                    .await
                    .unwrap_or_else(|error| {
                        panic!("drop leaked fixture schema `{schema}`: {error}")
                    });
            }
        }
    };

    let result = std::panic::AssertUnwindSafe(async {
        sweep.await;
        body().await;
    })
    .catch_unwind()
    .await;

    let unlocked = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(CI_PRIVILEGE_FIXTURE_LOCK)
        .execute(&mut *lock_conn)
        .await;
    if let Err(error) = unlocked {
        eprintln!("with_privilege_fixture_lock: releasing the advisory lock FAILED: {error}");
    }
    drop(lock_conn);
    lock_pool.close().await;
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

pub async fn with_throwaway_role<Fut>(admin: &PgPool, role: &str, body: impl FnOnce() -> Fut)
where
    Fut: std::future::Future<Output = ()>,
{
    let result = std::panic::AssertUnwindSafe(body()).catch_unwind().await;
    let quoted: Result<String, _> = sqlx::query_scalar("SELECT quote_ident($1)")
        .bind(role)
        .fetch_one(admin)
        .await;
    match quoted {
        Ok(quoted) => {
            for statement in [
                format!("DROP OWNED BY {quoted} CASCADE"),
                format!("DROP ROLE IF EXISTS {quoted}"),
            ] {
                if let Err(error) = admin.execute(statement.as_str()).await {
                    eprintln!("with_throwaway_role: `{statement}` failed (role may leak): {error}");
                }
            }
        }
        Err(error) => eprintln!("with_throwaway_role: could not quote `{role}`: {error}"),
    }
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

pub async fn with_fixture_migration_lock<Fut>(
    admin_url: &str,
    admin: &PgPool,
    schema: &str,
    migrate: impl FnOnce() -> Fut,
) where
    Fut: std::future::Future<Output = ()>,
{
    let lock_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(admin_url)
        .await
        .expect("connect the fixture-migration lock session");
    let mut lock_conn = lock_pool
        .acquire()
        .await
        .expect("acquire the dedicated fixture-migration lock connection");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(CI_PRIVILEGE_FIXTURE_LOCK)
        .execute(&mut *lock_conn)
        .await
        .expect("take the privilege-fixture advisory lock for migration");

    let migrated = std::panic::AssertUnwindSafe(migrate())
        .catch_unwind()
        .await;
    let revoked = std::panic::AssertUnwindSafe(revoke_scheduler_grants(admin, schema))
        .catch_unwind()
        .await;

    if let Err(error) = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(CI_PRIVILEGE_FIXTURE_LOCK)
        .execute(&mut *lock_conn)
        .await
    {
        eprintln!("with_fixture_migration_lock: releasing the advisory lock FAILED: {error}");
    }
    drop(lock_conn);
    lock_pool.close().await;
    if let Err(payload) = migrated {
        std::panic::resume_unwind(payload);
    }
    if let Err(payload) = revoked {
        std::panic::resume_unwind(payload);
    }
}

async fn revoke_scheduler_grants(admin: &PgPool, schema: &str) {
    let quoted: String = sqlx::query_scalar("SELECT quote_ident($1)")
        .bind(schema)
        .fetch_one(admin)
        .await
        .unwrap_or_else(|error| panic!("quote fixture schema `{schema}`: {error}"));
    let statement = format!(
        "DO $revoke$
         DECLARE
           target record;
         BEGIN
           IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'myelin_ci_region_scheduler') THEN
             RETURN;
           END IF;
           FOR target IN
             SELECT c.oid::regclass AS relation, a.attname
               FROM pg_class c
               JOIN pg_namespace n ON n.oid = c.relnamespace
               JOIN pg_attribute a ON a.attrelid = c.oid
              WHERE n.nspname = '{schema}'
                AND c.relkind IN ('r', 'p', 'v', 'm')
                AND a.attnum > 0 AND NOT a.attisdropped
           LOOP
             EXECUTE format(
               'REVOKE ALL (%I) ON TABLE %s FROM myelin_ci_region_scheduler',
               target.attname, target.relation);
           END LOOP;
           EXECUTE 'REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA {quoted}                     FROM myelin_ci_region_scheduler';
           EXECUTE 'REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA {quoted}                     FROM myelin_ci_region_scheduler';
           EXECUTE 'REVOKE ALL ON SCHEMA {quoted} FROM myelin_ci_region_scheduler';
         END
         $revoke$;"
    );
    admin.execute(statement.as_str()).await.unwrap_or_else(|error| {
        panic!("revoke scheduler grants from fixture schema `{schema}`: {error}")
    });
}
