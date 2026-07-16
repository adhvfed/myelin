//! **CT-004a — the DURABLE CI `cost_event` projection store, KILL-9 PROVEN through the store.**
//!
//! The sibling `integration_ci_p28_ct004_durability.rs` proved the metering SQL survives a
//! kill-9/reopen by running the RAW `INSERT_COST_EVENT_QUERY` against a per-pid temp table — NOT
//! through a production store (there was none: metering was "model-only"). CT-004a builds the real
//! store ([`myelin_ci_controlplane::CiCostEventStore`]) and re-proves the durability THROUGH it:
//!
//!   1. **A committed settle survives kill-9/reopen.** `store.settle(...)` (its own tx) COMMITS; the
//!      pool is dropped WITHOUT a graceful close (the process "dies"); a brand-new store over a FRESH
//!      pool reads the exact rows back via `store.cost_events_for_run(...)` — settled stays settled,
//!      cost attributed to `(run_id, job_id)`, wholesale ≠ markup intact. The state lived in Postgres,
//!      never in process memory.
//!   2. **An UNCOMMITTED settle leaves NO ghost row.** `store.settle_in_tx(&mut tx, ...)` runs on a
//!      transaction that is then DROPPED without commit (the crash before the co-commit completes) —
//!      after reopen there is ZERO cost row for that run (no half-billed ghost).
//!   3. **Money-parity: a re-delivered settle records EXACTLY ONCE.** A second `settle` of the same
//!      metered units affects 0 rows (`ON CONFLICT (tenant_id, cost_id) DO NOTHING` via the
//!      deterministic `cost_id`), and the read-back count is unchanged — the reserve/settle bookends
//!      do not double-bill through the CI projection.
//!
//! **Isolation.** The store executes the BYTE-IDENTICAL production constants (`INTO cost_event`,
//! hardcoded). To keep that verbatim while staying isolated from other tests AND from Storage's
//! money-ledger `cost_event` (the migration-`0050` name collision documented on
//! `myelin_ci_controlplane::ci_cost_event_store`), each pool sets its `search_path` to a per-pid
//! schema in which ONLY CI's `cost_event` (CI schema) exists — so the store's unqualified `cost_event`
//! resolves there, unmodified.
//!
//! Gated behind the `integration` cargo feature. Run against the docker-compose dev stack:
//!
//!   eval "$(scripts/dev-stack.sh env)"
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_ct004a_cost_store_durability -- --nocapture
#![cfg(feature = "integration")]

use myelin_ci_controlplane::{CiCostEventStore, CostEventRow, CostKind, Meter, CREATE_COST_EVENT_DDL};
use myelin_flow::MinorUnits;
use myelin_tenancy::TenantId;
use sqlx::types::Uuid;
use sqlx::{Executor, PgPool};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// The per-pid schema every pool in this test pins `search_path` to — so the store's unqualified
/// `cost_event` resolves to an ISOLATED CI-schema table (never Storage's money-ledger `cost_event`
/// nor another test's rows).
fn schema_name() -> String {
    format!("ci_ct004a_{}", std::process::id())
}

/// Open a FRESH admin pool whose connections pin `search_path` to the per-pid schema. Calling this
/// after `drop(prev_pool)` models a process restart (the "reopen" half of a kill-9 durability proof:
/// the only way the state is still here is if it lives in Postgres).
async fn reopen() -> PgPool {
    let schema = schema_name();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _meta| {
            let schema = schema.clone();
            Box::pin(async move {
                // Pin the search_path so the store's unqualified `cost_event` resolves to our schema.
                conn.execute(format!("SET search_path TO {schema}").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&admin_url())
        .await
        .expect("reconnect to dev Postgres as admin (is the stack up? eval \"$(scripts/dev-stack.sh env)\")")
}

/// A stable uuid from a name — deterministic (a simple FNV-1a fill) so a reopened pool can assert
/// equality against the SAME id the pre-crash pool wrote.
fn uid(name: &str) -> Uuid {
    let mut bytes = [0u8; 16];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    bytes[..8].copy_from_slice(&h.to_be_bytes());
    let mut h2: u64 = h ^ 0x00ff_00ff_00ff_00ff;
    for b in name.bytes().rev() {
        h2 ^= b as u64;
        h2 = h2.wrapping_mul(0x0000_0100_0000_01b3);
    }
    bytes[8..].copy_from_slice(&h2.to_be_bytes());
    Uuid::from_bytes(bytes)
}

/// The two metered units a settle records for `(run, job)` — CPU + memory, with wholesale ≠ markup on
/// each (the arch §8 two-column invariant, exercised across meters). `run_id`/`job_id` are UUID
/// strings (the durable column type).
fn settle_rows(tenant: &TenantId, run: Uuid, job: Uuid) -> Vec<CostEventRow> {
    vec![
        CostEventRow {
            tenant: tenant.clone(),
            run_id: run.to_string(),
            job_id: job.to_string(),
            meter: Meter::CpuSeconds,
            amount: 120,
            wholesale: MinorUnits(100),
            markup: MinorUnits(20),
            kind: CostKind::Ci,
        },
        CostEventRow {
            tenant: tenant.clone(),
            run_id: run.to_string(),
            job_id: job.to_string(),
            meter: Meter::MemGbSeconds,
            amount: 4096,
            wholesale: MinorUnits(50),
            markup: MinorUnits(10),
            kind: CostKind::Ci,
        },
    ]
}

#[tokio::test]
async fn cost_store_settle_survives_kill9_no_ghost_no_double_bill() {
    let schema = schema_name();
    let tenant = TenantId("acme".into());
    let region = "fr-par";
    let run = uid("ct004a-run-ok");
    let job = uid("ct004a-job-ok");
    let crash_run = uid("ct004a-run-crash");
    let crash_job = uid("ct004a-job-crash");

    // ── Fresh schema + CI-schema `cost_event` (isolated from Storage's `0050` cost_event). ──
    let p1 = reopen().await;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&p1)
        .await
        .expect("drop any prior schema");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&p1)
        .await
        .expect("create the per-pid schema");
    // The pool's search_path already points at `schema`; the unqualified DDL lands there.
    sqlx::query(CREATE_COST_EVENT_DDL)
        .execute(&p1)
        .await
        .expect("apply CI cost_event DDL into the isolated schema");

    // ── COMMITTED settle THROUGH the store (its own tx → commit). ──
    let store1 = CiCostEventStore::with_pg(p1.clone());
    let rows = settle_rows(&tenant, run, job);
    let affected = store1
        .settle(region, &rows)
        .await
        .expect("the committed settle records the metered units");
    assert_eq!(affected, 2, "two metered units recorded (cost_events_per_unit == 1 each)");

    // ── Money-parity: a RE-DELIVERED settle records EXACTLY ONCE (deterministic cost_id + ON CONFLICT). ──
    let redelivered = store1
        .settle(region, &rows)
        .await
        .expect("a re-delivered settle is a no-op success");
    assert_eq!(
        redelivered, 0,
        "a doubly-delivered settle records ONCE (double-effect = 0 — the bookends do not double-bill)"
    );

    // ── UNCOMMITTED settle THROUGH the store: run on a tx, then DROP it without commit. ──
    {
        let mut tx = p1.begin().await.unwrap();
        let crash_rows = settle_rows(&tenant, crash_run, crash_job);
        let n = store1
            .settle_in_tx(&mut tx, region, &crash_rows)
            .await
            .expect("the in-tx settle inserts (uncommitted)");
        assert_eq!(n, 2, "the in-tx settle wrote its rows (pending commit)");
        // Drop the tx WITHOUT commit → rollback (the crash before the co-commit completes).
        drop(tx);
    }

    // ── KILL-9: drop the pool + store without a graceful close. The process "dies". ──
    drop(store1);
    drop(p1);

    // ── REOPEN: a brand-new pool + a FRESH store read the state back from Postgres. ──
    let p2 = reopen().await;
    let store2 = CiCostEventStore::with_pg(p2.clone());

    // (1) The committed settle survived kill-9/reopen — exact rows, attribution + wholesale ≠ markup.
    let read = store2
        .cost_events_for_run(&tenant, &run.to_string())
        .await
        .expect("read back the settled units after reopen");
    assert_eq!(
        read.len(),
        2,
        "both committed metered units survive kill-9/reopen (durable, not in-memory)"
    );
    // Canonical (job_id, meter) order: CpuSeconds ('cpu_seconds') sorts before MemGbSeconds ('mem_gb_seconds').
    assert_eq!(read[0].meter, Meter::CpuSeconds);
    assert_eq!(read[0].job_id, job.to_string(), "attributed to its (run, job)");
    assert_eq!(read[0].wholesale, MinorUnits(100));
    assert_eq!(read[0].markup, MinorUnits(20));
    assert_ne!(
        read[0].wholesale, read[0].markup,
        "wholesale ≠ markup (the two cost columns are distinct, arch 02 §8)"
    );
    assert_eq!(read[0].kind, CostKind::Ci, "settled kind stays ci after reopen");
    assert_eq!(read[1].meter, Meter::MemGbSeconds);
    assert_eq!(read[1].wholesale, MinorUnits(50));
    assert_eq!(read[1].markup, MinorUnits(10));

    // Re-delivery did NOT duplicate across the crash: still exactly two rows for the run.
    // (settle idempotency proven above; the read-back count confirms no double-billing persisted.)

    // (2) The UNCOMMITTED (crashed-between-steps) settle left NO ghost cost row.
    let ghost = store2
        .cost_events_for_run(&tenant, &crash_run.to_string())
        .await
        .expect("read back the crashed run after reopen");
    assert!(
        ghost.is_empty(),
        "the uncommitted settle left NO ghost cost row (no half-billed run — all-or-nothing)"
    );

    // ── Cleanup: drop the per-pid schema. ──
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&p2)
        .await
        .ok();
    println!(
        "[CT-004a] PASS cost_store: committed settle survives kill-9/reopen (2 units, attributed to \
         (run,job), wholesale≠markup, kind=ci); re-delivered settle → 0 rows (double-effect = 0, no \
         double-bill); uncommitted settle-in-tx → 0 ghost rows (all-or-nothing) — proven THROUGH the store"
    );
}
