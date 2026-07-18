//! # GT-005 — a governed MCP `tools/call` lands the REAL durable git effect (mint → EffectApi → GT-003).
//!
//! The MR-021 `GovernedRouter` is the chokepoint (`mint_run_token → revocation consult → HITL gate →
//! EffectApi::apply`, audited). GT-005 connects it to the real git effect by injecting
//! [`myelin_edge::GitEffectApi`] — the concrete `EffectApi` body bound to the DURABLE GT-003 backend
//! ([`myelin_edge::DurableGitBackend`]) under the run's verified `(tenant, region)` scope. This drives
//! the MCP server as a real JSON-RPC peer and proves, against the running protocol + an on-disk repo:
//!
//!  - `tools/call git.open_pr` (NOT requires_approval) → MINTS a per-run token, routes through
//!    `EffectApi::apply`, and the PR PERSISTS on disk (a fresh durable read reflects it); the result is
//!    attributed to the run token + principal + tool (audit).
//!  - `tools/call git.merge` (requires_approval) → HITL-gated WITHOUT approval (withheld, NOT applied);
//!    WITH approval it routes to `EffectApi::apply` → the GT-003 merge gate BLOCKS (the repo-owned
//!    branch-protection policy is unmet) → a loud Denied carrying the gate reason. The MCP tool
//!    REFLECTS the server gate; it never bypasses it (the PR stays open on disk).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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

use myelin_edge::{DurableGitBackend, GitEffectApi};
use myelin_mcp::{GovernedRouter, McpServer, RunPrincipal, ToolRegistry};

const TENANT: &str = "acme";
const REGION: &str = "eu-west";

fn now() -> Timestamp {
    Timestamp("2026-06-29T00:00:00Z".into())
}

fn temp_root(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    p.push(format!("myelin-mcp-gt005-{tag}-{nanos}"));
    p
}

fn agent_principal(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt-local-claude".into()),
            on_behalf_of: Some(PrincipalId("human:operator".into())),
        },
        TenantId(TENANT.into()),
    );
    p.region = Region(REGION.into());
    p
}

fn human_principal(id: &str) -> Principal {
    let mut p = Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, TenantId(TENANT.into()));
    p.region = Region(REGION.into());
    p
}

/// Build a governed MCP server whose `EffectApi` body is the REAL git effect over `backend`.
fn governed_git_server(backend: Arc<DurableGitBackend>) -> McpServer {
    let s7 = RevocationStore::new();
    let minter =
        RunTokenMinter::with_signer_and_tuples(s7, None, Arc::new(StructuralTokenSigner::new()));

    let agent = agent_principal("agent:claude");
    let trigger = human_principal("human:operator");
    let scope = TenantScope::from_verified_token(&trigger, Region(REGION.into()));

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
        agent: agent.clone(),
        trigger_actor: trigger,
        run_id: RunId("mcp-run-1".into()),
        input,
        caveats: DelegationCaveats(vec!["git:run".into()]),
        kind: MachineKind::Agent,
        ttl: FailStaticBound { static_max_secs: 300 },
    };

    // THE GT-005 INJECTION: the concrete git EffectApi body over the durable backend, bound to the
    // run's verified (tenant, region) + acting principal. R2.4: + the server-side HITL verdict
    // store (the in-memory double of the durable agent_hitl_gate arm) and the approver set.
    let effect = GitEffectApi::new(backend, TENANT, REGION, agent);
    let router = GovernedRouter::new(
        minter,
        principal,
        Box::new(effect),
        myelin_storage::hitl_gate_durable::HitlVerdictStore::new(),
        vec![PrincipalId("human:operator".into())],
    );
    McpServer::with_router_and_clock(ToolRegistry::with_git(), router, Arc::new(now))
}

fn call(name: &str, args: serde_json::Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": args }
    })
    .to_string()
}

/// R2.4: a re-drive PRESENTS the server-issued opaque gate id — the router looks it up in the
/// server-side verdict store (a caller-supplied `granted` boolean is inert and never sent here).
fn call_with_gate(name: &str, args: serde_json::Value, gate_id: &str) -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": args, "approval": { "gateId": gate_id } }
    })
    .to_string()
}

/// `git.open_pr` through the governed router lands a DURABLE PR (fresh read reflects it), attributed to
/// the minted run token + principal + tool.
#[test]
fn open_pr_routes_through_effect_api_and_persists_durably() {
    let root = temp_root("openpr");
    let backend = Arc::new(DurableGitBackend::rooted_inmem_for_test(&root));
    backend.create_repo(TENANT, REGION, "alpha").expect("create repo");

    let server = governed_git_server(backend.clone());

    // Pre-state: no PR yet.
    assert!(backend.get_pr(TENANT, REGION, "alpha", 1).unwrap().is_none());

    let resp = server
        .handle_line(&call(
            "git.open_pr",
            serde_json::json!({ "repo": "alpha", "title": "Alpha PR", "head_oid": "deadbeef", "base_ref": "refs/heads/main" }),
        ))
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["result"]["isError"], false, "open_pr applied: {v}");

    // Attribution: the effect routed THROUGH EffectApi under the minted run token (jti) + principal +
    // tool — the event id is produced by GitEffectApi from the RunCtx it was handed.
    let jti = v["result"]["_meta"]["runToken"].as_str().unwrap();
    let event_id = v["result"]["_meta"]["eventId"].as_str().unwrap();
    assert!(event_id.contains("git.pr.open"), "the durable git effect ran: {event_id}");
    assert!(event_id.contains(jti), "attributed to the minted run token: {event_id}");
    assert!(event_id.contains("principal:agent:claude"), "attributed to the principal: {event_id}");
    assert!(event_id.contains("tool:git.open_pr"));

    // THE DURABLE EFFECT: a FRESH read of the on-disk backend reflects the new PR.
    let rec = backend
        .get_pr(TENANT, REGION, "alpha", 1)
        .unwrap()
        .expect("the PR persisted durably through EffectApi");
    assert_eq!(rec.number, 1);
    assert_eq!(rec.author_pseudonym, "agent:claude@acme.noreply");

    // A SECOND fresh backend instance over the SAME root (a simulated restart) still serves it.
    let fresh = DurableGitBackend::rooted_inmem_for_test(&root);
    assert!(fresh.get_pr(TENANT, REGION, "alpha", 1).unwrap().is_some(), "survives a fresh backend");

    let _ = std::fs::remove_dir_all(&root);
}

/// `git.merge` is HITL-gated (frozen `requires_approval`): WITHOUT approval it is withheld (not
/// applied); WITH approval it routes to `EffectApi::apply` → the GT-003 merge gate BLOCKS (unmet
/// branch-protection policy) → Denied with the gate reason. The PR is NEVER merged (the gate is
/// server-enforced; the MCP tool reflects it, never bypasses).
#[test]
fn merge_is_hitl_gated_then_reflects_the_server_gate_block() {
    let root = temp_root("merge");
    let backend = Arc::new(DurableGitBackend::rooted_inmem_for_test(&root));
    backend.create_repo(TENANT, REGION, "alpha").expect("create repo");
    // Repo-owned branch protection: require a CI context that is never green → the merge gate must block.
    backend
        .set_branch_protection(
            TENANT,
            REGION,
            "alpha",
            &serde_json::json!({
                "rulesets": [{
                    "ref_pattern": "refs/heads/main",
                    "required_contexts": ["ci/build"],
                    "required_approvals": 1
                }]
            }),
        )
        .expect("set branch protection");
    // Open PR #1 (no green checks, no approvals) — the merge gate will block it.
    backend
        .open_pr(
            TENANT,
            REGION,
            "alpha",
            &serde_json::json!({ "title": "Alpha PR", "head_oid": "deadbeef" }),
            &agent_principal("agent:claude"),
        )
        .expect("open pr");

    let server = governed_git_server(backend.clone());

    // (1) WITHOUT approval → HITL-gated (withheld, NOT applied). No EffectApi::apply ran. The
    //     gate id is the R2.4 opaque server-issued token backing a `waiting` verdict row.
    let gated = server
        .handle_line(&call("git.merge", serde_json::json!({ "repo": "alpha", "number": 1 })))
        .unwrap();
    let g: serde_json::Value = serde_json::from_str(&gated).unwrap();
    let gate_id = g["result"]["_meta"]["gateId"].as_str().expect("git.merge is HITL-gated").to_string();
    assert!(g["result"]["_meta"]["eventId"].is_null(), "a gated merge did NOT apply");

    // (2) A DISTINCT human approves SERVER-SIDE (R2.4 — never a caller boolean), then the re-drive
    //     presenting the gate id routes through EffectApi::apply → the server merge gate BLOCKS.
    //     Denied, carrying the gate reason (the tool REFLECTS the gate, never bypasses).
    server
        .router()
        .unwrap()
        .approve_gate(&human_principal("human:operator"), &gate_id)
        .expect("the human operator approves the merge card");
    let denied = server
        .handle_line(&call_with_gate(
            "git.merge",
            serde_json::json!({ "repo": "alpha", "number": 1 }),
            &gate_id,
        ))
        .unwrap();
    let d: serde_json::Value = serde_json::from_str(&denied).unwrap();
    assert_eq!(d["result"]["isError"], true, "the gate-blocked merge is denied: {d}");
    let reason = d["result"]["_meta"]["reason"].as_str().unwrap();
    assert!(reason.contains("merge blocked by policy"), "the gate reason is surfaced: {reason}");

    // THE GATE WAS NOT BYPASSED: the PR is still open on disk (never merged).
    let rec = backend.get_pr(TENANT, REGION, "alpha", 1).unwrap().unwrap();
    assert_ne!(
        format!("{:?}", rec.state),
        "Merged",
        "the merge gate is server-enforced; the MCP tool cannot bypass it"
    );

    let _ = std::fs::remove_dir_all(&root);
}
