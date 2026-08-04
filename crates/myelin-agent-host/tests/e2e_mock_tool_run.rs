//! # HERMETIC tool-executing drill — the FIRST real tool run wired green, NO network, NO DB.
//!
//! Proves the tools-enabled composition ([`dispatch_metered_llm_run_with_tools`]) drives a REAL
//! [`LlmAgentRuntime`](myelin_agent_model::LlmAgentRuntime) run that ACTUALLY INVOKES a governed READ
//! tool and answers over its result — using a network-free scripted brain (a `ModelClient` that emits
//! a `git.read_check_status` tool call, then a final answer that ECHOES the tool result) and a
//! fake-but-real-shaped executor, so the whole tool path — `validate_call` (the security checkpoint) →
//! `execute` → append the `ToolResult` → step again → answer — wires green with no external deps.
//!
//! This is the identical composition + F1 + per-turn metering path the LIVE Luna tool drill
//! (`e2e_luna_tool_live.rs`) exercises against real Luna + the real Git check-status subsystem on live
//! Postgres — only the brain (scripted vs Luna) and the executor (fake vs the durable
//! [`GitCheckStatusReadExecutor`]) differ. The run is metered for TWO turns (the tool turn + the
//! answer turn), the reserve/settle ledger stays balanced, and the run tears down cleanly.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use myelin_agent::{EffectKind, ToolCall, ToolDef, ToolResult};
use myelin_agent_host::{
    dispatch_metered_llm_run_with_tools, git_check_status_read_tool_def,
    git_check_status_read_tool_schema, LlmRunTask, MicroUsd, RunSubstrateWiring, RunWallet,
    ToolCatalogue, WalletError, GIT_READ_CHECK_STATUS_TOOL,
};
use myelin_agent_model::{
    ModelClient, ModelError, ModelReply, ModelRequest, ModelResponse, ModelTurn, ToolCallRequest,
    Usage,
};
use myelin_agent_service::{price, ToolExecError, ToolExecutor, LUNA_RATES};
use myelin_events::{MonotonicMinter, OutboxStore};
use myelin_flow::WfJournal;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_storage::reserve_settle::CostLedger;
use myelin_tenancy::{Region, TenantId};

const REPO: &str = "myelin://01J0HOSTTOOL/git/repo/core";
const COMMIT: &str = "abc123def456";

/// The fixed per-turn usage (same profile as the metering drills): 459 micro-USD per priced turn.
const USAGE: Usage = Usage::Reported {
    input: 1_000,
    cached_input: 500,
    output: 200,
};

/// A network-free in-memory [`RunWallet`] double: a balance + a per-turn debit log (fail-closed).
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

/// **The scripted brain: emit a `git.read_check_status` tool call on the FIRST step; on the step that
/// carries the tool RESULT back, answer by ECHOING that result.** Deterministic (no counter): it keys
/// its decision on whether the request already carries a tool-result turn — so the "reflect the tool
/// result in the final answer" is a real threading proof, not a canned string.
struct ScriptedToolBrain;

impl ModelClient for ScriptedToolBrain {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let tool_result = request.turns.iter().rev().find_map(|turn| match turn {
            ModelTurn::ToolResults(results) => results.first().map(|r| r.content.clone()),
            _ => None,
        });
        match tool_result {
            // The tool has run — the loop threaded its result back. Answer over it.
            Some(content) => Ok(ModelResponse {
                reply: ModelReply::Final {
                    content: format!("Based on the check status: {content}"),
                },
                usage: USAGE,
            }),
            // First step — call the read tool with valid arguments (repo + commit).
            None => Ok(ModelResponse {
                reply: ModelReply::ToolCalls(vec![ToolCallRequest {
                    id: "call-check-1".into(),
                    name: GIT_READ_CHECK_STATUS_TOOL.into(),
                    arguments: serde_json::json!({ "repo": REPO, "commit": COMMIT }),
                }]),
                usage: USAGE,
            }),
        }
    }
}

/// **A fake-but-real-shaped READ executor**: no DB, returns the SAME TEXT shape
/// [`GitCheckStatusReadExecutor`](myelin_agent_host::GitCheckStatusReadExecutor) formats from real
/// rows, and records that it was invoked (the tool-was-called witness).
struct FakeCheckReadExecutor {
    invocations: AtomicUsize,
}

impl ToolExecutor for FakeCheckReadExecutor {
    fn execute(&self, def: &ToolDef, call: &ToolCall) -> Result<ToolResult, ToolExecError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        // The loop only routes a Read tool here (route_of(Read) == Direct); assert the shape.
        assert_eq!(def.effect_kind, EffectKind::Read);
        let repo = call.arguments.get("repo").and_then(|v| v.as_str()).unwrap();
        let commit = call
            .arguments
            .get("commit")
            .and_then(|v| v.as_str())
            .unwrap();
        Ok(ToolResult(format!(
            "check status for commit {commit} in repo {repo}: \
             ci/build = Success (run attempt 1, Trusted, cost_settled=true)"
        )))
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

/// **A real tool-executing metered run: the scripted brain CALLS the read tool, the executor returns
/// the real-shaped result, the brain answers over it, and the wallet is debited for BOTH turns.**
#[test]
fn mock_tool_run_invokes_the_read_tool_and_meters_two_turns() {
    let tenant = TenantId("01J0HOSTTOOL".into());
    let region = Region("fr-par".into());
    let run_id = "Rtool-mock-1";

    // The per-turn charge (same pricing the loop uses — no magic-number drift).
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

    // Seed $1.00; the run should debit exactly two turns (tool turn + answer turn).
    let wallet = MemWallet::with_balance(1_000_000);

    // The REAL permissioned catalogue (the loop validates each call against it) + the model-facing
    // advertised schema (what the brain is told it may call).
    let catalogue = ToolCatalogue::new([git_check_status_read_tool_def()]);
    let advertised = [git_check_status_read_tool_schema()];
    let executor = FakeCheckReadExecutor {
        invocations: AtomicUsize::new(0),
    };

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
        run_id,
        "You are a hosted agent with tools. Use the read tool when asked, then answer.",
        format!("Read the CI checks for repo {REPO} at commit {COMMIT} and report the build state."),
    )
    .with_max_output_tokens(64)
    .with_now_secs(1000);

    let report = dispatch_metered_llm_run_with_tools(
        &wallet,
        region,
        &task,
        &mut wiring,
        Box::new(ScriptedToolBrain),
        &catalogue,
        &executor,
        &advertised,
    )
    .expect("the mock tool run completes");

    // THE TOOL WAS ACTUALLY INVOKED — the executor ran the real read exactly once.
    assert_eq!(
        executor.invocations.load(Ordering::SeqCst),
        1,
        "the read tool was executed once"
    );

    // The agent's final answer REFLECTS the tool result (the loop threaded it back into the model).
    assert!(
        report.answer.contains("ci/build = Success"),
        "the answer reflects the seeded tool result: {:?}",
        report.answer
    );
    assert!(report.outcome.0.contains("completed"), "loop completed");

    // The wallet was DEBITED for BOTH turns (the tool turn + the answer turn) — a real tool-executing
    // run is billed for each priced model step.
    assert_eq!(
        report.charged_micro,
        per_turn.0 * 2,
        "two priced turns (tool + answer)"
    );
    assert_eq!(wallet.debit_rows(run_id), 2, "two run-linked debits");
    assert_eq!(wallet.balance(&tenant), MicroUsd(1_000_000 - per_turn.0 * 2));

    // The reserve/settle ledger stayed balanced + the survival signals fired + torn down.
    assert!(report.telemetry.ledger_balanced(), "reserved == settled");
    assert_eq!(report.telemetry.traces_written(), 1);
    assert_eq!(report.telemetry.tokens_revoked(), 1, "token torn down");
    assert_eq!(report.telemetry.runs_completed(), 1);
}

/// **The security checkpoint holds: a tool call whose UNTRUSTED arguments fail the tool's schema is
/// rejected BEFORE the executor runs (fail-closed), and the run tears down.** The brain omits the
/// required `commit` argument; `validate_call` aborts the run and the executor is NEVER reached.
#[test]
fn invalid_tool_arguments_are_rejected_before_the_executor_runs() {
    struct BadArgsBrain;
    impl ModelClient for BadArgsBrain {
        fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            Ok(ModelResponse {
                reply: ModelReply::ToolCalls(vec![ToolCallRequest {
                    id: "call-bad".into(),
                    name: GIT_READ_CHECK_STATUS_TOOL.into(),
                    // Missing the required `commit` — must fail the schema checkpoint.
                    arguments: serde_json::json!({ "repo": REPO }),
                }]),
                usage: USAGE,
            })
        }
    }

    let tenant = TenantId("01J0HOSTTOOLBAD".into());
    let region = Region("fr-par".into());
    let wallet = MemWallet::with_balance(1_000_000);
    let catalogue = ToolCatalogue::new([git_check_status_read_tool_def()]);
    let advertised = [git_check_status_read_tool_schema()];
    let executor = FakeCheckReadExecutor {
        invocations: AtomicUsize::new(0),
    };
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
        "Rtool-bad-1",
        "sys",
        "prompt",
    )
    .with_now_secs(1000);

    let err = dispatch_metered_llm_run_with_tools(
        &wallet,
        region,
        &task,
        &mut wiring,
        Box::new(BadArgsBrain),
        &catalogue,
        &executor,
        &advertised,
    )
    .expect_err("a schema-invalid tool call aborts the run fail-closed");

    // The untrusted arguments were NEVER handed to the executor (the checkpoint held).
    assert_eq!(
        executor.invocations.load(Ordering::SeqCst),
        0,
        "the executor is never reached on a validation reject"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("commit") || msg.to_lowercase().contains("valid"),
        "loud validation rejection: {msg}"
    );
    // The first (tool-call) turn was still priced + debited before the reject (the model call
    // happened); the run tore down without a negative balance.
    assert!(wallet.balance(&tenant).0 <= 1_000_000);
}
