//! # Integration: the MR-006 BINDING end-to-end — `mint_run_token → EffectApi::apply`.
//!
//! Drives the MCP server as a real JSON-RPC peer (newline-delimited JSON-RPC in, responses out) with
//! a GOVERNANCE ROUTER wired over a REAL [`RunTokenMinter`] (genuine per-run mint + durable
//! revocation consult) and a reference [`SkeletonEffectApi`] chokepoint. Proves, against the running
//! protocol:
//!   - `initialize` → capabilities;
//!   - `tools/list` → the git tools with their frozen `requiresApproval` flags;
//!   - `tools/call` a non-gated tool → it MINTS a per-run token + routes through `EffectApi::apply`
//!     under a RunCtx attributed to the run (NOT a bare PAT, NOT a direct mutation);
//!   - a `requires_approval` tool → HITL-gated (withheld, NOT applied) without approval; applied WITH;
//!   - a REVOKED run token → denied (never routed);
//!   - a malformed request → a JSON-RPC error, no panic.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct FailingWriter;

impl std::io::Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "injected broken stdout",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

use myelin_events::{MonotonicMinter, OutboxStore, Timestamp};
use myelin_identity::{
    DelegationCaveats, FailStaticBound, Principal, PrincipalId, PrincipalKind, RevokeTarget, RunId,
    RuntimeRef,
};
use myelin_identity_service::delegation::DelegationInput;
use myelin_identity_service::machine_auth::{Authority, MachineKind};
use myelin_identity_service::mint::{RunTokenMinter, StructuralTokenSigner};
use myelin_identity_service::revocation::RevocationStore;
use myelin_identity_service::ResolvedDelegationPolicy;
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

use myelin_mcp::governance::{mcp_effect_key, SkeletonEffectApi};
use myelin_mcp::{
    AuditPhase, CallOutcome, GateApproverPolicy, GovernanceAudit, GovernanceAuditRecord,
    GovernedRouter, McpServer, OutboxGovernanceAudit, RunPrincipal, ToolRegistry,
};
use myelin_storage::hitl_gate_durable::{GateDecideError, GateRecord, GateState, HitlVerdictStore};

fn now() -> Timestamp {
    Timestamp("2026-06-26T00:00:00Z".into())
}

struct TestApprovers(Vec<PrincipalId>);

impl GateApproverPolicy for TestApprovers {
    fn eligible_approvers(
        &self,
        _tool: &str,
        _args: &serde_json::Value,
    ) -> Result<Vec<PrincipalId>, String> {
        Ok(self.0.clone())
    }
}

struct FailOutcomeAudit;

impl GovernanceAudit for FailOutcomeAudit {
    fn record(&self, record: GovernanceAuditRecord<'_>) -> Result<(), String> {
        match record.phase {
            AuditPhase::Attempt => Ok(()),
            AuditPhase::Outcome
            | AuditPhase::Approved
            | AuditPhase::Rejected
            | AuditPhase::Expired => Err("injected durable audit outage".into()),
        }
    }
}

struct FailExpiryAudit;

impl GovernanceAudit for FailExpiryAudit {
    fn record(&self, record: GovernanceAuditRecord<'_>) -> Result<(), String> {
        if record.phase == AuditPhase::Expired {
            assert!(
                record.gate_id.is_some(),
                "an expiry fact must identify the exact durable gate"
            );
            Err("injected expiry-audit outage".into())
        } else {
            Ok(())
        }
    }
}

fn agent_principal(id: &str, tenant: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt-local-claude".into()),
            on_behalf_of: Some(PrincipalId("human:operator".into())),
        },
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn human_principal(id: &str, tenant: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

/// A non-human machine principal (R2.4b — used to prove the distinct-HUMAN approver rule refuses a
/// machine/service approver even when it is distinct from the requester).
fn service_principal(id: &str, tenant: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Service,
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

/// Build a server with a governed router over a REAL minter (shared S7 store returned so a test can
/// revoke the run token).
fn governed_router() -> GovernedRouter {
    let grants = [
        "pull_request.merge",
        "repo.push",
        "pull_request.review",
        "repo.approve_untrusted_ci",
    ];
    governed_router_with_input(DelegationInput {
        agent_policy: Authority::of(grants),
        delegation: Authority::of(grants),
        tenant_policy: Authority::of(grants),
        trigger_actor_held: Authority::of(grants),
    })
}

fn governed_router_with_input(input: DelegationInput) -> GovernedRouter {
    governed_router_with_trigger(input, "trigger-jti", i64::MAX)
}

fn governed_router_with_trigger(
    input: DelegationInput,
    trigger_jti: &str,
    trigger_expires_at_unix: i64,
) -> GovernedRouter {
    governed_router_with_trigger_and_audit(
        input,
        trigger_jti,
        trigger_expires_at_unix,
        Arc::new(OutboxGovernanceAudit::new(
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()),
        )),
    )
}

fn governed_router_with_trigger_and_audit(
    input: DelegationInput,
    trigger_jti: &str,
    trigger_expires_at_unix: i64,
    audit: Arc<dyn GovernanceAudit>,
) -> GovernedRouter {
    governed_router_with_trigger_audit_and_ttl(
        input,
        trigger_jti,
        trigger_expires_at_unix,
        audit,
        300,
    )
}

fn governed_router_with_trigger_audit_and_ttl(
    input: DelegationInput,
    trigger_jti: &str,
    trigger_expires_at_unix: i64,
    audit: Arc<dyn GovernanceAudit>,
    run_ttl_secs: u64,
) -> GovernedRouter {
    governed_router_with_verdicts(
        input,
        trigger_jti,
        trigger_expires_at_unix,
        audit,
        run_ttl_secs,
        HitlVerdictStore::new(),
    )
}

fn governed_router_with_verdicts(
    input: DelegationInput,
    trigger_jti: &str,
    trigger_expires_at_unix: i64,
    audit: Arc<dyn GovernanceAudit>,
    run_ttl_secs: u64,
    verdicts: HitlVerdictStore,
) -> GovernedRouter {
    let s7 = RevocationStore::new();
    // A REAL per-run minter (the structural floor signer is the EI-01 §1 named seam; the real
    // PASETO/Ed25519 signer swaps in behind the SAME `TokenSigner` trait — the mint's intersection +
    // scope + TTL logic is unchanged). Construction lives in this `tests/` crate (excluded from the
    // no-structural-crypto-in-prod scanner), never in `src`.
    let minter =
        RunTokenMinter::with_signer_and_tuples(s7, None, Arc::new(StructuralTokenSigner::new()));

    let agent = agent_principal("agent:claude", "acme");
    let trigger = human_principal("human:operator", "acme");
    let scope = TenantScope::from_verified_token(&trigger, Region("eu-west".into()));

    let caveats = input.delegation.grants().map(str::to_string).collect();

    let run_id = RunId("mcp-run-1".into());
    let agent_id = PrincipalId("agent:claude".into());
    let resolved_policy = ResolvedDelegationPolicy::synthetic_for_test(
        run_id.clone(),
        agent_id.clone(),
        trigger.principal_id.clone(),
        input,
        1,
    );
    let principal = RunPrincipal {
        scope,
        agent_id,
        agent,
        trigger_actor: trigger,
        trigger_credential_jti: trigger_jti.into(),
        trigger_expires_at_unix,
        run_id,
        resolved_policy,
        caveats: DelegationCaveats(caveats),
        kind: MachineKind::Agent,
        ttl: FailStaticBound {
            static_max_secs: run_ttl_secs,
        },
    };

    // R2.4: the SERVER-SIDE verdict store (in-memory test double of the durable agent_hitl_gate
    // arm) + the approver set. The agent principal is deliberately INCLUDED here to prove the
    // router structurally excludes the requester from its own gate's eligible approvers.
    let approvers = vec![
        PrincipalId("human:operator".into()),
        PrincipalId("agent:claude".into()),
    ];
    GovernedRouter::with_approver_policy(
        minter,
        principal,
        Box::new(SkeletonEffectApi::new()),
        verdicts,
        Arc::new(TestApprovers(approvers)),
        audit,
    )
}

fn governed_server() -> McpServer {
    McpServer::with_router_and_clock(ToolRegistry::with_git(), governed_router(), Arc::new(now))
}

#[test]
fn declared_caps_fail_closed_for_missing_and_attenuated_delegation() {
    let registry = ToolRegistry::with_git();
    let open_pr = registry.resolve("git.open_pr").expect("registered tool");

    let no_grant = governed_router_with_input(DelegationInput {
        agent_policy: Authority::of(["pull_request.review"]),
        delegation: Authority::of(["pull_request.review"]),
        tenant_policy: Authority::of(["pull_request.review"]),
        trigger_actor_held: Authority::of(["pull_request.review"]),
    });
    let outcome = no_grant.call(open_pr, &serde_json::json!({"repo": "alpha"}), &now(), None);
    match outcome {
        CallOutcome::Denied { reason, .. } => assert!(reason.contains("repo.push")),
        other => panic!("missing declared capability must deny before EffectApi: {other:?}"),
    }

    // Agent + tenant + delegator-held all contain repo.push, but the delegation caveat attenuates
    // it away. A union bug or a check against only the agent ceiling would incorrectly apply.
    let attenuated = governed_router_with_input(DelegationInput {
        agent_policy: Authority::of(["repo.push", "pull_request.review"]),
        delegation: Authority::of(["pull_request.review"]),
        tenant_policy: Authority::of(["repo.push", "pull_request.review"]),
        trigger_actor_held: Authority::of(["repo.push", "pull_request.review"]),
    });
    let outcome = attenuated.call(open_pr, &serde_json::json!({"repo": "alpha"}), &now(), None);
    match outcome {
        CallOutcome::Denied { reason, .. } => assert!(reason.contains("repo.push")),
        other => panic!("attenuated-away capability must deny: {other:?}"),
    }
}

/// Exchange lines within one logical session. Some HITL tests pause between exchanges while a human
/// decides a gate, so EOF teardown is exercised separately rather than between these turns.
fn drive(server: &McpServer, lines: &[&str]) -> Vec<serde_json::Value> {
    lines
        .iter()
        .filter_map(|line| server.handle_line(line))
        .map(|line| serde_json::from_str(&line).expect("each response line is JSON"))
        .collect()
}

#[test]
fn handshake_and_tools_list_over_the_running_protocol() {
    let server = governed_server();
    let resps = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        ],
    );
    // The notification produced NO response → exactly two responses.
    assert_eq!(resps.len(), 2, "the notification yields no response");
    assert_eq!(resps[0]["result"]["serverInfo"]["name"], "myelin-mcp");
    let tools = resps[1]["result"]["tools"].as_array().unwrap();
    let merge = tools.iter().find(|t| t["name"] == "git.merge").unwrap();
    assert_eq!(
        merge["annotations"]["requiresApproval"], true,
        "git.merge is HITL-gated"
    );
    let review = tools
        .iter()
        .find(|t| t["name"] == "git.submit_review")
        .unwrap();
    assert_eq!(review["annotations"]["requiresApproval"], false);
}

#[test]
fn non_gated_tool_mints_a_run_token_and_routes_through_effect_api() {
    let server = governed_server();
    // git.submit_review is NOT requires_approval → it flows mint -> is_live -> EffectApi::apply.
    let resps = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.submit_review","arguments":{"number":7}}}"#,
        ],
    );
    let r = &resps[0];
    assert_eq!(r["result"]["isError"], false);
    let jti = r["result"]["_meta"]["runToken"].as_str().unwrap();
    assert!(
        jti.starts_with("runtok:agent:claude:mcp-run-1"),
        "the run token jti is bound to (agent, run): {jti}"
    );

    // The EVENT ID is produced by SkeletonEffectApi FROM the RunCtx it was handed — so its presence
    // proves the call genuinely routed THROUGH EffectApi::apply, and its contents prove the RunCtx
    // carried the minted jti + the principal (the audit attribution).
    let event_id = r["result"]["_meta"]["eventId"].as_str().unwrap();
    assert!(
        event_id.contains(jti),
        "the effect was applied under the minted run token (attribution): {event_id}"
    );
    assert!(
        event_id.contains("principal:agent:claude"),
        "attributed to the agent principal: {event_id}"
    );
    assert!(event_id.contains("tool:git.submit_review"));

    // The run actually minted a per-run token (not a bare PAT) + audited the call to the run.
    let router = server.router().unwrap();
    assert!(
        router.current_token().is_some(),
        "a per-run token was minted"
    );
    let audit = router.audit();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].principal, "agent:claude");
    assert_eq!(audit[0].tool, "git.submit_review");
    assert!(matches!(audit[0].outcome, CallOutcome::Applied { .. }));
}

#[test]
fn requires_approval_tool_is_hitl_gated_before_apply() {
    let server = governed_server();
    // git.merge is requires_approval = true (frozen). With NO approval → withheld (Gated), NOT applied.
    let resps = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7}}}"#,
        ],
    );
    let r = &resps[0];
    assert!(
        r["result"]["_meta"]["gateId"].is_string(),
        "git.merge is gated on HITL"
    );
    assert!(
        r["result"]["_meta"]["eventId"].is_null(),
        "a gated effect was NOT applied (no event id)"
    );
    assert!(matches!(
        server.router().unwrap().audit()[0].outcome,
        CallOutcome::Gated { .. }
    ));
}

/// **R2.4 Defect A (the caller-boolean bypass, closed):** a bare `{"approval":{"granted":true}}`
/// SUPPLIED BY THE CALLER — which for an autonomous agent IS the agent — must NOT clear the HITL
/// gate. Approval is a SERVER-SIDE verdict looked up by the gate, never a caller-supplied boolean.
/// (This test is the rewrite of the old `requires_approval_tool_applies_after_explicit_approval`,
/// whose happy path literally demonstrated the bypass: it fails RED on the pre-R2.4 code.)
#[test]
fn a_caller_supplied_granted_boolean_never_applies() {
    let server = governed_server();
    let resps = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7},"approval":{"granted":true}}}"#,
        ],
    );
    let r = &resps[0];
    assert!(
        r["result"]["_meta"]["eventId"].is_null(),
        "a caller-supplied `granted: true` must NOT apply a requires_approval tool (server-side \
         verdict only): {r}"
    );
    assert!(
        matches!(
            server.router().unwrap().audit()[0].outcome,
            CallOutcome::Gated { .. } | CallOutcome::Denied { .. }
        ),
        "the call is withheld or refused — never Applied off a caller boolean"
    );
}

/// **R2.4: a gated tool returns an OPAQUE server-issued gate id** — not the old guessable
/// deterministic `hitl:{jti}:{tool}` display string. Two independent servers gating the same call
/// mint DIFFERENT ids (the id is unpredictable, so a caller cannot pre-compute it); a retried call
/// on the SAME server re-surfaces the SAME pending gate (no duplicate spawn).
#[test]
fn a_gated_tool_returns_an_opaque_unguessable_gate_id() {
    let call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7}}}"#;

    let server = governed_server();
    let resps = drive(&server, &[call]);
    let gate_id = resps[0]["result"]["_meta"]["gateId"]
        .as_str()
        .unwrap()
        .to_string();
    let jti = resps[0]["result"]["_meta"]["runToken"].as_str().unwrap();

    // NOT the guessable deterministic display string, and not derived from the visible jti.
    assert_ne!(
        gate_id,
        format!("hitl:{jti}:git.merge"),
        "the gate id must not be the old deterministic display string"
    );
    assert!(
        !gate_id.contains(jti) && !gate_id.contains("git.merge"),
        "the gate id must not embed the guessable call facts: {gate_id}"
    );

    // A second, independent server gating the IDENTICAL call mints a DIFFERENT id.
    let other = governed_server();
    let other_resps = drive(&other, &[call]);
    let other_gate = other_resps[0]["result"]["_meta"]["gateId"]
        .as_str()
        .unwrap();
    assert_ne!(
        gate_id, other_gate,
        "gate ids are unpredictable across servers/processes"
    );

    // A RETRY on the same server re-surfaces the SAME pending gate (one row, one card).
    let retry = drive(&server, &[call]);
    assert_eq!(
        retry[0]["result"]["_meta"]["gateId"].as_str().unwrap(),
        gate_id,
        "a retried gated call re-surfaces the same pending gate"
    );
}

/// **R2.4 / R2.4b — the full server-side approval loop:** gated → the gate row is `waiting` in the
/// store → the agent CANNOT approve its own gate (distinct-approver) AND a NON-HUMAN (machine)
/// principal cannot approve at all (distinct-HUMAN, R2.4b) → re-driving with the gate id while it is
/// still waiting does NOT apply → a DISTINCT HUMAN approves in the store → the re-drive presenting
/// that gate id applies through EffectApi.
#[test]
fn approval_is_a_server_side_verdict_by_a_distinct_human_principal() {
    let server = governed_server();
    let call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7}}}"#;

    // (1) WITHHELD → an opaque gate id, a `waiting` row server-side, 0 mutation.
    let gated = drive(&server, &[call]);
    let gate_id = gated[0]["result"]["_meta"]["gateId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(gated[0]["result"]["_meta"]["eventId"].is_null());
    let router = server.router().unwrap();
    let rec = router
        .gate_verdict(&gate_id)
        .expect("the gate is a server-side row");
    assert_eq!(rec.state, GateState::Waiting);
    assert_eq!(rec.requested_by, "agent:claude");
    assert!(
        !rec.approver_filter.contains(&"agent:claude".to_string()),
        "the requesting agent is structurally excluded from its own gate's approver set"
    );

    // (2) SELF-APPROVAL refused server-side (the agent IS the MCP caller — it can never clear
    //     its own gate, whatever it sends).
    assert!(
        matches!(
            router.approve_gate(&agent_principal("agent:claude", "acme"), &gate_id, &now(),),
            Err(GateDecideError::SelfApproval) | Err(GateDecideError::NotEligible)
        ),
        "the agent principal cannot approve its own gate"
    );
    // (2b) R2.4b — a DISTINCT NON-HUMAN principal (a service/agent) cannot approve either: the
    //      HITL gate structurally requires a HUMAN approver (closes the machine-collusion gap).
    assert_eq!(
        router.approve_gate(&service_principal("svc:ci-robot", "acme"), &gate_id, &now(),),
        Err(GateDecideError::MachineApproverRefused),
        "a distinct MACHINE principal is refused — the gate requires a HUMAN approver"
    );
    assert_eq!(
        router.gate_verdict(&gate_id).unwrap().state,
        GateState::Waiting,
        "the machine-refused approval left the gate undecided"
    );

    // (3) Re-driving WITH the gate id while the gate is still waiting → still withheld, 0 apply.
    let redrive = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"git.merge","arguments":{{"repo":"acme/web","number":7}},"approval":{{"gateId":"{gate_id}"}}}}}}"#
    );
    let pending = drive(&server, &[redrive.as_str()]);
    assert!(
        pending[0]["result"]["_meta"]["eventId"].is_null(),
        "a still-waiting gate does not apply: {}",
        pending[0]
    );
    assert_eq!(
        pending[0]["result"]["_meta"]["gateId"].as_str().unwrap(),
        gate_id
    );

    // (4) A DISTINCT human principal approves — SERVER-SIDE.
    router
        .approve_gate(&human_principal("human:operator", "acme"), &gate_id, &now())
        .expect("a distinct eligible human approves");
    let rec = router.gate_verdict(&gate_id).unwrap();
    assert_eq!(rec.state, GateState::Approved);
    assert_eq!(rec.decided_by.as_deref(), Some("human:operator"));

    // (5) The re-drive presenting the approved gate id now applies through EffectApi.
    let applied = drive(&server, &[redrive.as_str()]);
    let event_id = applied[0]["result"]["_meta"]["eventId"]
        .as_str()
        .expect("applied");
    assert!(
        event_id.contains("tool:git.merge"),
        "the approved effect applied: {event_id}"
    );
}

/// **R2.4: a made-up / forged gate id is DENIED** — the store has no such verdict; the caller
/// cannot conjure an approval by presenting an id of its own invention.
#[test]
fn a_made_up_gate_id_is_denied() {
    let server = governed_server();
    let resps = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7},"approval":{"gateId":"gate:0123456789abcdef0123456789abcdef"}}}"#,
        ],
    );
    let r = &resps[0];
    assert_eq!(
        r["result"]["isError"], true,
        "a forged gate id is a loud deny: {r}"
    );
    assert!(r["result"]["_meta"]["eventId"].is_null(), "0 mutation");
    let reason = r["result"]["_meta"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("not granted server-side"),
        "the deny names the rule: {reason}"
    );
}

/// **R2.4: an approval is bound to the EXACT effect (tool + args), never the tool name** — an
/// approved gate for PR 7 presented on a re-drive for PR 8 is denied; and a REJECTED gate stays
/// denied forever (0 mutation, AG-8).
#[test]
fn an_approval_never_transfers_to_a_sibling_effect_and_a_reject_is_final() {
    let server = governed_server();
    let call7 = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7}}}"#;
    let gated = drive(&server, &[call7]);
    let gate7 = gated[0]["result"]["_meta"]["gateId"]
        .as_str()
        .unwrap()
        .to_string();
    let router = server.router().unwrap();
    router
        .approve_gate(&human_principal("human:operator", "acme"), &gate7, &now())
        .unwrap();

    // The approved gate for PR 7 does NOT clear a re-drive of PR 8 (same tool, different effect).
    let cross = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"git.merge","arguments":{{"repo":"acme/web","number":8}},"approval":{{"gateId":"{gate7}"}}}}}}"#
    );
    let denied = drive(&server, &[cross.as_str()]);
    assert_eq!(
        denied[0]["result"]["isError"], true,
        "approval never transfers across effects"
    );
    assert!(denied[0]["result"]["_meta"]["eventId"].is_null());

    // A REJECTED gate is final: re-driving with it is denied (never re-approvable).
    let call9 = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":9}}}"#;
    let gated9 = drive(&server, &[call9]);
    let gate9 = gated9[0]["result"]["_meta"]["gateId"]
        .as_str()
        .unwrap()
        .to_string();
    router
        .reject_gate(&human_principal("human:operator", "acme"), &gate9, &now())
        .unwrap();
    assert!(
        router
            .approve_gate(&human_principal("human:operator", "acme"), &gate9, &now(),)
            .is_err(),
        "a rejected gate never re-transitions"
    );
    let redrive9 = format!(
        r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"git.merge","arguments":{{"repo":"acme/web","number":9}},"approval":{{"gateId":"{gate9}"}}}}}}"#
    );
    let d9 = drive(&server, &[redrive9.as_str()]);
    assert_eq!(
        d9[0]["result"]["isError"], true,
        "a rejected effect is withheld forever (AG-8)"
    );
    assert!(d9[0]["result"]["_meta"]["eventId"].is_null(), "0 mutation");
}

#[test]
fn a_revoked_run_token_is_denied_never_routed() {
    let server = governed_server();
    // First call mints + applies (under a live token).
    let first = server
        .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.submit_review","arguments":{"number":1}}}"#)
        .unwrap();
    let first: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(first["result"]["isError"], false);

    // The run is killed mid-flight → the per-run token's jti is torn down (MR-011 durable revocation).
    let router = server.router().unwrap();
    let token = router.current_token().unwrap();
    router
        .minter()
        .teardown(&router.principal().scope, &token, &now());

    // A subsequent tools/call under the REVOKED run token is DENIED — never routed to EffectApi.
    let second = server
        .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"git.submit_review","arguments":{"number":2}}}"#)
        .unwrap();
    let second: serde_json::Value = serde_json::from_str(&second).unwrap();
    assert_eq!(
        second["result"]["isError"], true,
        "a revoked run token is denied"
    );
    let reason = second["result"]["_meta"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("revoked"),
        "denied for revocation: {reason}"
    );
}

#[test]
fn lazy_mint_refuses_expired_or_revoked_trigger_and_clamps_run_life() {
    let input = || DelegationInput {
        agent_policy: Authority::of(["repo.push"]),
        delegation: Authority::of(["repo.push"]),
        tenant_policy: Authority::of(["repo.push"]),
        trigger_actor_held: Authority::of(["repo.push"]),
    };
    let at = Timestamp("2026-06-26T00:00:00Z".into());
    let at_unix = chrono::DateTime::parse_from_rfc3339(&at.0)
        .unwrap()
        .timestamp();
    let registry = ToolRegistry::with_git();
    let tool = registry.resolve("git.open_pr").unwrap();
    let args = serde_json::json!({"repo":"alpha"});

    let expired = governed_router_with_trigger(input(), "expired-trigger", at_unix);
    assert!(matches!(
        expired.call(tool, &args, &at, None),
        CallOutcome::Denied { reason, .. } if reason.contains("trigger credential is expired")
    ));
    assert!(expired.current_token().is_none());

    let revoked = governed_router_with_trigger(input(), "revoked-trigger", at_unix + 60);
    revoked.minter().revocations().revoke(
        &revoked.principal().scope,
        &RevokeTarget::Jti("revoked-trigger".into()),
        at.clone(),
    );
    assert!(matches!(
        revoked.call(tool, &args, &at, None),
        CallOutcome::Denied { reason, .. } if reason.contains("trigger credential is revoked")
    ));
    assert!(revoked.current_token().is_none());

    let clamped = governed_router_with_trigger(input(), "short-trigger", at_unix + 2);
    assert!(matches!(
        clamped.call(tool, &args, &at, None),
        CallOutcome::Applied { .. }
    ));
    let token = clamped.current_token().unwrap();
    clamped.minter().revocations().revoke(
        &clamped.principal().scope,
        &RevokeTarget::Jti("short-trigger".into()),
        at.clone(),
    );
    assert!(matches!(
        clamped.call(tool, &args, &Timestamp("2026-06-26T00:00:01Z".into()), None),
        CallOutcome::Applied { .. }
    ));
    let after_trigger = Timestamp("2026-06-26T00:00:03Z".into());
    assert!(
        !clamped
            .minter()
            .is_live(&clamped.principal().scope, &token, &after_trigger),
        "run token must not outlive its authenticated trigger credential"
    );
}

#[test]
fn malformed_request_is_a_jsonrpc_error_no_panic() {
    let server = governed_server();
    let resps = drive(&server, &["{ not valid json"]);
    assert_eq!(
        resps[0]["error"]["code"], -32700,
        "parse error, never a panic"
    );
}

#[test]
fn stdio_eof_tears_down_the_minted_run_token() {
    let server = governed_server();
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.submit_review","arguments":{"number":1}}}"#;
    let mut output = Vec::new();

    server
        .run(request.as_bytes(), &mut output)
        .expect("stdio session");

    let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["result"]["isError"], false);
    let router = server.router().unwrap();
    let token = router
        .current_token()
        .expect("the session minted a run token");
    assert!(
        !router
            .minter()
            .is_live(&router.principal().scope, &token, &now()),
        "EOF immediately revokes the session token"
    );
}

#[test]
fn stdio_output_error_still_tears_down_the_minted_run_token() {
    let server = governed_server();
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.submit_review","arguments":{"number":1}}}"#;
    let error = server
        .run(request.as_bytes(), FailingWriter)
        .expect_err("broken stdout must surface");
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    let router = server.router().unwrap();
    let token = router
        .current_token()
        .expect("request minted before write failed");
    assert!(
        !router
            .minter()
            .is_live(&router.principal().scope, &token, &now()),
        "every output-error exit path tears down the run token"
    );
}

#[test]
fn post_mutation_audit_failure_is_indeterminate_terminal_and_tears_down() {
    let grants = ["repo.push"];
    let router = governed_router_with_trigger_and_audit(
        DelegationInput {
            agent_policy: Authority::of(grants),
            delegation: Authority::of(grants),
            tenant_policy: Authority::of(grants),
            trigger_actor_held: Authority::of(grants),
        },
        "trigger-jti",
        i64::MAX,
        Arc::new(FailOutcomeAudit),
    );
    let server = McpServer::with_router_and_clock(ToolRegistry::with_git(), router, Arc::new(now));
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.open_pr","arguments":{"repo":"alpha"}}}"#;
    let mut output = Vec::new();
    let error = server
        .run(request.as_bytes(), &mut output)
        .expect_err("indeterminate session must stop after its response");
    assert!(error.to_string().contains("indeterminate"));
    let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(response["result"]["_meta"]["fatal"], true);
    let router = server.router().unwrap();
    assert!(router.is_fatal());
    let token = router.current_token().unwrap();
    assert!(
        !router
            .minter()
            .is_live(&router.principal().scope, &token, &now()),
        "terminal output path must teardown the run token"
    );
}

#[test]
fn post_gate_open_audit_failure_is_also_indeterminate_and_terminal() {
    let grants = ["pull_request.merge"];
    let router = governed_router_with_trigger_and_audit(
        DelegationInput {
            agent_policy: Authority::of(grants),
            delegation: Authority::of(grants),
            tenant_policy: Authority::of(grants),
            trigger_actor_held: Authority::of(grants),
        },
        "trigger-jti",
        i64::MAX,
        Arc::new(FailOutcomeAudit),
    );
    let registry = ToolRegistry::with_git();
    let outcome = router.call(
        registry.resolve("git.merge").unwrap(),
        &serde_json::json!({"repo":"alpha","number":1}),
        &now(),
        None,
    );
    assert!(matches!(outcome, CallOutcome::Indeterminate { .. }));
    assert!(router.is_fatal());
}

#[test]
fn expiry_audit_failure_is_fail_loud_after_state_commit_and_terminates_session() {
    let grants = ["pull_request.merge"];
    let router = governed_router_with_trigger_audit_and_ttl(
        DelegationInput {
            agent_policy: Authority::of(grants),
            delegation: Authority::of(grants),
            tenant_policy: Authority::of(grants),
            trigger_actor_held: Authority::of(grants),
        },
        "trigger-jti",
        i64::MAX,
        Arc::new(FailExpiryAudit),
        7_200,
    );
    let registry = ToolRegistry::with_git();
    let merge = registry.resolve("git.merge").unwrap();
    let opened_at = Timestamp("2026-06-26T00:00:00Z".into());
    let gate_id = match router.call(
        merge,
        &serde_json::json!({"repo":"alpha","number":1}),
        &opened_at,
        None,
    ) {
        CallOutcome::Gated { gate_id, .. } => gate_id,
        other => panic!("expected gate, got {other:?}"),
    };

    let expired_at = Timestamp("2026-06-26T01:00:00Z".into());
    assert!(matches!(
        router.call(
            merge,
            &serde_json::json!({"repo":"alpha","number":1}),
            &expired_at,
            Some(&gate_id),
        ),
        CallOutcome::Indeterminate { .. }
    ));
    assert_eq!(
        router.gate_verdict(&gate_id).unwrap().state,
        GateState::Expired
    );
    assert!(router.is_fatal());
}

#[test]
fn mcp_expiry_leaves_unrelated_shared_gate_untouched_and_audits_exact_gate() {
    let args = serde_json::json!({"repo":"alpha","number":77});
    let exact_effect = mcp_effect_key("git.merge", &args);
    let mut verdicts = HitlVerdictStore::new();
    let scope = TenantScope::from_verified_token(
        &human_principal("human:operator", "acme"),
        Region("eu-west".into()),
    );
    let record = |gate_id: &str, run_id: &str, effect_id: &str, requester: &str| GateRecord {
        gate_id: gate_id.into(),
        run_id: run_id.into(),
        effect_id: effect_id.into(),
        risk_summary: Vec::new(),
        cost_estimate: 0,
        approver_filter: vec!["human:operator".into()],
        state: GateState::Waiting,
        card_ref: None,
        requested_by: requester.into(),
        decided_by: None,
        opened_at_unix: 1,
        decided_at_unix: None,
        expires_at_unix: 2,
        approval_consumed_at_unix: None,
    };
    verdicts
        .open(
            &scope,
            record("gate:mcp-due", "mcp-run-1", &exact_effect, "agent:claude"),
        )
        .unwrap();
    verdicts
        .open(
            &scope,
            record(
                "gate:shared-due",
                "agent-service-run",
                "agent-service:v1:deploy:opaque",
                "agent:shared-service",
            ),
        )
        .unwrap();
    let audit_store = OutboxStore::new();
    let grants = ["pull_request.merge"];
    let router = governed_router_with_verdicts(
        DelegationInput {
            agent_policy: Authority::of(grants),
            delegation: Authority::of(grants),
            tenant_policy: Authority::of(grants),
            trigger_actor_held: Authority::of(grants),
        },
        "trigger-jti",
        i64::MAX,
        Arc::new(OutboxGovernanceAudit::new(
            audit_store.clone(),
            Arc::new(MonotonicMinter::new()),
        )),
        7_200,
        verdicts,
    );
    let registry = ToolRegistry::with_git();
    assert!(matches!(
        router.call(registry.resolve("git.merge").unwrap(), &args, &now(), None),
        CallOutcome::Gated { .. }
    ));
    assert_eq!(
        router.gate_verdict("gate:mcp-due").unwrap().state,
        GateState::Expired
    );
    assert_eq!(
        router.gate_verdict("gate:shared-due").unwrap().state,
        GateState::Waiting
    );
    assert!(audit_store.committed_rows().iter().any(|row| {
        row.envelope.type_.0 == "git.merge.expired"
            && row.envelope.subject.0.ends_with("/hitl-gate/gate:mcp-due")
    }));
}

#[test]
fn governed_calls_read_the_clock_afresh() {
    let reads = Arc::new(AtomicUsize::new(0));
    let clock_reads = Arc::clone(&reads);
    let clock = Arc::new(move || {
        if clock_reads.fetch_add(1, Ordering::SeqCst) == 0 {
            Timestamp("2026-06-26T00:00:00Z".into())
        } else {
            Timestamp("2026-06-26T00:05:01Z".into())
        }
    });
    let server =
        McpServer::with_router_and_clock(ToolRegistry::with_git(), governed_router(), clock);
    let call = |id| {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"git.submit_review","arguments":{{"number":{id}}}}}}}"#
        )
    };

    let first: serde_json::Value =
        serde_json::from_str(&server.handle_line(&call(1)).unwrap()).unwrap();
    let second: serde_json::Value =
        serde_json::from_str(&server.handle_line(&call(2)).unwrap()).unwrap();

    assert_eq!(first["result"]["isError"], false);
    assert_eq!(second["result"]["isError"], true);
    assert!(second["result"]["_meta"]["reason"]
        .as_str()
        .unwrap()
        .contains("expired"));
    assert_eq!(reads.load(Ordering::SeqCst), 2);
}
