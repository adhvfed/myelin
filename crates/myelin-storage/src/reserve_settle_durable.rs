use sqlx::Row;

use myelin_tenancy::TenantId;

use crate::migration::{Migration, Migrations};
use crate::pg::PgError;
use crate::provider::{ProviderError, SubstrateProvider};
use crate::reserve_settle::{
    CostEvent, MeteredUnit, MicroUsd, Reservation, ReservationState, ReserveError, RunId,
    SettleError, SettleOutcome,
};

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

pub fn reserve_settle_durable_migrations() -> Migrations {
    Migrations::of([Migration::plain("0050_cost_ledger", COST_LEDGER_MIGRATION)])
}

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

#[derive(Clone)]
pub struct DurableCostLedger {
    provider: SubstrateProvider,
    rt: tokio::runtime::Handle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DurableSettleError {
    Ledger(SettleError),
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
    pub fn new(provider: SubstrateProvider) -> DurableCostLedger {
        Self::with_runtime(provider, tokio::runtime::Handle::current())
    }

    pub fn with_runtime(
        provider: SubstrateProvider,
        rt: tokio::runtime::Handle,
    ) -> DurableCostLedger {
        DurableCostLedger { provider, rt }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    fn block<T>(&self, fut: impl std::future::Future<Output = Result<T, ProviderError>>) -> T {
        tokio::task::block_in_place(|| self.rt.block_on(fut)).unwrap_or_else(|e| {
            panic!(
                "FAIL-STATIC: durable cost ledger store fault (the cost row did not commit): {e}"
            )
        })
    }

    pub fn reserve(
        &self,
        tenant: TenantId,
        run: RunId,
        amount: MicroUsd,
        available: MicroUsd,
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

    pub fn begin(&self, tenant: &TenantId, run: &RunId) -> Result<(), SettleError> {
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let run_s = run.0.clone();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move { begin_on_conn(conn, &tenant_s, &region, &run_s).await })
        }))
    }

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

    pub async fn cancel_unstarted_in_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        tenant: &TenantId,
        run: &RunId,
    ) -> Result<MicroUsd, DurableSettleError> {
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
        let reserved = MicroUsd(
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

    pub fn cancel_unstarted(
        &self,
        tenant: &TenantId,
        run: &RunId,
    ) -> Result<MicroUsd, SettleError> {
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
                    MicroUsd(row.try_get::<i64, _>("reserved").map_err(cost_row_decode)? as u64);
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
                    ReservationState::InFlight
                    | ReservationState::Settled
                    | ReservationState::Cancelled => Ok(Err(SettleError::NoSuchReservation)),
                }
            })
        }))
    }

    pub fn state_of(&self, tenant: &TenantId, run: &RunId) -> Option<ReservationState> {
        self.reservation_of(tenant, run).map(|reservation| reservation.state)
    }

    pub fn reservation_of(&self, tenant: &TenantId, run: &RunId) -> Option<Reservation> {
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
                    return Ok(None);
                };
                let reserved = row.try_get::<i64, _>("reserved").map_err(cost_row_decode)?;
                let state = row.try_get::<String, _>("state").map_err(cost_row_decode)?;
                Ok(Some(Reservation {
                    tenant: TenantId(tenant_s),
                    run: RunId(run_s),
                    reserved: MicroUsd(reserved as u64),
                    state: parse_state(&state)?,
                }))
            })
        }))
    }

    pub fn inflight_interrupt_count(&self) -> u64 {
        0
    }

    pub fn outstanding_reservations(&self, tenant: &TenantId) -> Result<MicroUsd, ReserveError> {
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let sum: i64 = self.block(self.provider.with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                sqlx::query_scalar(
                    "SELECT COALESCE(SUM(reserved), 0)::bigint FROM cost_reservation \
                     WHERE tenant_id = $1 AND region = $2 \
                       AND state IN ('reserved', 'inflight')",
                )
                .bind(&tenant_s)
                .bind(&region)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))
            })
        }));
        Ok(MicroUsd(sum as u64))
    }

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
    let reserved = MicroUsd(row.try_get::<i64, _>("reserved").map_err(cost_row_decode)? as u64);
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

fn outcome_for(events: Vec<CostEvent>, reserved: MicroUsd) -> Result<SettleOutcome, SettleError> {
    let mut billed = MicroUsd::ZERO;
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
                unit,
                wholesale: MicroUsd(
                    r.try_get::<i64, _>("wholesale").map_err(cost_row_decode)? as u64
                ),
                markup: MicroUsd(r.try_get::<i64, _>("markup").map_err(cost_row_decode)? as u64),
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
