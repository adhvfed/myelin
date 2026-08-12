#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::agent_wallet::{
    agent_wallet_charge_migrations, agent_wallet_migrations, AgentWallet, CreditKind, DebitOutcome,
    MicroUsd, WalletError,
};
use myelin_storage::migration::HotTables;
use myelin_storage::SubstrateProvider;
use myelin_tenancy::TenantId;

fn app_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            config.database_url = database_url;
        }
    }
    config
}

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

async fn migrate_admin() -> SubstrateProvider {
    let admin = SubstrateProvider::connect(admin_config(&app_config()), 4)
        .await
        .expect("integration tests require the configured Postgres backend");
    admin
        .migrate(&agent_wallet_migrations(), &HotTables::none())
        .await
        .expect("apply the agent-wallet migration (0080)");
    admin
        .migrate(&agent_wallet_charge_migrations(), &HotTables::none())
        .await
        .expect("apply replay-safe charge keys (0095)");
    admin
}

async fn app_provider() -> SubstrateProvider {
    SubstrateProvider::connect(app_config(), 6)
        .await
        .expect("connect app role")
}

async fn ledger_sum(pool: &sqlx::PgPool, tenant: &str, region: &str) -> i64 {
    let mut tx = pool.begin().await.expect("begin ledger-sum tx");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
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
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
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
async fn agent_wallet_charges_each_model_turn_once() {
    let _admin = migrate_admin().await;
    let app = app_provider().await;
    let region = app.config().region.clone();
    let suffix = uniq();
    let tenant = TenantId(format!("01J0ONCE{suffix}"));
    let run_id = format!("run-keyed-{suffix}");
    let charge_key = format!("{run_id}/model-turn/0");
    let wallet = AgentWallet::new(app.clone());

    wallet
        .credit(&tenant, MicroUsd(1_000), CreditKind::Topup, None)
        .expect("the organization funds its agent wallet");
    assert_eq!(
        wallet.debit_once(&tenant, MicroUsd(500), &run_id, &charge_key),
        Ok(DebitOutcome::Applied(MicroUsd(500))),
        "the first observation of a model turn spends its measured cost",
    );

    let rows_after_first_charge = ledger_row_count(app.db_pool(), &tenant.0, &region).await;
    assert_eq!(
        wallet.debit_once(&tenant, MicroUsd(500), &run_id, &charge_key),
        Ok(DebitOutcome::Replayed(MicroUsd(500))),
        "workflow replay recognizes the same logical charge",
    );
    assert_eq!(
        wallet.balance(&tenant),
        MicroUsd(500),
        "replay spends nothing"
    );
    assert_eq!(
        ledger_row_count(app.db_pool(), &tenant.0, &region).await,
        rows_after_first_charge,
        "one logical model turn leaves exactly one immutable debit row",
    );

    assert_eq!(
        wallet.debit_once(&tenant, MicroUsd(501), &run_id, &charge_key),
        Err(WalletError::ChargeConflict),
        "replay cannot reinterpret the amount behind a charge key",
    );
    assert_eq!(
        wallet.debit_once(
            &tenant,
            MicroUsd(500),
            &format!("different-{run_id}"),
            &charge_key,
        ),
        Err(WalletError::ChargeConflict),
        "replay cannot move a durable charge to a different run",
    );
    assert_eq!(
        wallet.debit_once(&tenant, MicroUsd(1), &run_id, ""),
        Err(WalletError::InvalidChargeKey),
        "an empty operation identity is never silently downgraded to an ordinary debit",
    );
    assert_eq!(wallet.balance(&tenant), MicroUsd(500));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_wallet_durable_contract() {
    let admin = migrate_admin().await;
    let app = app_provider().await;
    let region = app.config().region.clone();
    let suffix = uniq();

    let tenant = TenantId(format!("01J0WALLET{suffix}"));
    let wallet = AgentWallet::new(app.clone());

    assert_eq!(wallet.balance(&tenant), MicroUsd::ZERO, "empty wallet is 0");

    let after_topup = wallet
        .credit(&tenant, MicroUsd(5_000_000), CreditKind::Topup, None)
        .expect("topup credits");
    assert_eq!(after_topup, MicroUsd(5_000_000));
    assert_eq!(wallet.balance(&tenant), MicroUsd(5_000_000));

    let after_debit = wallet
        .debit(&tenant, MicroUsd(1_234), &format!("run-{suffix}"))
        .expect("debit within balance");
    assert_eq!(after_debit, MicroUsd(4_998_766));

    let after_refund = wallet
        .credit(
            &tenant,
            MicroUsd(1_000),
            CreditKind::Refund,
            Some(&format!("run-{suffix}")),
        )
        .expect("refund credits");
    assert_eq!(after_refund, MicroUsd(4_999_766));

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

    let wallet2 = AgentWallet::new(app_provider().await);
    assert_eq!(
        wallet2.balance(&tenant),
        MicroUsd(4_999_766),
        "the balance survived reconstruction from a fresh pool"
    );

    let before = wallet2.balance(&tenant);
    let rows_before = ledger_row_count(app.db_pool(), &tenant.0, &region).await;
    let err = wallet2
        .debit(
            &tenant,
            MicroUsd(999_999_999),
            &format!("run-broke-{suffix}"),
        )
        .expect_err("an over-balance debit is refused");
    assert_eq!(
        err,
        WalletError::InsufficientBalance {
            requested: MicroUsd(999_999_999),
            available: before,
        }
    );
    assert_eq!(
        wallet2.balance(&tenant),
        before,
        "balance unchanged by a refused debit"
    );
    assert_eq!(
        ledger_row_count(app.db_pool(), &tenant.0, &region).await,
        rows_before,
        "a refused debit writes NO ledger row (fail-closed, no partial debit)"
    );

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

    let big = TenantId(format!("01J0BIG{suffix}"));
    assert_eq!(
        wallet2.credit(&big, MicroUsd(i64::MAX as u64 + 1), CreditKind::Topup, None),
        Err(WalletError::AmountTooLarge),
    );
    assert_eq!(ledger_row_count(app.db_pool(), &big.0, &region).await, 0);
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

    let mut tx = admin
        .db_pool()
        .begin()
        .await
        .expect("begin owner mutate tx");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
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
        upd.as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default()
            .contains("immutable"),
        "UPDATE on agent_wallet_ledger must RAISE 'immutable' (owner/trigger): {upd:?}"
    );
    tx.rollback().await.ok();

    let mut tx = admin
        .db_pool()
        .begin()
        .await
        .expect("begin owner delete tx");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
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
        del.as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default()
            .contains("immutable"),
        "DELETE on agent_wallet_ledger must RAISE 'immutable' (owner/trigger): {del:?}"
    );
    tx.rollback().await.ok();

    let mut tx = app.db_pool().begin().await.expect("begin app mutate tx");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
    .bind(&tenant.0)
    .bind(&region)
    .execute(&mut *tx)
    .await
    .expect("scope app mutate tx");
    let app_upd =
        sqlx::query("UPDATE agent_wallet_ledger SET amount_micro = 0 WHERE tenant_id = $1")
            .bind(&tenant.0)
            .execute(&mut *tx)
            .await;
    assert!(
        app_upd.is_err(),
        "the app role must NOT be able to UPDATE agent_wallet_ledger (REVOKE + trigger)"
    );
    tx.rollback().await.ok();
    assert_eq!(
        ledger_sum(app.db_pool(), &tenant.0, &region).await,
        4_999_766,
        "the ledger is unchanged after the refused mutations"
    );

    let other = TenantId(format!("01J0OTHER{suffix}"));
    assert_eq!(
        wallet2.balance(&other),
        MicroUsd::ZERO,
        "a different tenant sees its OWN (empty) wallet, never the funded tenant's"
    );

    let mut tx = app.db_pool().begin().await.expect("begin RLS tx");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
    .bind(&other.0)
    .bind(&region)
    .execute(&mut *tx)
    .await
    .expect("scope RLS tx as `other`");
    let visible: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_wallet_ledger WHERE tenant_id = $1")
            .bind(&tenant.0)
            .fetch_one(&mut *tx)
            .await
            .expect("count cross-tenant rows");
    assert_eq!(
        visible, 0,
        "RLS hides `tenant`'s ledger rows from a session scoped to `other`"
    );
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

    for t in [&tenant.0, &big.0] {
        let _ = sqlx::query("DELETE FROM agent_wallet WHERE tenant_id = $1")
            .bind(t)
            .execute(admin.db_pool())
            .await;
    }
}
