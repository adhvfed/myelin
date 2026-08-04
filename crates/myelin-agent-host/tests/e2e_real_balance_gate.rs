//! # HERMETIC drill — the reserve gate is a REAL "no balance → no run" gate (unified-wallet slice 3).
//!
//! Before slice 3 the hosted-run reserve gate read a hardcoded nominal `available`, disconnected from
//! the prepaid balance — a broke tenant still dispatched (and only halted later at the per-turn
//! debit). This drill proves the gate now reads the ACTUAL wallet balance minus the tenant's
//! outstanding reservations: a tenant who cannot afford the reserve `estimate` is REFUSED at dispatch
//! (the run never starts, nothing is billed), and a funded tenant dispatches and completes. No
//! network, no DB — a `MockModelClient` brain + an in-memory wallet double, exactly the F1 core the
//! live Luna drill exercises against real Luna + real Postgres.

use std::sync::{Arc, Mutex};

use myelin_agent_host::{
    dispatch_metered_llm_run, LlmRunTask, MicroUsd, RunSubstrateWiring, RunWallet, Tools,
    WalletError,
};
use myelin_agent_model::mock::MockModelClient;
use myelin_agent_model::{ModelReply, ModelResponse, Usage};
use myelin_events::{MonotonicMinter, OutboxStore};
use myelin_flow::WfJournal;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_storage::reserve_settle::{CostLedger, RunId};
use myelin_tenancy::{Region, TenantId};

/// A network-free, DB-free in-memory [`RunWallet`] double: a balance + a debit witness.
struct MemWallet {
    balance: Mutex<u64>,
    debits: Mutex<Vec<(String, u64)>>,
}

impl MemWallet {
    fn with_balance(micro: u64) -> MemWallet {
        MemWallet {
            balance: Mutex::new(micro),
            debits: Mutex::new(Vec::new()),
        }
    }
    fn debit_rows(&self, run_id: &str) -> usize {
        self.debits
            .lock()
            .unwrap()
            .iter()
            .filter(|(r, _)| r == run_id)
            .count()
    }
}

impl RunWallet for MemWallet {
    fn balance(&self, _tenant: &TenantId) -> MicroUsd {
        MicroUsd(*self.balance.lock().unwrap())
    }
    fn debit(
        &self,
        _tenant: &TenantId,
        amount: MicroUsd,
        run_id: &str,
    ) -> Result<MicroUsd, WalletError> {
        let mut bal = self.balance.lock().unwrap();
        match bal.checked_sub(amount.0) {
            None => Err(WalletError::InsufficientBalance {
                requested: amount,
                available: MicroUsd(*bal),
            }),
            Some(new) => {
                *bal = new;
                self.debits
                    .lock()
                    .unwrap()
                    .push((run_id.to_string(), amount.0));
                Ok(MicroUsd(new))
            }
        }
    }
}

fn agent_principal(tenant: &TenantId) -> Principal {
    Principal::stub(
        PrincipalId("psn:host-agent".into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("host".into()),
            on_behalf_of: None,
        },
        tenant.clone(),
    )
}

const USAGE: Usage = Usage::Reported {
    input: 1_000,
    cached_input: 500,
    output: 200,
};

fn brain() -> MockModelClient {
    MockModelClient::ok(ModelResponse {
        reply: ModelReply::Final {
            content: "ready".into(),
        },
        usage: USAGE,
    })
}

/// **A tenant whose balance is below the reserve `estimate` is REFUSED at dispatch — the run never
/// starts and nothing is billed.** The default `estimate` is `MicroUsd(100_000)`; a wallet holding
/// `50_000` leaves `available = balance − outstanding = 50_000 − 0 = 50_000 < 100_000`, so the reserve
/// refuses and the run is never dispatched (this is the real "no balance → no run" gate, not the later
/// per-turn debit halt).
#[test]
fn insufficient_balance_is_refused_at_dispatch_run_never_starts() {
    let tenant = TenantId("01J0GATEBROKE".into());
    let region = Region("fr-par".into());

    // Below the default estimate (100_000) — cannot even afford the reserve.
    let wallet = MemWallet::with_balance(50_000);

    let mut ledger = CostLedger::new();
    let outbox = OutboxStore::new();
    let mut wiring = RunSubstrateWiring {
        ledger: &mut ledger,
        outbox: &outbox,
        id_minter: Arc::new(MonotonicMinter::new()),
        journal: WfJournal::new(),
    };

    let task = LlmRunTask::new(
        tenant.clone(),
        agent_principal(&tenant),
        "psn:host-agent",
        "Rrefuse-1",
        "sys",
        "prompt",
    );

    let err = dispatch_metered_llm_run(
        &wallet,
        region,
        &task,
        &mut wiring,
        Box::new(brain()),
        Tools::none(),
    )
    .expect_err("a tenant who cannot afford the estimate is refused at dispatch");

    // Refused by the REAL reserve gate (no balance → no run), not the per-turn debit.
    let msg = err.to_string();
    assert!(
        msg.contains("no balance, no run") || msg.contains("dispatch refused"),
        "the reserve gate refused the dispatch: {msg}"
    );

    // The run NEVER started: no reservation was written, and nothing was billed (no debit).
    assert!(
        wiring
            .ledger
            .state_of(&tenant, &RunId::new("Rrefuse-1"))
            .is_none(),
        "a refused dispatch leaves NO reservation — the run never started"
    );
    assert_eq!(wallet.balance(&tenant), MicroUsd(50_000), "balance untouched");
    assert_eq!(wallet.debit_rows("Rrefuse-1"), 0, "nothing was billed");
}

/// **A funded tenant dispatches and completes** — the same wiring, with a balance well above the
/// reserve `estimate`, admits the run, drives it to a metered completion, and debits the per-turn
/// charge. Proves the gate ADMITS when `balance − outstanding ≥ estimate` (the affordability check is
/// real in both directions, not a blanket refusal).
#[test]
fn sufficient_balance_dispatches_and_completes() {
    let tenant = TenantId("01J0GATEFUNDED".into());
    let region = Region("fr-par".into());

    let wallet = MemWallet::with_balance(1_000_000);

    let mut ledger = CostLedger::new();
    let outbox = OutboxStore::new();
    let mut wiring = RunSubstrateWiring {
        ledger: &mut ledger,
        outbox: &outbox,
        id_minter: Arc::new(MonotonicMinter::new()),
        journal: WfJournal::new(),
    };

    let task = LlmRunTask::new(
        tenant.clone(),
        agent_principal(&tenant),
        "psn:host-agent",
        "Rfunded-1",
        "sys",
        "Reply with the single word: ready.",
    )
    .with_max_output_tokens(16)
    .with_now_secs(1000);

    let report = dispatch_metered_llm_run(
        &wallet,
        region,
        &task,
        &mut wiring,
        Box::new(brain()),
        Tools::none(),
    )
    .expect("a funded tenant dispatches and the run completes");

    assert_eq!(report.answer, "ready");
    assert!(
        report.outcome.0.contains("completed"),
        "loop completed: {:?}",
        report.outcome
    );
    // The run was admitted and billed once; the nominal reserve/settle ledger stayed balanced.
    assert!(report.charged_micro > 0, "the funded run was billed");
    assert_eq!(wallet.debit_rows("Rfunded-1"), 1, "one run-linked debit");
    assert!(report.telemetry.ledger_balanced(), "reserved == settled");
    assert_eq!(report.telemetry.runs_completed(), 1);
}
