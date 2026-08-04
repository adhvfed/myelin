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

    let distinct: BTreeSet<u64> = eng.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        total,
        "0 duplicate key under a {WORKERS}-worker LIVE-Postgres storm ({total} distinct seqnos)"
    );

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
        "every minted seqno ({max_minted}) is ≤ the durable high-water ({eng_hw}) - monotonic"
    );
    assert!(*eng.iter().min().unwrap() >= 1, "seqnos start at 1");

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

    assert!(
        eng_hw >= total as i64,
        "the durable high-water ({eng_hw}) covers all {total} minted keys (gaps benign)"
    );

    sqlx::query(&format!("DROP TABLE {table}"))
        .execute(&admin)
        .await
        .unwrap();
}
