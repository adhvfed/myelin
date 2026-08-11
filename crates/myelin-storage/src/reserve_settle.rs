use myelin_tenancy::TenantId;
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;

pub use crate::money::MicroUsd;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerUnavailable {
    detail: String,
}

impl LedgerUnavailable {
    pub(crate) fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    pub fn diagnostic(&self) -> &str {
        &self.detail
    }
}

impl core::fmt::Display for LedgerUnavailable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("the durable cost ledger is unavailable")
    }
}

impl std::error::Error for LedgerUnavailable {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RunId(pub String);

impl RunId {
    pub fn new(token: impl Into<String>) -> RunId {
        RunId(token.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReservationState {
    Reserved,
    InFlight,
    Settled,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reservation {
    pub tenant: TenantId,
    pub run: RunId,
    pub reserved: MicroUsd,
    pub state: ReservationState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CostEvent {
    pub tenant: TenantId,
    pub run: RunId,
    pub unit: String,
    pub wholesale: MicroUsd,
    pub markup: MicroUsd,
}

impl CostEvent {
    pub fn billed(&self) -> Option<MicroUsd> {
        self.wholesale.checked_add(self.markup)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeteredUnit {
    pub unit: &'static str,
    pub wholesale: MicroUsd,
    pub markup: MicroUsd,
}

impl MeteredUnit {
    pub fn total(&self) -> Option<MicroUsd> {
        self.wholesale.checked_add(self.markup)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReserveError {
    InsufficientBalance {
        requested: MicroUsd,
        available: MicroUsd,
    },
    DuplicateReservation,
    AmountOverflow,
    StoreUnavailable(LedgerUnavailable),
}

impl core::fmt::Display for ReserveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ReserveError::InsufficientBalance {
                requested,
                available,
            } => write!(
                f,
                "reserve refused: insufficient balance (requested {} micro-USD, {} available) \
                 - no balance, no run (storage §9)",
                requested.0, available.0
            ),
            ReserveError::DuplicateReservation => write!(
                f,
                "reserve refused: a reservation already exists for this (tenant, run) \
                 - a dispatch is reserved exactly once"
            ),
            ReserveError::AmountOverflow => write!(
                f,
                "reserve refused: integer micro-USD arithmetic overflowed u64 (loud, never a silent wrap)"
            ),
            ReserveError::StoreUnavailable(error) => write!(
                f,
                "reserve refused: {error}; the run was not started"
            ),
        }
    }
}

impl std::error::Error for ReserveError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettleError {
    NoSuchReservation,
    UsageDivergence,
    AmountOverflow,
    StoreUnavailable(LedgerUnavailable),
}

impl core::fmt::Display for SettleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SettleError::NoSuchReservation => write!(
                f,
                "settle refused: no reservation exists for this (tenant, run) - \
                 a settle never invents a charge"
            ),
            SettleError::UsageDivergence => write!(
                f,
                "settle refused: metered units diverge from the already-recorded settlement"
            ),
            SettleError::AmountOverflow => write!(
                f,
                "settle refused: integer micro-USD arithmetic overflowed u64"
            ),
            SettleError::StoreUnavailable(error) => {
                write!(f, "settle refused: {error}; no charge was invented")
            }
        }
    }
}

impl std::error::Error for SettleError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettleOutcome {
    pub cost_events: Vec<CostEvent>,
    pub billed_total: MicroUsd,
    pub refunded: MicroUsd,
}

pub struct CostLedger {
    backend: CostBackend,
}

enum CostBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(MemoryCostLedger),
    Durable(Box<crate::reserve_settle_durable::DurableCostLedger>),
}

#[cfg(any(test, feature = "test-support"))]
impl Default for CostLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl CostLedger {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> CostLedger {
        CostLedger {
            backend: CostBackend::Memory(MemoryCostLedger::default()),
        }
    }

    pub fn with_pg(provider: crate::provider::SubstrateProvider) -> CostLedger {
        CostLedger {
            backend: CostBackend::Durable(Box::new(
                crate::reserve_settle_durable::DurableCostLedger::new(provider),
            )),
        }
    }

    pub fn reserve(
        &mut self,
        tenant: TenantId,
        run: RunId,
        amount: MicroUsd,
        available: MicroUsd,
    ) -> Result<Reservation, ReserveError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.reserve(tenant, run, amount, available),
            CostBackend::Durable(d) => d.reserve(tenant, run, amount, available),
        }
    }

    pub fn begin(&mut self, tenant: &TenantId, run: &RunId) -> Result<(), SettleError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.begin(tenant, run),
            CostBackend::Durable(d) => d.begin(tenant, run),
        }
    }

    pub fn settle(
        &mut self,
        tenant: &TenantId,
        run: &RunId,
        units: &[MeteredUnit],
    ) -> Result<SettleOutcome, SettleError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.settle(tenant, run, units),
            CostBackend::Durable(d) => d.settle(tenant, run, units),
        }
    }

    pub fn cancel_unstarted(
        &mut self,
        tenant: &TenantId,
        run: &RunId,
    ) -> Result<MicroUsd, SettleError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.cancel_unstarted(tenant, run),
            CostBackend::Durable(d) => d.cancel_unstarted(tenant, run),
        }
    }

    pub fn state_of(
        &self,
        tenant: &TenantId,
        run: &RunId,
    ) -> Result<Option<ReservationState>, LedgerUnavailable> {
        self.reservation_of(tenant, run)
            .map(|reservation| reservation.map(|reservation| reservation.state))
    }

    pub fn reservation_of(
        &self,
        tenant: &TenantId,
        run: &RunId,
    ) -> Result<Option<Reservation>, LedgerUnavailable> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => Ok(m.reservation_of(tenant, run)),
            CostBackend::Durable(d) => d.reservation_of(tenant, run),
        }
    }

    pub fn cost_events_for(
        &self,
        tenant: &TenantId,
        run: &RunId,
    ) -> Result<Vec<CostEvent>, LedgerUnavailable> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => Ok(m.cost_events_for(tenant, run)),
            CostBackend::Durable(d) => d.cost_events_for(tenant, run),
        }
    }

    pub fn inflight_interrupt_count(&self) -> u64 {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.inflight_interrupt_count(),
            CostBackend::Durable(d) => d.inflight_interrupt_count(),
        }
    }

    pub fn outstanding_reservations(&self, tenant: &TenantId) -> Result<MicroUsd, ReserveError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.outstanding_reservations(tenant),
            CostBackend::Durable(d) => d.outstanding_reservations(tenant),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Default)]
pub struct MemoryCostLedger {
    reservations: HashMap<(TenantId, RunId), Reservation>,
    cost_events: Vec<CostEvent>,
    inflight_interrupt_count: u64,
}

#[cfg(any(test, feature = "test-support"))]
impl MemoryCostLedger {
    pub fn new() -> MemoryCostLedger {
        MemoryCostLedger::default()
    }

    pub fn reserve(
        &mut self,
        tenant: TenantId,
        run: RunId,
        amount: MicroUsd,
        available: MicroUsd,
    ) -> Result<Reservation, ReserveError> {
        if self
            .reservations
            .contains_key(&(tenant.clone(), run.clone()))
        {
            return Err(ReserveError::DuplicateReservation);
        }
        if available < amount {
            return Err(ReserveError::InsufficientBalance {
                requested: amount,
                available,
            });
        }
        let reservation = Reservation {
            tenant: tenant.clone(),
            run: run.clone(),
            reserved: amount,
            state: ReservationState::Reserved,
        };
        self.reservations.insert((tenant, run), reservation.clone());
        Ok(reservation)
    }

    pub fn begin(&mut self, tenant: &TenantId, run: &RunId) -> Result<(), SettleError> {
        let key = (tenant.clone(), run.clone());
        let reservation = self
            .reservations
            .get_mut(&key)
            .ok_or(SettleError::NoSuchReservation)?;
        match reservation.state {
            ReservationState::Reserved | ReservationState::InFlight => {
                reservation.state = ReservationState::InFlight;
                Ok(())
            }
            ReservationState::Settled | ReservationState::Cancelled => {
                Err(SettleError::NoSuchReservation)
            }
        }
    }

    pub fn settle(
        &mut self,
        tenant: &TenantId,
        run: &RunId,
        units: &[MeteredUnit],
    ) -> Result<SettleOutcome, SettleError> {
        let key = (tenant.clone(), run.clone());
        let reservation = self
            .reservations
            .get(&key)
            .ok_or(SettleError::NoSuchReservation)?
            .clone();

        if reservation.state == ReservationState::Settled {
            let outcome = self.recorded_outcome(tenant, run, reservation.reserved)?;
            if !metered_units_match(&outcome.cost_events, units) {
                return Err(SettleError::UsageDivergence);
            }
            return Ok(outcome);
        }
        if reservation.state == ReservationState::Cancelled {
            return Err(SettleError::NoSuchReservation);
        }

        let mut events = Vec::with_capacity(units.len());
        let mut billed = MicroUsd::ZERO;
        for u in units {
            let event = CostEvent {
                tenant: tenant.clone(),
                run: run.clone(),
                unit: u.unit.to_string(),
                wholesale: u.wholesale,
                markup: u.markup,
            };
            let unit_total = event.billed().ok_or(SettleError::AmountOverflow)?;
            billed = billed
                .checked_add(unit_total)
                .ok_or(SettleError::AmountOverflow)?;
            events.push(event);
        }

        let billed_capped = if billed > reservation.reserved {
            reservation.reserved
        } else {
            billed
        };
        let refunded = reservation
            .reserved
            .checked_sub(billed_capped)
            .ok_or(SettleError::AmountOverflow)?;

        self.cost_events.extend(events.iter().cloned());
        if let Some(r) = self.reservations.get_mut(&key) {
            r.state = ReservationState::Settled;
        }

        Ok(SettleOutcome {
            cost_events: events,
            billed_total: billed_capped,
            refunded,
        })
    }

    fn recorded_outcome(
        &self,
        tenant: &TenantId,
        run: &RunId,
        reserved: MicroUsd,
    ) -> Result<SettleOutcome, SettleError> {
        let events: Vec<CostEvent> = self
            .cost_events
            .iter()
            .filter(|e| &e.tenant == tenant && &e.run == run)
            .cloned()
            .collect();
        let mut billed = MicroUsd::ZERO;
        for e in &events {
            let t = e.billed().ok_or(SettleError::AmountOverflow)?;
            billed = billed.checked_add(t).ok_or(SettleError::AmountOverflow)?;
        }
        let billed_capped = if billed > reserved { reserved } else { billed };
        let refunded = reserved
            .checked_sub(billed_capped)
            .ok_or(SettleError::AmountOverflow)?;
        Ok(SettleOutcome {
            cost_events: events,
            billed_total: billed_capped,
            refunded,
        })
    }

    pub fn cancel_unstarted(
        &mut self,
        tenant: &TenantId,
        run: &RunId,
    ) -> Result<MicroUsd, SettleError> {
        let key = (tenant.clone(), run.clone());
        let reservation = self
            .reservations
            .get_mut(&key)
            .ok_or(SettleError::NoSuchReservation)?;
        match reservation.state {
            ReservationState::Reserved => {
                let refund = reservation.reserved;
                reservation.state = ReservationState::Cancelled;
                Ok(refund)
            }
            ReservationState::InFlight
            | ReservationState::Settled
            | ReservationState::Cancelled => Err(SettleError::NoSuchReservation),
        }
    }

    pub fn state_of(&self, tenant: &TenantId, run: &RunId) -> Option<ReservationState> {
        self.reservation_of(tenant, run)
            .map(|reservation| reservation.state)
    }

    pub fn reservation_of(&self, tenant: &TenantId, run: &RunId) -> Option<Reservation> {
        self.reservations
            .get(&(tenant.clone(), run.clone()))
            .cloned()
    }

    pub fn cost_events_for(&self, tenant: &TenantId, run: &RunId) -> Vec<CostEvent> {
        self.cost_events
            .iter()
            .filter(|e| &e.tenant == tenant && &e.run == run)
            .cloned()
            .collect()
    }

    pub fn inflight_interrupt_count(&self) -> u64 {
        self.inflight_interrupt_count
    }

    pub fn outstanding_reservations(&self, tenant: &TenantId) -> Result<MicroUsd, ReserveError> {
        let mut total = MicroUsd::ZERO;
        for reservation in self.reservations.values() {
            if &reservation.tenant == tenant
                && matches!(
                    reservation.state,
                    ReservationState::Reserved | ReservationState::InFlight
                )
            {
                total = total
                    .checked_add(reservation.reserved)
                    .ok_or(ReserveError::AmountOverflow)?;
            }
        }
        Ok(total)
    }
}

#[cfg(any(test, feature = "test-support"))]
fn metered_units_match(events: &[CostEvent], units: &[MeteredUnit]) -> bool {
    events.len() == units.len()
        && events.iter().zip(units).all(|(event, unit)| {
            event.unit == unit.unit
                && event.wholesale == unit.wholesale
                && event.markup == unit.markup
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveSettleSignal {
    pub tenant: TenantId,
    pub metered_units: u64,
    pub cost_events: u64,
    pub inflight_interrupt_count: u64,
    pub wholesale_total: MicroUsd,
    pub markup_total: MicroUsd,
}

impl ReserveSettleSignal {
    pub fn is_green(&self) -> bool {
        self.cost_events == self.metered_units && self.inflight_interrupt_count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId::from_token("01J0ACME")
    }

    fn run() -> RunId {
        RunId::new("01J0RUN_AGENT")
    }

    #[test]
    fn reserve_admits_on_balance_and_refuses_on_no_balance() {
        let mut ledger = CostLedger::new();
        let res = ledger
            .reserve(tenant(), run(), MicroUsd(500), MicroUsd(1_000))
            .expect("a funded reserve admits");
        assert_eq!(res.state, ReservationState::Reserved);
        assert_eq!(res.reserved, MicroUsd(500));

        let run2 = RunId::new("01J0RUN_BROKE");
        let err = ledger
            .reserve(tenant(), run2.clone(), MicroUsd(900), MicroUsd(100))
            .expect_err("an unfunded reserve is refused");
        assert_eq!(
            err,
            ReserveError::InsufficientBalance {
                requested: MicroUsd(900),
                available: MicroUsd(100),
            }
        );
        assert!(
            ledger.state_of(&tenant(), &run2).unwrap().is_none(),
            "a refused reserve writes NO row - the run never dispatched"
        );
    }

    #[test]
    fn exact_balance_is_affordable() {
        let mut ledger = CostLedger::new();
        let res = ledger.reserve(tenant(), run(), MicroUsd(100), MicroUsd(100));
        assert!(res.is_ok(), "available == amount must be affordable");
    }

    #[test]
    fn duplicate_reserve_is_rejected() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(100), MicroUsd(1_000))
            .unwrap();
        let err = ledger
            .reserve(tenant(), run(), MicroUsd(100), MicroUsd(1_000))
            .expect_err("a second reserve for the same run is rejected");
        assert_eq!(err, ReserveError::DuplicateReservation);
    }

    #[test]
    fn settle_records_one_cost_event_per_metered_unit_with_split() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(1_000), MicroUsd(5_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();

        let units = vec![
            MeteredUnit {
                unit: "llm.tokens",
                wholesale: MicroUsd(120),
                markup: MicroUsd(30),
            },
            MeteredUnit {
                unit: "ci.minute",
                wholesale: MicroUsd(200),
                markup: MicroUsd(50),
            },
        ];
        let outcome = ledger.settle(&tenant(), &run(), &units).unwrap();

        assert_eq!(outcome.cost_events.len(), 2);
        assert_eq!(ledger.cost_events_for(&tenant(), &run()).unwrap().len(), 2);

        let e0 = &outcome.cost_events[0];
        assert_eq!(e0.wholesale, MicroUsd(120));
        assert_eq!(e0.markup, MicroUsd(30));
        assert_eq!(e0.billed(), Some(MicroUsd(150)));
        assert_ne!(e0.wholesale, e0.markup, "wholesale and markup are distinct");

        assert_eq!(outcome.billed_total, MicroUsd(400));
        assert_eq!(outcome.refunded, MicroUsd(600));

        assert_eq!(
            ledger.state_of(&tenant(), &run()),
            Ok(Some(ReservationState::Settled))
        );
    }

    #[test]
    fn settle_is_capped_at_the_reservation() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(100), MicroUsd(1_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units = vec![MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(500),
            markup: MicroUsd(500),
        }];
        let outcome = ledger.settle(&tenant(), &run(), &units).unwrap();
        assert_eq!(
            outcome.billed_total,
            MicroUsd(100),
            "billed is capped at the reserved amount"
        );
        assert_eq!(outcome.refunded, MicroUsd::ZERO);
    }

    #[test]
    fn double_settle_does_not_double_charge() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(1_000), MicroUsd(5_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units = vec![MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(120),
            markup: MicroUsd(30),
        }];
        let first = ledger.settle(&tenant(), &run(), &units).unwrap();
        let second = ledger.settle(&tenant(), &run(), &units).unwrap();
        assert_eq!(first, second, "a re-settle returns the SAME outcome");
        assert_eq!(
            ledger.cost_events_for(&tenant(), &run()).unwrap().len(),
            1,
            "no further cost events on re-settle - no double-charge"
        );
        let divergent = ledger.settle(
            &tenant(),
            &run(),
            &[MeteredUnit {
                unit: "llm.tokens",
                wholesale: MicroUsd(121),
                markup: MicroUsd(30),
            }],
        );
        assert_eq!(
            divergent,
            Err(SettleError::UsageDivergence),
            "ack-loss replay cannot change the recorded units"
        );
        assert_eq!(ledger.cost_events_for(&tenant(), &run()).unwrap().len(), 1);
    }

    #[test]
    fn cancel_never_interrupts_an_in_flight_run() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(500), MicroUsd(1_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let err = ledger
            .cancel_unstarted(&tenant(), &run())
            .expect_err("an in-flight run is NEVER cancelled");
        assert_eq!(err, SettleError::NoSuchReservation);
        assert_eq!(
            ledger.state_of(&tenant(), &run()),
            Ok(Some(ReservationState::InFlight))
        );
        assert_eq!(
            ledger.inflight_interrupt_count(),
            0,
            "no in-flight reservation was ever interrupted"
        );
    }

    #[test]
    fn cancel_refunds_an_unstarted_run() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(500), MicroUsd(1_000))
            .unwrap();
        let refund = ledger.cancel_unstarted(&tenant(), &run()).unwrap();
        assert_eq!(refund, MicroUsd(500), "the full reservation is refunded");
        assert_eq!(
            ledger.state_of(&tenant(), &run()),
            Ok(Some(ReservationState::Cancelled))
        );
    }

    #[test]
    fn settle_without_reservation_is_refused() {
        let mut ledger = CostLedger::new();
        let err = ledger
            .settle(&tenant(), &run(), &[])
            .expect_err("settle never invents a charge");
        assert_eq!(err, SettleError::NoSuchReservation);
    }

    #[test]
    fn settled_run_cannot_reenter_flight() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(100), MicroUsd(1_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        ledger.settle(&tenant(), &run(), &[]).unwrap();
        let err = ledger
            .begin(&tenant(), &run())
            .expect_err("a settled run cannot re-enter flight");
        assert_eq!(err, SettleError::NoSuchReservation);
    }

    #[test]
    fn minor_units_arithmetic_is_checked() {
        assert_eq!(MicroUsd(u64::MAX).checked_add(MicroUsd(1)), None);
        assert_eq!(MicroUsd(5).checked_sub(MicroUsd(10)), None);
        assert_eq!(
            MicroUsd(u64::MAX).checked_add(MicroUsd(0)),
            Some(MicroUsd(u64::MAX))
        );
    }

    #[test]
    fn settle_overflow_is_loud() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(u64::MAX), MicroUsd(u64::MAX))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units = vec![MeteredUnit {
            unit: "boom",
            wholesale: MicroUsd(u64::MAX),
            markup: MicroUsd(1),
        }];
        let err = ledger.settle(&tenant(), &run(), &units).unwrap_err();
        assert_eq!(err, SettleError::AmountOverflow);
    }

    #[test]
    fn errors_display_loud_and_specific() {
        let r = ReserveError::InsufficientBalance {
            requested: MicroUsd(900),
            available: MicroUsd(100),
        }
        .to_string();
        assert!(r.contains("no balance, no run"), "must cite the floor: {r}");
        assert!(!r.is_empty());
        let s = SettleError::NoSuchReservation.to_string();
        assert!(
            s.contains("never invents a charge"),
            "must be specific: {s}"
        );

        let outage = LedgerUnavailable::new("pool closed at postgres://secret-host/myelin");
        assert_eq!(outage.to_string(), "the durable cost ledger is unavailable");
        assert_eq!(
            outage.diagnostic(),
            "pool closed at postgres://secret-host/myelin"
        );
        assert!(
            !ReserveError::StoreUnavailable(outage)
                .to_string()
                .contains("secret-host"),
            "a user-facing refusal does not disclose provider details"
        );
    }

    #[test]
    fn synthetic_run_emits_a_green_drill_artifact() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(1_000), MicroUsd(5_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units = vec![
            MeteredUnit {
                unit: "llm.tokens",
                wholesale: MicroUsd(120),
                markup: MicroUsd(30),
            },
            MeteredUnit {
                unit: "ci.minute",
                wholesale: MicroUsd(200),
                markup: MicroUsd(50),
            },
        ];
        let outcome = ledger.settle(&tenant(), &run(), &units).unwrap();

        let wholesale_total = MicroUsd(120 + 200);
        let markup_total = MicroUsd(30 + 50);
        let signal = ReserveSettleSignal {
            tenant: tenant(),
            metered_units: units.len() as u64,
            cost_events: outcome.cost_events.len() as u64,
            inflight_interrupt_count: ledger.inflight_interrupt_count(),
            wholesale_total,
            markup_total,
        };

        assert!(
            signal.is_green(),
            "the synthetic-run drill must be GREEN: {signal:?}"
        );
        assert_eq!(signal.cost_events, 2);
        assert_eq!(signal.metered_units, 2);
        assert_eq!(signal.inflight_interrupt_count, 0);
        assert_ne!(signal.wholesale_total, signal.markup_total);
        assert_eq!(signal.wholesale_total, MicroUsd(320));
        assert_eq!(signal.markup_total, MicroUsd(80));
    }

    #[test]
    fn metered_unit_total_sums_wholesale_and_markup() {
        let u = MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(120),
            markup: MicroUsd(30),
        };
        assert_eq!(u.total(), Some(MicroUsd(150)));
        let overflow = MeteredUnit {
            unit: "boom",
            wholesale: MicroUsd(u64::MAX),
            markup: MicroUsd(1),
        };
        assert_eq!(
            overflow.total(),
            None,
            "an overflowing unit total is a loud None"
        );
    }

    #[test]
    fn settle_billed_equal_to_reserved_is_not_clamped() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(150), MicroUsd(1_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units = vec![MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(120),
            markup: MicroUsd(30),
        }];
        let outcome = ledger.settle(&tenant(), &run(), &units).unwrap();
        assert_eq!(outcome.billed_total, MicroUsd(150));
        assert_eq!(outcome.refunded, MicroUsd::ZERO);
    }

    #[test]
    fn re_settle_reconstructs_exact_amounts_isolated_per_run() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(1_000), MicroUsd(9_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units_a = vec![MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(120),
            markup: MicroUsd(30),
        }];
        ledger.settle(&tenant(), &run(), &units_a).unwrap();

        let run_b = RunId::new("01J0RUN_B");
        ledger
            .reserve(tenant(), run_b.clone(), MicroUsd(1_000), MicroUsd(9_000))
            .unwrap();
        ledger.begin(&tenant(), &run_b).unwrap();
        let units_b = vec![MeteredUnit {
            unit: "ci.minute",
            wholesale: MicroUsd(500),
            markup: MicroUsd(0),
        }];
        ledger.settle(&tenant(), &run_b, &units_b).unwrap();

        let again = ledger.settle(&tenant(), &run(), &units_a).unwrap();
        assert_eq!(again.cost_events.len(), 1, "only run A's one event");
        assert_eq!(again.billed_total, MicroUsd(150));
        assert_eq!(again.refunded, MicroUsd(850));
        assert_eq!(again.cost_events[0].unit, "llm.tokens");
    }

    #[test]
    fn re_settle_clamps_an_over_run_to_the_reservation() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(100), MicroUsd(9_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units = vec![MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(1_000),
            markup: MicroUsd(0),
        }];
        let first = ledger.settle(&tenant(), &run(), &units).unwrap();
        assert_eq!(
            first.billed_total,
            MicroUsd(100),
            "first settle clamps to reserved"
        );

        let again = ledger.settle(&tenant(), &run(), &units).unwrap();
        assert_eq!(again.billed_total, MicroUsd(100));
        assert_eq!(again.refunded, MicroUsd::ZERO);
    }

    #[test]
    fn re_settle_billed_equal_to_reserved_uses_unclamped_value() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(150), MicroUsd(9_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units = vec![MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(120),
            markup: MicroUsd(30),
        }];
        ledger.settle(&tenant(), &run(), &units).unwrap();
        let again = ledger.settle(&tenant(), &run(), &units).unwrap();
        assert_eq!(again.billed_total, MicroUsd(150));
        assert_eq!(again.refunded, MicroUsd::ZERO);
    }

    #[test]
    fn cost_events_for_isolates_by_tenant_and_run() {
        let mut ledger = CostLedger::new();
        let other_tenant = TenantId::from_token("01J0OTHER");
        let run_b = RunId::new("01J0RUN_B");

        for (t, r, w) in [
            (tenant(), run(), 100u64),
            (tenant(), run_b.clone(), 200),
            (other_tenant.clone(), run(), 300),
        ] {
            ledger
                .reserve(t.clone(), r.clone(), MicroUsd(1_000), MicroUsd(9_000))
                .unwrap();
            ledger.begin(&t, &r).unwrap();
            ledger
                .settle(
                    &t,
                    &r,
                    &[MeteredUnit {
                        unit: "u",
                        wholesale: MicroUsd(w),
                        markup: MicroUsd(0),
                    }],
                )
                .unwrap();
        }

        let events = ledger.cost_events_for(&tenant(), &run()).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].wholesale, MicroUsd(100));
        assert_eq!(ledger.cost_events_for(&tenant(), &run_b).unwrap().len(), 1);
        assert_eq!(
            ledger.cost_events_for(&tenant(), &run_b).unwrap()[0].wholesale,
            MicroUsd(200)
        );
        assert_eq!(
            ledger.cost_events_for(&other_tenant, &run()).unwrap()[0].wholesale,
            MicroUsd(300)
        );
    }

    #[test]
    fn outstanding_sums_unsettled_and_excludes_settled_and_cancelled() {
        let mut ledger = CostLedger::new();
        let big = MicroUsd(1_000_000);

        let r_reserved = RunId::new("01J0R_RESERVED");
        ledger
            .reserve(tenant(), r_reserved, MicroUsd(100), big)
            .unwrap();

        let r_inflight = RunId::new("01J0R_INFLIGHT");
        ledger
            .reserve(tenant(), r_inflight.clone(), MicroUsd(200), big)
            .unwrap();
        ledger.begin(&tenant(), &r_inflight).unwrap();

        let r_settled = RunId::new("01J0R_SETTLED");
        ledger
            .reserve(tenant(), r_settled.clone(), MicroUsd(400), big)
            .unwrap();
        ledger.begin(&tenant(), &r_settled).unwrap();
        ledger.settle(&tenant(), &r_settled, &[]).unwrap();

        let r_cancelled = RunId::new("01J0R_CANCELLED");
        ledger
            .reserve(tenant(), r_cancelled.clone(), MicroUsd(800), big)
            .unwrap();
        ledger.cancel_unstarted(&tenant(), &r_cancelled).unwrap();

        assert_eq!(
            ledger.outstanding_reservations(&tenant()),
            Ok(MicroUsd(300))
        );
    }

    #[test]
    fn outstanding_is_tenant_isolated() {
        let mut ledger = CostLedger::new();
        let other = TenantId::from_token("01J0OTHER");
        ledger
            .reserve(tenant(), run(), MicroUsd(100), MicroUsd(1_000))
            .unwrap();
        ledger
            .reserve(
                other.clone(),
                RunId::new("01J0R_OTHER"),
                MicroUsd(500),
                MicroUsd(1_000),
            )
            .unwrap();
        assert_eq!(
            ledger.outstanding_reservations(&tenant()),
            Ok(MicroUsd(100)),
            "only THIS tenant's outstanding is summed"
        );
        assert_eq!(ledger.outstanding_reservations(&other), Ok(MicroUsd(500)));
        assert_eq!(
            ledger.outstanding_reservations(&TenantId::from_token("01J0EMPTY")),
            Ok(MicroUsd::ZERO)
        );
    }

    #[test]
    fn second_reserve_is_bounded_by_the_first_runs_outstanding() {
        let mut ledger = CostLedger::new();
        let balance = MicroUsd(1_000);

        let run1 = RunId::new("01J0R_ONE");
        ledger
            .reserve(tenant(), run1, MicroUsd(700), balance)
            .unwrap();

        let outstanding = ledger.outstanding_reservations(&tenant()).unwrap();
        let available = MicroUsd(balance.0.saturating_sub(outstanding.0));
        assert_eq!(available, MicroUsd(300));

        let run2 = RunId::new("01J0R_TWO");
        let err = ledger
            .reserve(tenant(), run2.clone(), MicroUsd(400), available)
            .expect_err("the second run over-reserves past the balance");
        assert_eq!(
            err,
            ReserveError::InsufficientBalance {
                requested: MicroUsd(400),
                available: MicroUsd(300),
            }
        );
        assert!(
            ledger.state_of(&tenant(), &run2).unwrap().is_none(),
            "the refused second run wrote no reservation"
        );

        let run3 = RunId::new("01J0R_THREE");
        ledger
            .reserve(tenant(), run3, MicroUsd(300), available)
            .expect("a run within the remaining balance is admitted");
        assert_eq!(
            ledger.outstanding_reservations(&tenant()),
            Ok(MicroUsd(1_000))
        );
    }

    #[test]
    fn a_red_signal_is_not_green() {
        let red_interrupt = ReserveSettleSignal {
            tenant: tenant(),
            metered_units: 2,
            cost_events: 2,
            inflight_interrupt_count: 1,
            wholesale_total: MicroUsd(320),
            markup_total: MicroUsd(80),
        };
        assert!(!red_interrupt.is_green(), "an interrupt must read RED");

        let red_mismatch = ReserveSettleSignal {
            tenant: tenant(),
            metered_units: 2,
            cost_events: 1,
            inflight_interrupt_count: 0,
            wholesale_total: MicroUsd(320),
            markup_total: MicroUsd(80),
        };
        assert!(
            !red_mismatch.is_green(),
            "a cost-event mismatch must read RED"
        );
    }
}
