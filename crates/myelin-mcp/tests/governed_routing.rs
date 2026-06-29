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

use std::io::Cursor;
use std::sync::Arc;

use myelin_events::Timestamp;
use myelin_identity::{
    DelegationCaveats, FailStaticBound, Principal, PrincipalId, PrincipalKind, RunId, RuntimeRef,
};
use myelin_identity_service::delegation::DelegationInput;
use myelin_identity_service::machine_auth::{Authority, MachineKind};
use myelin_identity_service::mint::{RunTokenMinter, StructuralTokenSigner};
use myelin_identity_service::revocation::RevocationStore;
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

use myelin_mcp::governance::SkeletonEffectApi;
use myelin_mcp::{CallOutcome, GovernedRouter, McpServer, RunPrincipal, ToolRegistry};

fn now() -> Timestamp {
    Timestamp("2026-06-26T00:00:00Z".into())
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
    let mut p = Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, TenantId(tenant.into()));
    p.region = Region("eu-west".into());
    p
}

/// Build a server with a governed router over a REAL minter (shared S7 store returned so a test can
/// revoke the run token).
fn governed_server() -> McpServer {
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

    let grants = ["git:run"];
    let input = DelegationInput {
        agent_policy: Authority::of(grants),
        delegation: Authority::of(grants),
        tenant_policy: Authority::of(grants),
        trigger_actor_held: Authority::of(grants),
    };

    let principal = RunPrincipal {
        scope,
        agent_id: PrincipalId("agent:claude".into()),
        agent,
        trigger_actor: trigger,
        run_id: RunId("mcp-run-1".into()),
        input,
        caveats: DelegationCaveats(vec!["git:run".into()]),
        kind: MachineKind::Agent,
        ttl: FailStaticBound { static_max_secs: 300 },
    };

    let router = GovernedRouter::new(minter, principal, Box::new(SkeletonEffectApi::new()));
    McpServer::with_router(ToolRegistry::with_git(), router, now())
}

/// Drive the server over a real reader/writer with newline-delimited JSON-RPC (the stdio framing).
fn drive(server: &McpServer, lines: &[&str]) -> Vec<serde_json::Value> {
    let input = lines.join("\n");
    let mut out = Vec::new();
    server.run(Cursor::new(input.into_bytes()), &mut out).expect("run loop");
    String::from_utf8(out)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("each response line is JSON"))
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
    assert_eq!(merge["annotations"]["requiresApproval"], true, "git.merge is HITL-gated");
    let review = tools.iter().find(|t| t["name"] == "git.submit_review").unwrap();
    assert_eq!(review["annotations"]["requiresApproval"], false);
}

#[test]
fn non_gated_tool_mints_a_run_token_and_routes_through_effect_api() {
    let server = governed_server();
    // git.submit_review is NOT requires_approval → it flows mint -> is_live -> EffectApi::apply.
    let resps = drive(
        &server,
        &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.submit_review","arguments":{"number":7}}}"#],
    );
    let r = &resps[0];
    assert_eq!(r["result"]["isError"], false);
    let jti = r["result"]["_meta"]["runToken"].as_str().unwrap();
    assert!(jti.starts_with("runtok:agent:claude:mcp-run-1"), "the run token jti is bound to (agent, run): {jti}");

    // The EVENT ID is produced by SkeletonEffectApi FROM the RunCtx it was handed — so its presence
    // proves the call genuinely routed THROUGH EffectApi::apply, and its contents prove the RunCtx
    // carried the minted jti + the principal (the audit attribution).
    let event_id = r["result"]["_meta"]["eventId"].as_str().unwrap();
    assert!(event_id.contains(jti), "the effect was applied under the minted run token (attribution): {event_id}");
    assert!(event_id.contains("principal:agent:claude"), "attributed to the agent principal: {event_id}");
    assert!(event_id.contains("tool:git.submit_review"));

    // The run actually minted a per-run token (not a bare PAT) + audited the call to the run.
    let router = server.router().unwrap();
    assert!(router.current_token().is_some(), "a per-run token was minted");
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
        &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7}}}"#],
    );
    let r = &resps[0];
    assert!(r["result"]["_meta"]["gateId"].is_string(), "git.merge is gated on HITL");
    assert!(r["result"]["_meta"]["eventId"].is_null(), "a gated effect was NOT applied (no event id)");
    assert!(matches!(server.router().unwrap().audit()[0].outcome, CallOutcome::Gated { .. }));
}

#[test]
fn requires_approval_tool_applies_after_explicit_approval() {
    let server = governed_server();
    // The same git.merge, re-driven WITH an explicit HITL approval → routes through EffectApi::apply.
    let resps = drive(
        &server,
        &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7},"approval":{"granted":true}}}"#],
    );
    let r = &resps[0];
    assert_eq!(r["result"]["isError"], false);
    let event_id = r["result"]["_meta"]["eventId"].as_str().unwrap();
    assert!(event_id.contains("tool:git.merge"), "the approved merge was applied through EffectApi");
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
    router.minter().teardown(&router.principal().scope, &token, &now());

    // A subsequent tools/call under the REVOKED run token is DENIED — never routed to EffectApi.
    let second = server
        .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"git.submit_review","arguments":{"number":2}}}"#)
        .unwrap();
    let second: serde_json::Value = serde_json::from_str(&second).unwrap();
    assert_eq!(second["result"]["isError"], true, "a revoked run token is denied");
    let reason = second["result"]["_meta"]["reason"].as_str().unwrap();
    assert!(reason.contains("revoked"), "denied for revocation: {reason}");
}

#[test]
fn malformed_request_is_a_jsonrpc_error_no_panic() {
    let server = governed_server();
    let resps = drive(&server, &["{ not valid json"]);
    assert_eq!(resps[0]["error"]["code"], -32700, "parse error, never a panic");
}
