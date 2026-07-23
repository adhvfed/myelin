//! # Durable PG backing for the reserve/settle cost ledger (MR-009b W6b / P-ST-16, contract 11.7)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/storage.md` §9
//! (*Storage holds the durable ledger*). This module is the REAL durable backing behind
//! [`crate::reserve_settle::CostLedger`]: the in-memory `HashMap`/`Vec` core is now the
//! `test-support`-gated TEST DOUBLE arm, and [`DurableCostLedger`] is the always-compiled production
//! backend over the `cost_reservation` + `cost_event` tables.
//!
//! ## The four invariants preserved on Pg (the mutation floor, proven live)
//! 1. **Never-interrupt-in-flight** — structural: there is no SQL path that tears down an `InFlight`
//!    row; [`Self::cancel_unstarted`] only refunds a `Reserved` row. The interrupt counter is `0` by
//!    construction (there is no column/UPDATE that increments it).
//! 2. **One-cost-event-per-unit** — [`Self::settle`] inserts exactly one `cost_event` row per
//!    [`MeteredUnit`] supplied.
//! 3. **Settle-capped-at-reserved** — the billed total is clamped to the reservation's `reserved`.
//! 4. **Exact-idempotent double-settle** — a settle on an already-`Settled` row re-reads its ordered
//!    `cost_event` rows and accepts only byte-equivalent units, inserting NOTHING. Divergent replay
//!    is refused instead of being mistaken for an acknowledgement-loss retry.
//!
//! ## RLS posture — cost rows ARE tenant-owned billing data (FORCE RLS, `with_tenant_tx`)
//! Unlike the erasure-record ledgers (non-shred-erasable, NO RLS), the cost reservation/event rows are
//! **tenant-scoped billing data** — a tenant's spend must be structurally unreachable from another
//! tenant. So both tables carry the SAME FORCE-RLS `(tenant, region)` policy `pg.rs`/`pseudonym_map`
//! install, and every op runs through the MR-022 [`SubstrateProvider::with_tenant_tx`] convention
//! (transaction-scoped GUCs, no cross-checkout bleed). Every statement also carries the explicit
//! `(tenant_id, region)` predicate (defence in depth behind the policy).
//!
//! ## Sync API over an async store — the write-through bridge
//! [`crate::reserve_settle::CostLedger`]'s API is SYNC (its consumers — the agent-run gate, the drills
//! — are sync). [`DurableCostLedger`] captures the tokio runtime handle at construction and bridges
//! each op onto it (`block_in_place` + `block_on`, the Wave-5 KMS convention). Each op is ONE
//! `with_tenant_tx` transaction (read current state → decide → write), so reserve/settle/cancel are
//! atomic per `(tenant, run)`.
//!
//! Amounts are `u64` minor-units; Postgres `bigint` is `i64`. Values round-trip via a lossless
//! two's-complement reinterpret (`as i64` / `as u64`) and ALL arithmetic (sum/cap/refund) is done in
//! Rust on the `u64` side (checked), so the full `u64` range is exact.

use sqlx::Row;

use myelin_tenancy::TenantId;

use crate::migration::{Migration, Migrations};
use crate::pg::PgError;
use crate::provider::{ProviderError, SubstrateProvider};
use crate::reserve_settle::{
    CostEvent, MeteredUnit, MinorUnits, Reservation, ReservationState, ReserveError, RunId,
    SettleError, SettleOutcome,
};

// =================================================================================================
// Migration 0050 — the tenant-owned (FORCE-RLS) cost_reservation + cost_event tables.
// =================================================================================================

/// The `cost_reservation` + `cost_event` tables (contract 11.7) + their FORCE-RLS `(tenant, region)`
/// policies. **Tenant-owned billing data — RLS-tightest** (a tenant's spend is structurally
/// unreachable cross-tenant). `cost_reservation` is `(tenant, region, run)`-keyed with the reserved
/// amount + lifecycle state; `cost_event` appends one row per metered unit (`ord` orders them). The
/// RLS policy is the SAME shape `pg.rs` installs on `rebac_tuple`. Forward-only (`IF NOT EXISTS` /
/// `DROP POLICY IF EXISTS` before `CREATE POLICY` — idempotent, forward-only-legal).
pub const COST_LEDGER_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS cost_reservation (
    tenant_id text   NOT NULL,
    region    text   NOT NULL,
    run_id    text   NOT NULL,
    reserved  bigint NOT NULL,
    state     text   NOT NULL,
    PRIMARY KEY (tenant_id, region, run_id)
);
CREATE TABLE IF NOT EXISTS cost_event (
    tenant_id text   NOT NULL,
    region    text   NOT NULL,
    run_id    text   NOT NULL,
    ord       integer NOT NULL,
    unit      text   NOT NULL,
    wholesale bigint NOT NULL,
    markup    bigint NOT NULL,
    PRIMARY KEY (tenant_id, region, run_id, ord)
);
ALTER TABLE cost_reservation ENABLE ROW LEVEL SECURITY;
ALTER TABLE cost_reservation FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON cost_reservation;
CREATE POLICY myelin_tenant_isolation ON cost_reservation \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));
ALTER TABLE cost_event ENABLE ROW LEVEL SECURITY;
ALTER TABLE cost_event FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON cost_event;
CREATE POLICY myelin_tenant_isolation ON cost_event \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

/// The forward-only migration set the durable cost ledger binds to (id `0050`, in the free `0050+`
/// range). Applied via the MR-022 [`SubstrateProvider::migrate`] at boot; idempotent on re-boot.
pub fn reserve_settle_durable_migrations() -> Migrations {
    Migrations::of([Migration::plain("0050_cost_ledger", COST_LEDGER_MIGRATION)])
}

// =================================================================================================
// State <-> text.
// =================================================================================================

fn state_token(s: ReservationState) -> &'static str {
    match s {
        ReservationState::Reserved => "reserved",
        ReservationState::InFlight => "inflight",
        ReservationState::Settled => "settled",
        ReservationState::Cancelled => "cancelled",
    }
}

fn parse_state(s: &str) -> Result<ReservationState, PgError> {
    match s {
        "reserved" => Ok(ReservationState::Reserved),
        "inflight" => Ok(ReservationState::InFlight),
        "settled" => Ok(ReservationState::Settled),
        "cancelled" => Ok(ReservationState::Cancelled),
        _ => Err(PgError::Query(
            "cost_reservation row has an invalid state".to_string(),
        )),
    }
}

// =================================================================================================
// DurableCostLedger — the always-compiled production backend over the cost tables (FORCE RLS).
// =================================================================================================

/// The REAL durable reserve/settle cost ledger (production default) over the `cost_reservation` +
/// `cost_event` tables, RLS-scoped through the MR-022 `with_tenant_tx` convention. Cloneable; holds the
/// tokio runtime handle so the SYNC ledger API bridges onto the async store.
#[derive(Clone)]
pub struct DurableCostLedger {
    provider: SubstrateProvider,
    rt: tokio::runtime::Handle,
}

/// A caller-transaction durable-settle failure. Domain refusals remain typed; storage failures are
/// deliberately redacted because billing rows and connection details are not safe error payloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DurableSettleError {
    /// The ledger rejected the requested state transition or exact replay.
    Ledger(SettleError),
    /// PostgreSQL did not complete the settlement statement sequence.
    Store,
}

impl std::fmt::Display for DurableSettleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ledger(error) => write!(f, "{error}"),
            Self::Store => write!(f, "durable cost settlement did not commit"),
        }
    }
}

impl std::error::Error for DurableSettleError {}

impl DurableCostLedger {
    /// Build the durable ledger over the MR-022 provider. **Must be called inside a tokio runtime**
    /// (captures `Handle::current()` for the sync→async bridge).
    pub fn new(provider: SubstrateProvider) -> DurableCostLedger {
        Self::with_runtime(provider, tokio::runtime::Handle::current())
    }

    /// Build the durable ledger with the composition root's explicit runtime handle. Production
    /// runner hooks use this form because their synchronous callbacks execute on a dedicated OS
    /// thread outside the Tokio runtime.
    pub fn with_runtime(
        provider: SubstrateProvider,
        rt: tokio::runtime::Handle,
    ) -> DurableCostLedger {
        DurableCostLedger { provider, rt }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    /// Drive an async op on the runtime handle (the sync→async bridge). A hard DB fault (the store is
    /// down / unreachable) is FAIL-STATIC LOUD — the cost ledger's typed errors are domain refusals,
    /// not infra faults, so an infra fault must never be silently coerced into a settle/reserve outcome.
    fn block<T>(&self, fut: impl std::future::Future<Output = Result<T, ProviderError>>) -> T {
        tokio::task::block_in_place(|| self.rt.block_on(fut)).unwrap_or_else(|e| {
            panic!(
                "FAIL-STATIC: durable cost ledger store fault (the cost row did not commit): {e}"
            )
        })
    }

    /// **Reserve-at-dispatch** (invariant 1: no balance → no run). One tenant-scoped tx: reject a
    /// duplicate `(tenant, run)`; reject an insufficient balance (nothing is written); else INSERT the
    /// `Reserved` row.
    pub fn reserve(
        &self,
        tenant: TenantId,
        run: RunId,
        amount: MinorUnits,
        available: MinorUnits,
    ) -> Result<Reservation, ReserveError> {
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let run_s = run.0.clone();
        let res: Result<Reservation, ReserveError> = self.block(self.provider.with_tenant_tx(
            &tenant.0.clone(),
            move |conn| {
                let tenant = tenant;
                let run = run;
                Box::pin(async move {
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS (SELECT 1 FROM cost_reservation \
                         WHERE tenant_id = $1 AND region = $2 AND run_id = $3)",
                    )
                    .bind(&tenant_s)
                    .bind(&region)
                    .bind(&run_s)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    if exists {
                        return Ok(Err(ReserveError::DuplicateReservation));
                    }
                    if available < amount {
                        return Ok(Err(ReserveError::InsufficientBalance {
                            requested: amount,
                            available,
                        }));
                    }
                    sqlx::query(
                        "INSERT INTO cost_reservation (tenant_id, region, run_id, reserved, state) \
                         VALUES ($1, $2, $3, $4, $5)",
                    )
                    .bind(&tenant_s)
                    .bind(&region)
                    .bind(&run_s)
                    .bind(amount.0 as i64)
                    .bind(state_token(ReservationState::Reserved))
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(Ok(Reservation {
                        tenant,
                        run,
                        reserved: amount,
                        state: ReservationState::Reserved,
                    }))
                })
            },
        ));
        res
    }

    /// **Mark a reserved run in-flight** (idempotent on `Reserved`/`InFlight`; a settled/cancelled run
    /// cannot re-enter flight — the monotonic progression).
    pub fn begin(&self, tenant: &TenantId, run: &RunId) -> Result<(), SettleError> {
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let run_s = run.0.clone();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move { begin_on_conn(conn, &tenant_s, &region, &run_s).await })
        }))
    }

    /// Mark a reserved run in-flight inside a caller-owned tenant-scoped transaction. This is the
    /// launch-side counterpart to [`Self::settle_in_tx`]: a caller can verify its immutable launch
    /// authority and advance the exact reservation under the same row-locking transaction.
    pub async fn begin_in_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        tenant: &TenantId,
        run: &RunId,
    ) -> Result<(), DurableSettleError> {
        begin_on_conn(conn, &tenant.0, &self.region(), &run.0)
            .await
            .map_err(|_| DurableSettleError::Store)?
            .map_err(DurableSettleError::Ledger)
    }

    /// **Settle-on-completion** (invariants 2/3/4). Exact-idempotent: a settle on an already-`Settled`
    /// run re-reads its `cost_event` rows and accepts only the same ordered units. Otherwise it records
    /// one event per unit, caps the billed total at the reservation, and moves the row to `Settled`.
    pub fn settle(
        &self,
        tenant: &TenantId,
        run: &RunId,
        units: &[MeteredUnit],
    ) -> Result<SettleOutcome, SettleError> {
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let run_s = run.0.clone();
        let units: Vec<MeteredUnit> = units.to_vec();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move { settle_on_conn(conn, &tenant_s, &region, &run_s, &units).await })
        }))
    }

    /// Settle using a transaction the caller already scoped to this tenant and region. Money truth,
    /// CI attribution, claim consumption, and workflow signalling can therefore share one commit.
    /// The reservation row is locked, and a retry must reproduce the exact ordered units.
    pub async fn settle_in_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        tenant: &TenantId,
        run: &RunId,
        units: &[MeteredUnit],
    ) -> Result<SettleOutcome, DurableSettleError> {
        settle_on_conn(conn, &tenant.0, &self.region(), &run.0, units)
            .await
            .map_err(|_| DurableSettleError::Store)?
            .map_err(DurableSettleError::Ledger)
    }

    /// Cancel an unstarted reservation inside a caller-owned tenant-scoped transaction. A replay of
    /// the same already-cancelled reservation returns the original refund; in-flight or settled work
    /// is never interrupted. This lets a workflow co-commit skip accounting with the refund state.
    pub async fn cancel_unstarted_in_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        tenant: &TenantId,
        run: &RunId,
    ) -> Result<MinorUnits, DurableSettleError> {
        let tenant_s = tenant.0.as_str();
        let region = self.region();
        let row = sqlx::query(
            "SELECT reserved, state FROM cost_reservation \
             WHERE tenant_id = $1 AND region = $2 AND run_id = $3 FOR UPDATE",
        )
        .bind(tenant_s)
        .bind(&region)
        .bind(&run.0)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|_| DurableSettleError::Store)?
        .ok_or(DurableSettleError::Ledger(SettleError::NoSuchReservation))?;
        let reserved = MinorUnits(
            row.try_get::<i64, _>("reserved")
                .map_err(|_| DurableSettleError::Store)? as u64,
        );
        let state = row
            .try_get::<String, _>("state")
            .map_err(|_| DurableSettleError::Store)?;
        match parse_state(&state).map_err(|_| DurableSettleError::Store)? {
            ReservationState::Reserved => {
                sqlx::query(
                    "UPDATE cost_reservation SET state = $4 \
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
                )
                .bind(tenant_s)
                .bind(&region)
                .bind(&run.0)
                .bind(state_token(ReservationState::Cancelled))
                .execute(&mut *conn)
                .await
                .map_err(|_| DurableSettleError::Store)?;
                Ok(reserved)
            }
            ReservationState::Cancelled => Ok(reserved),
            ReservationState::InFlight | ReservationState::Settled => {
                Err(DurableSettleError::Ledger(SettleError::NoSuchReservation))
            }
        }
    }

    /// **Refund an UNSTARTED run** (the ONLY teardown — never touches an `InFlight`/`Settled` row: the
    /// never-interrupt-in-flight invariant). Refunds the reserved amount.
    pub fn cancel_unstarted(
        &self,
        tenant: &TenantId,
        run: &RunId,
    ) -> Result<MinorUnits, SettleError> {
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let run_s = run.0.clone();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT reserved, state FROM cost_reservation \
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
                )
                .bind(&tenant_s)
                .bind(&region)
                .bind(&run_s)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                let Some(row) = row else {
                    return Ok(Err(SettleError::NoSuchReservation));
                };
                let reserved =
                    MinorUnits(row.try_get::<i64, _>("reserved").map_err(cost_row_decode)? as u64);
                let state = row.try_get::<String, _>("state").map_err(cost_row_decode)?;
                match parse_state(&state)? {
                    ReservationState::Reserved => {
                        sqlx::query(
                            "UPDATE cost_reservation SET state = $4 \
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
                        )
                        .bind(&tenant_s)
                        .bind(&region)
                        .bind(&run_s)
                        .bind(state_token(ReservationState::Cancelled))
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        Ok(Ok(reserved))
                    }
                    // NEVER interrupt in-flight (or teardown settled/cancelled) — refuse, untouched.
                    ReservationState::InFlight
                    | ReservationState::Settled
                    | ReservationState::Cancelled => Ok(Err(SettleError::NoSuchReservation)),
                }
            })
        }))
    }

    /// The current state of a reservation (or `None`).
    pub fn state_of(&self, tenant: &TenantId, run: &RunId) -> Option<ReservationState> {
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let run_s = run.0.clone();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                let state: Option<String> = sqlx::query_scalar(
                    "SELECT state FROM cost_reservation \
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
                )
                .bind(&tenant_s)
                .bind(&region)
                .bind(&run_s)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                state.map(|state| parse_state(&state)).transpose()
            })
        }))
    }

    /// **The in-flight-interrupt counter** — `0` by construction: there is NO SQL path that increments
    /// it (the never-interrupt-in-flight invariant is structural on the durable arm too).
    pub fn inflight_interrupt_count(&self) -> u64 {
        0
    }

    /// Every cost event recorded for a `(tenant, run)` (the durable audit) — owned rows (the durable
    /// arm cannot lend a reference into the DB).
    pub fn cost_events_for(&self, tenant: &TenantId, run: &RunId) -> Vec<CostEvent> {
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let run_s = run.0.clone();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                let rows = sqlx::query(
                    "SELECT unit, wholesale, markup FROM cost_event \
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3 ORDER BY ord",
                )
                .bind(&tenant_s)
                .bind(&region)
                .bind(&run_s)
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                rows_to_events(&tenant_s, &run_s, &rows)
            })
        }))
    }
}

async fn begin_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant_s: &str,
    region: &str,
    run_s: &str,
) -> Result<Result<(), SettleError>, PgError> {
    let state: Option<String> = sqlx::query_scalar(
        "SELECT state FROM cost_reservation \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 FOR UPDATE",
    )
    .bind(tenant_s)
    .bind(region)
    .bind(run_s)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;
    let Some(state) = state else {
        return Ok(Err(SettleError::NoSuchReservation));
    };
    match parse_state(&state)? {
        ReservationState::Reserved | ReservationState::InFlight => {
            sqlx::query(
                "UPDATE cost_reservation SET state = $4 \
                 WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
            )
            .bind(tenant_s)
            .bind(region)
            .bind(run_s)
            .bind(state_token(ReservationState::InFlight))
            .execute(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
            Ok(Ok(()))
        }
        ReservationState::Settled | ReservationState::Cancelled => {
            Ok(Err(SettleError::NoSuchReservation))
        }
    }
}

async fn settle_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant_s: &str,
    region: &str,
    run_s: &str,
    units: &[MeteredUnit],
) -> Result<Result<SettleOutcome, SettleError>, PgError> {
    let row = sqlx::query(
        "SELECT reserved, state FROM cost_reservation \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 FOR UPDATE",
    )
    .bind(tenant_s)
    .bind(region)
    .bind(run_s)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;
    let Some(row) = row else {
        return Ok(Err(SettleError::NoSuchReservation));
    };
    let reserved = MinorUnits(row.try_get::<i64, _>("reserved").map_err(cost_row_decode)? as u64);
    let state = row.try_get::<String, _>("state").map_err(cost_row_decode)?;
    let state = parse_state(&state)?;

    if state == ReservationState::Settled {
        let events = recorded_events(conn, tenant_s, region, run_s).await?;
        if !durable_units_match(&events, units) {
            return Ok(Err(SettleError::UsageDivergence));
        }
        return Ok(outcome_for(events, reserved));
    }
    if state == ReservationState::Cancelled {
        return Ok(Err(SettleError::NoSuchReservation));
    }

    let events = units
        .iter()
        .map(|unit| CostEvent {
            tenant: TenantId(tenant_s.to_string()),
            run: RunId(run_s.to_string()),
            unit: unit.unit.to_string(),
            wholesale: unit.wholesale,
            markup: unit.markup,
        })
        .collect::<Vec<_>>();
    let outcome = match outcome_for(events.clone(), reserved) {
        Ok(outcome) => outcome,
        Err(error) => return Ok(Err(error)),
    };

    for (ord, event) in events.iter().enumerate() {
        let ord = match i32::try_from(ord) {
            Ok(ord) => ord,
            Err(_) => return Ok(Err(SettleError::AmountOverflow)),
        };
        sqlx::query(
            "INSERT INTO cost_event \
               (tenant_id, region, run_id, ord, unit, wholesale, markup) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(tenant_s)
        .bind(region)
        .bind(run_s)
        .bind(ord)
        .bind(event.unit.as_str())
        .bind(event.wholesale.0 as i64)
        .bind(event.markup.0 as i64)
        .execute(&mut *conn)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
    }
    sqlx::query(
        "UPDATE cost_reservation SET state = $4 \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind(tenant_s)
    .bind(region)
    .bind(run_s)
    .bind(state_token(ReservationState::Settled))
    .execute(&mut *conn)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;

    Ok(Ok(outcome))
}

/// Re-read one already-settled run's ordered durable unit log.
async fn recorded_events(
    conn: &mut sqlx::PgConnection,
    tenant_s: &str,
    region: &str,
    run_s: &str,
) -> Result<Vec<CostEvent>, PgError> {
    let rows = sqlx::query(
        "SELECT unit, wholesale, markup FROM cost_event \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 ORDER BY ord",
    )
    .bind(tenant_s)
    .bind(region)
    .bind(run_s)
    .fetch_all(conn)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;
    rows_to_events(tenant_s, run_s, &rows)
}

fn outcome_for(events: Vec<CostEvent>, reserved: MinorUnits) -> Result<SettleOutcome, SettleError> {
    let mut billed = MinorUnits::ZERO;
    for e in &events {
        let t = match e.billed() {
            Some(t) => t,
            None => return Err(SettleError::AmountOverflow),
        };
        billed = match billed.checked_add(t) {
            Some(b) => b,
            None => return Err(SettleError::AmountOverflow),
        };
    }
    let billed_capped = if billed > reserved { reserved } else { billed };
    let refunded = match reserved.checked_sub(billed_capped) {
        Some(r) => r,
        None => return Err(SettleError::AmountOverflow),
    };
    Ok(SettleOutcome {
        cost_events: events,
        billed_total: billed_capped,
        refunded,
    })
}

fn durable_units_match(events: &[CostEvent], units: &[MeteredUnit]) -> bool {
    events.len() == units.len()
        && events.iter().zip(units).all(|(event, unit)| {
            event.unit == unit.unit
                && event.wholesale == unit.wholesale
                && event.markup == unit.markup
        })
}

fn rows_to_events(
    tenant_s: &str,
    run_s: &str,
    rows: &[sqlx::postgres::PgRow],
) -> Result<Vec<CostEvent>, PgError> {
    rows.iter()
        .map(|r| {
            let unit: String = r.try_get("unit").map_err(cost_row_decode)?;
            Ok(CostEvent {
                tenant: TenantId(tenant_s.to_string()),
                run: RunId(run_s.to_string()),
                // `CostEvent.unit` is an OWNED `String` (MR-009b W6b2), so the rebuilt event simply
                // carries the durable label — no `Box::leak` (the pre-W6b2 `&'static str` workaround
                // is gone).
                unit,
                wholesale: MinorUnits(
                    r.try_get::<i64, _>("wholesale").map_err(cost_row_decode)? as u64
                ),
                markup: MinorUnits(r.try_get::<i64, _>("markup").map_err(cost_row_decode)? as u64),
            })
        })
        .collect()
}

fn cost_row_decode(error: sqlx::Error) -> PgError {
    PgError::Query(format!("cost ledger row decode failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::parse_state;

    #[test]
    fn unknown_durable_state_is_a_redacted_error_not_a_panic() {
        let error = parse_state("attacker-controlled-state")
            .expect_err("an unknown durable state must fail closed");
        assert!(error
            .to_string()
            .contains("cost_reservation row has an invalid state"));
        assert!(!error.to_string().contains("attacker-controlled-state"));
    }
}
