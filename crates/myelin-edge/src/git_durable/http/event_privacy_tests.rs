use super::*;
use myelin_identity::{DataRole, PrincipalStatus, RuntimeRef};
use myelin_tenancy::Region as IdRegion;

#[test]
fn production_refstore_context_scrubs_all_raw_agent_identifiers() {
    let principal = Principal::new(
        myelin_tenancy::TenantId("acme".into()),
        IdRegion("fr-par".into()),
        PrincipalId("agent:raw@example.test".into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("runtime://raw-machine/session".into()),
            on_behalf_of: Some(PrincipalId("person@example.test".into())),
        },
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let first = DurableGitBackend::emit_ctx("acme", "fr-par", &principal)
        .expect("the test clock is available");
    let second = DurableGitBackend::emit_ctx("acme", "fr-par", &principal)
        .expect("the test clock is available");
    assert_eq!(first.actor, second.actor, "the tenant pseudonym is stable");
    assert_ne!(first.actor.0.principal_id, principal.principal_id);
    let serialized = serde_json::to_string(&first.actor).unwrap();
    for raw in [
        "agent:raw@example.test",
        "runtime://raw-machine/session",
        "person@example.test",
    ] {
        assert!(
            !serialized.contains(raw),
            "raw Agent identifier leaked: {raw}"
        );
    }

    let request = RepoActorContext::new("acme", "fr-par", "core", &principal).for_pr(42);
    let debug = format!("{request:?}");
    assert!(!debug.contains("agent:raw@example.test"));
    assert!(debug.contains("principal: \"<redacted>\""));
}

#[test]
fn fork_import_diagnostics_are_sanitized_before_public_boundaries() {
    let raw = DurableError::Git(
        "failed to index /srv/tenants/acme/private-fork.git: object secretdeadbeef".into(),
    );
    let public = sanitize_fork_import_error(raw).to_string();
    assert_eq!(
        public,
        "durable git op failed: fork commit import could not be completed"
    );
    assert!(!public.contains("/srv/tenants"));
    assert!(!public.contains("secretdeadbeef"));
}
