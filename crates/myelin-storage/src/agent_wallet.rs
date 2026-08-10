use myelin_tenancy::TenantId;
use sqlx::Row;

use crate::migration::{Migration, Migrations};
use crate::pg::PgError;
use crate::provider::{ProviderError, SubstrateProvider};

pub use crate::money::MicroUsd;

pub const AGENT_WALLET_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS agent_wallet (
    tenant_id     text        NOT NULL,
    region        text        NOT NULL,
    balance_micro bigint      NOT NULL DEFAULT 0 CHECK (balance_micro >= 0),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, region)
);
CREATE TABLE IF NOT EXISTS agent_wallet_ledger (
    tenant_id    text        NOT NULL,
    region       text        NOT NULL,
    entry_id     uuid        NOT NULL DEFAULT gen_random_uuid(),
    kind         text        NOT NULL CHECK (kind IN ('topup','debit','refund')),
    amount_micro bigint      NOT NULL CHECK (amount_micro >= 0),
    run_id       text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, entry_id)
);
ALTER TABLE agent_wallet ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_wallet FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON agent_wallet;
CREATE POLICY myelin_tenant_isolation ON agent_wallet \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));
ALTER TABLE agent_wallet_ledger ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_wallet_ledger FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON agent_wallet_ledger;
CREATE POLICY myelin_tenant_isolation ON agent_wallet_ledger \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));
REVOKE UPDATE, DELETE ON agent_wallet_ledger FROM myelin_app;
CREATE OR REPLACE FUNCTION myelin_reject_agent_wallet_ledger_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
BEGIN
  RAISE EXCEPTION 'agent_wallet_ledger is immutable';
END
$myelin$;
DROP TRIGGER IF EXISTS agent_wallet_ledger_reject_mutation ON agent_wallet_ledger;
CREATE TRIGGER agent_wallet_ledger_reject_mutation
BEFORE UPDATE OR DELETE ON agent_wallet_ledger
FOR EACH ROW EXECUTE FUNCTION myelin_reject_agent_wallet_ledger_mutation();";

pub fn agent_wallet_migrations() -> Migrations {
    Migrations::of([Migration::plain("0080_agent_wallet", AGENT_WALLET_MIGRATION)])
}

pub const AGENT_WALLET_CHARGE_KEY_MIGRATION: &str = "\
ALTER TABLE agent_wallet_ledger
    ADD COLUMN IF NOT EXISTS charge_key text;
ALTER TABLE agent_wallet_ledger
    DROP CONSTRAINT IF EXISTS agent_wallet_ledger_charge_key_bound;
ALTER TABLE agent_wallet_ledger
    ADD CONSTRAINT agent_wallet_ledger_charge_key_bound
    CHECK (charge_key IS NULL OR length(charge_key) BETWEEN 1 AND 512);
CREATE UNIQUE INDEX IF NOT EXISTS agent_wallet_ledger_charge_once
    ON agent_wallet_ledger (tenant_id, region, charge_key)
    WHERE charge_key IS NOT NULL;";

pub fn agent_wallet_charge_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0095_agent_wallet_charge_key",
        AGENT_WALLET_CHARGE_KEY_MIGRATION,
    )])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreditKind {
    Topup,
    Refund,
}

impl CreditKind {
    fn ledger_kind(self) -> &'static str {
        match self {
            CreditKind::Topup => "topup",
            CreditKind::Refund => "refund",
        }
    }
}

const DEBIT_KIND: &str = "debit";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalletError {
    InsufficientBalance {
        requested: MicroUsd,
        available: MicroUsd,
    },
    AmountTooLarge,
    BalanceOverflow,
    InvalidChargeKey,
    ChargeConflict,
}

impl core::fmt::Display for WalletError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WalletError::InsufficientBalance {
                requested,
                available,
            } => write!(
                f,
                "wallet debit refused: insufficient balance (requested {} micro-USD, {} available) \
                 - no balance, no spend (fail-closed, nothing written)",
                requested.0, available.0
            ),
            WalletError::AmountTooLarge => write!(
                f,
                "wallet op refused: amount (or resulting balance) exceeds the bigint range \
                 (> i64::MAX micro-USD) - refused fail-closed, never a lossy store"
            ),
            WalletError::BalanceOverflow => write!(
                f,
                "wallet credit refused: the running balance sum overflowed u64 (loud, never a silent wrap)"
            ),
            WalletError::InvalidChargeKey => write!(
                f,
                "wallet debit refused: charge key must contain between 1 and 512 bytes"
            ),
            WalletError::ChargeConflict => write!(
                f,
                "wallet charge key was already used for a different run or amount"
            ),
        }
    }
}

impl std::error::Error for WalletError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebitOutcome {
    Applied(MicroUsd),
    Replayed(MicroUsd),
}

impl DebitOutcome {
    pub fn balance(self) -> MicroUsd {
        match self {
            Self::Applied(balance) | Self::Replayed(balance) => balance,
        }
    }
}

#[derive(Clone)]
pub struct AgentWallet {
    provider: SubstrateProvider,
    rt: tokio::runtime::Handle,
}

impl AgentWallet {
    pub fn new(provider: SubstrateProvider) -> AgentWallet {
        Self::with_runtime(provider, tokio::runtime::Handle::current())
    }

    pub fn with_runtime(provider: SubstrateProvider, rt: tokio::runtime::Handle) -> AgentWallet {
        AgentWallet { provider, rt }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    fn block<T>(&self, fut: impl std::future::Future<Output = Result<T, ProviderError>>) -> T {
        tokio::task::block_in_place(|| self.rt.block_on(fut)).unwrap_or_else(|e| {
            panic!("FAIL-STATIC: durable agent wallet store fault (the wallet row did not commit): {e}")
        })
    }

    pub fn credit(
        &self,
        tenant: &TenantId,
        amount: MicroUsd,
        kind: CreditKind,
        run_id: Option<&str>,
    ) -> Result<MicroUsd, WalletError> {
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let run_id = run_id.map(|s| s.to_string());
        self.block(self.provider.with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                credit_on_conn(conn, &tenant_s, &region, amount, kind, run_id.as_deref()).await
            })
        }))
    }

    pub fn debit(
        &self,
        tenant: &TenantId,
        amount: MicroUsd,
        run_id: &str,
    ) -> Result<MicroUsd, WalletError> {
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let run_id = run_id.to_string();
        let outcome = self.block(self.provider.with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(
                async move { debit_on_conn(conn, &tenant_s, &region, amount, &run_id, None).await },
            )
        }))?;
        Ok(outcome.balance())
    }

    pub fn debit_once(
        &self,
        tenant: &TenantId,
        amount: MicroUsd,
        run_id: &str,
        charge_key: &str,
    ) -> Result<DebitOutcome, WalletError> {
        if charge_key.is_empty() || charge_key.len() > 512 {
            return Err(WalletError::InvalidChargeKey);
        }
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let run_id = run_id.to_string();
        let charge_key = charge_key.to_string();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                debit_on_conn(conn, &tenant_s, &region, amount, &run_id, Some(&charge_key)).await
            })
        }))
    }

    pub fn balance(&self, tenant: &TenantId) -> MicroUsd {
        let region = self.region();
        let tenant_s = tenant.0.clone();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move { read_balance(conn, &tenant_s, &region).await })
        }))
        .unwrap_or(MicroUsd::ZERO)
    }
}

async fn read_balance(
    conn: &mut sqlx::PgConnection,
    tenant_s: &str,
    region: &str,
) -> Result<Option<MicroUsd>, PgError> {
    let bal: Option<i64> = sqlx::query_scalar(
        "SELECT balance_micro FROM agent_wallet WHERE tenant_id = $1 AND region = $2",
    )
    .bind(tenant_s)
    .bind(region)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| PgError::Query(e.to_string()))?;
    Ok(bal.map(MicroUsd::from_bigint))
}

async fn credit_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant_s: &str,
    region: &str,
    amount: MicroUsd,
    kind: CreditKind,
    run_id: Option<&str>,
) -> Result<Result<MicroUsd, WalletError>, PgError> {
    if !amount.fits_bigint() {
        return Ok(Err(WalletError::AmountTooLarge));
    }
    ensure_wallet_row(conn, tenant_s, region).await?;
    let current = lock_balance(conn, tenant_s, region).await?;

    let new_balance = match current.checked_add(amount) {
        Some(b) => b,
        None => return Ok(Err(WalletError::BalanceOverflow)),
    };
    let new_bigint = match new_balance.to_bigint() {
        Some(v) => v,
        None => return Ok(Err(WalletError::BalanceOverflow)),
    };
    let amount_bigint = amount.to_bigint().expect("amount fits bigint (checked above)");

    sqlx::query(
        "INSERT INTO agent_wallet_ledger (tenant_id, region, kind, amount_micro, run_id) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(tenant_s)
    .bind(region)
    .bind(kind.ledger_kind())
    .bind(amount_bigint)
    .bind(run_id)
    .execute(&mut *conn)
    .await
    .map_err(|e| PgError::Query(e.to_string()))?;

    write_balance(conn, tenant_s, region, new_bigint).await?;
    Ok(Ok(new_balance))
}

async fn debit_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant_s: &str,
    region: &str,
    amount: MicroUsd,
    run_id: &str,
    charge_key: Option<&str>,
) -> Result<Result<DebitOutcome, WalletError>, PgError> {
    let Some(current) = lock_balance_optional(conn, tenant_s, region).await? else {
        return Ok(Err(WalletError::InsufficientBalance {
            requested: amount,
            available: MicroUsd::ZERO,
        }));
    };
    let amount_bigint = match amount.to_bigint() {
        Some(value) => value,
        None => return Ok(Err(WalletError::AmountTooLarge)),
    };
    if let Some(charge_key) = charge_key {
        let existing = sqlx::query_as::<_, (i64, Option<String>)>(
            "SELECT amount_micro, run_id FROM agent_wallet_ledger \
             WHERE tenant_id = $1 AND region = $2 AND charge_key = $3",
        )
        .bind(tenant_s)
        .bind(region)
        .bind(charge_key)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
        if let Some((existing_amount, existing_run)) = existing {
            return Ok(
                if existing_amount == amount_bigint && existing_run.as_deref() == Some(run_id) {
                    Ok(DebitOutcome::Replayed(current))
                } else {
                    Err(WalletError::ChargeConflict)
                },
            );
        }
    }

    let new_balance = match current.checked_sub(amount) {
        Some(b) => b,
        None => {
            return Ok(Err(WalletError::InsufficientBalance {
                requested: amount,
                available: current,
            }))
        }
    };
    let new_bigint = match new_balance.to_bigint() {
        Some(v) => v,
        None => return Ok(Err(WalletError::AmountTooLarge)),
    };
    sqlx::query(
        "INSERT INTO agent_wallet_ledger \
           (tenant_id, region, kind, amount_micro, run_id, charge_key) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(tenant_s)
    .bind(region)
    .bind(DEBIT_KIND)
    .bind(amount_bigint)
    .bind(run_id)
    .bind(charge_key)
    .execute(&mut *conn)
    .await
    .map_err(|e| PgError::Query(e.to_string()))?;
    write_balance(conn, tenant_s, region, new_bigint).await?;
    Ok(Ok(DebitOutcome::Applied(new_balance)))
}

async fn ensure_wallet_row(
    conn: &mut sqlx::PgConnection,
    tenant_s: &str,
    region: &str,
) -> Result<(), PgError> {
    sqlx::query(
        "INSERT INTO agent_wallet (tenant_id, region, balance_micro) VALUES ($1, $2, 0) \
         ON CONFLICT (tenant_id, region) DO NOTHING",
    )
    .bind(tenant_s)
    .bind(region)
    .execute(&mut *conn)
    .await
    .map_err(|e| PgError::Query(e.to_string()))?;
    Ok(())
}

async fn lock_balance(
    conn: &mut sqlx::PgConnection,
    tenant_s: &str,
    region: &str,
) -> Result<MicroUsd, PgError> {
    lock_balance_optional(conn, tenant_s, region)
        .await?
        .ok_or_else(|| PgError::Query("agent_wallet row vanished under FOR UPDATE".to_string()))
}

async fn lock_balance_optional(
    conn: &mut sqlx::PgConnection,
    tenant_s: &str,
    region: &str,
) -> Result<Option<MicroUsd>, PgError> {
    let row = sqlx::query(
        "SELECT balance_micro FROM agent_wallet WHERE tenant_id = $1 AND region = $2 FOR UPDATE",
    )
    .bind(tenant_s)
    .bind(region)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| PgError::Query(e.to_string()))?;
    match row {
        Some(row) => {
            let bal: i64 = row
                .try_get("balance_micro")
                .map_err(|e| PgError::Query(format!("agent_wallet balance decode failed: {e}")))?;
            Ok(Some(MicroUsd::from_bigint(bal)))
        }
        None => Ok(None),
    }
}

async fn write_balance(
    conn: &mut sqlx::PgConnection,
    tenant_s: &str,
    region: &str,
    balance_bigint: i64,
) -> Result<(), PgError> {
    sqlx::query(
        "UPDATE agent_wallet SET balance_micro = $3, updated_at = now() \
         WHERE tenant_id = $1 AND region = $2",
    )
    .bind(tenant_s)
    .bind(region)
    .bind(balance_bigint)
    .execute(&mut *conn)
    .await
    .map_err(|e| PgError::Query(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_kinds_map_to_ledger_tokens() {
        assert_eq!(CreditKind::Topup.ledger_kind(), "topup");
        assert_eq!(CreditKind::Refund.ledger_kind(), "refund");
        assert_eq!(DEBIT_KIND, "debit");
        assert_ne!(CreditKind::Topup.ledger_kind(), DEBIT_KIND);
        assert_ne!(CreditKind::Refund.ledger_kind(), DEBIT_KIND);
    }

    #[test]
    fn wallet_errors_display_loud_and_specific() {
        let insufficient = WalletError::InsufficientBalance {
            requested: MicroUsd(900),
            available: MicroUsd(100),
        }
        .to_string();
        assert!(
            insufficient.contains("insufficient balance"),
            "must cite the ceiling: {insufficient}"
        );
        assert!(insufficient.contains("nothing written"));
        assert!(!WalletError::AmountTooLarge.to_string().is_empty());
        assert!(WalletError::BalanceOverflow
            .to_string()
            .contains("never a silent wrap"));
    }

    #[test]
    fn migration_ddl_encodes_the_full_contract() {
        let ddl = AGENT_WALLET_MIGRATION;
        assert!(ddl.contains("CREATE TABLE IF NOT EXISTS agent_wallet ("));
        assert!(ddl.contains("CREATE TABLE IF NOT EXISTS agent_wallet_ledger ("));
        assert!(ddl.contains("balance_micro bigint      NOT NULL DEFAULT 0 CHECK (balance_micro >= 0)"));
        assert!(ddl.contains("PRIMARY KEY (tenant_id, region)"));
        assert!(ddl.contains("PRIMARY KEY (tenant_id, entry_id)"));
        assert!(ddl.contains("entry_id     uuid        NOT NULL DEFAULT gen_random_uuid()"));
        assert!(ddl.contains("kind IN ('topup','debit','refund')"));
        assert!(ddl.contains("amount_micro bigint      NOT NULL CHECK (amount_micro >= 0)"));
        assert_eq!(
            ddl.matches("FORCE ROW LEVEL SECURITY").count(),
            2,
            "both tables FORCE RLS"
        );
        assert_eq!(
            ddl.matches("current_setting('myelin.tenant_id', true)").count(),
            4,
            "the (tenant, region) policy is installed on both tables (USING + WITH CHECK each)"
        );
        assert!(ddl.contains("REVOKE UPDATE, DELETE ON agent_wallet_ledger FROM myelin_app"));
        assert!(ddl.contains("BEFORE UPDATE OR DELETE ON agent_wallet_ledger"));
        assert!(ddl.contains("'agent_wallet_ledger is immutable'"));
    }

    #[test]
    fn migration_set_binds_the_expected_id() {
        let ms = agent_wallet_migrations();
        assert_eq!(ms.0.len(), 1);
        assert_eq!(ms.0[0].id, "0080_agent_wallet");
    }

    #[test]
    fn charge_key_migration_makes_one_logical_charge_unique() {
        let ddl = AGENT_WALLET_CHARGE_KEY_MIGRATION;
        assert!(ddl.contains("ADD COLUMN IF NOT EXISTS charge_key text"));
        assert!(ddl.contains("length(charge_key) BETWEEN 1 AND 512"));
        assert!(ddl.contains("CREATE UNIQUE INDEX IF NOT EXISTS agent_wallet_ledger_charge_once"));
        assert!(ddl.contains("(tenant_id, region, charge_key)"));
        assert!(ddl.contains("WHERE charge_key IS NOT NULL"));

        let migrations = agent_wallet_charge_migrations();
        assert_eq!(migrations.0.len(), 1);
        assert_eq!(migrations.0[0].id, "0095_agent_wallet_charge_key");
    }
}
