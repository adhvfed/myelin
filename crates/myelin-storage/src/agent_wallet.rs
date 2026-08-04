//! # The durable prepaid AGENT WALLET (the hosted-agent prepaid balance)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/storage.md` §9
//! (the reserve/settle cost gate — *the wallet balance is Commercial's; Storage owns the durable
//! ledger correctness*). This module is the **prepaid balance the reserve/settle cost gate consumes**:
//! [`crate::reserve_settle::CostLedger`] takes `available` as a PARAMETER "from the Commercial
//! wallet" — this module is that wallet, co-located with the ledger it will later feed.
//!
//! ## THE UNIT — MICRO-DOLLARS (`1 unit = $0.000001`), a `u64`
//! Every amount in this wallet is an integer count of **micro-dollars** (`MicroUsd`): one unit is
//! one millionth of a US dollar, so **$1.00 = 1_000_000 units** and **1 cent = 10_000 units**. This
//! sub-cent scale is deliberate: a hosted-agent task can cost a small fraction of a cent (a ~2% cut
//! on a sub-cent Luna task is representable at this scale but would round to zero at cent-scale). A
//! float amount is **unrepresentable** — you cannot construct a fractional balance.
//!
//! **This is a DEDICATED agent wallet — deliberately SEPARATE from the shared, cent-scaled
//! [`crate::reserve_settle::MinorUnits`] `cost_event` ledger.** The two never share a table, a unit,
//! or an arithmetic path: `MicroUsd` (micro-dollars, this wallet) and `MinorUnits` (minor-units /
//! cents, the CI+agent cost ledger) are distinct types so a value from one can never be mistaken for
//! the other, and this wallet has ZERO impact on CI billing.
//!
//! ## The invariants (financial correctness is the bar)
//! 1. **`balance == Σ ledger`, always.** [`AgentWallet::balance`] reads the materialized
//!    [`agent_wallet.balance_micro`](AGENT_WALLET_MIGRATION) cache; every mutating op updates that
//!    cache **in the SAME `with_tenant_tx` transaction** as it appends the ledger row, so the cache
//!    is the exact running sum of the append-only ledger (`topup + refund` credit, `debit` debits) —
//!    they commit together or roll back together.
//! 2. **The ledger is append-only + IMMUTABLE.** `agent_wallet_ledger` is the source of truth. It has
//!    `REVOKE UPDATE, DELETE` from the app role AND a `BEFORE UPDATE OR DELETE` trigger that raises
//!    `'agent_wallet_ledger is immutable'` (defence in depth — even the owner cannot rewrite history
//!    without dropping the trigger). Balance is only ever moved by appending a new row.
//! 3. **`(tenant, region)` FORCE-RLS isolation.** Both tables ENABLE + FORCE row-level security with
//!    the `(tenant_id, region)` policy keyed on `current_setting('myelin.tenant_id')` /
//!    `current_setting('myelin.region')` (the same shape migration 0050 installs) — a tenant's balance
//!    is structurally unreachable from another tenant, and every op runs through the MR-022
//!    [`SubstrateProvider::with_tenant_tx`] convention (transaction-scoped GUCs, no cross-checkout bleed).
//! 4. **No double-spend / no partial debit.** [`AgentWallet::debit`] locks the wallet row
//!    (`SELECT … FOR UPDATE`) and refuses ([`WalletError::InsufficientBalance`]) when the balance is
//!    below the requested amount, writing NOTHING (fail-closed) — never a partial debit.
//! 5. **Checked, fail-closed arithmetic.** All sums/differences are `checked_add`/`checked_sub` on the
//!    `u64` side; a Σ overflow is a loud [`WalletError::BalanceOverflow`], never a wrap. An amount (or
//!    a resulting balance) that does not fit Postgres `bigint` (`i64`) is refused
//!    ([`WalletError::AmountTooLarge`]) BEFORE any write — the `u64`↔`i64` round-trip is lossless
//!    within `0..=i64::MAX`.
//!
//! ## Follow-on slices (named, NOT built here — this slice is JUST the wallet)
//! - **Wiring the wallet into the run lifecycle** (feeding [`AgentWallet::available`] into
//!   [`crate::reserve_settle::CostLedger::reserve`] at dispatch and crediting a refund / debiting the
//!   settled cost on completion) is a SEPARATE follow-on slice. Nothing here touches `handle_run`.
//! - **`available` == `balance` for now.** The reservation-integration that makes it
//!   `balance − outstanding_reservations` lands with that wiring slice; today the two are equal.
//! - **PRICING** (turning metered usage into a `MicroUsd` debit amount) is a separate follow-on slice.
//! - **Top-up is an internal/admin seed path for v1** (no Stripe): [`AgentWallet::credit`] with
//!   [`CreditKind::Topup`] is how the balance is funded.

use myelin_tenancy::TenantId;
use sqlx::Row;

use crate::migration::{Migration, Migrations};
use crate::pg::PgError;
use crate::provider::{ProviderError, SubstrateProvider};

// =================================================================================================
// Migration 0080 — the tenant-owned (FORCE-RLS) agent_wallet + IMMUTABLE agent_wallet_ledger tables.
// =================================================================================================

/// The prepaid agent-wallet schema: the materialized `agent_wallet` balance cache + the append-only,
/// IMMUTABLE `agent_wallet_ledger` source of truth, both `(tenant, region)` FORCE-RLS scoped.
///
/// - `agent_wallet` — one row per `(tenant, region)`; `balance_micro` is the materialized Σ of the
///   ledger, co-committed with every append.
/// - `agent_wallet_ledger` — one append-only row per money movement (`topup`/`refund` credit,
///   `debit`), `amount_micro >= 0`, keyed `(tenant_id, entry_id)`. `entry_id` defaults to a
///   DB-generated `gen_random_uuid()` (PG13+ built-in). Immutability is enforced BOTH by
///   `REVOKE UPDATE, DELETE` from the app role AND by a `BEFORE UPDATE OR DELETE` trigger that
///   raises `'agent_wallet_ledger is immutable'` (mirrors `ci_job_accounting`).
///
/// Forward-only + idempotent: `CREATE TABLE IF NOT EXISTS`, `DROP POLICY/TRIGGER IF EXISTS` before
/// the (non-`IF NOT EXISTS`-able) `CREATE POLICY`/`CREATE TRIGGER`, `CREATE OR REPLACE FUNCTION`.
/// The RLS policy is the SAME shape migration 0050 (`cost_reservation`/`cost_event`) installs.
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

/// The forward-only migration set the durable agent wallet binds to (id `0080`, in the free
/// post-`0071` range). Applied via the MR-022 [`SubstrateProvider::migrate`] at boot; idempotent on
/// re-boot. Wired into [`crate::provider::durable_migration_groups`] so a single boot call migrates it.
pub fn agent_wallet_migrations() -> Migrations {
    Migrations::of([Migration::plain("0080_agent_wallet", AGENT_WALLET_MIGRATION)])
}

// =================================================================================================
// MicroUsd — the frozen micro-dollar unit (distinct from the cent-scaled MinorUnits).
// =================================================================================================

/// An integer **micro-dollars** amount — the frozen unit of the prepaid agent wallet
/// (`1 MicroUsd = $0.000001`; `$1.00 = 1_000_000`; `1 cent = 10_000`). A `u64` so the arithmetic is
/// exact and a fractional balance is **unrepresentable**. All wallet arithmetic is checked
/// (`checked_add`/`checked_sub`) — an overflow is a loud typed error, never a silent wrap.
///
/// **Deliberately DISTINCT from [`crate::reserve_settle::MinorUnits`]** (minor-units / cents, the
/// shared cost ledger). They are different Rust types at a different scale, so a micro-dollar wallet
/// amount can NEVER be confused with a cent-scaled billing amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MicroUsd(pub u64);

impl MicroUsd {
    /// Zero micro-dollars (the additive identity — an empty wallet, a zero movement).
    pub const ZERO: MicroUsd = MicroUsd(0);

    /// Checked addition — `None` on `u64` overflow (the loud-not-silent rule; the caller turns it
    /// into a typed [`WalletError::BalanceOverflow`]).
    pub fn checked_add(self, other: MicroUsd) -> Option<MicroUsd> {
        self.0.checked_add(other.0).map(MicroUsd)
    }

    /// Checked subtraction — `None` if it would go negative (a debit can never drive the balance
    /// below zero; the caller turns it into [`WalletError::InsufficientBalance`]).
    pub fn checked_sub(self, other: MicroUsd) -> Option<MicroUsd> {
        self.0.checked_sub(other.0).map(MicroUsd)
    }

    /// Whether this amount fits Postgres `bigint` (`i64`) losslessly (`0..=i64::MAX`). Postgres
    /// `bigint` is signed, so a `u64` above `i64::MAX` cannot round-trip — the wallet refuses it
    /// fail-closed rather than corrupting a balance via two's-complement reinterpretation.
    pub fn fits_bigint(self) -> bool {
        self.0 <= i64::MAX as u64
    }

    /// The `bigint` (`i64`) wire value, or `None` if it does not fit (`> i64::MAX`).
    fn to_bigint(self) -> Option<i64> {
        if self.fits_bigint() {
            Some(self.0 as i64)
        } else {
            None
        }
    }

    /// Rebuild a `MicroUsd` from a `bigint` read back from the DB. The `balance_micro >= 0` /
    /// `amount_micro >= 0` CHECK constraints guarantee the stored value is non-negative, so the
    /// `i64 → u64` widening is lossless.
    fn from_bigint(v: i64) -> MicroUsd {
        MicroUsd(v as u64)
    }
}

/// A CREDIT movement's kind — the only two ways money is ADDED to the wallet. (A `debit` is its own
/// [`AgentWallet::debit`] op; it is never a `credit` kind, so a credit can never subtract.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreditKind {
    /// A prepaid **top-up** — the wallet is seeded/funded. For v1 this is an internal/admin path
    /// (no Stripe); a real payment provider fronts it in a follow-on slice.
    Topup,
    /// A **refund** — money returned to the wallet (e.g. an over-reservation released on settle, once
    /// the run-lifecycle wiring slice lands).
    Refund,
}

impl CreditKind {
    /// The `agent_wallet_ledger.kind` token this credit writes.
    fn ledger_kind(self) -> &'static str {
        match self {
            CreditKind::Topup => "topup",
            CreditKind::Refund => "refund",
        }
    }
}

/// The `agent_wallet_ledger.kind` token a debit writes.
const DEBIT_KIND: &str = "debit";

// =================================================================================================
// WalletError — typed, loud domain refusals (a hard store fault is fail-static, not an error value).
// =================================================================================================

/// A domain refusal from a wallet op. Each is a typed, LOUD value — a wallet op never silently
/// succeeds against an empty balance, an oversized amount, or an overflowing sum. (A hard DB fault
/// is NOT one of these: it is fail-static LOUD — see [`AgentWallet::block`] — because an infra fault
/// must never be coerced into a money outcome.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalletError {
    /// A [`AgentWallet::debit`] asked for more than the wallet holds — **nothing is written**
    /// (fail-closed, no partial debit). This is the prepaid ceiling.
    InsufficientBalance {
        /// The amount the debit asked for.
        requested: MicroUsd,
        /// The balance available (`< requested`).
        available: MicroUsd,
    },
    /// An amount (or the resulting balance) does not fit Postgres `bigint` (`> i64::MAX`) — refused
    /// BEFORE any write (fail-closed), never a lossy two's-complement store.
    AmountTooLarge,
    /// A credit's running-sum `checked_add` overflowed `u64` (or would exceed `i64::MAX`) — loud,
    /// never a silent wrap. Nothing is written.
    BalanceOverflow,
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
                 — no balance, no spend (fail-closed, nothing written)",
                requested.0, available.0
            ),
            WalletError::AmountTooLarge => write!(
                f,
                "wallet op refused: amount (or resulting balance) exceeds the bigint range \
                 (> i64::MAX micro-USD) — refused fail-closed, never a lossy store"
            ),
            WalletError::BalanceOverflow => write!(
                f,
                "wallet credit refused: the running balance sum overflowed u64 (loud, never a silent wrap)"
            ),
        }
    }
}

impl std::error::Error for WalletError {}

// =================================================================================================
// AgentWallet — the durable prepaid balance over the FORCE-RLS agent_wallet(_ledger) tables.
// =================================================================================================

/// **The durable prepaid AGENT WALLET (production default) over the `agent_wallet` +
/// `agent_wallet_ledger` tables, RLS-scoped through the MR-022 `with_tenant_tx` convention.**
///
/// Cloneable; holds the tokio runtime handle so a SYNC op surface bridges onto the async store (the
/// same sync→async bridge [`crate::reserve_settle_durable::DurableCostLedger`] uses — the wallet's
/// eventual consumer, the sync [`crate::reserve_settle::CostLedger::reserve`], takes `available` as a
/// plain value, so a sync `available()`/`balance()` composes with it directly).
///
/// Every mutating op is exactly ONE `with_tenant_tx` transaction (read current balance under a row
/// lock → decide → append the ledger row + update the balance cache), so a credit/debit is atomic
/// per `(tenant, region)` and `balance == Σ ledger` holds after any sequence.
#[derive(Clone)]
pub struct AgentWallet {
    provider: SubstrateProvider,
    rt: tokio::runtime::Handle,
}

impl AgentWallet {
    /// Build the wallet over the MR-022 provider. **Must be called inside a tokio runtime** (captures
    /// `Handle::current()` for the sync→async bridge).
    pub fn new(provider: SubstrateProvider) -> AgentWallet {
        Self::with_runtime(provider, tokio::runtime::Handle::current())
    }

    /// Build the wallet with an explicit runtime handle (for a composition root whose synchronous
    /// callbacks run on a dedicated OS thread outside the Tokio runtime).
    pub fn with_runtime(provider: SubstrateProvider, rt: tokio::runtime::Handle) -> AgentWallet {
        AgentWallet { provider, rt }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    /// Drive an async op on the runtime handle (the sync→async bridge). A hard DB fault (the store is
    /// down / unreachable) is FAIL-STATIC LOUD — the wallet's typed errors are domain refusals, not
    /// infra faults, so an infra fault must never be silently coerced into a credit/debit outcome.
    fn block<T>(&self, fut: impl std::future::Future<Output = Result<T, ProviderError>>) -> T {
        tokio::task::block_in_place(|| self.rt.block_on(fut)).unwrap_or_else(|e| {
            panic!("FAIL-STATIC: durable agent wallet store fault (the wallet row did not commit): {e}")
        })
    }

    /// **Credit the wallet** (a [`CreditKind::Topup`] seed or a [`CreditKind::Refund`]). One
    /// tenant-scoped tx: ensure the wallet row exists + lock it, `checked_add` the amount in Rust,
    /// append the immutable ledger row, and update the balance cache — co-committed. Returns the NEW
    /// balance. Refuses (writing nothing) an amount that does not fit `bigint`
    /// ([`WalletError::AmountTooLarge`]) or a sum that overflows ([`WalletError::BalanceOverflow`]).
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

    /// **Debit the wallet** (spend against the prepaid balance). One tenant-scoped tx: lock the wallet
    /// row; if `balance < amount` return [`WalletError::InsufficientBalance`] and write NOTHING
    /// (fail-closed, no partial debit); else append the immutable `debit` ledger row and update the
    /// balance cache — co-committed. Returns the NEW balance.
    pub fn debit(
        &self,
        tenant: &TenantId,
        amount: MicroUsd,
        run_id: &str,
    ) -> Result<MicroUsd, WalletError> {
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let run_id = run_id.to_string();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                debit_on_conn(conn, &tenant_s, &region, amount, &run_id).await
            })
        }))
    }

    /// The current wallet **balance** — the materialized `agent_wallet.balance_micro` (== Σ ledger),
    /// or [`MicroUsd::ZERO`] if the wallet has never been funded (no row yet).
    pub fn balance(&self, tenant: &TenantId) -> MicroUsd {
        let region = self.region();
        let tenant_s = tenant.0.clone();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move { read_balance(conn, &tenant_s, &region).await })
        }))
        .unwrap_or(MicroUsd::ZERO)
    }

    /// The **available** balance the reserve/settle gate consumes at dispatch. For THIS slice it
    /// equals [`Self::balance`]; the reservation-integration that makes it
    /// `balance − outstanding_reservations` is the follow-on run-lifecycle wiring slice.
    pub fn available(&self, tenant: &TenantId) -> MicroUsd {
        self.balance(tenant)
    }
}

/// Read the materialized balance for `(tenant, region)` — `None` if the wallet row does not exist.
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
    // Fail-closed BEFORE any write: an amount that cannot round-trip through bigint is refused.
    if !amount.fits_bigint() {
        return Ok(Err(WalletError::AmountTooLarge));
    }
    // Ensure the wallet row exists (balance 0), then lock it so concurrent movements on the same
    // (tenant, region) serialize — the row lock is what makes the read→add→write sequence atomic.
    ensure_wallet_row(conn, tenant_s, region).await?;
    let current = lock_balance(conn, tenant_s, region).await?;

    let new_balance = match current.checked_add(amount) {
        Some(b) => b,
        None => return Ok(Err(WalletError::BalanceOverflow)),
    };
    // The materialized balance is a bigint; a sum above i64::MAX cannot be stored — refuse (nothing
    // committed; the whole tx rolls back).
    let new_bigint = match new_balance.to_bigint() {
        Some(v) => v,
        None => return Ok(Err(WalletError::BalanceOverflow)),
    };
    let amount_bigint = amount.to_bigint().expect("amount fits bigint (checked above)");

    // Append the IMMUTABLE ledger row (entry_id is a DB-generated gen_random_uuid()).
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

    // Update the materialized balance cache IN THE SAME TX (balance == Σ ledger, co-committed).
    write_balance(conn, tenant_s, region, new_bigint).await?;
    Ok(Ok(new_balance))
}

async fn debit_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant_s: &str,
    region: &str,
    amount: MicroUsd,
    run_id: &str,
) -> Result<Result<MicroUsd, WalletError>, PgError> {
    // Lock the wallet row for update. If there is no row the balance is 0 — any positive debit is
    // insufficient and NOTHING is written (fail-closed). We do NOT create a row on a refused debit.
    let Some(current) = lock_balance_optional(conn, tenant_s, region).await? else {
        return Ok(Err(WalletError::InsufficientBalance {
            requested: amount,
            available: MicroUsd::ZERO,
        }));
    };

    // No double-spend / no partial debit: refuse if the balance cannot cover the amount.
    let new_balance = match current.checked_sub(amount) {
        Some(b) => b,
        None => {
            return Ok(Err(WalletError::InsufficientBalance {
                requested: amount,
                available: current,
            }))
        }
    };
    // new_balance <= current, and current came from a stored bigint, so this always fits; the checked
    // form keeps the invariant explicit.
    let new_bigint = match new_balance.to_bigint() {
        Some(v) => v,
        None => return Ok(Err(WalletError::AmountTooLarge)),
    };
    let amount_bigint = match amount.to_bigint() {
        Some(v) => v,
        None => return Ok(Err(WalletError::AmountTooLarge)),
    };

    // Append the IMMUTABLE debit ledger row + update the cache, co-committed in this one tx.
    sqlx::query(
        "INSERT INTO agent_wallet_ledger (tenant_id, region, kind, amount_micro, run_id) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(tenant_s)
    .bind(region)
    .bind(DEBIT_KIND)
    .bind(amount_bigint)
    .bind(run_id)
    .execute(&mut *conn)
    .await
    .map_err(|e| PgError::Query(e.to_string()))?;
    write_balance(conn, tenant_s, region, new_bigint).await?;
    Ok(Ok(new_balance))
}

/// Ensure the `(tenant, region)` wallet row exists (balance 0) so it can be row-locked. Idempotent
/// (`ON CONFLICT DO NOTHING`) — a concurrent creator does not error.
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

/// Lock the wallet row and read its balance. The row is assumed to exist (the caller ran
/// [`ensure_wallet_row`] first). `FOR UPDATE` serializes concurrent movements on this key.
async fn lock_balance(
    conn: &mut sqlx::PgConnection,
    tenant_s: &str,
    region: &str,
) -> Result<MicroUsd, PgError> {
    lock_balance_optional(conn, tenant_s, region)
        .await?
        .ok_or_else(|| PgError::Query("agent_wallet row vanished under FOR UPDATE".to_string()))
}

/// Lock the wallet row `FOR UPDATE` and read its balance, or `None` if the row does not exist.
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

/// Write the materialized balance cache for `(tenant, region)`. The row exists (the mutating ops all
/// run [`ensure_wallet_row`]/`FOR UPDATE` first), so this is a plain `UPDATE`.
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

    // ---- MicroUsd checked arithmetic + the bigint-fit boundary (DB-free) --------------------------

    /// The micro-dollar unit is checked: an overflow is a loud `None`, never a silent wrap.
    #[test]
    fn micro_usd_arithmetic_is_checked() {
        assert_eq!(MicroUsd(u64::MAX).checked_add(MicroUsd(1)), None);
        assert_eq!(MicroUsd(5).checked_sub(MicroUsd(10)), None);
        assert_eq!(MicroUsd(10).checked_sub(MicroUsd(10)), Some(MicroUsd::ZERO));
        assert_eq!(
            MicroUsd(1_000_000).checked_add(MicroUsd(10_000)),
            Some(MicroUsd(1_010_000)),
            "$1.00 + 1 cent = 1_010_000 micro-USD"
        );
    }

    /// The `bigint` (`i64`) fit boundary: `i64::MAX` fits, `i64::MAX + 1` does not (fail-closed).
    #[test]
    fn bigint_fit_boundary_is_exact() {
        let max = MicroUsd(i64::MAX as u64);
        assert!(max.fits_bigint(), "i64::MAX micro-USD fits bigint");
        assert_eq!(max.to_bigint(), Some(i64::MAX));

        let over = MicroUsd(i64::MAX as u64 + 1);
        assert!(!over.fits_bigint(), "i64::MAX + 1 does NOT fit bigint");
        assert_eq!(over.to_bigint(), None);

        // A value read back from a stored non-negative bigint round-trips losslessly.
        assert_eq!(MicroUsd::from_bigint(i64::MAX), MicroUsd(i64::MAX as u64));
        assert_eq!(MicroUsd::from_bigint(0), MicroUsd::ZERO);
    }

    /// Credit kinds map to the exact ledger tokens the CHECK constraint admits — and a debit's token
    /// is distinct from both (a credit can never be a debit).
    #[test]
    fn credit_kinds_map_to_ledger_tokens() {
        assert_eq!(CreditKind::Topup.ledger_kind(), "topup");
        assert_eq!(CreditKind::Refund.ledger_kind(), "refund");
        assert_eq!(DEBIT_KIND, "debit");
        assert_ne!(CreditKind::Topup.ledger_kind(), DEBIT_KIND);
        assert_ne!(CreditKind::Refund.ledger_kind(), DEBIT_KIND);
    }

    /// The wallet errors Display loud + specific (observability is part of the pass) — never empty.
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

    // ---- The migration DDL is well-formed (structural, DB-free) ------------------------------------

    /// The migration installs BOTH tables, FORCE-RLS on each, the immutability REVOKE + trigger, and
    /// the CHECK constraints — the financial-correctness contract, asserted structurally so a silent
    /// weakening of the DDL is caught without a live DB.
    #[test]
    fn migration_ddl_encodes_the_full_contract() {
        let ddl = AGENT_WALLET_MIGRATION;
        // Both tables.
        assert!(ddl.contains("CREATE TABLE IF NOT EXISTS agent_wallet ("));
        assert!(ddl.contains("CREATE TABLE IF NOT EXISTS agent_wallet_ledger ("));
        // Materialized balance cache + append-only ledger keys.
        assert!(ddl.contains("balance_micro bigint      NOT NULL DEFAULT 0 CHECK (balance_micro >= 0)"));
        assert!(ddl.contains("PRIMARY KEY (tenant_id, region)"));
        assert!(ddl.contains("PRIMARY KEY (tenant_id, entry_id)"));
        assert!(ddl.contains("entry_id     uuid        NOT NULL DEFAULT gen_random_uuid()"));
        // The kind + non-negative-amount CHECKs.
        assert!(ddl.contains("kind IN ('topup','debit','refund')"));
        assert!(ddl.contains("amount_micro bigint      NOT NULL CHECK (amount_micro >= 0)"));
        // FORCE RLS on BOTH tables, with the (tenant, region) policy keyed on the GUCs.
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
        // Immutability: REVOKE from the app role AND the raising trigger (defence in depth).
        assert!(ddl.contains("REVOKE UPDATE, DELETE ON agent_wallet_ledger FROM myelin_app"));
        assert!(ddl.contains("BEFORE UPDATE OR DELETE ON agent_wallet_ledger"));
        assert!(ddl.contains("'agent_wallet_ledger is immutable'"));
    }

    /// The migration set binds id `0080_agent_wallet` (the free post-`0071` range) to the DDL.
    #[test]
    fn migration_set_binds_the_expected_id() {
        let ms = agent_wallet_migrations();
        assert_eq!(ms.0.len(), 1);
        assert_eq!(ms.0[0].id, "0080_agent_wallet");
    }
}
