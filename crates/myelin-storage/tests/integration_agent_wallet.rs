//! # The durable prepaid AGENT WALLET, proven against LIVE Postgres.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build/test --workspace` stays
//! DB-free. Runs ONLY against the docker-compose dev stack (or the make-it-real env):
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     AWS_DEFAULT_REGION=fr-par cargo test -p myelin-storage --features integration \
//!       --test integration_agent_wallet -- --nocapture
//!
//! It proves the wallet's financial-correctness contract on the LIVE DB (a pass on any in-memory
//! model would NOT count — these all hit real Postgres, real RLS, the real immutability trigger):
//!   A. **credit/debit + `balance == Σ ledger`** — a topup, a debit, and a refund each move the
//!      materialized balance to the exact running sum of the append-only ledger.
//!   B. **durability** — a FRESH wallet over a FRESH pool reads the balance + ledger back.
//!   C. **insufficient-balance debit refused ATOMICALLY** — nothing is written (no ledger row, no
//!      balance change), no partial debit.
//!   D. **integer overflow fail-closed** — an amount `> i64::MAX`, and a credit whose sum would
//!      exceed `i64::MAX`, are refused writing nothing.
//!   E. **IMMUTABILITY enforced** — UPDATE and DELETE on `agent_wallet_ledger` both RAISE (the app
//!      role is REVOKEd; the owner hits the raising trigger).
//!   F. **RLS isolation** — one tenant cannot read or write another tenant's wallet/ledger rows.
//!
//! Skips gracefully if the DB is unreachable (like the sibling integration tests).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::agent_wallet::{
    agent_wallet_migrations, AgentWallet, CreditKind, MicroUsd, WalletError,
};
use myelin_storage::migration::HotTables;
use myelin_storage::SubstrateProvider;
use myelin_tenancy::TenantId;

fn admin_config(cfg: &MyelinConfig) -> MyelinConfig {
    let mut c = cfg.clone();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

fn uniq() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// Apply the agent-wallet migration (0080) as the admin/owner role. `None` (SKIP) if unreachable.
async fn migrate_admin() -> Option<SubstrateProvider> {
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return None;
        }
    };
    admin
        .migrate(&agent_wallet_migrations(), &HotTables::none())
        .await
        .expect("apply the agent-wallet migration (0080)");
    Some(admin)
}

/// A FRESH app-role provider (NOBYPASSRLS, reset-on-release) over a NEW pool — the kill-9-equivalent
/// reconstruction seam (new connections, nothing carried in-process).
async fn app_provider() -> SubstrateProvider {
    SubstrateProvider::connect(MyelinConfig::dev(), 6)
        .await
        .expect("connect app role")
}

/// The Σ of the ledger for `(tenant, region)`, computed DIRECTLY in SQL from the append-only rows
/// (`topup + refund` credit, `debit` debits) — the independent oracle the materialized balance must
/// equal. Read with the tenant/region GUCs set so RLS admits the rows.
async fn ledger_sum(pool: &sqlx::PgPool, tenant: &str, region: &str) -> i64 {
    let mut tx = pool.begin().await.expect("begin ledger-sum tx");
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)")
        .bind(tenant)
        .bind(region)
        .execute(&mut *tx)
        .await
        .expect("scope ledger-sum tx");
    let sum: Option<i64> = sqlx::query_scalar(
        "SELECT COALESCE(SUM(CASE WHEN kind = 'debit' THEN -amount_micro ELSE amount_micro END), 0)::bigint \
         FROM agent_wallet_ledger WHERE tenant_id = $1 AND region = $2",
    )
    .bind(tenant)
    .bind(region)
    .fetch_one(&mut *tx)
    .await
    .expect("sum the ledger");
    tx.commit().await.expect("commit ledger-sum tx");
    sum.unwrap_or(0)
}

async fn ledger_row_count(pool: &sqlx::PgPool, tenant: &str, region: &str) -> i64 {
    let mut tx = pool.begin().await.expect("begin count tx");
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)")
        .bind(tenant)
        .bind(region)
        .execute(&mut *tx)
        .await
        .expect("scope count tx");
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_wallet_ledger WHERE tenant_id = $1 AND region = $2",
    )
    .bind(tenant)
    .bind(region)
    .fetch_one(&mut *tx)
    .await
    .expect("count the ledger rows");
    tx.commit().await.expect("commit count tx");
    n
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_wallet_durable_contract() {
    let Some(admin) = migrate_admin().await else {
        return;
    };
    let app = app_provider().await;
    let region = app.config().region.clone();
    let suffix = uniq();

    // =============================================================================================
    // A — credit/debit + balance == Σ ledger, on live Pg (FORCE-RLS, with_tenant_tx).
    // =============================================================================================
    let tenant = TenantId(format!("01J0WALLET{suffix}"));
    let wallet = AgentWallet::new(app.clone());

    // A never-funded wallet reads 0 (no row yet).
    assert_eq!(wallet.balance(&tenant), MicroUsd::ZERO, "empty wallet is 0");

    // Top-up seed ($5.00 = 5_000_000 micro-USD).
    let after_topup = wallet
        .credit(&tenant, MicroUsd(5_000_000), CreditKind::Topup, None)
        .expect("topup credits");
    assert_eq!(after_topup, MicroUsd(5_000_000));
    assert_eq!(wallet.balance(&tenant), MicroUsd(5_000_000));

    // Debit a sub-cent agent task cost (1234 micro-USD ≈ $0.001234) — sub-cent precision is the point.
    let after_debit = wallet
        .debit(&tenant, MicroUsd(1_234), &format!("run-{suffix}"))
        .expect("debit within balance");
    assert_eq!(after_debit, MicroUsd(4_998_766));

    // A refund credit (e.g. a released over-reservation).
    let after_refund = wallet
        .credit(&tenant, MicroUsd(1_000), CreditKind::Refund, Some(&format!("run-{suffix}")))
        .expect("refund credits");
    assert_eq!(after_refund, MicroUsd(4_999_766));

    // balance == Σ ledger (the materialized cache equals the independent SQL sum of the ledger).
    let sum = ledger_sum(app.db_pool(), &tenant.0, &region).await;
    assert_eq!(
        sum, 4_999_766,
        "the ledger sums to the balance (topup 5_000_000 − debit 1_234 + refund 1_000)"
    );
    assert_eq!(
        wallet.balance(&tenant),
        MicroUsd(sum as u64),
        "materialized balance == Σ ledger"
    );
    assert_eq!(
        ledger_row_count(app.db_pool(), &tenant.0, &region).await,
        3,
        "three append-only ledger rows (topup, debit, refund)"
    );

    // =============================================================================================
    // B — durability: a FRESH wallet over a FRESH pool reads the balance back (kill-9 equivalent).
    // =============================================================================================
    let wallet2 = AgentWallet::new(app_provider().await);
    assert_eq!(
        wallet2.balance(&tenant),
        MicroUsd(4_999_766),
        "the balance survived reconstruction from a fresh pool"
    );

    // =============================================================================================
    // C — insufficient-balance debit refused ATOMICALLY (nothing written, no partial debit).
    // =============================================================================================
    let before = wallet2.balance(&tenant);
    let rows_before = ledger_row_count(app.db_pool(), &tenant.0, &region).await;
    let err = wallet2
        .debit(&tenant, MicroUsd(999_999_999), &format!("run-broke-{suffix}"))
        .expect_err("an over-balance debit is refused");
    assert_eq!(
        err,
        WalletError::InsufficientBalance {
            requested: MicroUsd(999_999_999),
            available: before,
        }
    );
    assert_eq!(wallet2.balance(&tenant), before, "balance unchanged by a refused debit");
    assert_eq!(
        ledger_row_count(app.db_pool(), &tenant.0, &region).await,
        rows_before,
        "a refused debit writes NO ledger row (fail-closed, no partial debit)"
    );

    // A debit against a NEVER-FUNDED wallet is insufficient (balance 0) and writes nothing.
    let broke = TenantId(format!("01J0BROKE{suffix}"));
    let broke_err = wallet2
        .debit(&broke, MicroUsd(1), &format!("run-{suffix}"))
        .expect_err("a debit on an unfunded wallet is refused");
    assert_eq!(
        broke_err,
        WalletError::InsufficientBalance {
            requested: MicroUsd(1),
            available: MicroUsd::ZERO,
        }
    );
    assert_eq!(
        ledger_row_count(app.db_pool(), &broke.0, &region).await,
        0,
        "no ledger row for an unfunded refused debit"
    );

    // =============================================================================================
    // D — integer overflow fail-closed (amount > i64::MAX; and a sum that would exceed i64::MAX).
    // =============================================================================================
    let big = TenantId(format!("01J0BIG{suffix}"));
    // An amount above the bigint range is refused writing nothing.
    assert_eq!(
        wallet2.credit(&big, MicroUsd(i64::MAX as u64 + 1), CreditKind::Topup, None),
        Err(WalletError::AmountTooLarge),
    );
    assert_eq!(ledger_row_count(app.db_pool(), &big.0, &region).await, 0);
    // Fund near the ceiling, then a further credit whose SUM would exceed i64::MAX is refused.
    wallet2
        .credit(&big, MicroUsd(i64::MAX as u64), CreditKind::Topup, None)
        .expect("fund near the bigint ceiling");
    assert_eq!(
        wallet2.credit(&big, MicroUsd(10), CreditKind::Topup, None),
        Err(WalletError::BalanceOverflow),
        "a sum above i64::MAX is refused",
    );
    assert_eq!(
        wallet2.balance(&big),
        MicroUsd(i64::MAX as u64),
        "the refused overflow credit left the balance untouched"
    );
    assert_eq!(
        ledger_row_count(app.db_pool(), &big.0, &region).await,
        1,
        "only the one successful near-ceiling topup is recorded",
    );

    // =============================================================================================
    // E — IMMUTABILITY: UPDATE and DELETE on agent_wallet_ledger both RAISE.
    // =============================================================================================
    // (i) The OWNER hits the raising trigger (GUCs set so RLS admits the row → the trigger fires).
    let mut tx = admin.db_pool().begin().await.expect("begin owner mutate tx");
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)")
        .bind(&tenant.0)
        .bind(&region)
        .execute(&mut *tx)
        .await
        .expect("scope owner mutate tx");
    let upd = sqlx::query("UPDATE agent_wallet_ledger SET amount_micro = 0 WHERE tenant_id = $1")
        .bind(&tenant.0)
        .execute(&mut *tx)
        .await;
    assert!(
        upd.as_ref().err().map(|e| e.to_string()).unwrap_or_default().contains("immutable"),
        "UPDATE on agent_wallet_ledger must RAISE 'immutable' (owner/trigger): {upd:?}"
    );
    tx.rollback().await.ok();

    let mut tx = admin.db_pool().begin().await.expect("begin owner delete tx");
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)")
        .bind(&tenant.0)
        .bind(&region)
        .execute(&mut *tx)
        .await
        .expect("scope owner delete tx");
    let del = sqlx::query("DELETE FROM agent_wallet_ledger WHERE tenant_id = $1")
        .bind(&tenant.0)
        .execute(&mut *tx)
        .await;
    assert!(
        del.as_ref().err().map(|e| e.to_string()).unwrap_or_default().contains("immutable"),
        "DELETE on agent_wallet_ledger must RAISE 'immutable' (owner/trigger): {del:?}"
    );
    tx.rollback().await.ok();

    // (ii) The APP role is additionally REVOKEd UPDATE/DELETE (defence in depth) — the mutation is
    // refused (either the privilege check or the trigger fires; both are a hard error, never a silent
    // rewrite). The ledger row is unchanged.
    let mut tx = app.db_pool().begin().await.expect("begin app mutate tx");
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)")
        .bind(&tenant.0)
        .bind(&region)
        .execute(&mut *tx)
        .await
        .expect("scope app mutate tx");
    let app_upd = sqlx::query("UPDATE agent_wallet_ledger SET amount_micro = 0 WHERE tenant_id = $1")
        .bind(&tenant.0)
        .execute(&mut *tx)
        .await;
    assert!(
        app_upd.is_err(),
        "the app role must NOT be able to UPDATE agent_wallet_ledger (REVOKE + trigger)"
    );
    tx.rollback().await.ok();
    // The immutable history is intact.
    assert_eq!(
        ledger_sum(app.db_pool(), &tenant.0, &region).await,
        4_999_766,
        "the ledger is unchanged after the refused mutations"
    );

    // =============================================================================================
    // F — RLS isolation: one tenant cannot read or write another tenant's wallet/ledger rows.
    // =============================================================================================
    // The wallet API scopes every op to the caller's tenant, so tenant `other` sees its own (empty)
    // wallet — never `tenant`'s funded balance.
    let other = TenantId(format!("01J0OTHER{suffix}"));
    assert_eq!(
        wallet2.balance(&other),
        MicroUsd::ZERO,
        "a different tenant sees its OWN (empty) wallet, never the funded tenant's"
    );

    // Direct proof at the DB: with the WRONG tenant's GUCs set, `tenant`'s rows are invisible AND a
    // write into `tenant`'s partition is refused by the WITH CHECK policy.
    let mut tx = app.db_pool().begin().await.expect("begin RLS tx");
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)")
        .bind(&other.0)
        .bind(&region)
        .execute(&mut *tx)
        .await
        .expect("scope RLS tx as `other`");
    let visible: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_wallet_ledger WHERE tenant_id = $1",
    )
    .bind(&tenant.0)
    .fetch_one(&mut *tx)
    .await
    .expect("count cross-tenant rows");
    assert_eq!(
        visible, 0,
        "RLS hides `tenant`'s ledger rows from a session scoped to `other`"
    );
    // A cross-tenant WRITE (insert a ledger row into `tenant`'s partition from `other`'s session) is
    // refused by the RLS WITH CHECK policy.
    let cross_write = sqlx::query(
        "INSERT INTO agent_wallet_ledger (tenant_id, region, kind, amount_micro) \
         VALUES ($1, $2, 'topup', 1)",
    )
    .bind(&tenant.0)
    .bind(&region)
    .execute(&mut *tx)
    .await;
    assert!(
        cross_write.is_err(),
        "a cross-tenant INSERT into `tenant`'s partition is refused by RLS WITH CHECK"
    );
    tx.rollback().await.ok();

    // =============================================================================================
    // Cleanup — the FORCE-RLS wallet rows use unique per-run tenants (no cross-run pollution). Remove
    // this run's rows via the admin/owner role (which can DELETE agent_wallet; the ledger's DELETE is
    // trigger-blocked, so its rows are left as durable evidence keyed by the unique tenant).
    // =============================================================================================
    for t in [&tenant.0, &big.0] {
        let _ = sqlx::query("DELETE FROM agent_wallet WHERE tenant_id = $1")
            .bind(t)
            .execute(admin.db_pool())
            .await;
    }
}
