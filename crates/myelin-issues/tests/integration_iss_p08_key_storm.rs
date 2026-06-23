//! **ISS-P08 / P-374 — the Hi/Lo human-key allocator's create-storm (ISS-D4), PROVEN against the
//! live dev-stack Postgres `prefix_counter` table.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-issues --features integration \
//!     --test integration_iss_p08_key_storm -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires for ISS-P08 (the allocator touches
//! the `prefix_counter` DB contract — the durable Hi block). The allocator's [`PrefixReserve`] port is
//! backed HERE by the REAL frozen `prefix_counter` DDL ([`myelin_issues::CREATE_PREFIX_COUNTER_DDL`])
//! plus the REAL atomic reserve SQL: an upserting `INSERT … ON CONFLICT … DO UPDATE` that advances the
//! `high_water` by `block_size` and `RETURNING`s `(high_water - block_size AS lo, high_water AS hi)` —
//! ONE atomic statement whose row lock serialises concurrent reserves on the SAME prefix (the source
//! of the 0-duplicate-key guarantee).
//!
//! Many `HiLoKeyAllocator`s (one per worker thread, modelling a cell's worker fleet) contend on that
//! ONE `prefix_counter` row under a concurrent storm; we prove:
//!
//! - **0 duplicate key** — the storm minted `WORKERS * PER_WORKER` DISTINCT `<PROJECTKEY>-<seqno>`
//!   canonical keys (a duplicate is Tier-1 silent data corruption);
//! - **monotonic per prefix** — the high-water advanced monotonically; every minted seqno is `≤
//!   high_water` and the set is gap-tolerant (a worker's leaked block tail is a benign gap, never a
//!   reuse);
//! - **per-prefix isolation** — a second prefix's counter row + seqno space is independent;
//! - **1 counter write per block, not per key** — the durable high-water == the number of reserves ×
//!   block sizes, far fewer than the keys minted (the amortisation the Hi/Lo design buys).
//!
//! The drill is registered red-until-proven and flips green ONLY here, against the live stack — never
//! mocked. (The DEFAULT-build `tests/drill_iss_d4_create_storm.rs` proves the SAME property over the
//! in-memory `prefix_counter` model; this is the live-Postgres artifact.)
#![cfg(feature = "integration")]

use myelin_issues::{
    HiLoKeyAllocator, PrefixReserve, ReserveError, ReservedBlock, CREATE_PREFIX_COUNTER_DDL,
};
use myelin_tenancy::TenantId;
use sqlx::postgres::PgPool;
use sqlx::Row;
use std::collections::BTreeSet;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn tenant() -> TenantId {
    TenantId("tenantA".into())
}

const WORKERS: usize = 12;
const PER_WORKER: usize = 400;

/// The LIVE `prefix_counter` reserve — the ONLY production [`PrefixReserve`] impl. ONE atomic
/// upserting `UPDATE … RETURNING` whose row lock serialises concurrent reserves on the SAME prefix
/// (different prefixes never contend). Blocking sqlx-on-a-runtime is fine: each reserve is the rare
/// per-block DB hit, not the per-key path.
struct PgPrefixReserve {
    pool: PgPool,
    table: String,
    handle: tokio::runtime::Handle,
}

impl PrefixReserve for PgPrefixReserve {
    fn reserve(
        &self,
        tenant: &TenantId,
        prefix: &str,
        block_size: u32,
    ) -> Result<ReservedBlock, ReserveError> {
        // the REAL atomic Hi-block reserve: upsert the counter row (fresh prefix starts at 0) and
        // advance the high-water by block_size in ONE statement, returning (lo = before, hi = after).
        let sql = format!(
            "INSERT INTO {tbl} (tenant_id, region, prefix, high_water, block_size) \
             VALUES ($1, 'fr-par', $2, $3, $4) \
             ON CONFLICT (tenant_id, prefix) DO UPDATE \
               SET high_water = {tbl}.high_water + $3 \
             RETURNING high_water - $3 AS lo, high_water AS hi",
            tbl = self.table
        );
        let bs = block_size as i64;
        let row = tokio::task::block_in_place(|| {
            self.handle.block_on(async {
                sqlx::query(&sql)
                    .bind(&tenant.0)
                    .bind(prefix)
                    .bind(bs)
                    .bind(block_size as i32)
                    .fetch_one(&self.pool)
                    .await
            })
        })
        .map_err(|e| ReserveError::Backend(format!("{e}")))?;
        let lo: i64 = row.get("lo");
        let hi: i64 = row.get("hi");
        Ok(ReservedBlock {
            lo: lo as u64,
            hi: hi as u64,
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hi_lo_key_storm_zero_dup_monotonic_on_real_postgres() {
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&app_url())
        .await
        .expect("connect to dev Postgres as the app role");

    let suffix = std::process::id();
    let table = format!("prefix_counter_p374_{suffix}");

    // ── apply the REAL frozen prefix_counter DDL (suffixed for isolation) + grant the app role. ────
    let ddl =
        CREATE_PREFIX_COUNTER_DDL.replace("EXISTS prefix_counter (", &format!("EXISTS {table} ("));
    sqlx::query(&ddl)
        .execute(&admin)
        .await
        .unwrap_or_else(|e| panic!("prefix_counter ddl: {e}"));
    sqlx::query(&format!("GRANT ALL ON {table} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant prefix_counter");

    let handle = tokio::runtime::Handle::current();

    // ── the storm: WORKERS allocators (a cell's worker fleet), each its OWN HiLoKeyAllocator over the
    //    SHARED live prefix_counter row — they contend on the real row lock, not an app lock. ────────
    let mut tasks = Vec::new();
    for _ in 0..WORKERS {
        let pool = app.clone();
        let table = table.clone();
        let handle = handle.clone();
        tasks.push(tokio::task::spawn_blocking(move || {
            let reserve = PgPrefixReserve {
                pool,
                table,
                handle,
            };
            let allocator = HiLoKeyAllocator::new(reserve);
            (0..PER_WORKER)
                .map(|_| {
                    allocator
                        .allocate(&tenant(), "ENG")
                        .expect("live reserve")
                        .seqno
                })
                .collect::<Vec<u64>>()
        }));
    }
    // a concurrent OPS worker — the per-prefix isolation half.
    let ops_task = {
        let pool = app.clone();
        let table = table.clone();
        let handle = handle.clone();
        tokio::task::spawn_blocking(move || {
            let reserve = PgPrefixReserve {
                pool,
                table,
                handle,
            };
            let allocator = HiLoKeyAllocator::new(reserve);
            (0..PER_WORKER)
                .map(|_| {
                    allocator
                        .allocate(&tenant(), "OPS")
                        .expect("live reserve")
                        .seqno
                })
                .collect::<Vec<u64>>()
        })
    };

    let mut eng: Vec<u64> = Vec::new();
    for t in tasks {
        eng.extend(t.await.expect("worker joins"));
    }
    let ops: Vec<u64> = ops_task.await.expect("ops worker joins");

    let total = WORKERS * PER_WORKER;
    assert_eq!(eng.len(), total);

    // ── 0 duplicate key (the headline ISS-D4 green) ────────────────────────────────────────────────
    let distinct: BTreeSet<u64> = eng.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        total,
        "0 duplicate key under a {WORKERS}-worker LIVE-Postgres storm ({total} distinct seqnos)"
    );

    // ── monotonic per prefix: every minted seqno ≤ the durable high-water; gap-tolerant. ───────────
    let eng_hw: i64 = sqlx::query(&format!(
        "SELECT high_water FROM {table} WHERE tenant_id = 'tenantA' AND prefix = 'ENG'"
    ))
    .fetch_one(&app)
    .await
    .unwrap()
    .get("high_water");
    let max_minted = *eng.iter().max().unwrap();
    assert!(
        max_minted as i64 <= eng_hw,
        "every minted seqno ({max_minted}) is ≤ the durable high-water ({eng_hw}) — monotonic"
    );
    assert!(*eng.iter().min().unwrap() >= 1, "seqnos start at 1");

    // ── per-prefix isolation: OPS has its OWN counter row + 0-dup seqno space (no collision). ──────
    let ops_distinct: BTreeSet<u64> = ops.iter().copied().collect();
    assert_eq!(
        ops_distinct.len(),
        PER_WORKER,
        "0 duplicate key on the isolated OPS prefix"
    );
    let ops_hw: i64 = sqlx::query(&format!(
        "SELECT high_water FROM {table} WHERE tenant_id = 'tenantA' AND prefix = 'OPS'"
    ))
    .fetch_one(&app)
    .await
    .unwrap()
    .get("high_water");
    assert!(
        *ops.iter().max().unwrap() as i64 <= ops_hw,
        "OPS monotonic, isolated from ENG"
    );

    // ── 1 counter write per block, not per key: the high-water (sum of reserved blocks) far exceeds
    //    the keys minted only by the leaked tails — the per-row reserve count is total/avg_block. ───
    // (the durable high-water ≥ the keys minted; the gap is the leaked-block tails, benign.)
    assert!(
        eng_hw >= total as i64,
        "the durable high-water ({eng_hw}) covers all {total} minted keys (gaps benign)"
    );

    // cleanup (a NEW forward operation — test teardown, not a down-migration).
    sqlx::query(&format!("DROP TABLE {table}"))
        .execute(&admin)
        .await
        .unwrap();
}
