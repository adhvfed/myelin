//! # HERMETIC cap-enforcement drill — the governed tool's `required_caps` are now REALLY enforced.
//!
//! A [`ToolDef`]'s `required_caps` (the git read tool's `["pull"]`) were declarative-but-UNENFORCED:
//! the driving loop `validate_call`d the untrusted arguments (schema) and dispatched to the executor,
//! but nothing checked that the run's principal was *authorized* to call the tool — only RLS/tenant
//! scoping gated the underlying read. [`CapEnforcingExecutor`] closes that gap: it consults the REAL
//! ReBAC decision engine ([`myelin_identity_service::StoreBackedCheck`], the same
//! [`IdentityService::check`] a human's request flows through) for every declared cap BEFORE the inner
//! executor runs, and DENIES fail-closed (an `Err` that aborts the run with teardown) when the grant
//! is absent.
//!
//! This proves the enforcement end-to-end over the platform loop, NO network / NO DB:
//! - **granted** — with a real `repo:core#pull@agent` grant seeded via the real `write_tuples` path,
//!   the scripted brain's tool call is ALLOWED, the executor runs, and the run completes + tears down;
//! - **denied** — the SAME run with NO grant is DENIED fail-closed: the run aborts, the inner executor
//!   is NEVER reached, and the error names the cap-enforcement deny.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use myelin_agent::{EffectKind, ToolCall, ToolDef, ToolResult};
use myelin_agent_host::{
    dispatch_metered_llm_run, git_check_status_read_tool_def,
    git_check_status_read_tool_schema, CapEnforcingExecutor, LlmRunTask, MicroUsd,
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

// The repo the run reads. The ReBAC engine canonicalises this URN to the `repo:core` tuple key, so
// the seeded grant on `repo:core` authorizes a check on this exact ref.
const REPO: &str = "myelin://01J0HOSTCAP/git/repo/core";
const COMMIT: &str = "capcommit0001";
const AGENT_ID: &str = "psn:host-agent";

/// The fixed per-turn usage the priced turns report.
const USAGE: Usage = Usage::Reported {
    input: 1_000,
    cached_input: 500,
    output: 200,
};

// ───────────────────────── the hermetic doubles (wallet + brain + inner executor) ─────────────────

/// A network-free in-memory [`RunWallet`] double (balance + per-turn debit log, fail-closed).
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
    fn debit(
        &self,
        _tenant: &TenantId,
        amount: MicroUsd,
        _run_id: &str,
    ) -> Result<MicroUsd, WalletError> {
        let mut bal = self.balance.lock().unwrap();
        match bal.checked_sub(amount.0) {
            None => Err(WalletError::InsufficientBalance {
                requested: amount,
                available: MicroUsd(*bal),
            }),
            Some(new) => {
                *bal = new;
                Ok(MicroUsd(new))
            }
        }
    }
}

/// The scripted brain: call the read tool on the first step; on the tool-result step, answer over it.
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

/// A fake-but-real-shaped READ executor: records that it was reached (the "the tool actually ran"
/// witness — it MUST stay 0 on a denied cap).
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
    fn execute(&self, def: &ToolDef, call: &ToolCall) -> Result<ToolResult, ToolExecError> {
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

/// Build the REAL ReBAC engine over an in-memory tuple store. When `grant_pull` is set, seed a genuine
/// `repo:core#pull@<agent>` grant through the real [`TupleStore::write_tuples`] path — under the SAME
/// `(tenant, region)` scope the engine's `check` derives from the agent's own verified token (the
/// principal's home region), so the grant and the check agree.
fn rebac_engine(agent: &Principal, grant_pull: bool) -> StoreBackedCheck {
    let tuples = TupleStore::new(OutboxStore::new());
    if grant_pull {
        // The engine derives its scope from the SUBJECT's own region (tenant-from-token); seed under
        // exactly that scope so the check sees the grant.
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

fn wiring<'a>(
    ledger: &'a mut CostLedger,
    outbox: &'a OutboxStore,
) -> RunSubstrateWiring<'a> {
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
        format!("Read the CI checks for repo {REPO} at commit {COMMIT} and report the build state."),
    )
    .with_max_output_tokens(64)
    .with_now_secs(1000)
}

/// **GRANTED: with a real `pull` grant, the cap gate ALLOWS the tool — the run executes + completes.**
#[test]
fn tool_call_is_allowed_when_the_principal_holds_the_required_cap() {
    let tenant = TenantId("01J0HOSTCAP".into());
    let region = Region("fr-par".into());
    let agent = agent_principal(&tenant);

    let identity: Arc<dyn myelin_identity::IdentityService + Send + Sync> =
        Arc::new(rebac_engine(&agent, /* grant_pull */ true));
    let inner = FakeCheckReadExecutor::new();
    // The cap-enforcing gate wraps the real read executor: every `required_cap` (git `pull`) is
    // checked on the real ReBAC engine before the inner executor runs.
    let gated = CapEnforcingExecutor::for_git_read_tool(identity, agent.clone(), &inner);

    let wallet = MemWallet::with_balance(1_000_000);
    let catalogue = ToolCatalogue::new([git_check_status_read_tool_def()]);
    let advertised = [git_check_status_read_tool_schema()];
    let mut ledger = CostLedger::new();
    let outbox = OutboxStore::new();
    let mut w = wiring(&mut ledger, &outbox);

    let report = dispatch_metered_llm_run(
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

    // The gate ALLOWED the call → the inner executor really ran the read exactly once.
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
    // It tore down cleanly (balanced ledger, trace, token revoked) — a normal metered run.
    assert!(report.telemetry.ledger_balanced(), "reserved == settled");
    assert_eq!(report.telemetry.tokens_revoked(), 1, "token torn down");
    assert_eq!(report.telemetry.runs_completed(), 1);
}

/// **DENIED (fail-closed): with NO grant, the cap gate DENIES the tool — the run aborts, teardown
/// fires, and the inner executor is NEVER reached.** The SAME run as above, only the grant is absent.
#[test]
fn tool_call_is_denied_fail_closed_when_the_principal_lacks_the_required_cap() {
    let tenant = TenantId("01J0HOSTCAP".into());
    let region = Region("fr-par".into());
    let agent = agent_principal(&tenant);

    let identity: Arc<dyn myelin_identity::IdentityService + Send + Sync> =
        Arc::new(rebac_engine(&agent, /* grant_pull */ false)); // NO grant.
    let inner = FakeCheckReadExecutor::new();
    let gated = CapEnforcingExecutor::for_git_read_tool(identity, agent.clone(), &inner);

    let wallet = MemWallet::with_balance(1_000_000);
    let catalogue = ToolCatalogue::new([git_check_status_read_tool_def()]);
    let advertised = [git_check_status_read_tool_schema()];
    let mut ledger = CostLedger::new();
    let outbox = OutboxStore::new();
    let mut w = wiring(&mut ledger, &outbox);

    let err = dispatch_metered_llm_run(
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

    // FAIL-CLOSED: the inner executor was NEVER reached — the deny happened before any read.
    assert_eq!(
        inner.invocations.load(Ordering::SeqCst),
        0,
        "the executor is never reached on a denied cap (fail-closed, no execute)"
    );
    // The abort names the cap-enforcement deny (routed through the same loud teardown path a
    // `validate_call` rejection takes).
    let msg = err.to_string();
    assert!(
        msg.contains("cap-enforcement DENY") && msg.contains("pull"),
        "the run aborts with a cap-enforcement deny naming the missing `pull` cap: {msg}"
    );
}
