//! **SRCH-P03 / P-166 — the per-tenant index-directory forward-only migration, PROVEN against the
//! live dev-stack Postgres (the REAL data-layer proof the binding policy requires).**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-search --features integration \
//!     --test integration_srch_p03_index_directory -- --nocapture
//!
//! This drill is **red-until-proven** and flips green ONLY here, against the live stack — never
//! mocked. It proves the contract-1.5 forward-only migration the Search shell runs at boot
//! ([`myelin_search::SEARCH_INDEX_DIR_MIGRATION`]):
//!   1. the REAL `CREATE TABLE … search_index_directory` DDL APPLIES forward-only against live
//!      Postgres (a CREATE, never a DROP);
//!   2. the `(tenant, region)` PRIMARY KEY keys **per-tenant, per-region** index directories — two
//!      tenants get two distinct directory rows, and a duplicate `(tenant, region)` is rejected by
//!      the PK (the residency-pinned, per-tenant index layout, §3.4);
//!   3. the directory row carries the per-tenant index DEK ref (`index_dek_ref`) — the
//!      encrypted-from-birth anchor (§3.1) the SRCH-P04 `IndexBackend` seals every segment under.
//!
//! The DDL SHAPE under test is byte-for-byte the production migration constant (only the table
//! identifier is suffixed for isolation + cleanup, so concurrent runs don't collide).
//!
//! **Floor (named):** the on-disk index *directory* itself (the Tantivy/vector segment store, S3-
//! backed) is the SRCH-P04 `IndexBackend` — NOT this prompt. Here the migration creates the
//! per-tenant directory *catalog* (its `(tenant, region)` key + its `pii_key_ref`); the
//! encrypted-from-birth seal/open seam over the per-tenant index DEK is proven in-process by the
//! `KmsEngine` crypto (the real AES-GCM primitive) in `src/layout.rs` (the live S3 object-store
//! backing is SRCH-P30 / the IndexBackend's blob backstop — named, not shipped here).

#![cfg(feature = "integration")]

use myelin_search::SEARCH_INDEX_DIR_MIGRATION;

/// The dev default mirrors the myelin-config dev DATABASE_URL; read inline so Search adds NO crate
/// edge (it stays the graph's terminal leaf consumer — no myelin-config dep).
fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// Rewrite the production `search_index_directory`-named DDL onto a uniquely-suffixed table so
/// concurrent test runs don't collide and the table is cleanable. The DDL SHAPE (columns, the
/// `(tenant, region)` PK) is unchanged — only the table identifier is suffixed.
fn rename(ddl: &str, tbl: &str) -> String {
    ddl.replace("search_index_directory", tbl)
}

#[tokio::test]
async fn index_directory_migration_applies_forward_only_with_per_tenant_pk() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");

    let suffix = std::process::id();
    let tbl = format!("search_index_directory_p166_{suffix}");

    // Always start clean (a prior aborted run may have left the table).
    let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}")).execute(&admin).await;

    // ── 1. The REAL forward-only CREATE applies against live Postgres (a CREATE, never a DROP). ──
    assert!(
        !myelin_substrate::is_destructive(SEARCH_INDEX_DIR_MIGRATION),
        "the per-tenant index-directory migration is forward-only (a CREATE, never a DROP)"
    );
    let create = rename(SEARCH_INDEX_DIR_MIGRATION, &tbl);
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the per-tenant index-directory CREATE forward-only against live Postgres");

    // ── 2. The `(tenant, region)` PRIMARY KEY exists (per-tenant, per-region directories). ───────
    let pk_cols: Vec<String> = sqlx::query(
        "SELECT a.attname AS col \
         FROM pg_index i \
         JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) \
         WHERE i.indrelid = $1::regclass AND i.indisprimary \
         ORDER BY a.attnum",
    )
    .bind(&tbl)
    .fetch_all(&admin)
    .await
    .expect("read the primary key columns")
    .iter()
    .map(|r| r.get::<String, _>("col"))
    .collect();
    assert_eq!(
        pk_cols,
        vec!["tenant".to_string(), "region".to_string()],
        "the per-tenant index directory is keyed by (tenant, region) — residency-pinned, §3.4"
    );

    // ── 3. Two tenants get two DISTINCT directory rows; each carries its per-tenant index DEK ref. ─
    sqlx::query(&format!(
        "INSERT INTO {tbl} (tenant, region, index_dek_ref) VALUES \
         ('acme', 'fr-par', 'kms://acme/0/tenant'), \
         ('globex', 'fr-par', 'kms://globex/0/tenant')"
    ))
    .execute(&admin)
    .await
    .expect("insert two distinct per-tenant index directories");

    let rows = sqlx::query(&format!(
        "SELECT tenant, index_dek_ref FROM {tbl} ORDER BY tenant"
    ))
    .fetch_all(&admin)
    .await
    .expect("read the two directory rows");
    assert_eq!(rows.len(), 2, "two distinct per-tenant index directories");
    assert_eq!(rows[0].get::<String, _>("tenant"), "acme");
    assert_eq!(
        rows[0].get::<String, _>("index_dek_ref"),
        "kms://acme/0/tenant",
        "the directory carries the per-tenant index DEK ref (the encrypted-from-birth anchor, §3.1)"
    );
    assert_eq!(rows[1].get::<String, _>("tenant"), "globex");

    // ── 4. A DUPLICATE (tenant, region) is rejected by the PK (one directory per tenant per cell). ─
    let dup = sqlx::query(&format!(
        "INSERT INTO {tbl} (tenant, region, index_dek_ref) VALUES ('acme', 'fr-par', 'kms://acme/0/tenant')"
    ))
    .execute(&admin)
    .await;
    assert!(
        dup.is_err(),
        "a duplicate (tenant, region) index directory is rejected by the PRIMARY KEY \
         (exactly one per-tenant, per-region index directory)"
    );

    // ── cleanup ───────────────────────────────────────────────────────────────────────────────
    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await
        .expect("drop the throwaway test table");
}
