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

use myelin_events::{MonotonicMinter, OutboxStore, Timestamp};
use myelin_identity::{
    DelegationCaveats, FailStaticBound, Principal, PrincipalId, PrincipalKind, RunId, RuntimeRef,
};
use myelin_identity_service::delegation::DelegationInput;
use myelin_identity_service::machine_auth::{Authority, MachineKind, StructuralTokenVerifier};
use myelin_identity_service::mint::{RunTokenAuthorizer, RunTokenMinter, StructuralTokenSigner};
use myelin_identity_service::revocation::RevocationStore;
use myelin_identity_service::ResolvedDelegationPolicy;
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

#[cfg(feature = "integration")]
use myelin_edge::{recover_placed_git_at_boot, GitDatabaseProviders};
use myelin_edge::{DenyAllRepos, DurableGitBackend, GitEffectApi, GrantBackedRepos};
use myelin_mcp::{
    GateApproverPolicy, GovernedRouter, McpServer, OutboxGovernanceAudit, RunPrincipal,
    ToolRegistry,
};

const TENANT: &str = "acme";
const REGION: &str = "eu-west";

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

fn now() -> Timestamp {
    Timestamp("2026-06-29T00:00:00Z".into())
}

fn temp_root(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    p.push(format!("myelin-mcp-gt005-{tag}-{nanos}"));
    p
}

/// Delete every `outbox` row for `tenant` (and any `outbox_quarantine` row referencing one),
/// tolerating the SAME documented shared-host race `myelin-storage`'s
/// `tests/common::delete_outbox_for_aggregate` already covers: this dev DB's `outbox`/
/// `outbox_quarantine` tables are shared with the real, live founder dogfood outbox-publisher
/// process, which can claim and quarantine one of this tenant's own event_ids (real reason
/// observed on this host: `wrong_relay_region` — this test's `REGION` differs from the live
/// relay's own configured region) in the narrow gap between our own claim and cleanup. A single
/// delete-quarantine-then-delete-outbox pass can lose that race; retrying a bounded number of
/// times closes the window without pretending to fully serialize against an independent,
/// concurrently-running production process (a bigger, separate concern than this test's own
/// cleanup — same reasoning `myelin-storage`'s helper documents).
#[cfg(feature = "integration")]
async fn delete_outbox_for_tenant(pool: &sqlx::PgPool, tenant: &str) {
    for _ in 0..5 {
        let _ = sqlx::query(
            "DELETE FROM outbox_quarantine WHERE event_id IN \
             (SELECT event_id FROM outbox WHERE envelope->>'tenant'=$1)",
        )
        .bind(tenant)
        .execute(pool)
        .await;
        if sqlx::query("DELETE FROM outbox WHERE envelope->>'tenant'=$1")
            .bind(tenant)
            .execute(pool)
            .await
            .is_ok()
        {
            return;
        }
    }
    panic!("delete_outbox_for_tenant: FK-blocked by outbox_quarantine after 5 retries for tenant {tenant}");
}

fn agent_principal(id: &str) -> Principal {
    agent_principal_in(id, TENANT, REGION)
}

fn agent_principal_in(id: &str, tenant: &str, region: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt-local-claude".into()),
            on_behalf_of: Some(PrincipalId("human:operator".into())),
        },
        TenantId(tenant.into()),
    );
    p.region = Region(region.into());
    p
}

fn human_principal(id: &str) -> Principal {
    human_principal_in(id, TENANT, REGION)
}

fn human_principal_in(id: &str, tenant: &str, region: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region(region.into());
    p
}

/// Build a governed MCP server whose `EffectApi` body is the REAL git effect over `backend`.
fn governed_git_server(backend: Arc<DurableGitBackend>) -> McpServer {
    governed_git_server_with_grants(
        backend,
        &[
            "pull_request.merge",
            "repo.push",
            "pull_request.review",
            "repo.approve_untrusted_ci",
        ],
    )
}

fn governed_git_server_with_grants(backend: Arc<DurableGitBackend>, grants: &[&str]) -> McpServer {
    governed_git_server_with_grants_scoped(backend, grants, TENANT, REGION)
}

fn governed_git_server_with_grants_scoped(
    backend: Arc<DurableGitBackend>,
    grants: &[&str],
    tenant: &str,
    region: &str,
) -> McpServer {
    governed_git_server_with_grants_scoped_at(backend, grants, tenant, region, now())
}

fn governed_git_server_with_grants_scoped_at(
    backend: Arc<DurableGitBackend>,
    grants: &[&str],
    tenant: &str,
    region: &str,
    current: Timestamp,
) -> McpServer {
    let s7 = RevocationStore::new();
    let boundary_now = current.clone();
    let boundary_authorizer = Arc::new(
        RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), s7.clone())
            .with_clock(move || boundary_now.clone()),
    );
    let minter =
        RunTokenMinter::with_signer_and_tuples(s7, None, Arc::new(StructuralTokenSigner::new()));

    let agent = agent_principal_in("agent:claude", tenant, region);
    let trigger = human_principal_in("human:operator", tenant, region);
    let scope = TenantScope::from_verified_token(&trigger, Region(region.into()));

    let input = DelegationInput {
        agent_policy: Authority::of(grants.iter().copied()),
        delegation: Authority::of(grants.iter().copied()),
        tenant_policy: Authority::of(grants.iter().copied()),
        trigger_actor_held: Authority::of(grants.iter().copied()),
    };

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
        agent: agent.clone(),
        trigger_actor: trigger,
        trigger_credential_jti: "trigger-jti".into(),
        trigger_expires_at_unix: i64::MAX,
        run_id,
        resolved_policy,
        caveats: DelegationCaveats(grants.iter().map(|g| (*g).to_string()).collect()),
        kind: MachineKind::Agent,
        ttl: FailStaticBound {
            static_max_secs: 300,
        },
    };

    // THE GT-005 INJECTION: the concrete git EffectApi body over the durable backend, bound to the
    // run's verified (tenant, region) + acting principal. R2.4: + the server-side HITL verdict
    // store (the in-memory double of the durable agent_hitl_gate arm) and the approver set.
    let effect = GitEffectApi::new(backend, tenant, region, agent, boundary_authorizer);
    let router = GovernedRouter::with_approver_policy(
        minter,
        principal,
        Box::new(effect),
        myelin_storage::hitl_gate_durable::HitlVerdictStore::new(),
        Arc::new(TestApprovers(vec![PrincipalId("human:operator".into())])),
        Arc::new(OutboxGovernanceAudit::new(
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()),
        )),
    );
    McpServer::with_router_and_clock(
        ToolRegistry::with_git(),
        router,
        Arc::new(move || current.clone()),
    )
}

#[test]
fn capability_and_object_rebac_are_independent_required_conjuncts() {
    let actor = agent_principal("agent:claude");
    let root_denied = temp_root("object-denied");
    let denied = DurableGitBackend::rooted_inmem_for_test(&root_denied);
    denied
        .create_repo(TENANT, REGION, "alpha")
        .expect("create repo");
    let denied = Arc::new(denied.with_repo_authorizer(Arc::new(DenyAllRepos)));
    let capability_only = governed_git_server(denied.clone());
    let response = capability_only
        .handle_line(&call(
            "git.open_pr",
            serde_json::json!({"repo":"alpha","title":"must not land","head_oid":"deadbeef"}),
        ))
        .unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert!(response["result"]["_meta"]["reason"]
        .as_str()
        .unwrap()
        .contains("object authorization denied"));
    assert!(denied
        .get_pr(TENANT, REGION, "alpha", 1, &actor)
        .unwrap()
        .is_none());

    let root_cap_denied = temp_root("cap-denied");
    let object_granted = DurableGitBackend::rooted_inmem_for_test(&root_cap_denied);
    object_granted
        .create_repo(TENANT, REGION, "alpha")
        .expect("create repo");
    let object_granted = Arc::new(object_granted.with_repo_authorizer(Arc::new(
        GrantBackedRepos::new().grant_write("agent:claude", TENANT, "alpha"),
    )));
    let object_only =
        governed_git_server_with_grants(object_granted.clone(), &["pull_request.review"]);
    let response = object_only
        .handle_line(&call(
            "git.open_pr",
            serde_json::json!({"repo":"alpha","title":"must not land","head_oid":"deadbeef"}),
        ))
        .unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert!(response["result"]["_meta"]["reason"]
        .as_str()
        .unwrap()
        .contains("outside the exact minted delegation intersection"));
    assert!(object_granted
        .get_pr(TENANT, REGION, "alpha", 1, &actor)
        .unwrap()
        .is_none());

    let _ = std::fs::remove_dir_all(root_denied);
    let _ = std::fs::remove_dir_all(root_cap_denied);
}

fn call(name: &str, args: serde_json::Value) -> String {
    call_with_key(name, args, &format!("test-{name}"))
}

fn call_with_key(name: &str, args: serde_json::Value, idempotency_key: &str) -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": name,
            "arguments": args,
            "_meta": { "com.myelin/idempotencyKey": idempotency_key }
        }
    })
    .to_string()
}

/// R2.4: a re-drive PRESENTS the server-issued opaque gate id — the router looks it up in the
/// server-side verdict store (a caller-supplied `granted` boolean is inert and never sent here).
fn call_with_gate(name: &str, args: serde_json::Value, gate_id: &str) -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": name,
            "arguments": args,
            "approval": { "gateId": gate_id },
            "_meta": { "com.myelin/idempotencyKey": format!("test-{name}") }
        }
    })
    .to_string()
}

/// `git.open_pr` through the governed router lands a DURABLE PR (fresh read reflects it), attributed to
/// the minted run token + principal + tool.
#[test]
fn open_pr_routes_through_effect_api_and_persists_durably() {
    let actor = agent_principal("agent:claude");
    let root = temp_root("openpr");
    let backend = Arc::new(DurableGitBackend::rooted_inmem_for_test(&root));
    backend
        .create_repo(TENANT, REGION, "alpha")
        .expect("create repo");

    let server = governed_git_server(backend.clone());

    // Pre-state: no PR yet.
    assert!(backend
        .get_pr(TENANT, REGION, "alpha", 1, &actor)
        .unwrap()
        .is_none());

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
    assert!(
        event_id.contains("git.pr.open"),
        "the durable git effect ran: {event_id}"
    );
    assert!(
        event_id.contains(jti),
        "attributed to the minted run token: {event_id}"
    );
    assert!(
        event_id.contains("principal:agent:claude"),
        "attributed to the principal: {event_id}"
    );
    assert!(event_id.contains("tool:git.open_pr"));

    // THE DURABLE EFFECT: a FRESH read of the on-disk backend reflects the new PR.
    let rec = backend
        .get_pr(TENANT, REGION, "alpha", 1, &actor)
        .unwrap()
        .expect("the PR persisted durably through EffectApi");
    assert_eq!(rec.number, 1);
    assert_eq!(rec.author_pseudonym, "agent:claude@acme.noreply");

    // A SECOND fresh backend instance over the SAME root (a simulated restart) still serves it.
    let fresh = DurableGitBackend::rooted_inmem_for_test(&root);
    assert!(
        fresh
            .get_pr(TENANT, REGION, "alpha", 1, &actor)
            .unwrap()
            .is_some(),
        "survives a fresh backend"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn caller_keys_distinguish_intentional_identical_calls() {
    let actor = agent_principal("agent:claude");
    let root = temp_root("invocation-keys");
    let backend = Arc::new(DurableGitBackend::rooted_inmem_for_test(&root));
    backend.create_repo(TENANT, REGION, "alpha").unwrap();
    let server = governed_git_server(backend.clone());
    let args = serde_json::json!({
        "repo": "alpha",
        "title": "Intentional duplicate shape",
        "head_oid": "deadbeef",
        "base_ref": "refs/heads/main"
    });

    let missing_key: serde_json::Value = serde_json::from_str(
        &server
            .handle_line(
                &serde_json::json!({
                    "jsonrpc":"2.0",
                    "id":1,
                    "method":"tools/call",
                    "params":{"name":"git.open_pr","arguments":args.clone()}
                })
                .to_string(),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(missing_key["error"]["code"], -32602);
    assert!(backend
        .get_pr(TENANT, REGION, "alpha", 1, &actor)
        .unwrap()
        .is_none());

    for key in ["intentional-call-1", "intentional-call-2"] {
        let response: serde_json::Value = serde_json::from_str(
            &server
                .handle_line(&call_with_key("git.open_pr", args.clone(), key))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(response["result"]["isError"], false, "{response}");
    }
    assert!(backend
        .get_pr(TENANT, REGION, "alpha", 2, &actor)
        .unwrap()
        .is_some());

    let _ = std::fs::remove_dir_all(&root);
}

/// A response-lost MCP re-drive under the same authenticated run reuses the exact operation digest:
/// PostgreSQL contains one PR, one review, and one lifecycle event for each mutation.
#[cfg(feature = "integration")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn response_lost_retry_is_exactly_once_for_open_review_and_events() {
    use myelin_config::{Mode, MyelinConfig};
    use myelin_events::{Actor, EmitContextBase, UlidMinter};
    use myelin_git::durable::DurableGitStore;
    use myelin_git::pg_pr_store::{MergeIntent, PrOperationId};
    use myelin_git::receive_pack::{
        CrashPoint, InMemoryObjectDb, Oid as PushOid, ProposedRefUpdate, PushOutcome, PushSession,
        Pusher, RefName, RefStore,
    };
    use myelin_storage::{
        all_durable_migrations, DurableCellRow, DurableLocalTenantRow, DurablePlacementBacking,
        DurablePlacementRow, HotTables, KmsEngine, PgBootstrap, PgOutboxBacking,
    };

    let mut cfg = MyelinConfig::from_env(Mode::DevDefaults).expect("integration config");
    cfg.region = REGION.into();
    let bootstrap = PgBootstrap::connect(cfg.clone(), 8)
        .await
        .expect("validated split roles");
    bootstrap
        .migrate_foundation()
        .await
        .expect("foundation migrations");
    bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("durable placement migrations");
    bootstrap
        .migrate(
            &myelin_git::pg_pr_store::git_pr_migrations(),
            &myelin_git::pg_pr_store::git_pr_hot_tables(),
        )
        .await
        .expect("Git PR migrations");
    bootstrap
        .verify_index_ready("git_pr_head_repo_idx")
        .await
        .expect("Git PR provenance index");
    bootstrap
        .verify_index_ready("git_pr_command_operation_scope_uidx")
        .await
        .expect("tenant/region-global PR operation index");
    let provider = bootstrap.into_runtime().await.expect("runtime provider");
    let handle = tokio::runtime::Handle::current();
    let root = temp_root("response-lost-pg");
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        handle.clone(),
    )));
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let live_tenant = format!("mcp-retry-tenant-{suffix}");
    let slug = format!("mcp-retry-{suffix}");
    let other_slug = format!("mcp-retry-other-{suffix}");
    let backend_outbox = outbox.clone();
    let recovery_provider = provider.clone();
    let check_admission_provider = provider
        .auxiliary_runtime_lane(1)
        .await
        .expect("dedicated check-admission lane");
    let backend = Arc::new(
        DurableGitBackend::rooted(
            &root,
            String::new(),
            GitDatabaseProviders::new(provider, check_admission_provider),
            Arc::new(KmsEngine::new()),
            handle,
            outbox,
            Arc::new(UlidMinter::new()),
        )
        .expect("production PG Git backend")
        .with_repo_authorizer(Arc::new(
            GrantBackedRepos::new()
                .grant_write("agent:claude", &live_tenant, &slug)
                .grant_write("agent:claude", &live_tenant, &other_slug),
        )),
    );
    backend
        .create_repo(&live_tenant, REGION, &slug)
        .expect("create target repo");
    backend
        .create_repo(&live_tenant, REGION, &other_slug)
        .expect("create second target repo");
    let git = DurableGitStore::rooted(&root);
    let loc = myelin_git::core::RepoLoc::new(&live_tenant, REGION, &slug);
    let repo = Arc::new(git.open_repo(&loc).expect("open target repo"));
    let other_loc = myelin_git::core::RepoLoc::new(&live_tenant, REGION, &other_slug);
    let other_repo = Arc::new(git.open_repo(&other_loc).expect("open second target repo"));
    let (base, _, _) = repo
        .build_file_commit(
            "refs/heads/main",
            "file.txt",
            b"base\n",
            "base",
            "psn@acme.noreply",
            "psn@acme.noreply",
        )
        .expect("base commit");
    repo.update_ref_cas(
        "refs/heads/main",
        None,
        Some(&base),
        "seed main",
        "psn@acme.noreply",
    )
    .expect("main ref");
    let (head, _, _) = repo
        .build_file_commit(
            "refs/heads/main",
            "file.txt",
            b"head\n",
            "head",
            "psn@acme.noreply",
            "psn@acme.noreply",
        )
        .expect("head commit");
    repo.update_ref_cas(
        "refs/heads/feature",
        None,
        Some(&head),
        "seed feature",
        "psn@acme.noreply",
    )
    .expect("feature ref");
    let (other_base, _, _) = other_repo
        .build_file_commit(
            "refs/heads/main",
            "file.txt",
            b"other base\n",
            "other base",
            "psn@acme.noreply",
            "psn@acme.noreply",
        )
        .expect("second-repo base commit");
    other_repo
        .update_ref_cas(
            "refs/heads/main",
            None,
            Some(&other_base),
            "seed second-repo main",
            "psn@acme.noreply",
        )
        .expect("second-repo main ref");
    let (other_head, _, _) = other_repo
        .build_file_commit(
            "refs/heads/main",
            "file.txt",
            b"other head\n",
            "other head",
            "psn@acme.noreply",
            "psn@acme.noreply",
        )
        .expect("second-repo head commit");
    other_repo
        .update_ref_cas(
            "refs/heads/feature",
            None,
            Some(&other_head),
            "seed second-repo feature",
            "psn@acme.noreply",
        )
        .expect("second-repo feature ref");

    let open = call_with_key(
        "git.open_pr",
        serde_json::json!({
            "repo": slug,
            "title": "Retry-safe PR",
            "head_ref": "refs/heads/feature",
            "head_oid": head.0.clone(),
            "base_ref": "refs/heads/main"
        }),
        "response-lost-open-1",
    );
    let mut open_jtis = Vec::new();
    for timestamp in ["2026-06-29T00:00:00Z", "2026-06-29T00:00:01Z"] {
        // A fresh server/router has no in-memory token state and therefore remints a new JTI. The
        // caller-stable namespaced key is the only invocation identity carried across this restart.
        let server = governed_git_server_with_grants_scoped_at(
            backend.clone(),
            &[
                "pull_request.merge",
                "repo.push",
                "pull_request.review",
                "repo.approve_untrusted_ci",
            ],
            &live_tenant,
            REGION,
            Timestamp(timestamp.into()),
        );
        let value: serde_json::Value =
            serde_json::from_str(&server.handle_line(&open).expect("open response")).unwrap();
        assert_eq!(
            value["result"]["isError"], false,
            "open retry applied: {value}"
        );
        open_jtis.push(
            value["result"]["_meta"]["runToken"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert_ne!(open_jtis[0], open_jtis[1], "restart reminted the run token");

    // The key namespace is tenant/region-global, not repository-local. Reusing the open key with
    // different arguments against another repository is rejected before that repository allocates
    // a PR number, writes state, or emits an event.
    let cross_repo = call_with_key(
        "git.open_pr",
        serde_json::json!({
            "repo": other_slug,
            "title": "Must not land",
            "head_ref": "refs/heads/feature",
            "head_oid": other_head.0.clone(),
            "base_ref": "refs/heads/main"
        }),
        "response-lost-open-1",
    );
    let cross_repo_server = governed_git_server_with_grants_scoped_at(
        backend.clone(),
        &[
            "pull_request.merge",
            "repo.push",
            "pull_request.review",
            "repo.approve_untrusted_ci",
        ],
        &live_tenant,
        REGION,
        Timestamp("2026-06-29T00:00:02Z".into()),
    );
    let cross_repo_value: serde_json::Value = serde_json::from_str(
        &cross_repo_server
            .handle_line(&cross_repo)
            .expect("cross-repo misuse response"),
    )
    .unwrap();
    assert_eq!(
        cross_repo_value["result"]["isError"], true,
        "{cross_repo_value}"
    );
    assert!(
        cross_repo_value["result"]["_meta"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("operation id conflicts with durable state")),
        "a valid second-repository request must reach the durable operation ledger: {cross_repo_value}"
    );
    let actor = agent_principal_in("agent:claude", &live_tenant, REGION);
    assert!(backend
        .get_pr(&live_tenant, REGION, &other_slug, 1, &actor)
        .unwrap()
        .is_none());

    // Reusing an invocation key for a different command in the same repository does not mint a
    // different operation identity. The durable command ledger sees a divergent payload and
    // rejects it loudly; no review/event is appended.
    let misused = call_with_key(
        "git.submit_review",
        serde_json::json!({"repo": slug, "number": 1, "verdict": "approve"}),
        "response-lost-open-1",
    );
    let misuse_server = governed_git_server_with_grants_scoped_at(
        backend.clone(),
        &[
            "pull_request.merge",
            "repo.push",
            "pull_request.review",
            "repo.approve_untrusted_ci",
        ],
        &live_tenant,
        REGION,
        Timestamp("2026-06-29T00:00:03Z".into()),
    );
    let misuse_value: serde_json::Value = serde_json::from_str(
        &misuse_server
            .handle_line(&misused)
            .expect("misuse response"),
    )
    .unwrap();
    assert_eq!(misuse_value["result"]["isError"], true, "{misuse_value}");
    assert!(backend
        .get_pr(&live_tenant, REGION, &slug, 1, &actor)
        .unwrap()
        .unwrap()
        .reviews
        .is_empty());

    let review = call_with_key(
        "git.submit_review",
        serde_json::json!({"repo": slug, "number": 1, "verdict": "approve"}),
        "response-lost-review-1",
    );
    let mut review_jtis = Vec::new();
    for timestamp in ["2026-06-29T00:00:04Z", "2026-06-29T00:00:05Z"] {
        let server = governed_git_server_with_grants_scoped_at(
            backend.clone(),
            &[
                "pull_request.merge",
                "repo.push",
                "pull_request.review",
                "repo.approve_untrusted_ci",
            ],
            &live_tenant,
            REGION,
            Timestamp(timestamp.into()),
        );
        let value: serde_json::Value =
            serde_json::from_str(&server.handle_line(&review).expect("review response")).unwrap();
        assert_eq!(
            value["result"]["isError"], false,
            "review retry applied: {value}"
        );
        review_jtis.push(
            value["result"]["_meta"]["runToken"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert_ne!(review_jtis[0], review_jtis[1], "review retry also reminted");

    let record = backend
        .get_pr(&live_tenant, REGION, &slug, 1, &actor)
        .unwrap()
        .expect("one PR");
    assert_eq!(
        record.reviews.len(),
        1,
        "review retry appended exactly once"
    );
    assert!(
        backend
            .get_pr(&live_tenant, REGION, &slug, 2, &actor)
            .unwrap()
            .is_none(),
        "open retry allocated no second PR"
    );

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_migration_url)
        .await
        .expect("admin assertions");
    let aggregate = format!("git/pr/{slug}:1");
    let opened_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE aggregate=$1 AND envelope->>'type_'='git.pr.opened'",
    )
    .bind(&aggregate)
    .fetch_one(&admin)
    .await
    .unwrap();
    let updated_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE aggregate=$1 AND envelope->>'type_'='git.pr.updated'",
    )
    .bind(&aggregate)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(opened_events, 1, "one open event across response loss");
    assert_eq!(updated_events, 1, "one review event across response loss");
    let cross_repo_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE aggregate LIKE $1 AND envelope->>'type_'='git.pr.opened'",
    )
    .bind(format!("git/pr/{other_slug}:%"))
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        cross_repo_events, 0,
        "cross-repo key misuse emitted no event"
    );
    let commands: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM git_pr_command
          WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3
            AND length(operation_id)=64 AND operation_id ~ '^[0-9a-f]{64}$'",
    )
    .bind(&live_tenant)
    .bind(REGION)
    .bind(&slug)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        commands, 2,
        "one digest-only command per logical MCP effect"
    );

    // Seed both boot-recovery classes. First, a committed ref witness whose filesystem CAS did not
    // run. Second, a durable pending merge intent/command whose locked target CAS still has to run.
    let recovery_ctx = EmitContextBase {
        tenant: TenantId(live_tenant.clone()),
        region: Region(REGION.into()),
        actor: Actor(actor.clone()),
        schema_ver: 1,
        occurred_at: now(),
        recorded_at: now(),
        caused_by: None,
    };
    let ref_store = RefStore::open_durable(
        repo.clone(),
        slug.clone(),
        recovery_ctx,
        backend_outbox,
        Arc::new(UlidMinter::new()),
    );
    let witness_ref = RefName::new("refs/heads/recovery-witness");
    let witness_push = PushSession {
        updates: vec![ProposedRefUpdate {
            ref_name: witness_ref.clone(),
            expected_old: PushOid::zero(),
            new_oid: PushOid::new(head.0.clone()),
            forced: false,
            commit_oids: vec![PushOid::new(head.0.clone())],
        }],
        quarantine: Vec::new(),
        pusher: Pusher {
            pseudonym: "git-event:test-recovery".into(),
            is_agent: false,
        },
    };
    assert!(matches!(
        ref_store
            .receive(
                &witness_push,
                &InMemoryObjectDb::new(),
                CrashPoint::AfterCommitBeforeApply,
            )
            .unwrap(),
        PushOutcome::Crashed(_)
    ));
    assert!(repo.read_ref(&witness_ref.0).unwrap().is_none());

    let merge_operation = PrOperationId::derive(
        "myelin.test.boot-merge.v1",
        &[live_tenant.as_bytes(), slug.as_bytes()],
    )
    .unwrap();
    let intent = MergeIntent {
        operation_id: merge_operation.digest().into(),
        actor_subject_id: actor.principal_id.0.clone(),
        base_ref: "refs/heads/main".into(),
        expected_old_oid: base.0.clone(),
        head_oid: head.0.clone(),
        head_repo_slug: slug.clone(),
    };
    let merge_payload = serde_json::to_vec(&(
        1_u64,
        intent.base_ref.as_str(),
        intent.head_oid.as_str(),
        intent.head_repo_slug.as_str(),
    ))
    .unwrap();
    let merge_hash = blake3::hash(&merge_payload).to_hex().to_string();
    sqlx::query(
        "UPDATE git_pr SET merge_intent=$5
          WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4",
    )
    .bind(&live_tenant)
    .bind(REGION)
    .bind(&slug)
    .bind(1_i64)
    .bind(serde_json::to_value(&intent).unwrap())
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO git_pr_command
         (tenant_id,region,repo_slug,operation_id,actor_subject_id,command_kind,payload_hash,
          pr_number,status,result)
         VALUES ($1,$2,$3,$4,$5,'merge',$6,1,'pending',NULL)",
    )
    .bind(&live_tenant)
    .bind(REGION)
    .bind(&slug)
    .bind(merge_operation.digest())
    .bind(&intent.actor_subject_id)
    .bind(merge_hash)
    .execute(&admin)
    .await
    .unwrap();

    let cell_id = format!("cell-mcp-recovery-{suffix}");
    let placements = DurablePlacementBacking::new(recovery_provider.db_pool().clone());
    placements
        .insert_cell(&DurableCellRow {
            cell_id: cell_id.clone(),
            region: REGION.into(),
            status: "Active".into(),
            isolation_kind: "Shared".into(),
            tenants_max: 100,
            write_qps_max: 1000,
            storage_bytes_max: 1_000_000,
            utilisation: 0,
            version: 1,
            endpoint: "http://127.0.0.1/recovery-test".into(),
        })
        .await
        .unwrap();
    placements
        .place_tenant(&DurablePlacementRow {
            tenant_id: live_tenant.clone(),
            region: REGION.into(),
            home_cell: cell_id.clone(),
            isolation_tier: "Shared".into(),
            slug: format!("tenant-{suffix}"),
            status: "Active".into(),
            member_cells: vec![cell_id.clone()],
        })
        .await
        .unwrap();
    placements
        .upsert_local_tenant(&DurableLocalTenantRow {
            cell_id: cell_id.clone(),
            tenant_id: live_tenant.clone(),
            isolation_tier: "Shared".into(),
            active: true,
        })
        .await
        .unwrap();

    let recovered = recover_placed_git_at_boot(&backend, &recovery_provider, &cell_id)
        .await
        .expect("placement-derived boot recovery");
    assert_eq!(recovered.tenants_recovered, 1);
    assert_eq!(recovered.repos_reconciled, 1);
    assert_eq!(recovered.refs_reapplied, 1, "ref witness reconciled first");
    assert_eq!(recovered.merges_recovered, 1, "pending merge drained");
    assert_eq!(repo.read_ref(&witness_ref.0).unwrap().unwrap().0, head.0);
    assert_eq!(repo.read_ref("refs/heads/main").unwrap().unwrap().0, head.0);
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM git_pr WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3
          AND merge_intent IS NOT NULL",
    )
    .bind(&live_tenant)
    .bind(REGION)
    .bind(&slug)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(pending, 0, "boot recovery cleared the durable intent");

    placements
        .set_placement_status(&live_tenant, "Offboarding")
        .await
        .unwrap();
    assert!(
        recover_placed_git_at_boot(&backend, &recovery_provider, &cell_id)
            .await
            .is_err(),
        "an active local entry with non-Active canonical placement fails boot loud"
    );

    delete_outbox_for_tenant(&admin, &live_tenant).await;
    for table in ["git_pr_command", "git_pr", "git_pr_counter"] {
        for repo_slug in [&slug, &other_slug] {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3"
            ))
            .bind(&live_tenant)
            .bind(REGION)
            .bind(repo_slug)
            .execute(&admin)
            .await
            .unwrap();
        }
    }
    sqlx::query("DELETE FROM local_tenant WHERE cell_id=$1 AND tenant_id=$2")
        .bind(&cell_id)
        .bind(&live_tenant)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM tenant_placement WHERE tenant_id=$1")
        .bind(&live_tenant)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM cell WHERE cell_id=$1")
        .bind(&cell_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    let _ = std::fs::remove_dir_all(root);
}

/// `git.merge` is HITL-gated (frozen `requires_approval`): WITHOUT approval it is withheld (not
/// applied); WITH approval it routes to `EffectApi::apply` → the GT-003 merge gate BLOCKS (unmet
/// branch-protection policy) → Denied with the gate reason. The PR is NEVER merged (the gate is
/// server-enforced; the MCP tool reflects it, never bypasses).
#[test]
fn merge_is_hitl_gated_then_reflects_the_server_gate_block() {
    let actor = agent_principal("agent:claude");
    let root = temp_root("merge");
    let backend = Arc::new(DurableGitBackend::rooted_inmem_for_test(&root));
    backend
        .create_repo(TENANT, REGION, "alpha")
        .expect("create repo");
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
        .handle_line(&call(
            "git.merge",
            serde_json::json!({ "repo": "alpha", "number": 1 }),
        ))
        .unwrap();
    let g: serde_json::Value = serde_json::from_str(&gated).unwrap();
    let gate_id = g["result"]["_meta"]["gateId"]
        .as_str()
        .expect("git.merge is HITL-gated")
        .to_string();
    assert!(
        g["result"]["_meta"]["eventId"].is_null(),
        "a gated merge did NOT apply"
    );

    // (2) A DISTINCT human approves SERVER-SIDE (R2.4 — never a caller boolean), then the re-drive
    //     presenting the gate id routes through EffectApi::apply → the server merge gate BLOCKS.
    //     Denied, carrying the gate reason (the tool REFLECTS the gate, never bypasses).
    server
        .router()
        .unwrap()
        .approve_gate(&human_principal("human:operator"), &gate_id, &now())
        .expect("the human operator approves the merge card");
    let denied = server
        .handle_line(&call_with_gate(
            "git.merge",
            serde_json::json!({ "repo": "alpha", "number": 1 }),
            &gate_id,
        ))
        .unwrap();
    let d: serde_json::Value = serde_json::from_str(&denied).unwrap();
    assert_eq!(
        d["result"]["isError"], true,
        "the gate-blocked merge is denied: {d}"
    );
    let reason = d["result"]["_meta"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("merge blocked by policy"),
        "the gate reason is surfaced: {reason}"
    );

    // THE GATE WAS NOT BYPASSED: the PR is still open on disk (never merged).
    let rec = backend
        .get_pr(TENANT, REGION, "alpha", 1, &actor)
        .unwrap()
        .unwrap();
    assert_ne!(
        format!("{:?}", rec.state),
        "Merged",
        "the merge gate is server-enforced; the MCP tool cannot bypass it"
    );

    let _ = std::fs::remove_dir_all(&root);
}
