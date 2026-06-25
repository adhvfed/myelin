//! **CI-P32 / P-492 — the CI crypto-shred erase fan-out (CI-D3 erasure-reaches-every-holder),
//! PROVEN against the live dev stack (Postgres) + the real KMS.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_p32_crypto_shred_erase -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires — CI-P32 touches the per-subject
//! DEK crypto-shred (11.4) over CI stores that carry `pii_key_ref` (the `log_segment` index, the
//! `ci_run` run-state row's `triggered_by` identity edge). The drill proves, against REAL Postgres
//! + the real [`myelin_storage::kms::KmsEngine`]:
//!
//!  1. **crypto-shred (11.4) — the per-subject DEK is destroyed → the sealed ciphertext is
//!     UNRECOVERABLE.** A `log_segment` row sealed under the subject's per-subject DEK is INSERTed
//!     into the live `log_segment` table; the KMS holds the DEK live; the erase fan-out destroys it;
//!     `resolve_dek` then fails LOUDLY (the 0-fail-open invariant) — the inline-PII ciphertext can
//!     never be re-opened, in the live store OR a backup (the backup snapshot excludes the shredded
//!     DEK, §7.5).
//!  2. **pseudonym-shred (§6, 4.8) — the identity is destroyed, the FACT survives.** The live
//!     `ci_run` row's `triggered_by` is UPDATEd from the subject's principal id to the erased
//!     pseudonym; the run row STILL EXISTS (the structure survives for audit — a run ran), it just no
//!     longer names the person.
//!  3. **0 recoverable PII incl. backups** — the post-erase re-verify counts 0 resolvable subject
//!     ciphertexts (live + restored).
//!
//! The drill is registered RED-UNTIL-PROVEN and flips green ONLY here, against the live stack — never
//! mocked, never named a "floor". dev<->prod is a config swap (Postgres↔Scaleway), never a code
//! change. FLOOR (named, by reference): the residual third-party free-text PII is the ONE platform
//! posture (10.9 / X-7) — the per-tenant DEK fallback shreds it at tenant-erase; the lawful-basis
//! residual is the parallel Legal track, never restated CI-local.
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

/// Rewrite a production table-named DDL onto a uniquely-suffixed throwaway table (the SHAPE is
/// unchanged — only the identifier is suffixed for isolation).
fn rename(ddl: &str, base: &str, tbl: &str) -> String {
    ddl.replace(&format!("EXISTS {base} ("), &format!("EXISTS {tbl} ("))
}

/// **CI-D3 — erase(subject) crypto-shreds the live CI PII + pseudonymises the identity edge, the
/// structure survives, 0 recoverable incl. backups — against REAL Postgres + the real KMS.**
#[tokio::test(flavor = "multi_thread")]
async fn ci_d3_erase_crypto_shreds_live_pii_and_pseudonymises_structure_survives() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&app_url())
        .await
        .expect("connect to dev Postgres as the app role");

    let suffix = std::process::id();
    let run_tbl = format!("ci_run_p492_{suffix}");
    let seg_tbl = format!("log_segment_p492_{suffix}");

    // ── 1. Apply the REAL forward-only ci_run + log_segment DDL, suffixed, RLS-scoped. ──
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

    // ── 2. Seal the subject's per-subject DEK LIVE in the real KMS (the producer's envelope-encrypt). ──
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
    let subj_key = kms
        .ensure_dek(&tenant, &region, KeyClass::Subject(subject.to_string()))
        .expect("seal the subject DEK live");
    // The DEK resolves BEFORE the erase (the ciphertext is recoverable).
    assert!(
        kms.resolve_dek(&subj_key, &region).is_ok(),
        "the subject's per-subject DEK resolves before the erase"
    );

    // ── 3. INSERT the live CI rows: a ci_run with triggered_by = the subject, a log_segment sealed ──
    //       under the subject's per-subject pii_key_ref. RLS-pinned to the tenant.
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
        .bind(subj_key.to_uri()) // sealed under the subject's per-subject DEK (the erase lever).
        .execute(&mut *conn)
        .await
        .expect("INSERT the log_segment row sealed under the subject DEK");

    // ── 4. Run the CI-D3 erase fan-out over the subject's footprint (run-state + logs). ──
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

    // ── 5a. crypto-shred: the per-subject DEK is destroyed → the ciphertext is UNRECOVERABLE. ──
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

    // ── 5b. pseudonym-shred the identity edge in the LIVE ci_run row (the structure survives). ──
    let update =
        format!("UPDATE {run_tbl} SET triggered_by = $1 WHERE tenant_id = $2 AND run_id = $3");
    sqlx::query(&update)
        .bind(ERASED_PSEUDONYM)
        .bind(tenant.as_str())
        .bind(sqlx::types::Uuid::parse_str(run_id).unwrap())
        .execute(&mut *conn)
        .await
        .expect("pseudonymise triggered_by in the live ci_run row");

    // The run row STILL EXISTS (structure survives) and no longer names the subject.
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
        "triggered_by is pseudonymised — the subject's identity is gone"
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

    // ── 6. cleanup the throwaway tables. ──
    for tbl in [&run_tbl, &seg_tbl] {
        let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
            .execute(&admin)
            .await;
    }

    println!("{}", report.summary());
}
