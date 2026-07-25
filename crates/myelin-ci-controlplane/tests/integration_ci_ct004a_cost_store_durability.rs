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
//! **CT-004m — the tables come from the REAL migration path.** The store executes the BYTE-IDENTICAL
//! production constants (`INTO ci_cost_event`, hardcoded). Rather than a hand-written bare CREATE, this
//! test now applies the REAL forward-only [`myelin_ci_controlplane::ci_durable_migrations`] set (the
//! SAME migrations both CI service mains apply at boot — `ci_run` + `check_attempt` + `ci_cost_event`,
//! each `(tenant, region)`-first + FORCE-RLS via the platform `myelin_make_tenant_scoped` helper) into
//! a per-pid schema. So the durability proof runs against the tables exactly as the real migration
//! path produces them — no stopgap bare DDL. The per-pid schema keeps concurrent runs isolated; the
//! CT-004m rename (CI's table is `ci_cost_event`, distinct from Storage's money-ledger `cost_event`,
//! migration `0050`) is what lets the two coexist in the ONE shared `myelin` DB (proven in the sibling
//! `integration_ci_ct004m_cost_event_no_collision`). The pool connects as the migration/owner role
//! (`myelin_admin`, BYPASSRLS) to create the isolated schema; store operations still carry a verified
//! tenant/region scope and use the production transaction-local GUC convention.
//!
//! Gated behind the `integration` cargo feature. Run against the docker-compose dev stack:
//!
//!   eval "$(scripts/dev-stack.sh env)"
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_ct004a_cost_store_durability -- --nocapture
#![cfg(feature = "integration")]

mod common;

use common::with_schema_cleanup;
use myelin_ci_controlplane::{
    ci_durable_migrations, CiCostEventStore, CiCostStoreError, CostEventRow, CostKind, Meter,
};
use myelin_flow::MinorUnits;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};
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

fn verified_scope(tenant: &TenantId, region: &str) -> TenantScope {
    let mut principal = Principal::stub(
        PrincipalId("cost-store-test".into()),
        PrincipalKind::Service,
        tenant.clone(),
    );
    principal.region = Region(region.into());
    TenantScope::from_verified_token(&principal, principal.region.clone())
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
                // Pin the search_path so the store's unqualified `ci_cost_event` resolves to our
                // schema first; `public` follows so the platform RLS helper `myelin_make_tenant_scoped`
                // (defined in public, called by the real ci_durable_migrations) resolves.
                conn.execute(format!("SET search_path TO {schema}, public").as_str())
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
    // A cleanup-dedicated pool, independent of the `p1`/`p2` pools the kill-9 drill below drops mid-
    // test — `with_schema_cleanup` unconditionally drops `schema` through THIS pool when the test body
    // (success, assertion failure, or panic) finishes.
    let cleanup_pool = reopen().await;
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_pool, &schema_for_cleanup, move || async move {
    let tenant = TenantId("acme".into());
    let region = "fr-par";
    let scope = verified_scope(&tenant, region);
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
    // The pool's search_path already points at `schema`; apply the REAL forward-only CI durable
    // migration set (ci_run + check_attempt + ci_cost_event, each with FORCE-RLS) — the SAME set both
    // CI mains apply at boot. Each migration DDL is multi-statement (CREATE + myelin_make_tenant_scoped),
    // so it runs via the simple-query protocol (`Executor::execute(&str)`), landing in `schema`.
    for m in ci_durable_migrations().0.iter() {
        p1.execute(m.ddl)
            .await
            .unwrap_or_else(|e| panic!("apply CI durable migration {} into the schema: {e}", m.id));
    }

    // ── COMMITTED settle THROUGH the store (its own tx → commit). ──
    let store1 = CiCostEventStore::with_pg(p1.clone(), Region(region.into()));
    let rows = settle_rows(&tenant, run, job);
    let affected = store1
        .settle(&scope, &rows)
        .await
        .expect("the committed settle records the metered units");
    assert_eq!(
        affected, 2,
        "two metered units recorded (cost_events_per_unit == 1 each)"
    );

    // ── Money-parity: a RE-DELIVERED settle records EXACTLY ONCE (deterministic cost_id + ON CONFLICT). ──
    let redelivered = store1
        .settle(&scope, &rows)
        .await
        .expect("a re-delivered settle is a no-op success");
    assert_eq!(
        redelivered, 0,
        "a doubly-delivered settle records ONCE (double-effect = 0 — the bookends do not double-bill)"
    );

    // ── VERIFY-ON-CONFLICT (#13): a re-delivery of the SAME unit (same cost_id) with a DIFFERENT
    //    amount is REFUSED LOUDLY, not silently dropped. The idempotency key is (tenant, run, job,
    //    meter) — not the amount — so `ON CONFLICT DO NOTHING` alone would keep the first amount and
    //    hide the divergence in the billing table. ──
    let mut divergent = settle_rows(&tenant, run, job);
    divergent[0].amount = 999; // same (run, job, CpuSeconds) → same cost_id; amount 999 ≠ recorded 120
    let err = store1
        .settle(&scope, &divergent)
        .await
        .expect_err("a divergent re-delivered settle must be refused, never silently dropped");
    match err {
        CiCostStoreError::AmountDivergence {
            column,
            recorded,
            incoming,
        } => {
            assert_eq!(column, "amount");
            assert_eq!(
                recorded, 120,
                "the FIRST settle's amount is preserved (never overwritten)"
            );
            assert_eq!(incoming, 999, "the divergent incoming amount is surfaced");
        }
        other => panic!("expected AmountDivergence, got {other}"),
    }
    // The refusal did NOT mutate the recorded row: the read-back is still the original amount.
    let after = store1
        .cost_events_for_run(&scope, &run.to_string())
        .await
        .expect("read back after the refused divergent settle");
    let cpu = after
        .iter()
        .find(|r| r.meter == Meter::CpuSeconds)
        .expect("the CpuSeconds unit");
    assert_eq!(
        cpu.amount, 120,
        "the divergent settle was refused — the recorded amount is unchanged"
    );

    // ── UNCOMMITTED settle THROUGH the store: run on a tx, then DROP it without commit. ──
    {
        let mut tx = p1.begin().await.unwrap();
        sqlx::query(
            "SELECT set_config('myelin.tenant_id', $1, true), \
                    set_config('myelin.region', $2, true)",
        )
        .bind(scope.tenant().as_str())
        .bind(&scope.region().0)
        .execute(&mut *tx)
        .await
        .expect("scope the caller-owned co-commit transaction");
        let crash_rows = settle_rows(&tenant, crash_run, crash_job);
        let n = store1
            .settle_in_tx(&mut tx, &scope, &crash_rows)
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
    let store2 = CiCostEventStore::with_pg(p2.clone(), Region(region.into()));

    // (1) The committed settle survived kill-9/reopen — exact rows, attribution + wholesale ≠ markup.
    let read = store2
        .cost_events_for_run(&scope, &run.to_string())
        .await
        .expect("read back the settled units after reopen");
    assert_eq!(
        read.len(),
        2,
        "both committed metered units survive kill-9/reopen (durable, not in-memory)"
    );
    // Canonical (job_id, meter) order: CpuSeconds ('cpu_seconds') sorts before MemGbSeconds ('mem_gb_seconds').
    assert_eq!(read[0].meter, Meter::CpuSeconds);
    assert_eq!(
        read[0].job_id,
        job.to_string(),
        "attributed to its (run, job)"
    );
    assert_eq!(read[0].wholesale, MinorUnits(100));
    assert_eq!(read[0].markup, MinorUnits(20));
    assert_ne!(
        read[0].wholesale, read[0].markup,
        "wholesale ≠ markup (the two cost columns are distinct, arch 02 §8)"
    );
    assert_eq!(
        read[0].kind,
        CostKind::Ci,
        "settled kind stays ci after reopen"
    );
    assert_eq!(read[1].meter, Meter::MemGbSeconds);
    assert_eq!(read[1].wholesale, MinorUnits(50));
    assert_eq!(read[1].markup, MinorUnits(10));

    // Re-delivery did NOT duplicate across the crash: still exactly two rows for the run.
    // (settle idempotency proven above; the read-back count confirms no double-billing persisted.)

    // (2) The UNCOMMITTED (crashed-between-steps) settle left NO ghost cost row.
    let ghost = store2
        .cost_events_for_run(&scope, &crash_run.to_string())
        .await
        .expect("read back the crashed run after reopen");
    assert!(
        ghost.is_empty(),
        "the uncommitted settle left NO ghost cost row (no half-billed run — all-or-nothing)"
    );

    println!(
        "[CT-004a] PASS cost_store: committed settle survives kill-9/reopen (2 units, attributed to \
         (run,job), wholesale≠markup, kind=ci); re-delivered settle → 0 rows (double-effect = 0, no \
         double-bill); uncommitted settle-in-tx → 0 ghost rows (all-or-nothing) — proven THROUGH the store"
    );
    })
    .await;
}
