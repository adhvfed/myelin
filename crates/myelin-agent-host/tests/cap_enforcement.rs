use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use myelin_agent::{EffectKind, ToolCall, ToolDef, ToolResult};
use myelin_agent_host::{
    dispatch_test_run_with_synthetic_identity, git_check_status_read_tool_def,
    git_check_status_read_tool_schema, CapEnforcingExecutor, DebitOutcome, LlmRunTask, MicroUsd,
    RunSubstrateWiring, RunWallet, ToolCatalogue, Tools, WalletError, GIT_READ_CHECK_STATUS_TOOL,
};
use myelin_agent_model::{
    ModelClient, ModelError, ModelReply, ModelRequest, ModelResponse, ModelTurn, ToolCallRequest,
    Usage,
};
use myelin_agent_service::{ToolExecError, ToolExecutor};
use myelin_events::{MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::WfJournal;
use myelin_identity::{
    ObjectId, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, RuntimeRef, TupleDelta,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_storage::reserve_settle::CostLedger;
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

const REPO: &str = "myelin://01J0HOSTCAP/git/repo/core";
const COMMIT: &str = "capcommit0001";
const AGENT_ID: &str = "psn:host-agent";

const USAGE: Usage = Usage::Reported {
    input: 1_000,
    cached_input: 500,
    output: 200,
};

struct MemWallet {
    balance: Mutex<u64>,
}
impl MemWallet {
    fn with_balance(micro: u64) -> MemWallet {
        MemWallet {
            balance: Mutex::new(micro),
        }
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
        _run_id: &str,
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
impl FakeCheckReadExecutor {
    fn new() -> FakeCheckReadExecutor {
        FakeCheckReadExecutor {
            invocations: AtomicUsize::new(0),
        }
    }
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
        Ok(ToolResult(format!(
            "check status for commit {commit} in repo {repo}: \
             ci/build = Success (run attempt 1, Trusted, cost_settled=true)"
        )))
    }
}

fn agent_principal(tenant: &TenantId) -> Principal {
    Principal::stub(
        PrincipalId(AGENT_ID.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("host".into()),
            on_behalf_of: None,
        },
        tenant.clone(),
    )
}

fn rebac_engine(agent: &Principal, grant_pull: bool) -> StoreBackedCheck {
    let tuples = TupleStore::new(OutboxStore::new());
    if grant_pull {
        let scope = TenantScope::from_verified_token(agent, agent.region.clone());
        let admin = Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            agent.tenant.clone(),
        );
        tuples
            .write_tuples(
                &scope,
                &admin,
                &[TupleDelta::Add(RelationTuple {
                    object: ObjectId("repo:core".into()),
                    relation: RelName("pull".into()),
                    subject: PrincipalId(AGENT_ID.into()),
                    caveat: None,
                })],
                None,
                None,
                Timestamp("2026-06-19T00:00:00Z".into()),
            )
            .expect("seed the real `pull` grant via write_tuples");
    }
    StoreBackedCheck::new(tuples)
}

fn wiring<'a>(ledger: &'a mut CostLedger, outbox: &'a OutboxStore) -> RunSubstrateWiring<'a> {
    RunSubstrateWiring {
        ledger,
        outbox,
        id_minter: Arc::new(MonotonicMinter::new()),
        journal: WfJournal::new(),
    }
}

fn task(tenant: &TenantId, agent: Principal, run_id: &str) -> LlmRunTask {
    LlmRunTask::new(
        tenant.clone(),
        agent,
        AGENT_ID,
        run_id,
        "You are a hosted agent with tools. Use the read tool when asked, then answer.",
        format!(
            "Read the CI checks for repo {REPO} at commit {COMMIT} and report the build state."
        ),
    )
    .with_max_output_tokens(64)
    .with_now_secs(1000)
}

#[test]
fn tool_call_is_allowed_when_the_principal_holds_the_required_cap() {
    let tenant = TenantId("01J0HOSTCAP".into());
    let region = Region("fr-par".into());
    let agent = agent_principal(&tenant);

    let identity: Arc<dyn myelin_identity::IdentityService + Send + Sync> =
        Arc::new(rebac_engine(&agent, true));
    let inner = FakeCheckReadExecutor::new();
    let gated = CapEnforcingExecutor::for_git_read_tool(identity, agent.clone(), &inner);

    let wallet = MemWallet::with_balance(1_000_000);
    let catalogue = ToolCatalogue::new([git_check_status_read_tool_def()]);
    let advertised = [git_check_status_read_tool_schema()];
    let mut ledger = CostLedger::new();
    let outbox = OutboxStore::new();
    let mut w = wiring(&mut ledger, &outbox);

    let report = dispatch_test_run_with_synthetic_identity(
        &wallet,
        region,
        &task(&tenant, agent, "Rcap-allow"),
        &mut w,
        Box::new(ScriptedToolBrain),
        Tools {
            catalogue: &catalogue,
            executor: &gated,
            advertised: &advertised,
        },
    )
    .expect("a granted principal's tool run completes");

    assert_eq!(
        inner.invocations.load(Ordering::SeqCst),
        1,
        "the granted cap let the real read execute"
    );
    assert!(
        report.answer.contains("ci/build = Success"),
        "the answer reflects the tool result: {:?}",
        report.answer
    );
    assert!(report.outcome.0.contains("completed"), "the run completed");
    assert!(report.telemetry.ledger_balanced(), "reserved == settled");
    assert_eq!(report.telemetry.tokens_revoked(), 1, "token torn down");
    assert_eq!(report.telemetry.runs_completed(), 1);
}

#[test]
fn tool_call_is_denied_fail_closed_when_the_principal_lacks_the_required_cap() {
    let tenant = TenantId("01J0HOSTCAP".into());
    let region = Region("fr-par".into());
    let agent = agent_principal(&tenant);

    let identity: Arc<dyn myelin_identity::IdentityService + Send + Sync> =
        Arc::new(rebac_engine(&agent, false));
    let inner = FakeCheckReadExecutor::new();
    let gated = CapEnforcingExecutor::for_git_read_tool(identity, agent.clone(), &inner);

    let wallet = MemWallet::with_balance(1_000_000);
    let catalogue = ToolCatalogue::new([git_check_status_read_tool_def()]);
    let advertised = [git_check_status_read_tool_schema()];
    let mut ledger = CostLedger::new();
    let outbox = OutboxStore::new();
    let mut w = wiring(&mut ledger, &outbox);

    let err = dispatch_test_run_with_synthetic_identity(
        &wallet,
        region,
        &task(&tenant, agent, "Rcap-deny"),
        &mut w,
        Box::new(ScriptedToolBrain),
        Tools {
            catalogue: &catalogue,
            executor: &gated,
            advertised: &advertised,
        },
    )
    .expect_err("a principal without the `pull` grant is DENIED fail-closed");

    assert_eq!(
        inner.invocations.load(Ordering::SeqCst),
        0,
        "the executor is never reached on a denied cap (fail-closed, no execute)"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("cap-enforcement DENY") && msg.contains("pull"),
        "the run aborts with a cap-enforcement deny naming the missing `pull` cap: {msg}"
    );
}
