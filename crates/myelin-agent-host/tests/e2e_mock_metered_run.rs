use std::sync::Mutex;

use myelin_agent_host::{
    dispatch_test_run_with_synthetic_identity, DebitOutcome, LlmRunTask, MicroUsd,
    RunSubstrateWiring, RunWallet, Tools, WalletError,
};
use myelin_agent_model::mock::MockModelClient;
use myelin_agent_model::{ModelReply, ModelResponse, Usage};
use myelin_agent_service::{price, LUNA_RATES};
use myelin_events::{MonotonicMinter, OutboxStore};
use myelin_flow::WfJournal;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_storage::reserve_settle::CostLedger;
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

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
    fn debit_once(
        &self,
        _tenant: &TenantId,
        amount: MicroUsd,
        run_id: &str,
        _charge_key: &str,
    ) -> Result<DebitOutcome, WalletError> {
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
                Ok(DebitOutcome::Applied(MicroUsd(new)))
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

#[test]
fn mock_metered_run_debits_once_and_returns_the_answer() {
    let tenant = TenantId("01J0HOSTMOCK".into());
    let region = Region("fr-par".into());

    let per_turn = price(
        &myelin_agent::TokenUsage::Reported {
            input: 1_000,
            cached_input: 500,
            output: 200,
        },
        &LUNA_RATES,
    )
    .expect("prices without overflow")
    .total()
    .expect("total fits");

    let wallet = MemWallet::with_balance(1_000_000);

    let brain = MockModelClient::ok(ModelResponse {
        reply: ModelReply::Final {
            content: "ready".into(),
        },
        usage: USAGE,
    });

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
        "Rmock-1",
        "You are a terse assistant.",
        "Reply with the single word: ready.",
    )
    .with_max_output_tokens(16)
    .with_now_secs(1000);

    let report = dispatch_test_run_with_synthetic_identity(
        &wallet,
        region,
        &task,
        &mut wiring,
        Box::new(brain),
        Tools::none(),
    )
    .expect("the mock metered run completes");

    assert_eq!(report.answer, "ready", "the captured final answer");
    assert!(
        report.outcome.0.contains("completed"),
        "loop completed: {:?}",
        report.outcome
    );

    assert_eq!(
        report.charged_micro, per_turn.0,
        "one turn priced + debited"
    );
    assert_eq!(wallet.balance(&tenant), MicroUsd(1_000_000 - per_turn.0));
    assert_eq!(
        wallet.debit_rows("Rmock-1"),
        1,
        "exactly one run-linked debit"
    );

    assert!(report.telemetry.ledger_balanced(), "reserved == settled");
    assert_eq!(report.telemetry.traces_written(), 1);
    assert_eq!(report.telemetry.tokens_revoked(), 1, "token torn down");
    assert_eq!(report.telemetry.runs_completed(), 1);
}

#[test]
fn underfunded_wallet_halts_gracefully_without_going_negative() {
    let tenant = TenantId("01J0HOSTDRY".into());
    let region = Region("fr-par".into());

    let wallet = MemWallet::with_balance(0);
    let brain = MockModelClient::ok(ModelResponse {
        reply: ModelReply::Final {
            content: "should-not-bill".into(),
        },
        usage: USAGE,
    });
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
        "Rdry-1",
        "sys",
        "prompt",
    );

    let err = dispatch_test_run_with_synthetic_identity(
        &wallet,
        region,
        &task,
        &mut wiring,
        Box::new(brain),
        Tools::none(),
    )
    .expect_err("an unfunded run cannot bill, so it halts");
    let msg = err.to_string();
    assert!(
        msg.contains("spend cap") || msg.contains("balance"),
        "graceful halt: {msg}"
    );
    assert_eq!(wallet.balance(&tenant), MicroUsd(0), "never negative");
    assert_eq!(wallet.debit_rows("Rdry-1"), 0, "nothing billed");
}
