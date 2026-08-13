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

use myelin_mcp::governance::SkeletonEffectApi;
use myelin_mcp::{
    AuditPhase, CallOutcome, DirectReadError, DirectReadExecutor, GateApproverPolicy,
    GovernanceAudit, GovernanceAuditRecord, GovernanceAuditTarget, GovernedRouter,
    IssuedGovernedRun, McpServer, OutboxGovernanceAudit, ReadAuthorization, RunPrincipal,
    ToolRegistry,
};
use myelin_storage::hitl_gate_durable::{
    gate_ref_token, GateDecideError, GateRecord, GateState, HitlVerdictStore,
};

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

struct FailAttemptAudit;

impl GovernanceAudit for FailAttemptAudit {
    fn record(&self, record: GovernanceAuditRecord<'_>) -> Result<(), String> {
        if record.phase == AuditPhase::Attempt {
            Err("injected durable audit outage".into())
        } else {
            Ok(())
        }
    }
}

struct FailExpiryAudit;

impl GovernanceAudit for FailExpiryAudit {
    fn record(&self, record: GovernanceAuditRecord<'_>) -> Result<(), String> {
        if record.phase == AuditPhase::Expired {
            assert!(
                matches!(record.target, GovernanceAuditTarget::Gate(_)),
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

fn service_principal(id: &str, tenant: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Service,
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

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
    let caveats = DelegationCaveats(input.delegation.grants().map(str::to_string).collect());
    governed_router_with_caveats_and_verdicts(
        input,
        caveats,
        trigger_jti,
        trigger_expires_at_unix,
        audit,
        run_ttl_secs,
        verdicts,
    )
}

fn governed_router_with_caveats_and_verdicts(
    input: DelegationInput,
    caveats: DelegationCaveats,
    trigger_jti: &str,
    trigger_expires_at_unix: i64,
    audit: Arc<dyn GovernanceAudit>,
    run_ttl_secs: u64,
    verdicts: HitlVerdictStore,
) -> GovernedRouter {
    let s7 = RevocationStore::new();
    let minter =
        RunTokenMinter::with_signer_and_tuples(s7, None, Arc::new(StructuralTokenSigner::new()));

    let agent = agent_principal("agent:claude", "acme");
    let trigger = human_principal("human:operator", "acme");
    let scope = TenantScope::from_verified_token(&trigger, Region("eu-west".into()));

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
        caveats,
        kind: MachineKind::Agent,
        ttl: FailStaticBound {
            static_max_secs: run_ttl_secs,
        },
    };

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

struct RecordingReadExecutor {
    calls: AtomicUsize,
}

impl DirectReadExecutor for RecordingReadExecutor {
    fn execute(
        &self,
        principal: &Principal,
        authority: &ReadAuthorization,
        tool: &str,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, DirectReadError> {
        assert_eq!(principal.principal_id.0, "agent:claude");
        assert_eq!(principal.tenant.0, "acme");
        assert_eq!(principal.region.0, "eu-west");
        assert_eq!(authority.tool(), tool);
        assert_eq!(authority.required_caps(), ["run.view"]);
        assert!(!authority.jti().is_empty());
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::json!({
            "tool": tool,
            "run_id": arguments["run_id"],
        }))
    }
}

struct CountingReadExecutor {
    calls: AtomicUsize,
}

impl DirectReadExecutor for CountingReadExecutor {
    fn execute(
        &self,
        _principal: &Principal,
        _authority: &ReadAuthorization,
        _tool: &str,
        _arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, DirectReadError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::json!({"unexpected": true}))
    }
}

fn ci_read_router(grants: &[&str]) -> GovernedRouter {
    ci_read_router_with_audit(
        grants,
        Arc::new(OutboxGovernanceAudit::new(
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()),
        )),
    )
}

fn ci_read_router_with_audit(grants: &[&str], audit: Arc<dyn GovernanceAudit>) -> GovernedRouter {
    let input = DelegationInput {
        agent_policy: Authority::of(grants.iter().copied()),
        delegation: Authority::of(grants.iter().copied()),
        tenant_policy: Authority::of(grants.iter().copied()),
        trigger_actor_held: Authority::of(grants.iter().copied()),
    };
    governed_router_with_trigger_and_audit(input, "trigger-jti", i64::MAX, audit)
}

fn caveated_router(grants: &[&str], caveats: &[&str]) -> GovernedRouter {
    let input = DelegationInput {
        agent_policy: Authority::of(grants.iter().copied()),
        delegation: Authority::of(grants.iter().copied()),
        tenant_policy: Authority::of(grants.iter().copied()),
        trigger_actor_held: Authority::of(grants.iter().copied()),
    };
    governed_router_with_caveats_and_verdicts(
        input,
        DelegationCaveats(caveats.iter().map(|caveat| (*caveat).to_string()).collect()),
        "trigger-jti",
        i64::MAX,
        Arc::new(OutboxGovernanceAudit::new(
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()),
        )),
        300,
        HitlVerdictStore::new(),
    )
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
    let outcome = no_grant.call(
        open_pr,
        &serde_json::json!({"repo": "alpha"}),
        "no-grant-open",
        &now(),
        None,
    );
    match outcome {
        CallOutcome::Denied { reason, .. } => assert!(reason.contains("repo.push")),
        other => panic!("missing declared capability must deny before EffectApi: {other:?}"),
    }

    let attenuated = governed_router_with_input(DelegationInput {
        agent_policy: Authority::of(["repo.push", "pull_request.review"]),
        delegation: Authority::of(["pull_request.review"]),
        tenant_policy: Authority::of(["repo.push", "pull_request.review"]),
        trigger_actor_held: Authority::of(["repo.push", "pull_request.review"]),
    });
    let outcome = attenuated.call(
        open_pr,
        &serde_json::json!({"repo": "alpha"}),
        "attenuated-open",
        &now(),
        None,
    );
    match outcome {
        CallOutcome::Denied { reason, .. } => assert!(reason.contains("repo.push")),
        other => panic!("attenuated-away capability must deny: {other:?}"),
    }
}

#[test]
fn a_repository_caveat_denies_a_merge_before_asking_a_human_to_approve_it() {
    let server = McpServer::with_router_and_clock(
        ToolRegistry::for_cursors(&["git.merge.v1".into()]).unwrap(),
        caveated_router(
            &["pull_request.merge"],
            &["pull_request.merge", "repo:acme/web"],
        ),
        Arc::new(now),
    );

    let response = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/payroll","number":7},"_meta":{"com.myelin/idempotencyKey":"merge-payroll-7"}}}"#,
        ],
    );

    assert_eq!(response[0]["result"]["isError"], true);
    assert!(response[0]["result"]["_meta"]["reason"]
        .as_str()
        .unwrap()
        .contains("outside the signed delegation scope"));
    assert!(
        response[0]["result"]["_meta"]["gateId"].is_null(),
        "an impossible operation must not leave a misleading approval card"
    );
    assert!(matches!(
        server.router().unwrap().audit()[0].outcome,
        CallOutcome::Denied { .. }
    ));
}

#[test]
fn a_repository_caveat_denies_a_read_before_the_storage_adapter_sees_it() {
    let recorder = Arc::new(CountingReadExecutor {
        calls: AtomicUsize::new(0),
    });
    let server = McpServer::with_router_reads_and_clock(
        ToolRegistry::for_cursors(&["git.read_file.v1".into()]).unwrap(),
        caveated_router(&["repo.pull"], &["repo.pull", "repo:acme/web"]),
        recorder.clone(),
        Arc::new(now),
    );

    let response = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.read_file","arguments":{"repo":"acme/payroll","ref":"main","path":"README.md"}}}"#,
        ],
    );

    assert_eq!(response[0]["result"]["isError"], true);
    assert!(response[0]["result"]["_meta"]["reason"]
        .as_str()
        .unwrap()
        .contains("outside the signed delegation scope"));
    assert_eq!(
        recorder.calls.load(Ordering::SeqCst),
        0,
        "the adapter cannot accidentally broaden a signed repository scope"
    );
}

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
fn tools_list_is_the_exact_delegation_scoped_subset() {
    let grants = ["repo.push", "run.view"];
    let router = governed_router_with_input(DelegationInput {
        agent_policy: Authority::of(grants),
        delegation: Authority::of(grants),
        tenant_policy: Authority::of(grants),
        trigger_actor_held: Authority::of(grants),
    });
    let server = McpServer::with_router_reads_and_clock(
        ToolRegistry::with_git_and_ci_reads().unwrap(),
        router,
        Arc::new(RecordingReadExecutor {
            calls: AtomicUsize::new(0),
        }),
        Arc::new(now),
    );

    let listed = drive(
        &server,
        &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#],
    );
    let names = listed[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        [
            "ci.read_log",
            "ci.read_run",
            "git.open_pr",
            "git.write_file"
        ]
    );
    assert!(
        server.router().unwrap().current_token().is_some(),
        "discovery itself belongs to an attributed per-run identity"
    );
}

#[test]
fn an_edge_issued_run_uses_its_existing_identity_and_exact_activation_selection() {
    let minting_router = ci_read_router(&["repo.push", "run.view"]);
    minting_router
        .permitted_tool_names(&ToolRegistry::with_git_and_ci_reads().unwrap(), &now())
        .unwrap();
    let issued_token = minting_router.current_token().unwrap();
    let router = GovernedRouter::with_issued_run(
        minting_router.minter().clone(),
        IssuedGovernedRun::new(
            minting_router.principal().clone(),
            issued_token.clone(),
            ["repo.push".into(), "run.view".into()],
        )
        .unwrap(),
        Box::new(SkeletonEffectApi::new()),
        HitlVerdictStore::new(),
        Arc::new(TestApprovers(vec![PrincipalId("human:operator".into())])),
        Arc::new(OutboxGovernanceAudit::new(
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()),
        )),
    );
    assert_eq!(
        router.current_token(),
        Some(issued_token),
        "Edge's authenticated bearer is installed before the first MCP frame"
    );

    let selected =
        ToolRegistry::for_cursors(&["ci.read_run.v1".into(), "git.open_pr.v1".into()]).unwrap();
    let server = McpServer::with_router_reads_and_clock(
        selected,
        router,
        Arc::new(RecordingReadExecutor {
            calls: AtomicUsize::new(0),
        }),
        Arc::new(now),
    );
    let listed = drive(
        &server,
        &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#],
    );
    let names = listed[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, ["ci.read_run", "git.open_pr"]);
}

#[test]
fn an_edge_issued_run_binds_identity_bearer_and_grants_before_routing() {
    let minting_router = ci_read_router(&["run.view"]);
    minting_router
        .permitted_tool_names(&ToolRegistry::with_git_and_ci_reads().unwrap(), &now())
        .unwrap();
    let principal = minting_router.principal().clone();
    let token = minting_router.current_token().unwrap();

    assert!(IssuedGovernedRun::new(principal.clone(), token.clone(), Vec::new()).is_err());
    assert!(IssuedGovernedRun::new(
        principal.clone(),
        token.clone(),
        ["run.view with whitespace".into()],
    )
    .is_err());

    let mut missing_bearer = token;
    missing_bearer.token.clear();
    assert!(IssuedGovernedRun::new(principal, missing_bearer, ["run.view".into()]).is_err());
}

#[test]
fn tools_list_refuses_an_expired_trigger_before_disclosing_the_catalogue() {
    let grants = ["repo.push"];
    let router = governed_router_with_trigger(
        DelegationInput {
            agent_policy: Authority::of(grants),
            delegation: Authority::of(grants),
            tenant_policy: Authority::of(grants),
            trigger_actor_held: Authority::of(grants),
        },
        "expired-trigger",
        1,
    );
    let server = McpServer::with_router_and_clock(ToolRegistry::with_git(), router, Arc::new(now));

    let response = drive(
        &server,
        &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#],
    );
    assert_eq!(response[0]["error"]["code"], -32001);
    assert!(response[0].get("result").is_none());
}

#[test]
fn ci_read_projects_shared_schema_and_routes_directly_without_idempotency() {
    let recorder = Arc::new(RecordingReadExecutor {
        calls: AtomicUsize::new(0),
    });
    let registry = ToolRegistry::with_git_and_ci_reads().unwrap();
    let server = McpServer::with_router_reads_and_clock(
        registry,
        ci_read_router(&["run.view"]),
        recorder.clone(),
        Arc::new(now),
    );

    let listed = drive(
        &server,
        &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#],
    );
    let read_run = listed[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "ci.read_run")
        .unwrap();
    assert_eq!(read_run["annotations"]["readOnlyHint"], true);
    assert_eq!(
        read_run["inputSchema"],
        serde_json::from_str::<serde_json::Value>(
            &myelin_ci_controlplane::ci_tool_def("read_run").input_schema
        )
        .unwrap()
    );

    let response = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ci.read_run","arguments":{"run_id":"01234567-89ab-cdef-0123-456789abcdef"}}}"#,
        ],
    );
    assert_eq!(response[0]["result"]["isError"], false);
    let body: serde_json::Value = serde_json::from_str(
        response[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["tool"], "ci.read_run");
    assert_eq!(recorder.calls.load(Ordering::SeqCst), 1);
    assert!(
        response[0]["result"]["_meta"]["runToken"]
            .as_str()
            .is_some_and(|jti| !jti.is_empty()),
        "direct read is attributed to the exact minted run token"
    );

    let mutation_without_key = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"git.open_pr","arguments":{"repo":"alpha","title":"Open alpha"}}}"#,
        ],
    );
    assert_eq!(mutation_without_key[0]["error"]["code"], -32602);
}

#[test]
fn an_agent_read_leaves_a_durable_trace_around_the_exact_resource() {
    let audit_store = OutboxStore::new();
    let recorder = Arc::new(RecordingReadExecutor {
        calls: AtomicUsize::new(0),
    });
    let server = McpServer::with_router_reads_and_clock(
        ToolRegistry::for_cursors(&["ci.read_run.v1".into()]).unwrap(),
        ci_read_router_with_audit(
            &["run.view"],
            Arc::new(OutboxGovernanceAudit::new(
                audit_store.clone(),
                Arc::new(MonotonicMinter::new()),
            )),
        ),
        recorder.clone(),
        Arc::new(now),
    );

    let response = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ci.read_run","arguments":{"run_id":"01234567-89ab-cdef-0123-456789abcdef"}}}"#,
        ],
    );

    assert_eq!(response[0]["result"]["isError"], false);
    assert_eq!(recorder.calls.load(Ordering::SeqCst), 1);
    let rows = audit_store.committed_rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].envelope.type_.0, "agent.tool.read_attempted");
    assert_eq!(rows[1].envelope.type_.0, "agent.tool.read_succeeded");
    for row in rows {
        assert_eq!(row.envelope.actor.0.principal_id.0, "agent:claude");
        assert_eq!(
            row.envelope.subject.0,
            "myelin://acme/ci/run/01234567-89ab-cdef-0123-456789abcdef"
        );
        assert_eq!(row.envelope.payload["tool"], "ci.read_run");
        assert_eq!(row.envelope.payload["resource_ref"], row.envelope.subject.0);
        assert!(row.envelope.payload["token_ref"]
            .as_str()
            .is_some_and(|token| token.starts_with("jti:runtok:agent:claude:mcp-run-1")));
    }
}

#[test]
fn a_refused_read_is_audited_without_touching_the_resource_adapter() {
    let audit_store = OutboxStore::new();
    let recorder = Arc::new(RecordingReadExecutor {
        calls: AtomicUsize::new(0),
    });
    let server = McpServer::with_router_reads_and_clock(
        ToolRegistry::for_cursors(&["ci.read_run.v1".into()]).unwrap(),
        ci_read_router_with_audit(
            &["repo.push"],
            Arc::new(OutboxGovernanceAudit::new(
                audit_store.clone(),
                Arc::new(MonotonicMinter::new()),
            )),
        ),
        recorder.clone(),
        Arc::new(now),
    );

    let response = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ci.read_run","arguments":{"run_id":"01234567-89ab-cdef-0123-456789abcdef"}}}"#,
        ],
    );

    assert_eq!(response[0]["result"]["isError"], true);
    assert_eq!(recorder.calls.load(Ordering::SeqCst), 0);
    let rows = audit_store.committed_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].envelope.type_.0, "agent.tool.read_denied");
    assert_eq!(rows[0].envelope.actor.0.principal_id.0, "agent:claude");
    assert_eq!(
        rows[0].envelope.payload["outcome"],
        serde_json::json!({
            "kind": "denied",
            "reason_category": "authorization",
        })
    );
}

#[test]
fn no_durable_attempt_means_no_resource_read() {
    let recorder = Arc::new(RecordingReadExecutor {
        calls: AtomicUsize::new(0),
    });
    let server = McpServer::with_router_reads_and_clock(
        ToolRegistry::for_cursors(&["ci.read_run.v1".into()]).unwrap(),
        ci_read_router_with_audit(&["run.view"], Arc::new(FailAttemptAudit)),
        recorder.clone(),
        Arc::new(now),
    );

    let response = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ci.read_run","arguments":{"run_id":"01234567-89ab-cdef-0123-456789abcdef"}}}"#,
        ],
    );

    assert_eq!(response[0]["result"]["isError"], true);
    assert!(response[0]["result"]["_meta"]["reason"]
        .as_str()
        .unwrap()
        .contains("pre-read audit"));
    assert_eq!(recorder.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn an_unaudited_read_result_never_leaves_the_server() {
    let recorder = Arc::new(RecordingReadExecutor {
        calls: AtomicUsize::new(0),
    });
    let server = McpServer::with_router_reads_and_clock(
        ToolRegistry::for_cursors(&["ci.read_run.v1".into()]).unwrap(),
        ci_read_router_with_audit(&["run.view"], Arc::new(FailOutcomeAudit)),
        recorder.clone(),
        Arc::new(now),
    );

    let response = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ci.read_run","arguments":{"run_id":"01234567-89ab-cdef-0123-456789abcdef"}}}"#,
        ],
    );

    assert_eq!(recorder.calls.load(Ordering::SeqCst), 1);
    assert_eq!(response[0]["result"]["isError"], true);
    assert_eq!(response[0]["result"]["_meta"]["reason"], "unavailable");
    assert!(!response[0]["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("01234567-89ab-cdef-0123-456789abcdef"));
}

#[test]
fn ci_read_capability_and_revocation_deny_before_the_adapter() {
    let denied_recorder = Arc::new(RecordingReadExecutor {
        calls: AtomicUsize::new(0),
    });
    let denied = McpServer::with_router_reads_and_clock(
        ToolRegistry::with_git_and_ci_reads().unwrap(),
        ci_read_router(&["repo.push"]),
        denied_recorder.clone(),
        Arc::new(now),
    );
    let response = drive(
        &denied,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ci.read_run","arguments":{"run_id":"01234567-89ab-cdef-0123-456789abcdef"}}}"#,
        ],
    );
    assert_eq!(response[0]["result"]["isError"], true);
    assert!(response[0]["result"]["_meta"]["reason"]
        .as_str()
        .unwrap()
        .contains("run.view"));
    assert_eq!(denied_recorder.calls.load(Ordering::SeqCst), 0);

    let revoked_recorder = Arc::new(RecordingReadExecutor {
        calls: AtomicUsize::new(0),
    });
    let revoked = McpServer::with_router_reads_and_clock(
        ToolRegistry::with_git_and_ci_reads().unwrap(),
        ci_read_router(&["run.view"]),
        revoked_recorder.clone(),
        Arc::new(now),
    );
    let first = drive(
        &revoked,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ci.read_run","arguments":{"run_id":"01234567-89ab-cdef-0123-456789abcdef"}}}"#,
        ],
    );
    assert_eq!(first[0]["result"]["isError"], false);
    let router = revoked.router().unwrap();
    let token = router.current_token().unwrap();
    router
        .minter()
        .teardown(&router.principal().scope, &token, &now());
    let second = drive(
        &revoked,
        &[
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ci.read_run","arguments":{"run_id":"01234567-89ab-cdef-0123-456789abcdef"}}}"#,
        ],
    );
    assert_eq!(second[0]["result"]["isError"], true);
    assert!(second[0]["result"]["_meta"]["reason"]
        .as_str()
        .unwrap()
        .contains("revoked"));
    assert_eq!(revoked_recorder.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn non_gated_tool_mints_a_run_token_and_routes_through_effect_api() {
    let server = governed_server();
    let resps = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.submit_review","arguments":{"repo":"alpha","number":7,"verdict":"comment"},"_meta":{"com.myelin/idempotencyKey":"review-7"}}}"#,
        ],
    );
    let r = &resps[0];
    assert_eq!(r["result"]["isError"], false);
    let jti = r["result"]["_meta"]["runToken"].as_str().unwrap();
    assert!(
        jti.starts_with("runtok:agent:claude:mcp-run-1"),
        "the run token jti is bound to (agent, run): {jti}"
    );

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
    let resps = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7},"_meta":{"com.myelin/idempotencyKey":"merge-7"}}}"#,
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

#[test]
fn a_caller_supplied_granted_boolean_never_applies() {
    let server = governed_server();
    let resps = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7},"approval":{"granted":true},"_meta":{"com.myelin/idempotencyKey":"merge-7"}}}"#,
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
        "the call is withheld or refused - never Applied off a caller boolean"
    );
}

#[test]
fn a_gated_tool_returns_an_opaque_unguessable_gate_id() {
    let call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7},"_meta":{"com.myelin/idempotencyKey":"merge-7"}}}"#;

    let server = governed_server();
    let resps = drive(&server, &[call]);
    let gate_id = resps[0]["result"]["_meta"]["gateId"]
        .as_str()
        .unwrap()
        .to_string();
    let jti = resps[0]["result"]["_meta"]["runToken"].as_str().unwrap();

    assert_ne!(
        gate_id,
        format!("hitl:{jti}:git.merge"),
        "the gate id must not be the old deterministic display string"
    );
    assert!(
        !gate_id.contains(jti) && !gate_id.contains("git.merge"),
        "the gate id must not embed the guessable call facts: {gate_id}"
    );

    let other = governed_server();
    let other_resps = drive(&other, &[call]);
    let other_gate = other_resps[0]["result"]["_meta"]["gateId"]
        .as_str()
        .unwrap();
    assert_ne!(
        gate_id, other_gate,
        "gate ids are unpredictable across servers/processes"
    );

    let retry = drive(&server, &[call]);
    assert_eq!(
        retry[0]["result"]["_meta"]["gateId"].as_str().unwrap(),
        gate_id,
        "a retried gated call re-surfaces the same pending gate"
    );
}

#[test]
fn approval_is_a_server_side_verdict_by_a_distinct_human_principal() {
    let server = governed_server();
    let call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7},"_meta":{"com.myelin/idempotencyKey":"merge-7"}}}"#;

    let gated = drive(&server, &[call]);
    let gate_id = gated[0]["result"]["_meta"]["gateId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(gated[0]["result"]["_meta"]["eventId"].is_null());
    let router = server.router().unwrap();
    let rec = router
        .gate_verdict(&gate_id)
        .expect("the in-memory gate store is available")
        .expect("the gate is a server-side row");
    assert_eq!(rec.state, GateState::Waiting);
    assert_eq!(rec.requested_by, "agent:claude");
    assert!(
        !rec.approver_filter.contains(&"agent:claude".to_string()),
        "the requesting agent is structurally excluded from its own gate's approver set"
    );

    assert!(
        matches!(
            router.approve_gate(&agent_principal("agent:claude", "acme"), &gate_id, &now(),),
            Err(GateDecideError::SelfApproval) | Err(GateDecideError::NotEligible)
        ),
        "the agent principal cannot approve its own gate"
    );
    assert_eq!(
        router.approve_gate(&service_principal("svc:ci-robot", "acme"), &gate_id, &now(),),
        Err(GateDecideError::MachineApproverRefused),
        "a distinct MACHINE principal is refused - the gate requires a HUMAN approver"
    );
    assert_eq!(
        router.gate_verdict(&gate_id).unwrap().unwrap().state,
        GateState::Waiting,
        "the machine-refused approval left the gate undecided"
    );

    let redrive = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"git.merge","arguments":{{"repo":"acme/web","number":7}},"approval":{{"gateId":"{gate_id}"}},"_meta":{{"com.myelin/idempotencyKey":"merge-7"}}}}}}"#
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

    router
        .approve_gate(&human_principal("human:operator", "acme"), &gate_id, &now())
        .expect("a distinct eligible human approves");
    let rec = router.gate_verdict(&gate_id).unwrap().unwrap();
    assert_eq!(rec.state, GateState::Approved);
    assert_eq!(rec.decided_by.as_deref(), Some("human:operator"));

    let applied = drive(&server, &[call]);
    let event_id = applied[0]["result"]["_meta"]["eventId"]
        .as_str()
        .expect("applied");
    assert!(
        event_id.contains("tool:git.merge"),
        "the resumed host finds the exact durable approval without carrying a caller assertion: \
         {event_id}"
    );
    assert!(
        router
            .gate_verdict(&gate_id)
            .unwrap()
            .unwrap()
            .approval_consumed_at_unix
            .is_some(),
        "the one-shot server verdict was consumed by the effect"
    );

    let replayed = drive(&server, &[redrive.as_str()]);
    assert_eq!(
        replayed[0]["result"]["_meta"]["eventId"], applied[0]["result"]["_meta"]["eventId"],
        "a lost response can replay the approved logical effect with its same caller key"
    );

    let different_retry_key = format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"git.merge","arguments":{{"repo":"acme/web","number":7}},"approval":{{"gateId":"{gate_id}"}},"_meta":{{"com.myelin/idempotencyKey":"merge-7-again"}}}}}}"#
    );
    let refused = drive(&server, &[different_retry_key.as_str()]);
    assert_eq!(
        refused[0]["result"]["isError"], true,
        "a consumed approval cannot authorize a new logical effect with identical arguments"
    );
}

#[test]
fn a_made_up_gate_id_is_denied() {
    let server = governed_server();
    let resps = drive(
        &server,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7},"approval":{"gateId":"gate:0123456789abcdef0123456789abcdef"},"_meta":{"com.myelin/idempotencyKey":"merge-7"}}}"#,
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

#[test]
fn an_approval_never_transfers_to_a_sibling_effect_and_a_reject_is_final() {
    let server = governed_server();
    let call7 = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":7},"_meta":{"com.myelin/idempotencyKey":"merge-7"}}}"#;
    let gated = drive(&server, &[call7]);
    let gate7 = gated[0]["result"]["_meta"]["gateId"]
        .as_str()
        .unwrap()
        .to_string();
    let router = server.router().unwrap();
    router
        .approve_gate(&human_principal("human:operator", "acme"), &gate7, &now())
        .unwrap();

    let cross = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"git.merge","arguments":{{"repo":"acme/web","number":8}},"approval":{{"gateId":"{gate7}"}},"_meta":{{"com.myelin/idempotencyKey":"merge-8"}}}}}}"#
    );
    let denied = drive(&server, &[cross.as_str()]);
    assert_eq!(
        denied[0]["result"]["isError"], true,
        "approval never transfers across effects"
    );
    assert!(denied[0]["result"]["_meta"]["eventId"].is_null());

    let call9 = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"acme/web","number":9},"_meta":{"com.myelin/idempotencyKey":"merge-9"}}}"#;
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
        r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"git.merge","arguments":{{"repo":"acme/web","number":9}},"approval":{{"gateId":"{gate9}"}},"_meta":{{"com.myelin/idempotencyKey":"merge-9"}}}}}}"#
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
    let first = server
        .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.submit_review","arguments":{"repo":"alpha","number":1,"verdict":"comment"},"_meta":{"com.myelin/idempotencyKey":"review-1"}}}"#)
        .unwrap();
    let first: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(first["result"]["isError"], false);

    let router = server.router().unwrap();
    let token = router.current_token().unwrap();
    router
        .minter()
        .teardown(&router.principal().scope, &token, &now());

    let second = server
        .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"git.submit_review","arguments":{"repo":"alpha","number":2,"verdict":"comment"},"_meta":{"com.myelin/idempotencyKey":"review-2"}}}"#)
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
        expired.call(tool, &args, "expired-trigger", &at, None),
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
        revoked.call(tool, &args, "revoked-trigger", &at, None),
        CallOutcome::Denied { reason, .. } if reason.contains("trigger credential is revoked")
    ));
    assert!(revoked.current_token().is_none());

    let clamped = governed_router_with_trigger(input(), "short-trigger", at_unix + 2);
    assert!(matches!(
        clamped.call(tool, &args, "clamped-trigger", &at, None),
        CallOutcome::Applied { .. }
    ));
    let token = clamped.current_token().unwrap();
    clamped.minter().revocations().revoke(
        &clamped.principal().scope,
        &RevokeTarget::Jti("short-trigger".into()),
        at.clone(),
    );
    assert!(matches!(
        clamped.call(
            tool,
            &args,
            "clamped-trigger",
            &Timestamp("2026-06-26T00:00:01Z".into()),
            None,
        ),
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
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.submit_review","arguments":{"repo":"alpha","number":1,"verdict":"comment"},"_meta":{"com.myelin/idempotencyKey":"review-1"}}}"#;
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
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.submit_review","arguments":{"repo":"alpha","number":1,"verdict":"comment"},"_meta":{"com.myelin/idempotencyKey":"review-1"}}}"#;
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
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"git.open_pr","arguments":{"repo":"alpha","title":"Open alpha"},"_meta":{"com.myelin/idempotencyKey":"open-alpha"}}}"#;
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
        "post-gate-audit-1",
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
        "expiry-audit-1",
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
            "expiry-audit-1",
            &expired_at,
            Some(&gate_id),
        ),
        CallOutcome::Indeterminate { .. }
    ));
    assert_eq!(
        router.gate_verdict(&gate_id).unwrap().unwrap().state,
        GateState::Expired
    );
    assert!(router.is_fatal());
}

#[test]
fn mcp_expiry_leaves_unrelated_shared_gate_untouched_and_audits_exact_gate() {
    let args = serde_json::json!({"repo":"alpha","number":77});
    let exact_effect =
        myelin_mcp::governance::mcp_effect_key_for_call("git.merge", &args, "scope-expiry-1");
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
        router.call(
            registry.resolve("git.merge").unwrap(),
            &args,
            "scope-expiry-1",
            &now(),
            None,
        ),
        CallOutcome::Gated { .. }
    ));
    assert_eq!(
        router.gate_verdict("gate:mcp-due").unwrap().unwrap().state,
        GateState::Expired
    );
    assert_eq!(
        router
            .gate_verdict("gate:shared-due")
            .unwrap()
            .unwrap()
            .state,
        GateState::Waiting
    );
    let due_gate_ref = format!(":hitl-gate:{}", gate_ref_token("gate:mcp-due"));
    assert!(audit_store.committed_rows().iter().any(|row| {
        row.envelope.type_.0 == "git.merge.expired"
            && row.envelope.subject.0.ends_with(&due_gate_ref)
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
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"git.submit_review","arguments":{{"repo":"alpha","number":{id},"verdict":"comment"}},"_meta":{{"com.myelin/idempotencyKey":"review-{id}"}}}}}}"#
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
