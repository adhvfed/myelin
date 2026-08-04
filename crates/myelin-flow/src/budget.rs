use crate::engine::FlowTelemetry;
use crate::wfctx::WfCtx;
use myelin_storage::reserve_settle::{
    CostLedger, MeteredUnit, MicroUsd, ReserveError, RunId as LedgerRunId, SettleError,
    SettleOutcome,
};
use myelin_tenancy::TenantId;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wallet {
    available: MicroUsd,
}

impl Wallet {
    pub fn new(balance: MicroUsd) -> Wallet {
        Wallet { available: balance }
    }

    pub fn from_budget(budget: &crate::RunBudget) -> Wallet {
        let units = u64::try_from(budget.minor_units).unwrap_or(0);
        Wallet::new(MicroUsd(units))
    }

    pub fn balance(&self) -> MicroUsd {
        self.available
    }
}

pub type BudgetSettle = SettleOutcome;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BudgetError {
    Refused {
        requested: MicroUsd,
        available: MicroUsd,
    },
    DuplicateReservation,
    NoSuchReservation,
    UsageDivergence,
    AmountOverflow,
}

impl core::fmt::Display for BudgetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BudgetError::Refused {
                requested,
                available,
            } => write!(
                f,
                "reserve refused: wallet exhausted (requested {} minor-units, {} available) - \
                 no balance, no dispatch (durable-workflow §4.9); the dispatch never started",
                requested.0, available.0
            ),
            BudgetError::DuplicateReservation => {
                write!(f, "reserve refused: this (tenant, run) is already reserved")
            }
            BudgetError::NoSuchReservation => write!(
                f,
                "settle/begin refused: no reservation for this (tenant, run) - never invent a charge"
            ),
            BudgetError::UsageDivergence => write!(
                f,
                "settle refused: metered units diverge from the recorded settlement"
            ),
            BudgetError::AmountOverflow => {
                write!(f, "budget arithmetic overflowed u64 (loud, never a silent wrap)")
            }
        }
    }
}

impl std::error::Error for BudgetError {}

impl From<ReserveError> for BudgetError {
    fn from(e: ReserveError) -> Self {
        match e {
            ReserveError::InsufficientBalance {
                requested,
                available,
            } => BudgetError::Refused {
                requested,
                available,
            },
            ReserveError::DuplicateReservation => BudgetError::DuplicateReservation,
            ReserveError::AmountOverflow => BudgetError::AmountOverflow,
        }
    }
}

impl From<SettleError> for BudgetError {
    fn from(e: SettleError) -> Self {
        match e {
            SettleError::NoSuchReservation => BudgetError::NoSuchReservation,
            SettleError::UsageDivergence => BudgetError::UsageDivergence,
            SettleError::AmountOverflow => BudgetError::AmountOverflow,
        }
    }
}

#[derive(Clone)]
pub struct BudgetGate {
    inner: Arc<Mutex<GateInner>>,
    telemetry: Option<FlowTelemetry>,
}

struct GateInner {
    wallet: Wallet,
    ledger: CostLedger,
}

impl BudgetGate {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(wallet: Wallet) -> BudgetGate {
        BudgetGate::new_durable(wallet, CostLedger::new())
    }

    pub fn new_durable(wallet: Wallet, ledger: CostLedger) -> BudgetGate {
        BudgetGate {
            inner: Arc::new(Mutex::new(GateInner { wallet, ledger })),
            telemetry: None,
        }
    }

    pub fn with_pg(wallet: Wallet, provider: myelin_storage::SubstrateProvider) -> BudgetGate {
        BudgetGate::new_durable(wallet, CostLedger::with_pg(provider))
    }

    pub fn with_telemetry(mut self, telemetry: FlowTelemetry) -> BudgetGate {
        self.telemetry = Some(telemetry);
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn balance(&self) -> MicroUsd {
        self.lock().wallet.balance()
    }

    pub fn reserve(
        &self,
        tenant: &TenantId,
        run: &LedgerRunId,
        amount: MicroUsd,
    ) -> Result<(), BudgetError> {
        if let Some(t) = &self.telemetry {
            t.record_reserve_attempt();
        }
        let mut g = self.lock();
        let available = g.wallet.available;
        match g
            .ledger
            .reserve(tenant.clone(), run.clone(), amount, available)
        {
            Ok(_reservation) => {
                g.wallet.available = available
                    .checked_sub(amount)
                    .ok_or(BudgetError::AmountOverflow)?;
                Ok(())
            }
            Err(e) => {
                let err = BudgetError::from(e);
                if let (BudgetError::Refused { .. }, Some(t)) = (&err, &self.telemetry) {
                    t.record_reserve_reject();
                }
                Err(err)
            }
        }
    }

    pub fn begin(&self, tenant: &TenantId, run: &LedgerRunId) -> Result<(), BudgetError> {
        self.lock()
            .ledger
            .begin(tenant, run)
            .map_err(BudgetError::from)
    }

    pub fn settle(
        &self,
        tenant: &TenantId,
        run: &LedgerRunId,
        units: &[MeteredUnit],
    ) -> Result<BudgetSettle, BudgetError> {
        let mut g = self.lock();
        let already_settled = g.ledger.state_of(tenant, run)
            == Some(myelin_storage::reserve_settle::ReservationState::Settled);
        let outcome = g
            .ledger
            .settle(tenant, run, units)
            .map_err(BudgetError::from)?;
        if !already_settled {
            g.wallet.available = g
                .wallet
                .available
                .checked_add(outcome.refunded)
                .ok_or(BudgetError::AmountOverflow)?;
        }
        drop(g);
        if !already_settled {
            if let Some(t) = &self.telemetry {
                t.record_settle();
            }
        }
        Ok(outcome)
    }

    pub fn state_of(
        &self,
        tenant: &TenantId,
        run: &LedgerRunId,
    ) -> Option<myelin_storage::reserve_settle::ReservationState> {
        self.lock().ledger.state_of(tenant, run)
    }

    pub fn inflight_interrupt_count(&self) -> u64 {
        self.lock().ledger.inflight_interrupt_count()
    }
}

pub(crate) struct DispatchNoun {
    label: &'static str,
    tail: &'static str,
}

impl DispatchNoun {
    pub(crate) const ACTIVITY: DispatchNoun = DispatchNoun {
        label: "metered_activity",
        tail: "the activity never started",
    };
    pub(crate) const LONG_PARK: DispatchNoun = DispatchNoun {
        label: "schedule_and_run_job",
        tail: "the job was never dispatched",
    };
}

pub(crate) struct ReserveAdmit {
    pub(crate) ledger_run: LedgerRunId,
}

impl WfCtx {
    pub fn with_budget(mut self, gate: BudgetGate) -> Self {
        self.budget = Some(gate);
        self
    }

    pub(crate) fn budget(&self) -> Option<&BudgetGate> {
        self.budget.as_ref()
    }

    pub(crate) fn dispatch_ledger_run(&self, command_id: &str) -> LedgerRunId {
        LedgerRunId::new(format!("{}/{}", self.run_id(), command_id))
    }

    pub(crate) fn reserve_and_begin(
        &self,
        gate: &BudgetGate,
        cost: MicroUsd,
        noun: DispatchNoun,
    ) -> crate::WfResult<ReserveAdmit> {
        let ledger_run = self.dispatch_ledger_run(&self.peek_next_command_id());
        let tenant = self.tenant_id().clone();
        let fresh = match gate.reserve(&tenant, &ledger_run, cost) {
            Ok(()) => true,
            Err(BudgetError::DuplicateReservation) => false,
            Err(BudgetError::Refused {
                requested,
                available,
            }) => {
                return Err(crate::WfError::CoCommit(format!(
                    "{} refused at reserve: wallet exhausted (requested {} minor-units, {} available) \
                     - {} (§4.9)",
                    noun.label, requested.0, available.0, noun.tail
                )));
            }
            Err(other) => {
                return Err(crate::WfError::CoCommit(format!(
                    "{} reserve failed: {other}",
                    noun.label
                )));
            }
        };
        if fresh {
            gate.begin(&tenant, &ledger_run).map_err(|e| {
                crate::WfError::CoCommit(format!("{} begin failed: {e}", noun.label))
            })?;
        }
        Ok(ReserveAdmit { ledger_run })
    }

    pub fn metered_activity<F>(
        &mut self,
        policy: crate::RetryPolicy,
        cost: MicroUsd,
        units: Vec<MeteredUnit>,
        f: F,
    ) -> crate::WfResult<Vec<myelin_refs::ArtifactRef>>
    where
        F: Fn(&str, u32) -> Result<Vec<myelin_refs::ArtifactRef>, crate::ActivityError>,
    {
        let Some(gate) = self.budget().cloned() else {
            return self.activity(policy, f);
        };

        let admit = self.reserve_and_begin(&gate, cost, DispatchNoun::ACTIVITY)?;

        let outcome = self.activity(policy, f);

        match outcome {
            Ok(result) => {
                gate.settle(self.tenant_id(), &admit.ledger_run, &units)
                    .map_err(|e| {
                        crate::WfError::CoCommit(format!("metered_activity settle failed: {e}"))
                    })?;
                Ok(result)
            }
            Err(activity_err) => {
                let _ = gate.settle(self.tenant_id(), &admit.ledger_run, &[]);
                Err(activity_err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{SignalRow, SignalStore};
    use crate::job::{job_idem_token, JobKind, JobOutcome, JobRunner, JobSpec, JOB_DONE_SIGNAL};
    use crate::{RetryPolicy, WfJournal};
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_refs::ArtifactRef;
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }
    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }
    fn begin_ctx(outbox: &OutboxStore, journal: WfJournal, gate: BudgetGate) -> WfCtx {
        WfCtx::begin(
            outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_budget(gate)
    }

    fn unit(unit: &'static str, wholesale: u64, markup: u64) -> MeteredUnit {
        MeteredUnit {
            unit,
            wholesale: MicroUsd(wholesale),
            markup: MicroUsd(markup),
        }
    }

    #[derive(Default)]
    struct RecordingRunner {
        calls: AtomicUsize,
        dispatched: Mutex<Vec<JobSpec>>,
    }
    impl JobRunner for RecordingRunner {
        fn dispatch(&self, spec: &JobSpec) -> Result<(), crate::ActivityError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.dispatched.lock().unwrap().push(spec.clone());
            Ok(())
        }
    }

    fn deliver_job_done(signals: &SignalStore, idem_token: &str, result: Vec<ArtifactRef>) {
        signals.deliver(SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            signal_name: JOB_DONE_SIGNAL.into(),
            idem_key: idem_token.into(),
            payload: result,
            payload_key_ref: None,
            received_unix_ms: 0,
            consumed_seq: None,
        });
    }

    #[test]
    fn reserve_against_empty_wallet_refuses_the_dispatch_the_activity_never_runs() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let gate = BudgetGate::new(Wallet::new(MicroUsd::ZERO));
        let ran = Arc::new(AtomicUsize::new(0));
        let ran_c = ran.clone();

        let mut ctx = begin_ctx(&outbox, journal, gate.clone());
        let err = ctx
            .metered_activity(
                RetryPolicy::default_policy(),
                MicroUsd(100),
                vec![unit("llm.tokens", 80, 20)],
                move |_idem, _att| {
                    ran_c.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![ArtifactRef("x://y".into())])
                },
            )
            .expect_err("an empty wallet refuses the dispatch");
        assert!(
            matches!(err, crate::WfError::CoCommit(ref m) if m.contains("wallet exhausted")),
            "the refusal is a loud error, got {err:?}"
        );
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "the activity closure NEVER ran (no dispatch)"
        );
        let lr = LedgerRunId::new("R1/merge.queue:0");
        assert!(
            gate.state_of(&tenant(), &lr).is_none(),
            "a refused reserve writes no row"
        );
    }

    #[test]
    fn funded_metered_activity_reserves_runs_and_settles_into_the_same_wallet() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let gate = BudgetGate::new(Wallet::new(MicroUsd(1_000)));

        let mut ctx = begin_ctx(&outbox, journal, gate.clone());
        let out = ctx
            .metered_activity(
                RetryPolicy::default_policy(),
                MicroUsd(100),
                vec![unit("llm.tokens", 40, 20)],
                |_idem, _att| Ok(vec![ArtifactRef("myelin://acme/out".into())]),
            )
            .expect("a funded metered activity runs");
        assert_eq!(out, vec![ArtifactRef("myelin://acme/out".into())]);
        assert_eq!(
            gate.balance(),
            MicroUsd(940),
            "settled: only the billed 60 is drawn"
        );
        let lr = LedgerRunId::new("R1/merge.queue:0");
        assert_eq!(
            gate.state_of(&tenant(), &lr),
            Some(myelin_storage::reserve_settle::ReservationState::Settled)
        );
    }

    #[test]
    fn in_flight_activity_is_not_interrupted_by_exhaustion_second_dispatch_refused() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let gate = BudgetGate::new(Wallet::new(MicroUsd(100)));

        let body_gate = gate.clone();
        let mut ctx = begin_ctx(&outbox, journal, gate.clone());

        let out1 = ctx
            .metered_activity(
                RetryPolicy::default_policy(),
                MicroUsd(100),
                vec![unit("llm.tokens", 70, 30)],
                |_i, _a| Ok(vec![ArtifactRef("first".into())]),
            )
            .expect("first runs");
        assert_eq!(out1, vec![ArtifactRef("first".into())]);
        assert_eq!(
            body_gate.balance(),
            MicroUsd::ZERO,
            "wallet exhausted by the first"
        );

        let ran2 = Arc::new(AtomicUsize::new(0));
        let ran2_c = ran2.clone();
        let err = ctx
            .metered_activity(
                RetryPolicy::default_policy(),
                MicroUsd(50),
                vec![unit("llm.tokens", 30, 20)],
                move |_i, _a| {
                    ran2_c.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![ArtifactRef("second".into())])
                },
            )
            .expect_err("the second is refused - the wallet is exhausted");
        assert!(matches!(err, crate::WfError::CoCommit(_)));
        assert_eq!(
            ran2.load(Ordering::SeqCst),
            0,
            "the second activity NEVER ran"
        );
        assert_eq!(
            gate.inflight_interrupt_count(),
            0,
            "0 in-flight interrupts (the headline zero)"
        );
    }

    #[test]
    fn long_park_reserves_at_dispatch_and_settles_on_job_done_into_the_same_wallet() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();
        let gate = BudgetGate::new(Wallet::new(MicroUsd(500)));

        let token = job_idem_token("R1", "merge.queue:0");
        deliver_job_done(
            &signals,
            &token,
            vec![ArtifactRef("myelin://acme/ci/green".into())],
        );

        let mut ctx = WfCtx::begin(
            &outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_signals(signals)
        .with_budget(gate.clone());

        let out = ctx
            .metered_schedule_and_run_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
                &runner,
                None,
                MicroUsd(200),
                vec![unit("ci.minute", 100, 50)],
            )
            .expect("dispatch + complete");

        match out {
            JobOutcome::Completed { result, .. } => {
                assert_eq!(result, vec![ArtifactRef("myelin://acme/ci/green".into())]);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1, "dispatched once");
        assert_eq!(
            gate.balance(),
            MicroUsd(350),
            "settled the job into the same wallet"
        );
        assert_eq!(gate.inflight_interrupt_count(), 0, "0 interrupts");
    }

    #[test]
    fn resumed_long_park_settles_once_after_job_done_and_replay() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();
        let gate = BudgetGate::new(Wallet::new(MicroUsd(500)));
        let spec = || JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-8");
        let units = || vec![unit("ci.minute", 100, 50)];

        let mut first = WfCtx::begin(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_signals(signals.clone())
        .with_budget(gate.clone());
        assert_eq!(
            first
                .metered_schedule_and_run_job(spec(), &runner, None, MicroUsd(200), units(),)
                .unwrap(),
            JobOutcome::Parked
        );
        first.commit().unwrap();
        assert_eq!(
            gate.balance(),
            MicroUsd(300),
            "the dispatch reserve is held"
        );

        let token = job_idem_token("R1", "merge.queue:0");
        deliver_job_done(
            &signals,
            &token,
            vec![ArtifactRef("myelin://acme/ci/green".into())],
        );
        let mut resumed = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:01Z",
            42,
            journal.history_for(&tenant(), "R1"),
        )
        .with_signals(signals.clone())
        .with_budget(gate.clone());
        assert!(matches!(
            resumed
                .metered_schedule_and_run_job(spec(), &runner, None, MicroUsd(200), units(),)
                .unwrap(),
            JobOutcome::Completed { .. }
        ));
        resumed.commit().unwrap();
        assert_eq!(
            gate.balance(),
            MicroUsd(350),
            "the unused 50 is refunded once"
        );

        let mut replay = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:02Z",
            42,
            journal.history_for(&tenant(), "R1"),
        )
        .with_signals(signals)
        .with_budget(gate.clone());
        assert!(matches!(
            replay
                .metered_schedule_and_run_job(spec(), &runner, None, MicroUsd(200), units(),)
                .unwrap(),
            JobOutcome::Completed { .. }
        ));
        assert_eq!(
            gate.balance(),
            MicroUsd(350),
            "replay cannot double-refund"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            1,
            "replay cannot redispatch"
        );
    }

    #[test]
    fn long_park_dispatch_against_empty_wallet_is_refused_runner_never_called() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();
        let gate = BudgetGate::new(Wallet::new(MicroUsd(50)));

        let mut ctx = WfCtx::begin(
            &outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_signals(signals)
        .with_budget(gate.clone());

        let err = ctx
            .metered_schedule_and_run_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
                &runner,
                None,
                MicroUsd(200),
                vec![unit("ci.minute", 100, 50)],
            )
            .expect_err("an exhausted wallet refuses the dispatch");
        assert!(matches!(err, crate::WfError::CoCommit(ref m) if m.contains("wallet exhausted")));
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            0,
            "the job was NEVER handed to the runner (no dispatch)"
        );
    }

    #[test]
    fn double_settle_does_not_double_credit_the_wallet() {
        let gate = BudgetGate::new(Wallet::new(MicroUsd(1_000)));
        let lr = LedgerRunId::new("R1/cmd:0");
        gate.reserve(&tenant(), &lr, MicroUsd(100)).unwrap();
        gate.begin(&tenant(), &lr).unwrap();
        assert_eq!(gate.balance(), MicroUsd(900), "reserved 100");

        let units = vec![unit("u", 40, 20)];
        gate.settle(&tenant(), &lr, &units).unwrap();
        assert_eq!(
            gate.balance(),
            MicroUsd(940),
            "refunded 40 once (900 + 40)"
        );
        gate.settle(&tenant(), &lr, &units).unwrap();
        assert_eq!(
            gate.balance(),
            MicroUsd(940),
            "no double-credit on re-settle"
        );
    }

    #[test]
    fn reject_rate_telemetry_records_attempts_rejects_and_settles() {
        let telemetry = FlowTelemetry::new();
        let gate = BudgetGate::new(Wallet::new(MicroUsd(100))).with_telemetry(telemetry.clone());

        let lr1 = LedgerRunId::new("R1/cmd:0");
        gate.reserve(&tenant(), &lr1, MicroUsd(100))
            .expect("admits");
        gate.begin(&tenant(), &lr1).unwrap();
        gate.settle(&tenant(), &lr1, &[unit("u", 100, 0)]).unwrap();

        let lr2 = LedgerRunId::new("R1/cmd:1");
        gate.reserve(&tenant(), &lr2, MicroUsd(50))
            .expect_err("refused");

        assert_eq!(telemetry.reserve_attempted(), 2, "two reserve attempts");
        assert_eq!(telemetry.reserve_rejected(), 1, "one refused");
        assert_eq!(
            telemetry.reserve_reject_rate_bps(),
            5_000,
            "50% reject rate"
        );
        assert_eq!(telemetry.settled(), 1, "one settle recorded");
    }

    #[test]
    fn unmetered_wfctx_runs_the_activity_without_a_reserve() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = WfCtx::begin(
            &outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
        );
        let out = ctx
            .metered_activity(
                RetryPolicy::default_policy(),
                MicroUsd(100),
                vec![unit("u", 10, 0)],
                |_i, _a| Ok(vec![ArtifactRef("ran".into())]),
            )
            .expect("an un-metered activity runs");
        assert_eq!(out, vec![ArtifactRef("ran".into())]);
    }

    #[test]
    fn replay_re_keys_the_reserve_identically_no_double_debit() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let gate = BudgetGate::new(Wallet::new(MicroUsd(1_000)));

        let mut c1 = begin_ctx(&outbox, journal.clone(), gate.clone());
        c1.metered_activity(
            RetryPolicy::default_policy(),
            MicroUsd(100),
            vec![unit("u", 40, 20)],
            |_i, _a| Ok(vec![ArtifactRef("v1".into())]),
        )
        .expect("drive 1");
        c1.commit().expect("co-commit");
        assert_eq!(gate.balance(), MicroUsd(940), "drive 1 drew 60");
        let history = journal.history_for(&tenant(), "R1");

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_budget(gate.clone());
        let out2 = c2
            .metered_activity(
                RetryPolicy::default_policy(),
                MicroUsd(100),
                vec![unit("u", 40, 20)],
                |_i, _a| panic!("the activity must NOT re-run on replay"),
            )
            .expect("the replay drive");
        assert_eq!(
            out2,
            vec![ArtifactRef("v1".into())],
            "replay returns the journaled result"
        );
        assert_eq!(
            gate.balance(),
            MicroUsd(940),
            "0 DOUBLE-DEBIT on replay (re-keyed identically)"
        );
    }

    #[test]
    fn exhausted_metered_activity_settles_refunds_full_and_is_not_left_in_flight() {
        use myelin_storage::reserve_settle::ReservationState;
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let telemetry = FlowTelemetry::new();
        let gate =
            BudgetGate::new(Wallet::new(MicroUsd(1_000))).with_telemetry(telemetry.clone());

        let mut ctx = begin_ctx(&outbox, journal, gate.clone());
        let err = ctx
            .metered_activity(
                RetryPolicy { max_attempts: 2 },
                MicroUsd(100),
                vec![unit("llm.tokens", 40, 20)],
                |_idem, attempt| Err(crate::ActivityError(format!("hard failure {attempt}"))),
            )
            .expect_err("the activity exhausts its retries");
        assert!(
            matches!(err, crate::WfError::ActivityExhausted(_)),
            "the activity error surfaces to the body (retry / compensate / dequeue), got {err:?}"
        );

        let lr = LedgerRunId::new("R1/merge.queue:0");
        assert_eq!(
            gate.balance(),
            MicroUsd(1_000),
            "the reservation is fully refunded on exhaustion (no leak)"
        );
        assert_eq!(
            gate.state_of(&tenant(), &lr),
            Some(ReservationState::Settled),
            "the reservation settled on exhaustion - never left InFlight"
        );
        assert_eq!(
            telemetry.settled(),
            1,
            "the settle-on-exhaustion is recorded"
        );
        assert_eq!(gate.inflight_interrupt_count(), 0, "0 in-flight interrupts");
    }

    #[test]
    fn re_drive_after_exhaustion_does_not_double_refund() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let gate = BudgetGate::new(Wallet::new(MicroUsd(1_000)));

        let mut c1 = begin_ctx(&outbox, journal.clone(), gate.clone());
        let err1 = c1
            .metered_activity(
                RetryPolicy { max_attempts: 1 },
                MicroUsd(100),
                vec![unit("u", 40, 20)],
                |_i, _a| Err(crate::ActivityError("boom".into())),
            )
            .expect_err("drive 1 exhausts");
        assert!(matches!(err1, crate::WfError::ActivityExhausted(_)));
        c1.commit().expect("co-commit journals the activity_failed");
        assert_eq!(
            gate.balance(),
            MicroUsd(1_000),
            "drive 1 refunded the full reservation"
        );
        let history = journal.history_for(&tenant(), "R1");

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_budget(gate.clone());
        let err2 = c2
            .metered_activity(
                RetryPolicy { max_attempts: 1 },
                MicroUsd(100),
                vec![unit("u", 40, 20)],
                |_i, _a| panic!("the activity must NOT re-run on replay"),
            )
            .expect_err("the replay re-drives to the journaled failure");
        assert!(matches!(err2, crate::WfError::ActivityExhausted(_)));
        assert_eq!(
            gate.balance(),
            MicroUsd(1_000),
            "0 DOUBLE-REFUND on replay (the settle is idempotent)"
        );
    }

    #[test]
    fn wallet_from_budget_seeds_and_clamps() {
        let w = Wallet::from_budget(&crate::RunBudget { minor_units: 500 });
        assert_eq!(w.balance(), MicroUsd(500));
        let neg = Wallet::from_budget(&crate::RunBudget { minor_units: -5 });
        assert_eq!(
            neg.balance(),
            MicroUsd::ZERO,
            "a negative budget is an empty wallet"
        );
    }

    #[test]
    fn begin_moves_reservation_reserved_to_in_flight() {
        use myelin_storage::reserve_settle::ReservationState;
        let gate = BudgetGate::new(Wallet::new(MicroUsd(1_000)));
        let lr = LedgerRunId::new("R1/cmd:0");
        gate.reserve(&tenant(), &lr, MicroUsd(100)).unwrap();
        assert_eq!(
            gate.state_of(&tenant(), &lr),
            Some(ReservationState::Reserved),
            "before begin: Reserved (the one teardown-able state)"
        );
        gate.begin(&tenant(), &lr).unwrap();
        assert_eq!(
            gate.state_of(&tenant(), &lr),
            Some(ReservationState::InFlight),
            "after begin: InFlight - from here there is NO teardown path (never interrupt)"
        );
        gate.begin(&tenant(), &lr).unwrap();
        assert_eq!(
            gate.state_of(&tenant(), &lr),
            Some(ReservationState::InFlight)
        );
    }

    #[test]
    fn budget_errors_display_loud_and_specific() {
        let refused = BudgetError::Refused {
            requested: MicroUsd(200),
            available: MicroUsd(50),
        }
        .to_string();
        assert!(
            refused.contains("no balance, no dispatch"),
            "must cite the floor: {refused}"
        );
        assert!(BudgetError::NoSuchReservation
            .to_string()
            .contains("never invent a charge"));
    }
}
