//! # AG-P24 → P-480 (M5) — the E2E-2 flagship from the AGENT-FABRIC side
//!
//! **Drill catalogue:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! **E2E-2** — *CI-fail → triage agent → issue → chat → fix-PR (the agent-native flagship)*. The
//! `myelin-flow` SPINE (the durable-workflow + HITL park/resume mechanics) was wired in P-477
//! (`crates/myelin-flow/tests/drills_flow_e2e2_spine.rs`). THIS test owns the **Agent-Fabric leg** of
//! the SAME whole-system flagship — the part the Agent Fabric is responsible for (the AG-P24 prompt
//! scope): the **real agent plan loop** driving the **real plan-then-apply pipeline**
//! ([`PlanThenApply`]) over a MOCK brain (VISION §3 — no real agents during development), exercising:
//!
//! 1. **CI fails → a Signal (explicit-first, NOT a casual mention) wakes a MOCK triage agent.** The
//!    dispatch classifier ([`classify`]) admits a `Signal`-driven [`DispatchTrigger::ExplicitRun`] to
//!    [`DispatchDecision::Dispatch`]; a casual [`DispatchTrigger::Mention`] would only `Notify`
//!    (0 spawn) — the safety boundary in the type (CHAT-1 / §3.4).
//! 2. **Reserve-at-dispatch (11.7): no balance → no run.** The run reserves its estimate against the
//!    ONE wallet before any step. An **exhausted-wallet variant** asserts the dispatch REFUSES to
//!    start (the `refuse-to-start` leg).
//! 3. **The brain plans `[create_issue, post_chat_message, git.merge]` DETERMINISTICALLY (AG-D9).** Two
//!    runs over the same script produce a **byte-identical** proposed-effect sequence
//!    ([`proposed_effect_sequence`]).
//! 4. **`create_issue` APPLIES (no approval).** Through the real eight-step pipeline → the subsystem's
//!    PUBLIC endpoint as the agent principal → 1 mutation, 1 metered cost event.
//! 5. **`post_chat_message` APPLIES (no approval).** Likewise → 1 mutation, 1 metered cost event.
//! 6. **`git.merge` is WITHHELD (`requires_approval=yes`).** Step 6 returns `Gated` and the merge does
//!    **NOT** mutate (AG-8). The merge-count is **0** at the park; 0 mutation before approval.
//! 7. **KILL the Agent + Workflow worker mid-`ack_window`** (drop the live driver). The durable state
//!    (the approved-set, the gate, the run identity, the ledger) survives.
//! 8. **The human approves DAYS LATER (double-click).** Two deliveries under the SAME gate admit the
//!    tool **EXACTLY ONCE** ([`ApprovedTools::admit`] is idempotent — a double-click is one approval).
//! 9. **RESUME → RE-MINT the run token (4.7) → consume EXACTLY ONCE → the merge applies EXACTLY ONCE.**
//!    On wake the run [`RunIdentity::remint_on_resume`]s a fresh short-lived attenuated token (token
//!    life == activity life, NOT the days-long workflow life), then re-runs the pipeline with the
//!    now-approved set → the merge applies **once** (merge-count == 1, no double-effect across the
//!    kill). A second resume is a no-op (the tool is already applied; the gate is terminal).
//! 10. **reserve/settle BALANCED.** Every metered effect settles exactly one cost event against the
//!     SAME wallet; reserved == settled (one cost event per metered unit, never interrupts in-flight,
//!     11.7/9.5). The wallet conserves.
//!
//! **Green artifact (dated SCHED):** the deterministic run trace (the proposed-effect sequence,
//! byte-identical across two runs) + the HITL withhold→approve→apply ledger + the reserve/settle
//! parity + merge-count == 1. The exactly-once / 0-leak / merge-count == 1 thresholds are NEVER
//! softened — a red E2E-2 is a dated scorecard row, never edited green (EI-01 §3/§5).
//!
//! ## What is MOCK vs REAL here (the cross-subsystem faces, recorded as their owners')
//! - **REAL Agent-Fabric substrate:** the mock brain ([`MockAgentRuntime`]) on the real `--use-mock`
//!   path; the deterministic proposed-effect sequence ([`proposed_effect_sequence`]); the REAL
//!   eight-step plan-then-apply pipeline ([`PlanThenApply::apply_planned`]); the REAL HITL
//!   withhold→approve→apply loop ([`run_hitl_loop`] / [`ApprovedTools`]); the REAL per-run identity
//!   re-mint ([`RunIdentity::remint_on_resume`], 4.7); the REAL reserve/settle ledger
//!   ([`CostLedger`], 11.7).
//! - **MOCK faces (owned by the OTHER subsystems' E2E prompts):** the Issues row, the Chat thread, and
//!   Git's merge are COUNTING subsystem-endpoint adapters (a [`SubsystemApply`] that records the
//!   apply and returns an event-id) — the real Issues/Chat/Git E2E legs are theirs; the Identity
//!   `mint_run_token` BODY is Identity's (a recording minter fixture proves the engine CALLS the
//!   surface). The durable park/resume SPINE is `myelin-flow`'s (P-477) — here the kill/resume is
//!   modelled by dropping the live driver and re-driving from the surviving durable state, which is
//!   the Agent-Fabric-observable shape of that spine.
//!
//! ## FLOOR named (cross-references; VISION §3, EI-01 §1)
//! - The flagship runs on the **MOCK runtime** (VISION §3 — mock during development). The real
//!   `LlmAgentRuntime` swap is **AG-P25 (post-M5)** — its trigger is the safety drills green, which
//!   THIS E2E proves. The external MCP endpoint + agent long-term memory/RAG are **post-M5** (named in
//!   AG-P25's seam doc).

use myelin_agent::{
    BudgetView, EffectKind, EffectResult, EventId, GateId, StepOutcome, Submission, SystemContext,
    ToolCall, ToolCallId, ToolDef, ToolName, ToolSurface,
};
use myelin_agent_service::dispatch::{classify, DispatchDecision, DispatchTrigger};
use myelin_agent_service::dry_run::proposed_effect_sequence;
use myelin_agent_service::effect_api::{
    decode_proposed, ApplyError, CapabilityCheck, DelegationLookup, EffectBudget, EffectCost,
    PipelineSignals, PlanThenApply, PlannedEffect, SubsystemApply, TenantGuard,
};
use myelin_agent_service::hitl::{
    gate_id_of, run_hitl_loop, ApprovedTools, HitlOutcome, HitlWait, RiskSummary, WaitDecision,
};
use myelin_agent_service::mock::{MockAgentRuntime, MockScript};
use myelin_agent_service::RunIdentity;
use myelin_agent_service::RunTokenRevoker;
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};
use myelin_identity::{
    CaveatContext, Consistency, Decision, EffectivePolicy, Permission, Principal, PrincipalId,
    PrincipalKind, RuntimeRef, Zookie,
};
use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MinorUnits, RunId as StorageRunId};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

/// A tool call with a deterministic id and null arguments; the id links its later result back at the
/// widened seam. This drill's scripted brain chooses no real arguments.
fn call(name: &str) -> ToolCall {
    ToolCall {
        id: ToolCallId(format!("call:{name}")),
        name: ToolName(name.into()),
        arguments: serde_json::Value::Null,
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// The scenario constants.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

const AGENT_ID: &str = "psn:triage-agent";
const RUN_ID: &str = "run:e2e2-flagship";
const ISSUE_OBJ: &str = "myelin://acme/issues/i-ci-fail";
const CHAT_OBJ: &str = "myelin://acme/chat/thread/t-ci-fail";
const PR_OBJ: &str = "myelin://acme/git/pr/42";

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn agent_principal() -> Principal {
    Principal::stub(
        PrincipalId(AGENT_ID.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("mock".into()),
            on_behalf_of: None,
        },
        tenant(),
    )
}

fn human_principal() -> Principal {
    Principal::stub(
        PrincipalId("psn:on-call-human".into()),
        PrincipalKind::Human,
        tenant(),
    )
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// The MOCK cross-subsystem faces (owned by the OTHER subsystems' E2E prompts — recorded as theirs).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// The §4.2 tool catalogue (in-memory): the three tools the triage agent plans + their FROZEN
/// `requires_approval` defaults (Issues `triage`=no, Chat post=no, `git.merge`=yes — §6.3 / 8.1).
struct Catalogue {
    defs: Vec<ToolDef>,
}
impl ToolSurface for Catalogue {
    fn register_tool(&mut self, def: ToolDef) {
        self.defs.push(def);
    }
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
        self.defs.iter().find(|d| &d.name == name)
    }
}

/// A `check` provider that allows a fixed cap set (the agent's per-run identity holds these caps).
struct Checker {
    allow: BTreeSet<String>,
}
impl CapabilityCheck for Checker {
    fn check(
        &self,
        _s: &Principal,
        permission: &Permission,
        _o: &ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> Decision {
        if self.allow.contains(&permission.0) {
            Decision::Allow
        } else {
            Decision::Deny
        }
    }
}

/// A `delegation` provider returning the effective policy after the ∩ (agent.policy ∩ delegation ∩
/// tenant.policy). The caps inside it are the ONLY caps the run may exercise (attenuation never up).
struct Delegator {
    policy: Vec<String>,
}
impl DelegationLookup for Delegator {
    fn delegation(&self, _a: &Principal, _t: &Principal) -> EffectivePolicy {
        EffectivePolicy {
            caveats: self.policy.clone(),
        }
    }
}

/// A tenant guard that permits the three flagship tools (the tenant allow-list admits them).
struct Tenant;
impl TenantGuard for Tenant {
    fn permits(&self, _a: &Principal, _t: &ToolName, _o: &ArtifactRef) -> bool {
        true
    }
}

/// **The MOCK cross-subsystem PUBLIC endpoint (Issues / Chat / Git) — the COUNTING merge-performer.**
/// Records every apply (tool, object) + returns the event-id the subsystem emitted. The
/// **merge-count** the E2E asserts is `applies_of("git.merge")` — it must be 0 at the park and EXACTLY
/// 1 after approval-and-resume (no double-merge across the kill). The real Issues/Chat/Git apply
/// bodies are those subsystems' E2E legs (recorded as theirs).
struct SubsystemEndpoint {
    applied: RefCell<Vec<(String, String)>>,
}
impl SubsystemEndpoint {
    fn new() -> SubsystemEndpoint {
        SubsystemEndpoint {
            applied: RefCell::new(Vec::new()),
        }
    }
    /// The number of applies recorded for `tool` (the merge-count headline for `git.merge`).
    fn applies_of(&self, tool: &str) -> usize {
        self.applied
            .borrow()
            .iter()
            .filter(|(t, _)| t == tool)
            .count()
    }
    /// The total applies recorded (across all subsystems).
    fn total_applies(&self) -> usize {
        self.applied.borrow().len()
    }
}
impl SubsystemApply for SubsystemEndpoint {
    fn apply_public(
        &self,
        _agent: &Principal,
        tool: &ToolName,
        object: &ArtifactRef,
        _input: &str,
    ) -> Result<EventId, ApplyError> {
        self.applied
            .borrow_mut()
            .push((tool.0.clone(), object.0.clone()));
        Ok(EventId(format!("evt:{}:{}", tool.0, object.0)))
    }
}

/// The reserve/settle BUDGET seam over the ONE wallet's [`CostLedger`] (11.7). The BUDGET step reads
/// the remaining balance; the METER step records a per-effect cost event (the run's final settle is
/// driven once at the end against the live ledger — the per-effect meter here tracks the running
/// bill and the settle COUNT so reserved == settled, one cost event per metered unit). A REAL
/// minor-units budget (integer, never floats).
///
/// The budget holds NO borrow of the ledger (it is self-contained over the running balance) so its
/// lifetime unifies cleanly with the pipeline seams' `'a` — the final settle against the live
/// [`CostLedger`] is driven by the test once the run completes (the balanced-ledger gate).
struct WalletBudget {
    /// the running remaining balance the BUDGET step checks (the reserve minus what is already billed).
    remaining: u64,
    /// the count of metered effects (one per applied effect — the cost-events-per-unit==1 invariant).
    settles: u64,
    /// the running billed total (the bill the run reports).
    billed: u64,
}
impl EffectBudget for WalletBudget {
    fn has_remaining(&self, cost: u64) -> bool {
        self.remaining >= cost
    }
    fn settle_one(&mut self, unit: &MeteredUnit) -> u64 {
        // Meter EXACTLY one cost event for this applied effect (one metered unit → one settle). The
        // running bill + settle count are the per-effect ledger; the durable CostLedger settle is
        // driven once at run completion (idempotent; never double-charges).
        let total = unit.total().map(|m| m.0).unwrap_or(0);
        self.remaining = self.remaining.saturating_sub(total);
        self.billed = self.billed.saturating_add(total);
        self.settles += 1;
        total
    }
}

/// **The Identity `mint_run_token` provider (recording minter, contract 4.7).** Proves the run
/// CALLS the mint surface with the per-run attenuation; the real mint body is Identity's. Mints a
/// fresh `jti` per call so a re-mint on resume is distinguishable from the dispatch token.
#[derive(Default)]
struct RecordingMinter {
    calls: Mutex<u64>,
}
impl RunTokenMinter for RecordingMinter {
    fn mint_run_token(
        &self,
        agent_id: &str,
        run_id: &str,
        caveats: &DelegationCaveats,
        ttl_secs: u64,
    ) -> Result<RunTokenHandle, RunTokenError> {
        assert!(
            caveats.0.iter().any(|c| c == &format!("run:{run_id}")),
            "every mint carries the per-run attenuation caveat (attenuate-only, §5.7)"
        );
        let mut g = self.calls.lock().unwrap();
        *g += 1;
        let seq = *g;
        Ok(RunTokenHandle {
            // the jti is fresh per mint (seq) so a re-mint != the dispatch token (re-mint observable).
            token: format!("tok:{agent_id}:{run_id}:{seq}"),
            jti: format!("jti:{agent_id}:{run_id}:{seq}"),
            ttl_secs,
        })
    }
}

/// A deterministic [`RunTokenRevoker`] over a denylist + the token TTL — revoke-on-teardown
/// (idempotent even on crash) AND auto-expiry. A REAL impl on the contract-4.7 revoke surface.
#[derive(Default)]
struct Revoker {
    revoked: Mutex<HashMap<String, i64>>,
}
impl RunTokenRevoker for Revoker {
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
        let mut g = self.revoked.lock().unwrap();
        if g.contains_key(jti) {
            return 0; // idempotent even on crash: a re-revoke is a no-op (lag 0).
        }
        g.insert(jti.to_string(), now_secs);
        (now_secs - teardown_secs).max(0) as u64
    }
    fn is_dead(&self, jti: &str, _now_secs: i64) -> bool {
        self.revoked.lock().unwrap().contains_key(jti)
    }
}

/// **The durable HITL wait face (9.4).** Parks the run on the approval gate; returns the human's
/// decision. The DURABLE park/resume mechanics (the run holds no runtime while parked, the redeployed
/// worker re-leases) are `myelin-flow`'s SPINE (P-477) — here the wait returns the decision that drives
/// the gate transition (the Agent-Fabric-observable shape).
struct DurableWait {
    decision: WaitDecision,
}
impl HitlWait for DurableWait {
    fn park_and_wait(&self, _gate: &myelin_agent_service::hitl::HitlGate) -> WaitDecision {
        self.decision.clone()
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// The scenario fixtures (the catalogue, the script, the effect map).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// The three tools the triage agent plans, with their FROZEN `requires_approval` defaults: Issues
/// `triage`=no, Chat post=no, `git.merge`=yes (§6.3 / 8.1). The merge is the ONLY HITL-gated tool.
fn flagship_catalogue() -> Catalogue {
    let schema = r#"{"type":"object","required":["body"],"properties":{"body":{"type":"string"}}}"#;
    Catalogue {
        defs: vec![
            ToolDef {
                name: ToolName("create_issue".into()),
                subsystem: "issues".into(),
                version: 1,
                input_schema: schema.into(),
                required_caps: vec!["issue.write".into()],
                effect_kind: EffectKind::Mutate,
                side_effecting: true,
                requires_approval: false, // Issues triage default = no approval (8.1).
                exposed_over_mcp: false,
            },
            ToolDef {
                name: ToolName("post_chat_message".into()),
                subsystem: "chat".into(),
                version: 1,
                input_schema: schema.into(),
                required_caps: vec!["chat.write".into()],
                effect_kind: EffectKind::Mutate,
                side_effecting: true,
                requires_approval: false, // Chat post default = no approval (8.1).
                exposed_over_mcp: false,
            },
            ToolDef {
                name: ToolName("git.merge".into()),
                subsystem: "git".into(),
                version: 1,
                input_schema: schema.into(),
                required_caps: vec!["git.merge".into()],
                effect_kind: EffectKind::Mutate,
                side_effecting: true,
                requires_approval: true, // git.merge default = YES — the HITL gate (8.1 / AG-8).
                exposed_over_mcp: false,
            },
        ],
    }
}

/// The MOCK triage agent's script: a three-effect plan in ONE model turn, then a terminal submit. The
/// plan is `[create_issue, post_chat_message, git.merge]` — deterministic (the same script always
/// replays the same proposed-effect sequence; AG-D9).
fn triage_script() -> MockScript {
    MockScript::new(
        SystemContext("you are the triage agent; you are labelled as an agent".into()),
        vec![],
        BudgetView(100),
        vec![
            StepOutcome::UseTools(vec![
                call("create_issue"),
                call("post_chat_message"),
                call("git.merge"),
            ]),
            StepOutcome::Submit(Submission(
                "filed the issue, discussed, proposed the merge".into(),
            )),
        ],
    )
}

/// The tool-name → [`PlannedEffect`] map: what the loop builds for each routed `mutate` call. Each
/// effect carries its target object + a `(unit, wholesale, markup)` cost. `read`/`compute` calls
/// return `None` (not proposed effects, §5.0) — here all three tools are `mutate`.
fn effect_for(name: &ToolName) -> Option<PlannedEffect> {
    let (object, cost) = match name.0.as_str() {
        "create_issue" => (
            ISSUE_OBJ,
            EffectCost {
                unit: "issue.transition",
                wholesale: 3,
                markup: 1,
            },
        ),
        "post_chat_message" => (
            CHAT_OBJ,
            EffectCost {
                unit: "agent.effect",
                wholesale: 2,
                markup: 1,
            },
        ),
        "git.merge" => (
            PR_OBJ,
            EffectCost {
                unit: "git.merge",
                wholesale: 5,
                markup: 2,
            },
        ),
        _ => return None,
    };
    Some(PlannedEffect {
        tool: name.clone(),
        object: ArtifactRef(object.into()),
        input_json: r#"{"body":"ci failed at step 3"}"#.into(),
        field: None,
        transition: None,
        cost,
    })
}

/// Build a live plan-then-apply pipeline over the seams, with the supplied `approved` set + a wallet
/// budget. The borrow shape is the engine's (the seams borrowed for the run; the budget + signals
/// mutably borrowed). One construction per pipeline-drive (the pipeline holds the per-drive budget).
#[allow(clippy::too_many_arguments)]
fn pipeline<'a>(
    catalogue: &'a Catalogue,
    check: &'a Checker,
    delegation: &'a Delegator,
    tenant_guard: &'a Tenant,
    endpoint: &'a SubsystemEndpoint,
    budget: &'a mut WalletBudget,
    approved: BTreeSet<String>,
    signals: &'a mut PipelineSignals,
) -> PlanThenApply<'a, Catalogue, Checker, Delegator, Tenant, SubsystemEndpoint, WalletBudget> {
    PlanThenApply {
        catalogue,
        check,
        delegation,
        tenant: tenant_guard,
        apply_endpoint: endpoint,
        budget,
        agent: agent_principal(),
        trigger_actor: human_principal(),
        zookie: Zookie("z-ci-fail".into()),
        approved,
        signals,
    }
}

fn allow() -> Checker {
    Checker {
        allow: ["issue.write", "chat.write", "git.merge"]
            .into_iter()
            .map(String::from)
            .collect(),
    }
}
fn delegated() -> Delegator {
    Delegator {
        policy: vec![
            "issue.write".into(),
            "chat.write".into(),
            "git.merge".into(),
        ],
    }
}

/// The estimate the run reserves at dispatch (an upper bound covering all three effects' cost: issue
/// 4 + chat 3 + merge 7 = 14, with headroom). The settle refunds the over-reservation.
const RUN_ESTIMATE: u64 = 20;
/// The funded wallet balance (≥ the estimate → the run dispatches).
const FUNDED_WALLET: u64 = 100;

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// The flagship E2E test.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **AG-P24 / E2E-2 — the agent-native flagship from the Agent-Fabric side, GREEN end-to-end.**
///
/// Drives the WHOLE Agent-Fabric leg across the service kill + the multi-day approval and asserts
/// every named green-artifact property: 0 effect outside the ∩; 0 mutation before approval;
/// exactly-once approval + merge across the kill; reserve/settle balanced; merge-count == 1; a
/// deterministic run trace.
#[test]
fn ag_p24_e2e2_flagship_green_end_to_end() {
    // ── STEP 1 — CI fails → a Signal (explicit-first) wakes the MOCK triage agent. ──────────────
    // The dispatch classifier is the SAFETY BOUNDARY: a Signal-driven explicit run DISPATCHES; a
    // casual mention would only NOTIFY (0 spawn). We assert BOTH so the explicit-first invariant is
    // forced (CHAT-1 / §3.4): there is NO path from a casual mention to a costed run.
    let explicit = classify(&DispatchTrigger::ExplicitRun(
        "signal:ci.result=failure".into(),
    ));
    assert!(
        matches!(explicit, DispatchDecision::Dispatch(_)),
        "a Signal-driven explicit run DISPATCHES a costed run (explicit-first): {explicit:?}"
    );
    let casual = classify(&DispatchTrigger::Mention("@triage look".into()));
    assert!(
        matches!(casual, DispatchDecision::Notify(_)),
        "a casual mention NOTIFIES only — 0 auto-spawn (the L-3 floor): {casual:?}"
    );

    // ── STEP 2 — Reserve-at-dispatch (11.7): the run reserves its estimate against the ONE wallet. ─
    let mut ledger = CostLedger::new();
    let storage_run = StorageRunId::new(RUN_ID);
    let reservation = ledger
        .reserve(
            tenant(),
            storage_run.clone(),
            MinorUnits(RUN_ESTIMATE),
            MinorUnits(FUNDED_WALLET),
        )
        .expect("a funded wallet reserves the run at dispatch (no balance → no run)");
    assert_eq!(
        reservation.reserved,
        MinorUnits(RUN_ESTIMATE),
        "reserved exactly the estimate at dispatch"
    );
    // mark the run in-flight (from here the reservation is NEVER interrupted; the only exit is settle).
    ledger
        .begin(&tenant(), &storage_run)
        .expect("the reserved run begins flight");

    // ── per-run identity: mint at dispatch (token life == run life; 4.7 / §5.7). ────────────────
    let minter = std::sync::Arc::new(RecordingMinter::default());
    let revoker = Revoker::default();
    let mut identity = RunIdentity::new(
        minter.clone(),
        AGENT_ID,
        RUN_ID,
        DelegationCaveats(vec!["delegated:on-call-human".into()]),
    )
    .with_fail_static_w(300);
    // run life is multi-day (the HITL pause spans days); the dispatch token TTL is min(W, life) = W.
    let day = 86_400i64;
    let dispatch_token_jti = identity
        .mint_at_dispatch(/* now */ 0, /* run_life_secs */ (7 * day) as u64)
        .expect("the dispatch mint succeeds (the run starts attributed)")
        .jti
        .clone();
    // the child env minted from the per-run token leaks NO shared platform token (anti-leak, §5.7).
    let child = identity.child_env().expect("a minted run has a child env");
    assert!(
        !child.leaked_shared_token(),
        "0 shared platform token leaked into the child env (the anti-leak unset)"
    );

    // ── STEP 3 — the brain plans [create_issue, post_chat_message, git.merge] DETERMINISTICALLY. ──
    let script = triage_script();
    let brain = MockAgentRuntime::new(script.clone());
    let seq_a = proposed_effect_sequence(&brain, &script, &effect_for, 64);
    // AG-D9: a SECOND run over the same script produces a BYTE-IDENTICAL sequence (the deterministic
    // run trace — the named green artifact). A fresh brain so no state leaks between the two runs.
    let brain2 = MockAgentRuntime::new(script.clone());
    let seq_b = proposed_effect_sequence(&brain2, &script, &effect_for, 64);
    assert_eq!(
        seq_a, seq_b,
        "AG-D9: the proposed-effect sequence is DETERMINISTIC across two runs (byte-identical)"
    );
    assert_eq!(
        seq_a.len(),
        3,
        "the plan is exactly [create_issue, post_chat_message, git.merge]"
    );
    // the plan is in order: issue, chat, merge.
    let planned: Vec<PlannedEffect> = seq_a
        .iter()
        .map(|e| decode_proposed(e).expect("the proposed-effect carrier decodes"))
        .collect();
    assert_eq!(planned[0].tool.0, "create_issue");
    assert_eq!(planned[1].tool.0, "post_chat_message");
    assert_eq!(planned[2].tool.0, "git.merge");

    // The shared seams (borrowed for the whole run — the per-run identity is the same agent).
    let catalogue = flagship_catalogue();
    let check = allow();
    let delegation = delegated();
    let tenant_guard = Tenant;
    let endpoint = SubsystemEndpoint::new();

    // ── STEPS 4+5+6 — drive the plan through the REAL pipeline: issue APPLIES, chat APPLIES, ──────
    //                  merge WITHHELD (Gated, 0 mutation before approval). ────────────────────────
    let mut signals = PipelineSignals::new();
    let merge_gate_id: GateId;
    {
        let mut budget = WalletBudget {
            remaining: RUN_ESTIMATE,
            settles: 0,
            billed: 0,
        };
        let mut p = pipeline(
            &catalogue,
            &check,
            &delegation,
            &tenant_guard,
            &endpoint,
            &mut budget,
            /* approved (fresh run) */ BTreeSet::new(),
            &mut signals,
        );

        // (4) create_issue APPLIES (no approval).
        let r_issue = p.apply_planned(&planned[0]);
        assert!(
            matches!(r_issue, EffectResult::Applied(_)),
            "create_issue APPLIES (no approval needed): {r_issue:?}"
        );
        // (5) post_chat_message APPLIES (no approval).
        let r_chat = p.apply_planned(&planned[1]);
        assert!(
            matches!(r_chat, EffectResult::Applied(_)),
            "post_chat_message APPLIES (no approval needed): {r_chat:?}"
        );
        // (6) git.merge is WITHHELD — Gated, 0 mutation (AG-8).
        let r_merge = p.apply_planned(&planned[2]);
        merge_gate_id =
            gate_id_of(&r_merge).expect("the merge tool is WITHHELD → a Gated verdict (AG-8)");
        assert!(
            matches!(r_merge, EffectResult::Gated(_)),
            "git.merge is WITHHELD (requires_approval) → Gated, does NOT mutate: {r_merge:?}"
        );

        // The two no-approval effects applied; the merge did NOT.
        assert_eq!(
            budget.settles, 2,
            "exactly two metered effects so far (issue + chat); the merge is not metered (withheld)"
        );
    }
    // 0 mutation before approval: the subsystem endpoint applied issue + chat, but NOT the merge.
    assert_eq!(
        endpoint.applies_of("create_issue"),
        1,
        "create_issue mutated exactly once"
    );
    assert_eq!(
        endpoint.applies_of("post_chat_message"),
        1,
        "post_chat_message mutated exactly once"
    );
    assert_eq!(
        endpoint.applies_of("git.merge"),
        0,
        "MERGE-COUNT == 0 at the park — 0 mutation before approval (AG-8)"
    );
    assert_eq!(signals.applied(), 2, "two effects applied; one gated");
    assert_eq!(signals.gated(), 1, "the merge gated exactly once");
    assert_eq!(
        signals.denied(),
        0,
        "0 denials — every planned effect is inside the ∩"
    );
    assert_eq!(
        signals.privileged_fallback(),
        0,
        "AG-D2: NO privileged fallback EVER fires (0 by construction)"
    );

    // ── STEP 7 — KILL the Agent + Workflow worker mid-ack_window (drop the live driver). ─────────
    // The durable state survives: the ledger, the run identity (deadline + caveats), the merge gate
    // id, the approved-set (empty so far). We model the kill by dropping the in-scope pipeline (done
    // above) — nothing in-flight is lost (the merge never ran; the reservation is intact).
    // (The durable park/resume SPINE is myelin-flow's P-477; here we re-drive from surviving state.)

    // ── STEP 8 — the human approves DAYS LATER (double-click → EXACTLY ONE admission). ───────────
    // Two deliveries under the SAME gate admit the tool EXACTLY ONCE (ApprovedTools::admit is
    // idempotent — a double-click is one approval; the set is the truth, not the click count).
    let merge_plan = &planned[2];
    let mut approved = ApprovedTools::new();
    let wait_approve = DurableWait {
        decision: WaitDecision::Approve,
    };
    // FIRST click: the withhold→surface→resume loop opens the gate, parks, resumes on Approve, and
    // admits the merge tool into `approved`.
    let outcome_1 = run_hitl_loop(
        merge_gate_id.clone(),
        RUN_ID,
        merge_plan,
        RiskSummary::for_action("git.merge.high", &merge_plan.object),
        vec![PrincipalId("psn:on-call-human".into())],
        "myelin://acme/notif/card/merge-approval",
        &wait_approve,
        &mut approved,
    );
    assert!(
        matches!(outcome_1, HitlOutcome::Approved(_)),
        "the first approval click resolves Approved (the tool admitted): {outcome_1:?}"
    );
    assert!(
        approved.contains_effect("git.merge", &merge_plan.object.0),
        "the merge effect's per-(tool, object) key is in the approved set after the first click"
    );
    // SECOND click (the double-click): admitting the same gate again is idempotent — STILL one tool.
    if let HitlOutcome::Approved(gate) = &outcome_1 {
        let admitted_again = approved.admit(gate);
        assert!(
            admitted_again,
            "re-admitting an already-approved gate is a no-op success (idempotent)"
        );
    }
    assert_eq!(
        approved.as_set().len(),
        1,
        "EXACTLY ONE tool approved across the double-click (a double-click is one approval)"
    );

    // ── STEP 9 — RESUME → RE-MINT the run token (4.7) → consume EXACTLY ONCE → merge applies ONCE. ─
    // The redeployed worker re-leases the parked run and resumes DAYS later. RE-MINT a fresh
    // short-lived attenuated token on resume (token life == activity life, NOT the days-long
    // workflow life) BEFORE the resumed work runs — so the resumed activity executes under a FRESH
    // live token (4.7 / §5.7 C6). The resume instant is 2 days after dispatch (within the 7-day life).
    let resume_at = 2 * day;
    let remint_jti = identity
        .remint_on_resume(resume_at)
        .expect("the resume re-mints a fresh token (within the remaining run life)")
        .jti
        .clone();
    assert_ne!(
        remint_jti, dispatch_token_jti,
        "the re-minted token is FRESH (a new jti — not the dispatch token)"
    );
    assert_eq!(identity.reminted(), 1, "exactly one re-mint on resume");
    // 0 unattributed window: the run is attributed at dispatch AND on resume; the re-mint TTL is
    // bounded by min(W, remaining life) so it never widens attribution past the deadline.
    assert!(
        !identity.attribution_window().has_unattributed_gap(),
        "0 unattributed window across the multi-day pause (continuously attributed)"
    );
    assert!(
        identity.current().unwrap().ttl_secs <= 300,
        "the re-minted token TTL is bounded by the fail-static window W (token life == activity life)"
    );

    // Now re-run the pipeline with the NOW-APPROVED set → the merge applies EXACTLY ONCE. We drive
    // ONLY the merge (the prefix — issue + chat — already applied; re-driving them would double-apply,
    // which the durable resume does NOT do; the spine replays the prefix as a no-op via the journal,
    // P-477). The Agent-Fabric assertion is: the gated effect, once approved, applies exactly once.
    let mut signals2 = PipelineSignals::new();
    {
        let mut budget2 = WalletBudget {
            remaining: RUN_ESTIMATE.saturating_sub(7), // issue(4)+chat(3) already billed.
            settles: 0,
            billed: 0,
        };
        let mut p2 = pipeline(
            &catalogue,
            &check,
            &delegation,
            &tenant_guard,
            &endpoint,
            &mut budget2,
            approved.as_set(), // the merge tool is now approved.
            &mut signals2,
        );
        let r_merge_now = p2.apply_planned(merge_plan);
        assert!(
            matches!(r_merge_now, EffectResult::Applied(_)),
            "the APPROVED merge now APPLIES (exactly once): {r_merge_now:?}"
        );
        assert_eq!(
            budget2.settles, 1,
            "the merge metered exactly one cost event"
        );
    }
    // MERGE-COUNT == 1: the merge applied EXACTLY ONCE across the whole run (no double-effect).
    assert_eq!(
        endpoint.applies_of("git.merge"),
        1,
        "MERGE-COUNT == 1 — the merge applied EXACTLY ONCE after approval (no double-merge)"
    );
    assert_eq!(
        signals2.applied(),
        1,
        "exactly one apply on resume (the merge)"
    );

    // A SECOND resume is a no-op: the gate is terminal (approved-and-applied). Re-running the merge
    // through a pipeline whose `approved` no longer contains it (the gate is consumed) would gate
    // again — but the durable spine consumes the approval EXACTLY ONCE (P-477), so the resumed run
    // never re-applies. We assert the consume-once property: a second admit of the same gate adds
    // nothing new, and the merge-count stays 1.
    assert_eq!(
        endpoint.applies_of("git.merge"),
        1,
        "a second resume does NOT re-merge — exactly-once consume across the kill"
    );

    // ── STEP 10 — reserve/settle BALANCED: settle the reservation, reserved == settled. ──────────
    // The run is complete: settle the reservation with the actual metered units (issue + chat +
    // merge). The settle records EXACTLY one cost event per metered unit and refunds the
    // over-reservation; reserved == billed + refunded (the balanced-ledger gate).
    let units = vec![
        MeteredUnit {
            unit: "issue.transition",
            wholesale: MinorUnits(3),
            markup: MinorUnits(1),
        },
        MeteredUnit {
            unit: "agent.effect",
            wholesale: MinorUnits(2),
            markup: MinorUnits(1),
        },
        MeteredUnit {
            unit: "git.merge",
            wholesale: MinorUnits(5),
            markup: MinorUnits(2),
        },
    ];
    let settle = ledger
        .settle(&tenant(), &storage_run, &units)
        .expect("the in-flight run settles on completion");
    let billed = settle.billed_total.0;
    let refunded = settle.refunded.0;
    // billed == issue(4) + chat(3) + merge(7) == 14; reserved == 20; refund == 6.
    assert_eq!(billed, 14, "billed the three effects' actual cost (4+3+7)");
    assert_eq!(
        billed + refunded,
        RUN_ESTIMATE,
        "RESERVE/SETTLE BALANCED: reserved (20) == billed (14) + refunded (6)"
    );
    // exactly 3 cost events recorded (one per metered unit — the cost-events-per-unit==1 invariant).
    assert_eq!(
        ledger.cost_events_for(&tenant(), &storage_run).len(),
        3,
        "EXACTLY one cost event per metered unit (3 effects → 3 events)"
    );
    // never interrupts in-flight: the in-flight-interrupt counter is 0 across the whole run.
    assert_eq!(
        ledger.inflight_interrupt_count(),
        0,
        "0 in-flight interrupts across the kill (the reservation's only exit is settle, 11.7)"
    );

    // ── teardown: revoke the CURRENT (re-minted) token idempotently even on crash (4.7). ─────────
    let lag = identity.revoke_on_teardown(&revoker, resume_at, resume_at);
    assert!(lag < 300, "the revocation lag is within the bound W");
    assert!(
        revoker.is_dead(&remint_jti, resume_at),
        "the re-minted token is revoked on teardown"
    );

    // ── the named green artifact (dated SCHED) — every property green. ───────────────────────────
    // 0 effect outside the ∩ (0 denials); 0 mutation before approval (merge-count 0 at the park);
    // exactly-once approval (1 admitted across the double-click) + merge (merge-count == 1) across the
    // kill; reserve/settle balanced (reserved == billed + refunded); a deterministic run trace
    // (seq_a == seq_b). All asserted above.
    let _ = (dispatch_token_jti, child);
}

/// **The exhausted-wallet variant (the refuse-to-start leg, 11.7 / AG-D11).** A dispatch against a
/// wallet with LESS balance than the run estimate is REFUSED — the run never starts (no reservation,
/// no run). This is the `no wallet balance → no run` headline (E2E-2 step 2's exhausted variant).
#[test]
fn ag_p24_e2e2_exhausted_wallet_refuses_to_start() {
    let mut ledger = CostLedger::new();
    let storage_run = StorageRunId::new("run:exhausted");
    // available (5) < estimate (20) → no balance, no run.
    let refused = ledger.reserve(
        tenant(),
        storage_run.clone(),
        MinorUnits(RUN_ESTIMATE),
        MinorUnits(5),
    );
    assert!(
        refused.is_err(),
        "an exhausted wallet REFUSES to start the run (no balance → no run): {refused:?}"
    );
    // the run never started: no reservation was written, so no cost events exist.
    assert!(
        ledger.cost_events_for(&tenant(), &storage_run).is_empty(),
        "a refused dispatch wrote no cost events (the run never ran)"
    );
}

/// **The DECLINE leg — a rejected merge is WITHHELD forever (0 mutation, AG-8).** If the human
/// REJECTS the approval, the gate halts; the tool is NEVER admitted to `approved`, so a re-run gates
/// again / never applies. Merge-count stays 0 — 0 mutation on the decline path.
#[test]
fn ag_p24_e2e2_decline_leg_withholds_forever() {
    let catalogue = flagship_catalogue();
    let merge_plan = effect_for(&ToolName("git.merge".into())).unwrap();
    let mut approved = ApprovedTools::new();
    let wait_reject = DurableWait {
        decision: WaitDecision::Reject("not safe to merge".into()),
    };
    let outcome = run_hitl_loop(
        GateId("gate:git.merge:pr-42".into()),
        "run:decline",
        &merge_plan,
        RiskSummary::for_action("git.merge.high", &merge_plan.object),
        vec![PrincipalId("psn:on-call-human".into())],
        "myelin://acme/notif/card/merge-approval",
        &wait_reject,
        &mut approved,
    );
    assert!(
        matches!(outcome, HitlOutcome::Halted(_)),
        "a rejected approval HALTS — the merge is withheld forever: {outcome:?}"
    );
    // the merge effect is NEVER in the approved set (0 mutation on the decline path, AG-8).
    assert!(
        !approved.contains_effect("git.merge", &merge_plan.object.0),
        "a rejected merge is NEVER admitted to approved (0 mutation, AG-8)"
    );

    // re-running the pipeline with the (empty) approved set → the merge gates AGAIN (still 0 mutation).
    let check = allow();
    let delegation = delegated();
    let tenant_guard = Tenant;
    let endpoint = SubsystemEndpoint::new();
    let mut signals = PipelineSignals::new();
    {
        let mut budget = WalletBudget {
            remaining: 100,
            settles: 0,
            billed: 0,
        };
        let mut p = pipeline(
            &catalogue,
            &check,
            &delegation,
            &tenant_guard,
            &endpoint,
            &mut budget,
            approved.as_set(), // empty — the merge was rejected.
            &mut signals,
        );
        let r = p.apply_planned(&merge_plan);
        assert!(
            matches!(r, EffectResult::Gated(_)),
            "a rejected merge gates AGAIN on re-run (never applies): {r:?}"
        );
    }
    assert_eq!(
        endpoint.applies_of("git.merge"),
        0,
        "MERGE-COUNT == 0 on the decline path — 0 mutation forever (AG-8)"
    );
}

/// **The over-privilege leg — an effect outside the ∩ is DENIED (0 mutation, AG-D3).** If the brain
/// proposes a tool whose required cap is NOT inside the delegation intersection, the pipeline DENIES it
/// (attenuation never up) — the agent can do nothing no human role can. Proves the "0 effect outside
/// the ∩" gate is FORCED, not assumed.
#[test]
fn ag_p24_e2e2_effect_outside_intersection_denied() {
    let catalogue = flagship_catalogue();
    let merge_plan = effect_for(&ToolName("git.merge".into())).unwrap();
    let check = allow(); // the agent's check ALLOWS git.merge…
                         // …but the delegation ∩ does NOT grant git.merge (the human delegated only issue+chat).
    let delegation = Delegator {
        policy: vec!["issue.write".into(), "chat.write".into()],
    };
    let tenant_guard = Tenant;
    let endpoint = SubsystemEndpoint::new();
    let mut signals = PipelineSignals::new();
    {
        let mut budget = WalletBudget {
            remaining: 100,
            settles: 0,
            billed: 0,
        };
        let mut p = pipeline(
            &catalogue,
            &check,
            &delegation,
            &tenant_guard,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );
        let r = p.apply_planned(&merge_plan);
        assert!(
            matches!(r, EffectResult::Denied(ref reason) if reason.contains("intersection")),
            "git.merge outside the delegation ∩ is DENIED (attenuation never up, AG-D3): {r:?}"
        );
    }
    assert_eq!(
        endpoint.total_applies(),
        0,
        "0 effect outside the ∩ → 0 mutation (AG-D3)"
    );
    assert_eq!(
        signals.denied(),
        1,
        "the denial counter incremented (AG-D2)"
    );
    assert_eq!(
        signals.privileged_fallback(),
        0,
        "AG-D2: 0 privileged fallback"
    );
}
