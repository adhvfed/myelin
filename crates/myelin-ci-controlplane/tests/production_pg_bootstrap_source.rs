use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use sqlx::PgPool;

fn executable_on_path(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

#[test]
fn missing_migration_credential_exits_before_reaper_runner_or_service_boot() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ci-controlplane"))
        .env("DATABASE_URL", "postgres://runtime.invalid/myelin")
        .env_remove("DATABASE_MIGRATION_URL")
        .env("S3_ENDPOINT", "http://storage.invalid")
        .env("S3_REGION", "fr-par")
        .env("S3_ACCESS_KEY", "test-access")
        .env("S3_SECRET_KEY", "test-secret")
        .env("S3_BUCKET", "test-bucket")
        .env("REDIS_URL", "redis://cache.invalid")
        .env("NATS_URL", "nats://bus.invalid")
        .env("MYELIN_REGION", "fr-par")
        .env_remove("MYELIN_CI_RUNNER")
        .output()
        .expect("CI Controlplane process must launch");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr must be UTF-8");
    assert!(stderr.contains("DATABASE_MIGRATION_URL"));
    assert!(!stderr.contains("spawn the ci-pipeline-driver thread"));
    assert!(!stderr.contains("ci-controlplane service failed"));
}

#[test]
fn runner_activation_refuses_a_missing_executor_before_platform_or_database_access() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ci-controlplane"))
        .env_clear()
        .env("MYELIN_CI_RUNNER", "1")
        .output()
        .expect("CI Controlplane process must launch");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr must be UTF-8");
    assert!(stderr.contains("runner-host executor preflight refused"));
    assert!(stderr.contains("MYELIN_RUNSC_BIN is required"));
    assert!(!stderr.contains("platform configuration refused to start"));
    assert!(!stderr.contains("database bootstrap refused to start"));
    assert!(!stderr.contains("DATABASE_URL"));
}

#[test]
fn invalid_runner_setting_is_refused_before_any_database_attempt() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ci-controlplane"))
        .env_clear()
        .env("MYELIN_CI_RUNNER", "true")
        .output()
        .expect("CI Controlplane process must launch");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr must be UTF-8");
    assert!(stderr.contains("invalid MYELIN_CI_RUNNER value \"true\""));
    assert!(stderr.contains("allowed values are `0`, `1`, or unset"));
    assert!(!stderr.contains("database bootstrap refused to start"));
    assert!(!stderr.contains("DATABASE_URL"));
    assert!(!stderr.contains("DATABASE_MIGRATION_URL"));
}

#[tokio::test(flavor = "multi_thread")]
async fn boot_time_sigterm_is_latched_before_the_real_runner_host_can_claim() {
    let required = std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1");
    let Some(runsc) = std::env::var_os(myelin_ci_sandbox::gvisor::ENV_RUNSC_BIN)
        .map(std::path::PathBuf::from)
        .or_else(|| executable_on_path("runsc"))
    else {
        if required {
            panic!("MYELIN_REQUIRE_RUNSC=1 but runsc is unavailable");
        }
        eprintln!("SKIP live CI runner activation proof: runsc is unavailable");
        return;
    };
    let runsc = runsc
        .canonicalize()
        .expect("resolve the live runsc executable");
    let rootfs = myelin_ci_sandbox::resolved_gvisor_rootfs();
    if !rootfs.join("bin/sh").is_file() {
        if required {
            panic!(
                "MYELIN_REQUIRE_RUNSC=1 but the gVisor rootfs is unavailable at {}",
                rootfs.display()
            );
        }
        eprintln!(
            "SKIP live CI runner activation proof: gVisor rootfs is unavailable at {}",
            rootfs.display()
        );
        return;
    }
    let admin_url = "postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin";
    let admin = match PgPool::connect(admin_url).await {
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("SKIP live CI runner activation proof: {error}");
            return;
        }
    };
    let suffix = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    const PRESERVED_NEGATIVE_EVIDENCE_TENANT_ID: &str = "myelin";
    const PRESERVED_NEGATIVE_EVIDENCE_RUN_ID: &str = "5db61d81-6aea-7dd9-b3f1-035abcf56b26";

    let mut job_queue_exists = false;
    for table in ["ci_run", "job_queue"] {
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(format!("public.{table}"))
            .fetch_one(&admin)
            .await
            .expect("inspect public CI activation table");
        if table == "job_queue" {
            job_queue_exists = exists;
        }
        if exists {
            let active: i64 = if table == "ci_run" {
                sqlx::query_scalar(
                    "SELECT count(*) FROM public.ci_run \
                     WHERE state IN ('queued', 'leased', 'running') \
                       AND NOT (tenant_id = $1 AND run_id = $2::uuid)",
                )
                .bind(PRESERVED_NEGATIVE_EVIDENCE_TENANT_ID)
                .bind(PRESERVED_NEGATIVE_EVIDENCE_RUN_ID)
                .fetch_one(&admin)
                .await
                .expect(
                    "count active public CI rows, excluding the one preserved historical exception",
                )
            } else {
                sqlx::query_scalar(&format!("SELECT count(*) FROM public.{table} WHERE state IN ('queued', 'leased', 'running')"))
                    .fetch_one(&admin)
                    .await
                    .expect("count active public CI rows")
            };
            assert_eq!(
                active, 0,
                "the production-root smoke test refuses to execute pre-existing active work \
                 (excluding the one named, permanently-preserved historical negative-evidence \
                 row tenant_id='myelin'/run_id='5db61d81-6aea-7dd9-b3f1-035abcf56b26' in ci_run)"
            );
        }
    }
    if !job_queue_exists {
        if required {
            panic!("MYELIN_REQUIRE_RUNSC=1 but the production job_queue schema is unavailable");
        }
        eprintln!(
            "SKIP live CI runner activation proof: production job_queue schema is unavailable"
        );
        return;
    }
    let tenant_id = format!("ci-runner-shutdown-{suffix}");
    let seeded_job_id: String = sqlx::query_scalar(
        "INSERT INTO public.job_queue \
           (tenant_id, region, job_id, run_id, lane, labels, trust_tier, concurrency_group, \
            fair_key, idem_token, stage, state, claim_window_secs, reservation_write_version) \
         VALUES ($1, 'fr-par', gen_random_uuid(), gen_random_uuid(), 'interactive', \
                 ARRAY[]::text[], 'trusted', NULL, $1, $2, 'shutdown-proof', 'queued', 900, 2) \
         RETURNING job_id::text",
    )
    .bind(&tenant_id)
    .bind(format!("shutdown-proof-{suffix}"))
    .fetch_one(&admin)
    .await
    .expect("seed one uniquely-owned queued job behind the startup signal gate");
    let cell_id = format!("ci-runner-activation-{suffix}");
    let checkout_repo_root = std::env::temp_dir().join(format!("myelin-ci-checkout-{suffix}"));
    std::fs::create_dir_all(&checkout_repo_root).expect("create checkout repository root");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ci-controlplane"))
        .env_clear()
        .env(
            "DATABASE_URL",
            "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin",
        )
        .env("DATABASE_MIGRATION_URL", admin_url)
        .env(
            "MYELIN_CI_SCHEDULER_DATABASE_URL",
            "postgres://myelin_ci_scheduler_fr_par:myelin_ci_scheduler_dev_pw@localhost:5433/myelin",
        )
        .env("S3_ENDPOINT", "http://localhost:9000")
        .env("S3_REGION", "fr-par")
        .env("S3_ACCESS_KEY", "myelin_dev_access")
        .env("S3_SECRET_KEY", "myelin_dev_secret")
        .env("S3_BUCKET", "myelin-dev")
        .env("REDIS_URL", "redis://localhost:6380")
        .env("NATS_URL", "nats://localhost:4222")
        .env("MYELIN_REGION", "fr-par")
        .env("MYELIN_CELL_ID", &cell_id)
        .env("MYELIN_KMS_SEAL_KEY", "55".repeat(32))
        .env(
            "XDG_RUNTIME_DIR",
            std::env::var("XDG_RUNTIME_DIR")
                .expect("live rootless runsc proof requires XDG_RUNTIME_DIR"),
        )
        .env(myelin_ci_sandbox::gvisor::ENV_RUNSC_BIN, &runsc)
        .env(myelin_ci_sandbox::gvisor::ENV_GVISOR_ROOTFS, &rootfs)
        .env("MYELIN_CI_RUNNER", "1")
        .env("MYELIN_CI_CHECKOUT_REPO_ROOT", &checkout_repo_root)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch the production CI Controlplane binary");
    let stderr = child.stderr.take().expect("capture child stderr");
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_reader = captured.clone();
    let (armed_tx, armed_rx) = mpsc::sync_channel(1);
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let line = line.expect("read CI Controlplane stderr");
            captured_reader.lock().unwrap().push_str(&line);
            captured_reader.lock().unwrap().push('\n');
            if line.contains("shutdown handlers armed; startup termination is intake-gated") {
                let _ = armed_tx.try_send(());
            }
            if line.contains("started (region `fr-par`") {
                let _ = started_tx.try_send(());
            }
        }
    });

    let armed = armed_rx.recv_timeout(Duration::from_secs(10));
    let kill = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("signal the CI Controlplane process");
    let status = child.wait().expect("wait for the bounded production drain");
    reader.join().expect("join child stderr reader");
    let stderr = captured.lock().unwrap().clone();
    std::fs::remove_dir_all(&checkout_repo_root).expect("remove checkout repository root");

    let untouched_state: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT state, lease_owner FROM public.job_queue \
         WHERE tenant_id = $1 AND job_id = $2::uuid",
    )
    .bind(&tenant_id)
    .bind(&seeded_job_id)
    .fetch_optional(&admin)
    .await
    .expect("inspect the shutdown-gated queued job");
    let job_cleanup =
        sqlx::query("DELETE FROM public.job_queue WHERE tenant_id = $1 AND job_id = $2::uuid")
            .bind(&tenant_id)
            .bind(&seeded_job_id)
            .execute(&admin)
            .await;
    let cleanup = sqlx::query("DELETE FROM public.cell_token_root WHERE cell_id = $1")
        .bind(&cell_id)
        .execute(&admin)
        .await;
    admin.close().await;
    cleanup.expect("remove only the activation proof's disposable cell root");
    job_cleanup.expect("remove only the activation proof's disposable queued job");

    armed.unwrap_or_else(|error| {
        panic!("shutdown handlers were not armed before timeout: {error}; stderr={stderr}")
    });
    assert!(kill.success(), "SIGTERM must reach the live process");
    assert!(
        status.success(),
        "boot-time SIGTERM must cleanly drain the production root: status={status}; stderr={stderr}"
    );
    assert!(
        started_rx.try_recv().is_err(),
        "runner intake must never announce after bootstrap was termination-gated; stderr={stderr}"
    );
    assert_eq!(
        untouched_state,
        Some(("queued".to_owned(), None)),
        "work queued before boot-time SIGTERM must remain unclaimed"
    );
}
