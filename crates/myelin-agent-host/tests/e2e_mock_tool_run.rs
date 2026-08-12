use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use myelin_agent::{EffectKind, ToolCall, ToolDef, ToolResult};
use myelin_agent_host::{
    dispatch_test_run_with_synthetic_identity, git_check_status_read_tool_def,
    git_check_status_read_tool_schema, DebitOutcome, LlmRunTask, MicroUsd, RunSubstrateWiring,
    RunWallet, ToolCatalogue, Tools, WalletError, GIT_READ_CHECK_STATUS_TOOL,
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

const USAGE: Usage = Usage::Reported {
    input: 1_000,
    cached_input: 500,
    output: 200,
};

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

struct ScriptedToolBrain;

impl ModelClient for ScriptedToolBrain {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let tool_result = request.turns.iter().rev().find_map(|turn| match turn {
            ModelTurn::ToolResults(results) => results.first().map(|r| r.content.clone()),
            _ => None,
        });
        match tool_result {
            Some(content) => Ok(ModelResponse {
                reply: ModelReply::Final {
                    content: format!("Based on the check status: {content}"),
                },
                usage: USAGE,
            }),
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

struct FakeCheckReadExecutor {
    invocations: AtomicUsize,
}

impl ToolExecutor for FakeCheckReadExecutor {
    fn execute(
        &self,
        _context: &myelin_agent_service::ToolExecutionContext<'_>,
        def: &ToolDef,
        call: &ToolCall,
    ) -> Result<ToolResult, ToolExecError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        assert_eq!(def.effect_kind, EffectKind::Read);
        let repo = call.arguments.get("repo").and_then(|v| v.as_str()).unwrap();
        let commit = call
            .arguments
            .get("commit")
            .and_then(|v| v.as_str())
            .unwrap();
        Ok(ToolResult::Succeeded(format!(
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

#[test]
fn mock_tool_run_invokes_the_read_tool_and_meters_two_turns() {
    let tenant = TenantId("01J0HOSTTOOL".into());
    let region = Region("fr-par".into());
    let run_id = "Rtool-mock-1";

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
        format!(
            "Read the CI checks for repo {REPO} at commit {COMMIT} and report the build state."
        ),
    )
    .with_max_output_tokens(64)
    .with_now_secs(1000);

    let report = dispatch_test_run_with_synthetic_identity(
        &wallet,
        region,
        &task,
        &mut wiring,
        Box::new(ScriptedToolBrain),
        Tools {
            catalogue: &catalogue,
            executor: &executor,
            advertised: &advertised,
        },
    )
    .expect("the mock tool run completes");

    assert_eq!(
        executor.invocations.load(Ordering::SeqCst),
        1,
        "the read tool was executed once"
    );

    assert!(
        report.answer.contains("ci/build = Success"),
        "the answer reflects the seeded tool result: {:?}",
        report.answer
    );
    assert!(report.outcome.0.contains("completed"), "loop completed");

    assert_eq!(
        report.charged_micro,
        per_turn.0 * 2,
        "two priced turns (tool + answer)"
    );
    assert_eq!(wallet.debit_rows(run_id), 2, "two run-linked debits");
    assert_eq!(
        wallet.balance(&tenant),
        MicroUsd(1_000_000 - per_turn.0 * 2)
    );

    assert!(report.telemetry.ledger_balanced(), "reserved == settled");
    assert_eq!(report.telemetry.traces_written(), 1);
    assert_eq!(report.telemetry.tokens_revoked(), 1, "token torn down");
    assert_eq!(report.telemetry.runs_completed(), 1);
}

#[test]
fn invalid_tool_arguments_are_rejected_before_the_executor_runs() {
    struct BadArgsBrain;
    impl ModelClient for BadArgsBrain {
        fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            Ok(ModelResponse {
                reply: ModelReply::ToolCalls(vec![ToolCallRequest {
                    id: "call-bad".into(),
                    name: GIT_READ_CHECK_STATUS_TOOL.into(),
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

    let err = dispatch_test_run_with_synthetic_identity(
        &wallet,
        region,
        &task,
        &mut wiring,
        Box::new(BadArgsBrain),
        Tools {
            catalogue: &catalogue,
            executor: &executor,
            advertised: &advertised,
        },
    )
    .expect_err("a schema-invalid tool call aborts the run fail-closed");

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
    assert!(wallet.balance(&tenant).0 <= 1_000_000);
}
