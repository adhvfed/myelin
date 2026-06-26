//! # STOR-D5 / CP-D3 (the live, store-layer leg of P-CP-12) — **cross-region egress IMPOSSIBLE**,
//! proven against the LIVE docker-compose dev stack (real Postgres + real RLS), not modeled.
//!
//! This is the **store-layer twin** of the in-process four-layer enforcement
//! (`myelin-control-plane` `four_layer_e2e_drill`): the runtime `residency-pin` write boundary
//! (§5.3 layer 3) — *every write asserts `row.region == cell.region`* — is enforced **by Postgres**,
//! not by app code, via the `(tenant, region)` RLS `WITH CHECK (region = current_setting(...))`
//! policy on `rebac_tuple`. A write whose ROW region differs from the cell's session region is
//! REJECTED by the DB; a read for a tenant's data outside the cell's region returns ZERO rows. So:
//!
//!   - **CP-D3:** a write where `row.region ≠ cell.region` → the residency-pin write boundary
//!     REJECTS it (the DB raises a row-violates-policy error). 0 out-of-region writes admitted.
//!   - **STOR-D5:** read/replicate a tenant's data outside its region → IMPOSSIBLE (region is in the
//!     partition key; RLS keys on it). 0 cross-region PII egress.
//!
//! Run against the dev stack:
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-storage --features integration --test stor_d5_cross_region_egress_drill -- --nocapture
//!
//! **No engineering floor (P-CP-12):** the residency mechanism is fully built. The `[OPEN — LEGAL]`
//! region-change-as-DSR + slug-PII-screening residuals ship regardless (not engineering gates).
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};

use myelin_config::MyelinConfig;
use myelin_storage::pg::PgStore;

/// DDL/seed runs as the migration/owner role (myelin_admin); the egress probe ALSO connects as the
/// NOBYPASSRLS app role (myelin_app) so the DB — not app code — enforces the residency boundary.
fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// A process-unique suffix so concurrent runs never collide.
fn uniq() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )
}

/// **THE STOR-D5 / CP-D3 LIVE DRILL: the residency write boundary is enforced by Postgres.** A
/// store pinned to the cell's region `fr-par`:
///   1. ADMITS an in-region write (row region == cell region == fr-par) — the green leg.
///   2. REJECTS an out-of-region write (row region eu-central ≠ cell region fr-par) — the DB's RLS
///      `WITH CHECK` raises (CP-D3 / the residency-pin write boundary at the LIVE DB).
///   3. Proves cross-region READ is impossible: a session scoped to a DIFFERENT region than the
///      tenant's rows returns ZERO rows (STOR-D5 — region is in the partition key).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stor_d5_cross_region_egress_impossible() {
    let cfg = MyelinConfig::dev();
    assert_eq!(
        cfg.region, "fr-par",
        "the dev/prod region is fr-par (MYELIN_REGION)"
    );

    // The ADMIN (owner) store runs DDL only. RLS is enforced for the NOBYPASSRLS app role, NOT for
    // a superuser/owner — so every write-boundary PROBE below runs through the `myelin_app` store
    // (the real runtime role): the residency boundary must be enforced by the DB for the role the
    // service actually connects as.
    let admin = PgStore::connect(&admin_url(&cfg), &cfg.region, 2)
        .await
        .expect("connect Postgres as admin (is the stack up?)");
    admin
        .migrate()
        .await
        .expect("migrate (rebac_tuple + (tenant,region) RLS WITH CHECK)");

    // The STORE under test: the NOBYPASSRLS `myelin_app` role, pinned to the CELL's region (fr-par)
    // — the residency pin the harness injects. RLS (incl. the WITH CHECK write boundary) is in force.
    let store = PgStore::connect(&cfg.database_url, &cfg.region, 4)
        .await
        .expect("connect Postgres as the NOBYPASSRLS app role");

    let tag = uniq();
    let tenant = format!("tenant-d5-{tag}");

    // ── (1) GREEN: an in-region write (row region == cell region == fr-par) is ADMITTED. The normal
    //        production path (put_tuple) always writes self.region, so it is in-region by construction.
    store
        .put_tuple(&tenant, "doc-1", "reader", "user:alice")
        .await
        .expect("an in-region write (row.region == cell.region == fr-par) is admitted");

    // ── (2) CP-D3 RED: an OUT-OF-REGION write (row region eu-central ≠ cell region fr-par) is
    //        REJECTED by the DB's RLS WITH CHECK — the residency-pin write boundary at the LIVE DB.
    let out_of_region = store
        .put_tuple_in_region(&tenant, "eu-central", "doc-evil", "reader", "user:mallory")
        .await;
    assert!(
        out_of_region.is_err(),
        "CP-D3: an out-of-region write (row.region=eu-central, cell.region=fr-par) MUST be REJECTED \
         by the DB's RLS WITH CHECK (the residency-pin write boundary) — got {out_of_region:?}"
    );
    let err_text = format!("{:?}", out_of_region.unwrap_err());
    // Postgres raises a row-level-security policy violation (SQLSTATE 42501 / "row-level security").
    assert!(
        err_text.to_lowercase().contains("row-level security")
            || err_text.to_lowercase().contains("policy")
            || err_text.contains("42501"),
        "the rejection is a DB RLS policy violation (not an app-side check): {err_text}"
    );

    // The out-of-region row never landed: counting eu-central rows for this tenant (as admin,
    // scoped to eu-central) yields ZERO — 0 out-of-region writes admitted.
    let mut eu_conn = store
        .scoped_conn_in_region(&tenant, "eu-central")
        .await
        .expect("acquire a session scoped to eu-central");
    let eu_rows: i64 = {
        use sqlx::Row;
        let r = sqlx::query("SELECT count(*) AS n FROM rebac_tuple WHERE tenant_id = $1")
            .bind(&tenant)
            .fetch_one(&mut *eu_conn)
            .await
            .expect("count eu-central rows");
        r.get::<i64, _>("n")
    };
    assert_eq!(
        eu_rows, 0,
        "0 out-of-region writes admitted: no eu-central row for this tenant landed"
    );
    drop(eu_conn);

    // ── (3) STOR-D5: cross-region READ is impossible. A session scoped to a DIFFERENT region than
    //        the tenant's fr-par rows returns ZERO of them (region is in the partition key; RLS keys
    //        on it). 0 cross-region PII egress.
    let visible_from_eu = store
        .reverse_index_in_region(&tenant, "eu-central", "user:alice", "reader")
        .await
        .expect("read as an eu-central session");
    assert!(
        visible_from_eu.is_empty(),
        "STOR-D5: a session in eu-central sees ZERO of the tenant's fr-par rows — cross-region read \
         is impossible (0 cross-region PII egress); got {visible_from_eu:?}"
    );

    // The IN-region session DOES see the fr-par row (the read works within the residency boundary).
    let visible_from_fr = store
        .reverse_index(&tenant, "user:alice", "reader")
        .await
        .expect("read as the in-region (fr-par) session");
    assert_eq!(
        visible_from_fr,
        vec!["doc-1".to_string()],
        "the in-region session sees the tenant's fr-par row (the read works inside the boundary)"
    );

    println!(
        "[2026-06-19] PASS  drill=STOR-D5/CP-D3-CROSS-REGION-EGRESS  cell_region=fr-par  \
         in_region_write=admitted  out_of_region_write=REJECTED-by-DB-RLS-WITH-CHECK  \
         out_of_region_writes_admitted=0  cross_region_read_rows=0 (0 PII egress)  \
         in_region_read_rows={}  backend=real-PG (FORCE RLS (tenant,region) WITH CHECK)",
        visible_from_fr.len()
    );

    // cleanup (admin, scoped to fr-par so the RLS DELETE policy admits it). `scoped_conn` now
    // returns a tenant-scoped TRANSACTION (MR-013) — commit it so the DELETE actually lands.
    let mut conn = store
        .scoped_conn(&tenant)
        .await
        .expect("acquire fr-par session for cleanup");
    sqlx::query("DELETE FROM rebac_tuple WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&mut *conn)
        .await
        .ok();
    conn.commit().await.ok();
}
