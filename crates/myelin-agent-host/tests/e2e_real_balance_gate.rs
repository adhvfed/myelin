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

#[test]
fn insufficient_balance_is_refused_at_dispatch_run_never_starts() {
    let tenant = TenantId("01J0GATEBROKE".into());
    let region = Region("fr-par".into());

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

    let msg = err.to_string();
    assert!(
        msg.contains("no balance, no run") || msg.contains("dispatch refused"),
        "the reserve gate refused the dispatch: {msg}"
    );

    assert!(
        wiring
            .ledger
            .state_of(&tenant, &RunId::new("Rrefuse-1"))
            .is_none(),
        "a refused dispatch leaves NO reservation - the run never started"
    );
    assert_eq!(wallet.balance(&tenant), MicroUsd(50_000), "balance untouched");
    assert_eq!(wallet.debit_rows("Rrefuse-1"), 0, "nothing was billed");
}

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
    assert!(report.charged_micro > 0, "the funded run was billed");
    assert_eq!(wallet.debit_rows("Rfunded-1"), 1, "one run-linked debit");
    assert!(report.telemetry.ledger_balanced(), "reserved == settled");
    assert_eq!(report.telemetry.runs_completed(), 1);
}
