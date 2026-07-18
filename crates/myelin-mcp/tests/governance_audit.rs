use std::sync::Arc;

use myelin_events::{MonotonicMinter, OutboxStore, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RunId};
use myelin_mcp::{
    AuditPhase, CallOutcome, GovernanceAudit, GovernanceAuditRecord, OutboxGovernanceAudit,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

fn actor() -> Principal {
    let mut actor = Principal::stub(
        PrincipalId("psn:mcp-agent".into()),
        PrincipalKind::Service,
        TenantId("acme".into()),
    );
    actor.region = Region("eu-west".into());
    actor
}

#[test]
fn governance_audit_uses_minimized_tool_facts_never_trace_or_raw_reason() {
    let store = OutboxStore::new();
    let audit = OutboxGovernanceAudit::new(store.clone(), Arc::new(MonotonicMinter::new()));
    let actor = actor();
    let scope = TenantScope::from_verified_token(&actor, actor.region.clone());
    let run = RunId("run:audit".into());
    let now = Timestamp("2026-07-18T00:00:00Z".into());
    audit
        .record(GovernanceAuditRecord {
            scope: &scope,
            actor: &actor,
            run_id: &run,
            tool: "git.open_pr",
            jti: "jti:audit",
            phase: AuditPhase::Attempt,
            outcome: None,
            now: &now,
        })
        .unwrap();
    let denied = CallOutcome::Denied {
        reason: "private@example.test title=secret customer argument".into(),
        jti: "jti:audit".into(),
    };
    audit
        .record(GovernanceAuditRecord {
            scope: &scope,
            actor: &actor,
            run_id: &run,
            tool: "git.open_pr",
            jti: "jti:audit",
            phase: AuditPhase::Outcome,
            outcome: Some(&denied),
            now: &now,
        })
        .unwrap();

    let rows = store.committed_rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].envelope.type_.0, "git.open_pr.attempted");
    assert_eq!(rows[1].envelope.type_.0, "git.open_pr.denied");
    for row in rows {
        assert_ne!(row.envelope.type_.0, "agent.run.traced");
        assert_eq!(row.envelope.subject.0, "myelin://acme/agent/run/run:audit");
        let payload = row.envelope.payload.to_string();
        assert!(!payload.contains("private@example.test"));
        assert!(!payload.contains("secret"));
        assert!(!payload.contains("customer argument"));
        assert!(!payload.contains("psn:mcp-agent"));
    }
}

#[test]
fn governance_audit_refuses_unregistered_dynamic_tool_taxonomy() {
    let store = OutboxStore::new();
    let audit = OutboxGovernanceAudit::new(store.clone(), Arc::new(MonotonicMinter::new()));
    let actor = actor();
    let scope = TenantScope::from_verified_token(&actor, actor.region.clone());
    assert!(audit
        .record(GovernanceAuditRecord {
            scope: &scope,
            actor: &actor,
            run_id: &RunId("run:audit".into()),
            tool: "caller.controlled.tool",
            jti: "jti:audit",
            phase: AuditPhase::Attempt,
            outcome: None,
            now: &Timestamp("2026-07-18T00:00:00Z".into()),
        })
        .is_err());
    assert_eq!(store.committed_count(), 0);
}
