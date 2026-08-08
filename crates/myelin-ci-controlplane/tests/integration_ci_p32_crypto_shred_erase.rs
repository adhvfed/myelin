#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    ci_run_ref, drive_ci_d3_erasure_reaches_every_holder, ArtifactStore, CiSealedRow, CiStoreClass,
    CiSubjectFootprint, CREATE_CI_RUN_DDL, CREATE_LOG_SEGMENT_DDL, ERASED_PSEUDONYM,
    INSERT_LOG_SEGMENT_QUERY,
};
use myelin_storage::kms::{KekId, KeyClass, KmsEngine};
use myelin_tenancy::{Region, TenantId};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn rename(ddl: &str, base: &str, tbl: &str) -> String {
    ddl.replace(&format!("EXISTS {base} ("), &format!("EXISTS {tbl} ("))
}

#[tokio::test(flavor = "multi_thread")]
async fn ci_d3_erase_crypto_shreds_live_pii_and_pseudonymises_structure_survives() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? run `fed test:backend`)");
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&app_url())
        .await
        .expect("connect to dev Postgres as the app role");

    let suffix = std::process::id();
    let run_tbl = format!("ci_run_p492_{suffix}");
    let seg_tbl = format!("log_segment_p492_{suffix}");

    for (base, tbl, ddl) in [
        ("ci_run", &run_tbl, CREATE_CI_RUN_DDL),
        ("log_segment", &seg_tbl, CREATE_LOG_SEGMENT_DDL),
    ] {
        sqlx::query(&rename(ddl, base, tbl))
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("apply {base} forward-only: {e}"));
        sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{tbl}')"))
            .execute(&admin)
            .await
            .expect("RLS-scope the table");
        sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
            .execute(&admin)
            .await
            .expect("grant the app role");
    }

    let tenant = TenantId(format!("itest-ci-p492-{suffix}"));
    let region = Region("fr-par".into());
    let subject = "psn:ci-erase-me";
    let run_id = "55555555-5555-5555-5555-555555555555";
    let job_id = "66666666-6666-6666-6666-666666666666";

    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
    let subj_key = kms
        .ensure_dek(&tenant, &region, KeyClass::Subject(subject.to_string()))
        .expect("seal the subject DEK live");
    assert!(
        kms.resolve_dek(&subj_key, &region).is_ok(),
        "the subject's per-subject DEK resolves before the erase"
    );

    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)")
        .bind(tenant.as_str())
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn)
        .await
        .unwrap();

    let run_insert = format!(
        "INSERT INTO {run_tbl} (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, \
         definition_snapshot, trigger_kind, triggered_by, trust_tier, state, correlation_id) \
         VALUES ($1,'fr-par',$2,$3,$3,$3,'{{}}','manual',$4,'trusted','succeeded','corr-1')"
    );
    sqlx::query(&run_insert)
        .bind(tenant.as_str())
        .bind(sqlx::types::Uuid::parse_str(run_id).unwrap())
        .bind(sqlx::types::Uuid::parse_str(job_id).unwrap())
        .bind(subject)
        .execute(&mut *conn)
        .await
        .expect("INSERT the ci_run row with the subject as triggered_by");

    let seg_insert = INSERT_LOG_SEGMENT_QUERY.replace("log_segment", &seg_tbl);
    sqlx::query(&seg_insert)
        .bind(tenant.as_str())
        .bind("fr-par")
        .bind(sqlx::types::Uuid::parse_str(run_id).unwrap())
        .bind(sqlx::types::Uuid::parse_str(job_id).unwrap())
        .bind(0i32)
        .bind(Some("blake3:deadbeef"))
        .bind(0i64)
        .bind(64i64)
        .bind(subj_key.to_uri())
        .execute(&mut *conn)
        .await
        .expect("INSERT the log_segment row sealed under the subject DEK");

    let footprint = CiSubjectFootprint::new()
        .with_row(CiSealedRow::with_identity_edge(
            CiStoreClass::RunState,
            subj_key.clone(),
            ci_run_ref(tenant.as_str(), run_id),
            subject,
        ))
        .with_row(CiSealedRow::sealed(
            CiStoreClass::Logs,
            subj_key.clone(),
            ci_run_ref(tenant.as_str(), run_id),
        ));
    let mut store = ArtifactStore::new();
    let report = drive_ci_d3_erasure_reaches_every_holder(
        subject,
        &tenant,
        region.clone(),
        &footprint,
        &kms,
        &mut store,
    )
    .expect("CI-D3 erase fan-out runs");

    assert!(
        kms.resolve_dek(&subj_key, &region).is_err(),
        "after the erase the subject's per-subject DEK no longer resolves (ciphertext unrecoverable)"
    );
    assert_eq!(
        report.recoverable_live, 0,
        "0 recoverable PII in the live store"
    );
    assert_eq!(
        report.recoverable_after_restore, 0,
        "0 recoverable PII after a backup restore (the shredded DEK is excluded, §7.5)"
    );
    assert!(report.is_green(), "CI-D3 is GREEN: {}", report.summary());

    let update =
        format!("UPDATE {run_tbl} SET triggered_by = $1 WHERE tenant_id = $2 AND run_id = $3");
    sqlx::query(&update)
        .bind(ERASED_PSEUDONYM)
        .bind(tenant.as_str())
        .bind(sqlx::types::Uuid::parse_str(run_id).unwrap())
        .execute(&mut *conn)
        .await
        .expect("pseudonymise triggered_by in the live ci_run row");

    let row = sqlx::query(&format!(
        "SELECT triggered_by, state FROM {run_tbl} WHERE tenant_id = $1 AND run_id = $2"
    ))
    .bind(tenant.as_str())
    .bind(sqlx::types::Uuid::parse_str(run_id).unwrap())
    .fetch_one(&mut *conn)
    .await
    .expect("the ci_run row survives the erase (delete the identity, not the fact)");
    let triggered_by: Option<String> = row.get("triggered_by");
    let state: String = row.get("state");
    assert_eq!(
        triggered_by.as_deref(),
        Some(ERASED_PSEUDONYM),
        "triggered_by is pseudonymised - the subject's identity is gone"
    );
    assert_ne!(
        triggered_by.as_deref(),
        Some(subject),
        "the subject's principal id is NO LONGER in the live row"
    );
    assert_eq!(
        state, "succeeded",
        "the run FACT (it ran, it succeeded) survives for audit"
    );

    for tbl in [&run_tbl, &seg_tbl] {
        let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
            .execute(&admin)
            .await;
    }

    println!("{}", report.summary());
}
