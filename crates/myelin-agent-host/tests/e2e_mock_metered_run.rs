//! # HERMETIC end-to-end drill — the composition + F1 + per-turn metering, NO network, NO DB.
//!
//! Proves the hosted-agent composition ([`dispatch_metered_llm_run`]) drives a REAL
//! [`LlmAgentRuntime`](myelin_agent_model::LlmAgentRuntime) run to a metered completion using a
//! network-free [`MockModelClient`](myelin_agent_model::mock::MockModelClient) brain and an in-memory
//! wallet double — so the wiring (F1: the wallet is ALWAYS threaded; the per-turn debit fires; the
//! answer is captured) is verified GREEN in the hermetic suite, exactly the path the live Luna drill
//! (`e2e_luna_live.rs`) exercises against real Luna + real Postgres.
//!
//! This is the same F1 core the money entry ([`run_llm_agent`] / [`AgentHost::run_llm_agent`]) funnels
//! through — only the brain (mock vs Luna) and the wallet (in-memory vs durable) differ.

use std::sync::Mutex;

use myelin_agent_host::{
    dispatch_metered_llm_run, LlmRunTask, MicroUsd, RunSubstrateWiring, RunWallet, WalletError,
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

/// A network-free, DB-free in-memory [`RunWallet`] double: a balance + a debit log. Fail-closed on an
/// underfunded debit exactly like the durable wallet (no partial debit, no negative balance).
struct MemWallet {
    balance: Mutex<u64>,
    debits: Mutex<Vec<(String, u64)>>, // (run_id, amount) — the per-turn debit witness.
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
            // Fail-closed: nothing written on an underfunded debit (mirrors the durable wallet).
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

/// The fixed per-turn usage (same profile as the service's wallet-metering integration test):
/// wholesale = (1000·200_000 + 500·20_000 + 200·1_200_000)/1e6 = 450 ; markup = round(9.0) = 9 ;
/// total = 459 micro-USD.
const USAGE: Usage = Usage::Reported {
    input: 1_000,
    cached_input: 500,
    output: 200,
};

/// **A no-tools metered run: the mock brain answers on turn 0, the wallet is debited ONCE for the
/// priced usage, the answer is captured, and the ledger stays balanced.**
#[test]
fn mock_metered_run_debits_once_and_returns_the_answer() {
    let tenant = TenantId("01J0HOSTMOCK".into());
    let region = Region("fr-par".into());

    // The per-turn charge, computed by the SAME pricing the loop uses (no magic-number drift).
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

    // Seed $1.00; the run should debit exactly one turn.
    let wallet = MemWallet::with_balance(1_000_000);

    // The network-free brain: a single final answer with reported usage (no tool call → submit).
    let brain = MockModelClient::ok(ModelResponse {
        reply: ModelReply::Final {
            content: "ready".into(),
        },
        usage: USAGE,
    });

    // The in-memory nominal substrate (durable in production; in-memory here).
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

    let report = dispatch_metered_llm_run(&wallet, region, &task, &mut wiring, Box::new(brain))
        .expect("the mock metered run completes");

    // The run SUBMITTED a non-empty text answer, captured at the ModelClient seam.
    assert_eq!(report.answer, "ready", "the captured final answer");
    assert!(report.outcome.0.contains("completed"), "loop completed: {:?}", report.outcome);

    // The wallet was DEBITED exactly once for the priced usage (F1: metering fired).
    assert_eq!(report.charged_micro, per_turn.0, "one turn priced + debited");
    assert_eq!(wallet.balance(&tenant), MicroUsd(1_000_000 - per_turn.0));
    assert_eq!(wallet.debit_rows("Rmock-1"), 1, "exactly one run-linked debit");

    // The nominal reserve/settle ledger stayed balanced + the survival signals fired.
    assert!(report.telemetry.ledger_balanced(), "reserved == settled");
    assert_eq!(report.telemetry.traces_written(), 1);
    assert_eq!(report.telemetry.tokens_revoked(), 1, "token torn down");
    assert_eq!(report.telemetry.runs_completed(), 1);
}

/// **F1 witness — a run whose wallet cannot fund the turn halts GRACEFULLY (spend cap), never
/// unbilled and never negative.** An empty wallet trips the pre-step gate; a funded-but-too-small
/// wallet trips the post-debit refusal — both are structural (the run cannot proceed unpaid).
#[test]
fn underfunded_wallet_halts_gracefully_without_going_negative() {
    let tenant = TenantId("01J0HOSTDRY".into());
    let region = Region("fr-par".into());

    // Empty wallet → the pre-step spend gate stops the run BEFORE any paid call.
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

    let err = dispatch_metered_llm_run(&wallet, region, &task, &mut wiring, Box::new(brain))
        .expect_err("an unfunded run cannot bill, so it halts");
    // The run halted gracefully at the spend cap — never unbilled, never negative.
    let msg = err.to_string();
    assert!(msg.contains("spend cap") || msg.contains("balance"), "graceful halt: {msg}");
    assert_eq!(wallet.balance(&tenant), MicroUsd(0), "never negative");
    assert_eq!(wallet.debit_rows("Rdry-1"), 0, "nothing billed");
}
