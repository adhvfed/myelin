//! **The durable rebuild journal, PROVEN against live Postgres.**
//!
//! `crates/myelin-search/src/rebuild.rs` makes one guarantee that no in-process test can establish:
//! that two coordinators racing for the same `(tenant, region)` cannot BOTH proceed. The in-memory
//! journal double holds a `Mutex`, so its compare-and-set is atomic for free — which means the
//! in-memory tests prove the coordinator's LOGIC and prove nothing at all about the durable store.
//! The exclusion has to hold in Postgres, under real concurrency, or the whole safety argument is
//! decoration: a lost race means one coordinator wipes an index the other is replaying into.
//!
//! Gated behind the `integration` cargo feature so the default workspace build stays DB-free. Run
//! against the isolated dev stack:
//!
//! ```text
//! fed isolate enable && fed start postgres
//! DATABASE_URL="postgres://myelin_admin:myelin_dev_pw@localhost:<port>/myelin" \
//!   cargo test -p myelin-search --features integration --test integration_srch_rebuild_journal
//! ```
//!
//! What this proves, all against the REAL DDL and the REAL production query constants:
//!
//! 1. the forward-only migrations APPLY (a `CREATE`, never a `DROP`), and the partial index too;
//! 2. `(tenant, region)` is the PRIMARY KEY — a rebuild job is per-tenant per-region, never
//!    cell-wide, and a duplicate claim cannot create a second job row;
//! 3. the INITIAL claim is exclusive: of N concurrent inserters exactly ONE wins
//!    (`ON CONFLICT DO NOTHING` → `rows_affected() == 0` for the losers);
//! 4. the phase advance is exclusive: a holder carrying a STALE fence epoch matches no row, so a
//!    displaced coordinator cannot journal a transition over its replacement;
//! 5. a cross-tenant / cross-region write does not touch a neighbouring job row.

#![cfg(feature = "integration")]

use sqlx::Row;

use myelin_search::rebuild::{
    SEARCH_REBUILD_ACTIVE_INDEX_MIGRATION, SEARCH_REBUILD_JOB_MIGRATION,
};
use myelin_search::rebuild_durable::{
    INSERT_REBUILD_JOB_QUERY, SELECT_REBUILD_JOB_QUERY, UPDATE_REBUILD_JOB_QUERY,
};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// Rewrite the production-named DDL/queries onto a uniquely-suffixed table so concurrent runs do not
/// collide. Only the table identifier changes — the SHAPE (columns, PK, the `ON CONFLICT` and the
/// `WHERE fence_epoch` predicates) is byte-for-byte the production constant, which is the point.
fn rename(sql: &str, tbl: &str) -> String {
    sql.replace("search_rebuild_job", tbl)
}

#[tokio::test]
async fn the_durable_rebuild_journal_is_exclusive_under_real_concurrency() {
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the isolated stack up? `fed start postgres`)");

    let tbl = format!("search_rebuild_job_it_{}", std::process::id());
    let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await;

    // ── 1. Both forward-only migrations apply against live Postgres. ─────────────────────────────
    for ddl in [
        SEARCH_REBUILD_JOB_MIGRATION,
        SEARCH_REBUILD_ACTIVE_INDEX_MIGRATION,
    ] {
        assert!(
            !myelin_substrate::is_destructive(ddl),
            "the rebuild-job migrations are additive CREATEs, never DROPs"
        );
        sqlx::query(&rename(ddl, &tbl))
            .execute(&admin)
            .await
            .expect("apply the rebuild-job migration forward-only against live Postgres");
    }

    // Applying twice is a no-op (`IF NOT EXISTS`) — boot re-runs migrations on every start.
    for ddl in [
        SEARCH_REBUILD_JOB_MIGRATION,
        SEARCH_REBUILD_ACTIVE_INDEX_MIGRATION,
    ] {
        sqlx::query(&rename(ddl, &tbl))
            .execute(&admin)
            .await
            .expect("the migration is idempotent — boot applies it every start");
    }

    // ── 2. The PK is (tenant, region): a job is per-tenant per-region, never cell-wide. ──────────
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
        "a rebuild job is keyed per (tenant, region)"
    );

    // ── 3. The INITIAL claim is exclusive: of 8 concurrent claimants, EXACTLY ONE wins. ──────────
    //
    // This is the race that matters. If both won, both would fence, both would wipe, and one would
    // wipe the index the other had already begun replaying into.
    let insert = rename(INSERT_REBUILD_JOB_QUERY, &tbl);
    let mut claims = Vec::new();
    for holder in 0..8 {
        let pool = admin.clone();
        let sql = insert.clone();
        claims.push(tokio::spawn(async move {
            sqlx::query(&sql)
                .bind("acme") // tenant
                .bind("fr-par") // region
                .bind("claimed") // phase
                .bind(1_i64) // fence_epoch
                .bind(Option::<i64>::None) // high_water_mark
                .bind("") // owners_replayed
                .bind(Some(format!("worker-{holder}"))) // lease_holder
                .bind(1_000_i64) // lease_expires_at
                .execute(&pool)
                .await
                .expect("the claim statement executes")
                .rows_affected()
        }));
    }
    let mut winners = 0u64;
    for c in claims {
        winners += c.await.expect("claim task joins");
    }
    assert_eq!(
        winners, 1,
        "EXACTLY ONE of eight concurrent claimants may win the initial claim — a lost race means \
         two coordinators wipe the same index"
    );

    // Exactly one job row exists.
    let n: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&admin)
        .await
        .expect("count job rows")
        .get("n");
    assert_eq!(n, 1, "one job row per (tenant, region)");

    // ── 4. A phase advance carrying a STALE fence epoch matches NO row. ──────────────────────────
    //
    // The displaced-holder scenario: worker A claims at epoch 1, its lease expires, worker B takes
    // over and bumps the epoch to 2. Worker A — paused, not dead — wakes up and tries to journal a
    // phase transition. It must be refused, or it stomps on B's rebuild.
    let update = rename(UPDATE_REBUILD_JOB_QUERY, &tbl);
    let bump = |epoch: i64, expected: i64, phase: &'static str| {
        let pool = admin.clone();
        let sql = update.clone();
        async move {
            sqlx::query(&sql)
                .bind("acme")
                .bind("fr-par")
                .bind(phase)
                .bind(epoch)
                .bind(Option::<i64>::None)
                .bind("")
                .bind(Some("worker".to_string()))
                .bind(2_000_i64)
                .bind(expected)
                .execute(&pool)
                .await
                .expect("the update statement executes")
                .rows_affected()
        }
    };

    // Worker B takes over: epoch 1 → 2.
    assert_eq!(
        bump(2, 1, "fenced").await,
        1,
        "the current holder (expecting the stored epoch) advances"
    );
    // Worker A, still carrying epoch 1, tries to advance. Refused.
    assert_eq!(
        bump(1, 1, "wiped").await,
        0,
        "a holder carrying a STALE fence epoch matches no row — it cannot journal over its \
         replacement"
    );

    // The row still holds B's phase, not A's.
    let phase: String = sqlx::query(&rename(SELECT_REBUILD_JOB_QUERY, &tbl))
        .bind("acme")
        .bind("fr-par")
        .fetch_one(&admin)
        .await
        .expect("read the job row")
        .get("phase");
    assert_eq!(
        phase, "fenced",
        "the displaced holder's write did not land — the phase is the current holder's"
    );

    // ── 5. Cross-tenant / cross-region writes do not touch a neighbouring job. ───────────────────
    for (tenant, region) in [("globex", "fr-par"), ("acme", "nl-ams")] {
        let affected = sqlx::query(&insert)
            .bind(tenant)
            .bind(region)
            .bind("claimed")
            .bind(1_i64)
            .bind(Option::<i64>::None)
            .bind("")
            .bind(Some("neighbour".to_string()))
            .bind(1_000_i64)
            .execute(&admin)
            .await
            .expect("insert the neighbouring job")
            .rows_affected();
        assert_eq!(
            affected, 1,
            "a different (tenant, region) is a DIFFERENT job — it never contends"
        );
    }
    // acme/fr-par is untouched by either neighbour's claim.
    let phase: String = sqlx::query(&rename(SELECT_REBUILD_JOB_QUERY, &tbl))
        .bind("acme")
        .bind("fr-par")
        .fetch_one(&admin)
        .await
        .expect("re-read the original job row")
        .get("phase");
    assert_eq!(
        phase, "fenced",
        "a neighbouring tenant's / region's rebuild did not disturb this job"
    );

    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await
        .expect("drop the throwaway test table");
}
