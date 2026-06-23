//! **CI-P20 / P-363 — the log pipeline, PROVEN against the live dev stack (RustFS + Postgres).**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_p20_log_pipeline -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires — CI-P20 touches the BlobStore
//! (11.2 — object store) AND the T3 `(job, step, byte-range)` index (11.8 — DB):
//!  1. **11.2 — the sealed segment flushes to the REAL RustFS bucket.** `LogPipeline<S3BlobStore>`
//!     seals a segment; the `log_segment.blob_ref` content address resolves through the REAL
//!     `S3BlobStore::get` (re-hash-on-read) to the exact redacted bytes — byte-identical, against
//!     the live object store (NOT the fs floor, NOT a mock).
//!  2. **11.8 — the index rows apply + INSERT + read back against the REAL Postgres.** The
//!     `log_segment`/`log_anchor` forward-only DDL applies onto suffixed throwaway tables, the
//!     produced rows INSERT via the FROZEN bind-param SQL (`INSERT_LOG_SEGMENT_QUERY` /
//!     `UPSERT_LOG_ANCHOR_QUERY`), and a read-back proves the `(job, step, byte-range)` index is
//!     consistent — 0 dangling anchors (every anchor's range is within the sealed segment's span).
//!
//! The drill is registered RED-UNTIL-PROVEN and flips green ONLY here, against the live stack —
//! never mocked, never named a "floor". dev<->prod is a config swap (RustFS↔Scaleway), never a code
//! change.
#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    AnchorStatus, CoalesceBudget, LogCoord, LogPipeline, SealThreshold, SecretRedactor,
    INSERT_LOG_SEGMENT_QUERY, UPSERT_LOG_ANCHOR_QUERY,
};
use myelin_config::MyelinConfig;
use myelin_storage::s3blob::S3BlobStore;
use myelin_storage::{BlobStore, ContentHash};
use myelin_tenancy::{Region, TenantId};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// Rewrite a production table-named DDL onto a uniquely-suffixed table (the SHAPE is unchanged — only
/// the identifier is suffixed for isolation + cleanup).
fn rename(ddl: &str, base: &str, tbl: &str) -> String {
    ddl.replace(&format!("EXISTS {base} ("), &format!("EXISTS {tbl} ("))
}

/// **11.2 — the sealed segment flushes to the REAL RustFS object store + round-trips byte-identically.**
#[tokio::test(flavor = "multi_thread")]
async fn sealed_segment_flushes_to_real_rustfs_and_round_trips() {
    let cfg = MyelinConfig::dev();
    let handle = tokio::runtime::Handle::current();
    let tenant = TenantId(format!("itest-ci-p20-{}", std::process::id()));
    let region = Region("fr-par".into());

    // The whole seal + verify runs on ONE blocking thread (the sync BlobStore trait drives the async
    // SDK via block_in_place; the pipeline owns its S3BlobStore for the seal).
    let line = "deploying build artifact to fr-par now".to_string();
    let line_for_thread = line.clone();
    let (blob_ref, got_bytes) = {
        let handle = handle.clone();
        let cfg = cfg.clone();
        let tenant = tenant.clone();
        let region = region.clone();
        tokio::task::spawn_blocking(move || {
            // The pipeline seals to the REAL RustFS bucket (11.2 — not the fs floor).
            let store = S3BlobStore::connect(&cfg.s3, handle.clone());
            let mut p = LogPipeline::new(tenant.clone(), region, store, SecretRedactor::default())
                .with_thresholds(
                    CoalesceBudget::default(),
                    SealThreshold { seal_at_bytes: 1 },
                );
            let coord = LogCoord::new("01J0RUN", "01J0JOB", "1");
            p.ship_line(&coord, &line_for_thread)
                .expect("in-region ship");
            // seal_at_bytes = 1 → the line sealed immediately to a content-addressed RustFS blob.
            assert_eq!(p.segment_rows().len(), 1, "one sealed segment");
            let blob_ref = p.segment_rows()[0].blob_ref.clone().expect("blob_ref");

            // Read the blob back THROUGH the real S3BlobStore (re-hash-on-read integrity).
            let verify_store = S3BlobStore::connect(&cfg.s3, handle);
            let addr = ContentHash::parse(&blob_ref).expect("the blob_ref parses");
            let got = verify_store
                .get(&tenant, &addr)
                .expect("get the sealed segment back from RustFS (re-hash-verified)");
            (blob_ref, got)
        })
        .await
        .expect("the blocking seal+verify task joins")
    };

    // The sealed bytes round-trip byte-identically against the REAL object store (11.2).
    assert_eq!(
        got_bytes,
        line.as_bytes(),
        "the sealed segment round-trips through RustFS"
    );
    // The blob_ref is the content address of the sealed bytes (content-addressed, deterministic).
    let expected = ContentHash::blake3(line.as_bytes()).to_multihash_string();
    assert_eq!(
        blob_ref, expected,
        "the log_segment.blob_ref is the content address (11.2)"
    );
}

/// **11.8 — the `(job, step, byte-range)` index rows apply + INSERT + read back against REAL Postgres
/// (0 dangling anchors).**
#[tokio::test(flavor = "multi_thread")]
async fn log_index_rows_apply_insert_and_read_back_against_real_postgres() {
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
    let seg_tbl = format!("log_segment_p363_{suffix}");
    let anc_tbl = format!("log_anchor_p363_{suffix}");

    // ── 1. Apply the REAL forward-only log_segment / log_anchor CREATE TABLE (arch 01 §3.5), ──
    //       suffixed, + the platform-wide RLS scoping (the same helper every tenant table uses).
    for (base, tbl, ddl) in [
        (
            "log_segment",
            &seg_tbl,
            myelin_ci_controlplane::CREATE_LOG_SEGMENT_DDL,
        ),
        (
            "log_anchor",
            &anc_tbl,
            myelin_ci_controlplane::CREATE_LOG_ANCHOR_DDL,
        ),
    ] {
        sqlx::query(&rename(ddl, base, tbl))
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("apply {base} forward-only: {e}"));
        sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{tbl}')"))
            .execute(&admin)
            .await
            .expect("RLS-scope the index table");
        sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
            .execute(&admin)
            .await
            .expect("grant the app role");
    }

    // ── 2. Produce the index rows through the REAL pipeline (a fs-floor blob is fine here — this ──
    //       test proves the INDEX write half; the RustFS seal is the sibling test).
    let tenant = TenantId(format!("itest-ci-p20-idx-{suffix}"));
    let mut p = LogPipeline::new(
        tenant.clone(),
        Region("fr-par".into()),
        myelin_storage::FsBlobStore::new(),
        SecretRedactor::default(),
    )
    .with_thresholds(
        CoalesceBudget::default(),
        SealThreshold { seal_at_bytes: 40 },
    );
    let coord = LogCoord::new(
        "33333333-3333-3333-3333-333333333333",
        "44444444-4444-4444-4444-444444444444",
        "1",
    );
    for _ in 0..8 {
        p.ship_line(&coord, "0123456789").expect("ship"); // 10 bytes each → seals at 40
    }
    p.close_step(&coord, AnchorStatus::Failed)
        .expect("close the step");
    assert!(!p.segment_rows().is_empty(), "a segment sealed");
    assert_eq!(
        p.dangling_anchor_count(),
        0,
        "0 dangling anchors in the produced index"
    );

    // ── 3. INSERT the produced rows via the FROZEN bind-param SQL into REAL Postgres, RLS-pinned. ──
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

    let seg_insert = INSERT_LOG_SEGMENT_QUERY.replace("log_segment", &seg_tbl);
    for seg in p.segment_rows() {
        sqlx::query(&seg_insert)
            .bind(&seg.tenant_id)
            .bind(&seg.region)
            .bind(sqlx::types::Uuid::parse_str(&seg.run_id).unwrap())
            .bind(sqlx::types::Uuid::parse_str(&seg.job_id).unwrap())
            .bind(seg.segment_seq)
            .bind(seg.blob_ref.as_deref())
            .bind(seg.byte_start)
            .bind(seg.byte_end)
            .bind(&seg.pii_key_ref)
            .execute(&mut *conn)
            .await
            .expect("INSERT the log_segment row via the frozen bind-param SQL");
    }
    let anc_insert = UPSERT_LOG_ANCHOR_QUERY.replace("log_anchor", &anc_tbl);
    for anc in p.anchor_rows() {
        sqlx::query(&anc_insert)
            .bind(&anc.tenant_id)
            .bind(&anc.region)
            .bind(sqlx::types::Uuid::parse_str(&anc.run_id).unwrap())
            .bind(sqlx::types::Uuid::parse_str(&anc.job_id).unwrap())
            .bind(&anc.step_id)
            .bind(anc.byte_start)
            .bind(anc.byte_end)
            .bind(anc.status.token())
            .execute(&mut *conn)
            .await
            .expect("UPSERT the log_anchor row via the frozen bind-param SQL");
    }

    // ── 4. Read back + PROVE the index is consistent: 0 dangling anchors against the live rows. ──
    //       An anchor is dangling iff its byte_end exceeds the max sealed segment byte_end.
    let max_seg_end: i64 = sqlx::query(&format!(
        "SELECT COALESCE(MAX(byte_end), 0) AS m FROM {seg_tbl}"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("m");
    let dangling: i64 = sqlx::query(&format!(
        "SELECT COUNT(*) AS c FROM {anc_tbl} WHERE byte_end IS NOT NULL AND byte_end > $1"
    ))
    .bind(max_seg_end)
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("c");
    assert_eq!(
        dangling, 0,
        "0 dangling anchors against the LIVE Postgres index (every (job, step) anchor's range is \
         within the sealed segment span — 11.8 consistency)"
    );

    // a sanity read: the segment row's blob_ref is durably present (the (blob, offset) index).
    let seg_count: i64 = sqlx::query(&format!(
        "SELECT COUNT(*) AS c FROM {seg_tbl} WHERE blob_ref IS NOT NULL"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("c");
    assert!(
        seg_count >= 1,
        "the sealed segment's (blob, offset) row is durably present"
    );

    // ── cleanup ──
    sqlx::query(&format!("DROP TABLE IF EXISTS {seg_tbl}"))
        .execute(&admin)
        .await
        .ok();
    sqlx::query(&format!("DROP TABLE IF EXISTS {anc_tbl}"))
        .execute(&admin)
        .await
        .ok();
}
