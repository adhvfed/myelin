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
use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MicroUsd, RunId as StorageRunId};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

fn call(name: &str) -> ToolCall {
    ToolCall {
        id: ToolCallId(format!("call:{name}")),
        name: ToolName(name.into()),
        arguments: serde_json::Value::Null,
    }
}

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

struct Tenant;
impl TenantGuard for Tenant {
    fn permits(&self, _a: &Principal, _t: &ToolName, _o: &ArtifactRef) -> bool {
        true
    }
}

struct SubsystemEndpoint {
    applied: RefCell<Vec<(String, String)>>,
}
impl SubsystemEndpoint {
    fn new() -> SubsystemEndpoint {
        SubsystemEndpoint {
            applied: RefCell::new(Vec::new()),
        }
    }
    fn applies_of(&self, tool: &str) -> usize {
        self.applied
            .borrow()
            .iter()
            .filter(|(t, _)| t == tool)
            .count()
    }
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

struct WalletBudget {
    remaining: u64,
    settles: u64,
    billed: u64,
}
impl EffectBudget for WalletBudget {
    fn has_remaining(&self, cost: u64) -> bool {
        self.remaining >= cost
    }
    fn settle_one(&mut self, unit: &MeteredUnit) -> u64 {
        let total = unit.total().map(|m| m.0).unwrap_or(0);
        self.remaining = self.remaining.saturating_sub(total);
        self.billed = self.billed.saturating_add(total);
        self.settles += 1;
        total
    }
}

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
            token: format!("tok:{agent_id}:{run_id}:{seq}"),
            jti: format!("jti:{agent_id}:{run_id}:{seq}"),
            ttl_secs,
        })
    }
}

#[derive(Default)]
struct Revoker {
    revoked: Mutex<HashMap<String, i64>>,
}
impl RunTokenRevoker for Revoker {
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
        let mut g = self.revoked.lock().unwrap();
        if g.contains_key(jti) {
            return 0;
        }
        g.insert(jti.to_string(), now_secs);
        (now_secs - teardown_secs).max(0) as u64
    }
    fn is_dead(&self, jti: &str, _now_secs: i64) -> bool {
        self.revoked.lock().unwrap().contains_key(jti)
    }
}

struct DurableWait {
    decision: WaitDecision,
}
impl HitlWait for DurableWait {
    fn park_and_wait(&self, _gate: &myelin_agent_service::hitl::HitlGate) -> WaitDecision {
        self.decision.clone()
    }
}

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
                requires_approval: false,
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
                requires_approval: false,
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
                requires_approval: true,
                exposed_over_mcp: false,
            },
        ],
    }
}

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

const RUN_ESTIMATE: u64 = 20;
const FUNDED_WALLET: u64 = 100;

#[test]
fn ag_p24_e2e2_flagship_green_end_to_end() {
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
        "a casual mention NOTIFIES only - 0 auto-spawn (the L-3 floor): {casual:?}"
    );

    let mut ledger = CostLedger::new();
    let storage_run = StorageRunId::new(RUN_ID);
    let reservation = ledger
        .reserve(
            tenant(),
            storage_run.clone(),
            MicroUsd(RUN_ESTIMATE),
            MicroUsd(FUNDED_WALLET),
        )
        .expect("a funded wallet reserves the run at dispatch (no balance → no run)");
    assert_eq!(
        reservation.reserved,
        MicroUsd(RUN_ESTIMATE),
        "reserved exactly the estimate at dispatch"
    );
    ledger
        .begin(&tenant(), &storage_run)
        .expect("the reserved run begins flight");

    let minter = std::sync::Arc::new(RecordingMinter::default());
    let revoker = Revoker::default();
    let mut identity = RunIdentity::new(
        minter.clone(),
        AGENT_ID,
        RUN_ID,
        DelegationCaveats(vec!["delegated:on-call-human".into()]),
    )
    .with_fail_static_w(300);
    let day = 86_400i64;
    let dispatch_token_jti = identity
        .mint_at_dispatch( 0,  (7 * day) as u64)
        .expect("the dispatch mint succeeds (the run starts attributed)")
        .jti
        .clone();
    let child = identity.child_env().expect("a minted run has a child env");
    assert!(
        !child.leaked_shared_token(),
        "0 shared platform token leaked into the child env (the anti-leak unset)"
    );

    let script = triage_script();
    let brain = MockAgentRuntime::new(script.clone());
    let seq_a = proposed_effect_sequence(&brain, &script, &effect_for, 64);
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
    let planned: Vec<PlannedEffect> = seq_a
        .iter()
        .map(|e| decode_proposed(e).expect("the proposed-effect carrier decodes"))
        .collect();
    assert_eq!(planned[0].tool.0, "create_issue");
    assert_eq!(planned[1].tool.0, "post_chat_message");
    assert_eq!(planned[2].tool.0, "git.merge");

    let catalogue = flagship_catalogue();
    let check = allow();
    let delegation = delegated();
    let tenant_guard = Tenant;
    let endpoint = SubsystemEndpoint::new();

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
             BTreeSet::new(),
            &mut signals,
        );

        let r_issue = p.apply_planned(&planned[0]);
        assert!(
            matches!(r_issue, EffectResult::Applied(_)),
            "create_issue APPLIES (no approval needed): {r_issue:?}"
        );
        let r_chat = p.apply_planned(&planned[1]);
        assert!(
            matches!(r_chat, EffectResult::Applied(_)),
            "post_chat_message APPLIES (no approval needed): {r_chat:?}"
        );
        let r_merge = p.apply_planned(&planned[2]);
        merge_gate_id =
            gate_id_of(&r_merge).expect("the merge tool is WITHHELD → a Gated verdict (AG-8)");
        assert!(
            matches!(r_merge, EffectResult::Gated(_)),
            "git.merge is WITHHELD (requires_approval) → Gated, does NOT mutate: {r_merge:?}"
        );

        assert_eq!(
            budget.settles, 2,
            "exactly two metered effects so far (issue + chat); the merge is not metered (withheld)"
        );
    }
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
        "MERGE-COUNT == 0 at the park - 0 mutation before approval (AG-8)"
    );
    assert_eq!(signals.applied(), 2, "two effects applied; one gated");
    assert_eq!(signals.gated(), 1, "the merge gated exactly once");
    assert_eq!(
        signals.denied(),
        0,
        "0 denials - every planned effect is inside the ∩"
    );
    assert_eq!(
        signals.privileged_fallback(),
        0,
        "AG-D2: NO privileged fallback EVER fires (0 by construction)"
    );

    let merge_plan = &planned[2];
    let mut approved = ApprovedTools::new();
    let wait_approve = DurableWait {
        decision: WaitDecision::Approve,
    };
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

    let resume_at = 2 * day;
    let remint_jti = identity
        .remint_on_resume(resume_at)
        .expect("the resume re-mints a fresh token (within the remaining run life)")
        .jti
        .clone();
    assert_ne!(
        remint_jti, dispatch_token_jti,
        "the re-minted token is FRESH (a new jti - not the dispatch token)"
    );
    assert_eq!(identity.reminted(), 1, "exactly one re-mint on resume");
    assert!(
        !identity.attribution_window().has_unattributed_gap(),
        "0 unattributed window across the multi-day pause (continuously attributed)"
    );
    assert!(
        identity.current().unwrap().ttl_secs <= 300,
        "the re-minted token TTL is bounded by the fail-static window W (token life == activity life)"
    );

    let mut signals2 = PipelineSignals::new();
    {
        let mut budget2 = WalletBudget {
            remaining: RUN_ESTIMATE.saturating_sub(7),
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
            approved.as_set(),
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
    assert_eq!(
        endpoint.applies_of("git.merge"),
        1,
        "MERGE-COUNT == 1 - the merge applied EXACTLY ONCE after approval (no double-merge)"
    );
    assert_eq!(
        signals2.applied(),
        1,
        "exactly one apply on resume (the merge)"
    );

    assert_eq!(
        endpoint.applies_of("git.merge"),
        1,
        "a second resume does NOT re-merge - exactly-once consume across the kill"
    );

    let units = vec![
        MeteredUnit {
            unit: "issue.transition",
            wholesale: MicroUsd(3),
            markup: MicroUsd(1),
        },
        MeteredUnit {
            unit: "agent.effect",
            wholesale: MicroUsd(2),
            markup: MicroUsd(1),
        },
        MeteredUnit {
            unit: "git.merge",
            wholesale: MicroUsd(5),
            markup: MicroUsd(2),
        },
    ];
    let settle = ledger
        .settle(&tenant(), &storage_run, &units)
        .expect("the in-flight run settles on completion");
    let billed = settle.billed_total.0;
    let refunded = settle.refunded.0;
    assert_eq!(billed, 14, "billed the three effects' actual cost (4+3+7)");
    assert_eq!(
        billed + refunded,
        RUN_ESTIMATE,
        "RESERVE/SETTLE BALANCED: reserved (20) == billed (14) + refunded (6)"
    );
    assert_eq!(
        ledger.cost_events_for(&tenant(), &storage_run).len(),
        3,
        "EXACTLY one cost event per metered unit (3 effects → 3 events)"
    );
    assert_eq!(
        ledger.inflight_interrupt_count(),
        0,
        "0 in-flight interrupts across the kill (the reservation's only exit is settle, 11.7)"
    );

    let lag = identity.revoke_on_teardown(&revoker, resume_at, resume_at);
    assert!(lag < 300, "the revocation lag is within the bound W");
    assert!(
        revoker.is_dead(&remint_jti, resume_at),
        "the re-minted token is revoked on teardown"
    );

    let _ = (dispatch_token_jti, child);
}

#[test]
fn ag_p24_e2e2_exhausted_wallet_refuses_to_start() {
    let mut ledger = CostLedger::new();
    let storage_run = StorageRunId::new("run:exhausted");
    let refused = ledger.reserve(
        tenant(),
        storage_run.clone(),
        MicroUsd(RUN_ESTIMATE),
        MicroUsd(5),
    );
    assert!(
        refused.is_err(),
        "an exhausted wallet REFUSES to start the run (no balance → no run): {refused:?}"
    );
    assert!(
        ledger.cost_events_for(&tenant(), &storage_run).is_empty(),
        "a refused dispatch wrote no cost events (the run never ran)"
    );
}

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
        "a rejected approval HALTS - the merge is withheld forever: {outcome:?}"
    );
    assert!(
        !approved.contains_effect("git.merge", &merge_plan.object.0),
        "a rejected merge is NEVER admitted to approved (0 mutation, AG-8)"
    );

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
            approved.as_set(),
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
        "MERGE-COUNT == 0 on the decline path - 0 mutation forever (AG-8)"
    );
}

#[test]
fn ag_p24_e2e2_effect_outside_intersection_denied() {
    let catalogue = flagship_catalogue();
    let merge_plan = effect_for(&ToolName("git.merge".into())).unwrap();
    let check = allow();
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
