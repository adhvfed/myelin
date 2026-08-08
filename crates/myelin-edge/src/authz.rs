use myelin_identity::Principal;
use myelin_identity_service::{
    CredentialAudience, CredentialPurpose, RequestIdentity, VerifiedCapabilityContext,
};
use myelin_substrate::Authorizer;
use std::collections::BTreeSet;

pub const MOUNTED_EDGE_ACTIONS: &[&str] = &[
    "edge.whoami",
    "edge.events.subscribe",
    "issues.list",
    "issues.create",
    "issues.authorization_status",
    "issues.view",
    "issues.close",
    "ci.runs.list",
    "ci.run.view",
    "ci.run.log.read",
    "ci.run.log.watch",
    "notif.inbox.list",
    "notif.inbox.mark_read",
    "chat.conversations.list",
    "chat.conversation.create",
    "chat.messages.list",
    "chat.message.post",
    "knowledge.pages.list",
    "knowledge.page.create",
    "knowledge.page.view",
    "knowledge.page.save",
    "git.repos.list",
    "git.repo.create",
    "git.pr.view",
    "git.pr.checks",
    "git.blob.view",
    "git.blame.view",
    "git.blob.commit",
    "git.pr.open",
    "git.pr.review",
    "git.pr.endorse_fork_ci",
    "git.pr.merge",
    "git.repo.branch_protection.set",
    "git.checks.report",
    "git.search.code",
    "git.repo.view",
    "git.commits.log",
    "git.commit.diff",
    "git.prs.list",
    "git.prs.mine",
    "git.pr.commits",
    "git.pr.diff",
    "git.file.lines",
    "git.pr.threads.list",
    "git.pr.thread.create",
    "git.pr.comment.create",
    "git.pr.thread.resolve",
    "git.pr.review.start",
    "git.pr.review.comment",
    "git.pr.review.submit",
    "git.pr.review.discard",
    "git.refs.list",
    "git.tree.view",
    "git.blob.raw",
    "git.blob.download",
    "git.wire.upload_pack",
    "git.wire.receive_pack",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcceptedPurpose {
    HumanSession,
    OperatorBootstrap,
    AgentRun,
    Pat,
    CiJob,
    DeployKey,
}

const OP_AGENT_PAT: &[AcceptedPurpose] = &[
    AcceptedPurpose::HumanSession,
    AcceptedPurpose::OperatorBootstrap,
    AcceptedPurpose::AgentRun,
    AcceptedPurpose::Pat,
];
const OP_PAT: &[AcceptedPurpose] = &[
    AcceptedPurpose::HumanSession,
    AcceptedPurpose::OperatorBootstrap,
    AcceptedPurpose::Pat,
];
const OP_AGENT_PAT_DEPLOY: &[AcceptedPurpose] = &[
    AcceptedPurpose::HumanSession,
    AcceptedPurpose::OperatorBootstrap,
    AcceptedPurpose::AgentRun,
    AcceptedPurpose::Pat,
    AcceptedPurpose::DeployKey,
];
const CI_ONLY: &[AcceptedPurpose] = &[AcceptedPurpose::CiJob];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionRequirement {
    pub action: &'static str,
    pub required_capability: &'static str,
    pub accepted_purposes: &'static [AcceptedPurpose],
}

macro_rules! requirement {
    ($action:literal, $capability:literal, $purposes:expr) => {
        ActionRequirement {
            action: $action,
            required_capability: $capability,
            accepted_purposes: $purposes,
        }
    };
}

pub const ACTION_REQUIREMENTS: &[ActionRequirement] = &[
    requirement!("edge.whoami", "edge.identity.read", OP_AGENT_PAT),
    requirement!(
        "edge.events.subscribe",
        "edge.events.subscribe",
        OP_AGENT_PAT
    ),
    requirement!("issues.list", "issue.view", OP_AGENT_PAT),
    requirement!("issues.create", "issue.create", OP_AGENT_PAT),
    requirement!("issues.authorization_status", "issue.view", OP_AGENT_PAT),
    requirement!("issues.view", "issue.view", OP_AGENT_PAT),
    requirement!("issues.close", "issue.transition", OP_AGENT_PAT),
    requirement!("ci.runs.list", "run.view", OP_AGENT_PAT),
    requirement!("ci.run.view", "run.view", OP_AGENT_PAT),
    requirement!("ci.run.log.read", "run.view", OP_AGENT_PAT),
    requirement!("ci.run.log.watch", "run.view", OP_AGENT_PAT),
    requirement!("notif.inbox.list", "notification.read", OP_AGENT_PAT),
    requirement!(
        "notif.inbox.mark_read",
        "notification.write",
        OP_AGENT_PAT
    ),
    requirement!("chat.conversations.list", "chat.read", OP_AGENT_PAT),
    requirement!("chat.conversation.create", "chat.manage", OP_PAT),
    requirement!("chat.messages.list", "chat.read", OP_AGENT_PAT),
    requirement!("chat.message.post", "chat.post", OP_AGENT_PAT),
    requirement!("knowledge.pages.list", "knowledge.read", OP_AGENT_PAT),
    requirement!("knowledge.page.create", "knowledge.edit", OP_AGENT_PAT),
    requirement!("knowledge.page.view", "knowledge.read", OP_AGENT_PAT),
    requirement!("knowledge.page.save", "knowledge.edit", OP_AGENT_PAT),
    requirement!("git.repos.list", "repo.pull", OP_AGENT_PAT),
    requirement!("git.repo.create", "repo.create", OP_PAT),
    requirement!("git.pr.view", "repo.pull", OP_AGENT_PAT),
    requirement!("git.pr.checks", "repo.pull", OP_AGENT_PAT),
    requirement!("git.blob.view", "repo.pull", OP_AGENT_PAT),
    requirement!("git.blame.view", "repo.pull", OP_AGENT_PAT),
    requirement!("git.blob.commit", "repo.push", OP_AGENT_PAT),
    requirement!("git.pr.open", "repo.push", OP_AGENT_PAT),
    requirement!("git.pr.review", "pull_request.review", OP_AGENT_PAT),
    requirement!(
        "git.pr.endorse_fork_ci",
        "repo.approve_untrusted_ci",
        OP_AGENT_PAT
    ),
    requirement!("git.pr.merge", "pull_request.merge", OP_AGENT_PAT),
    requirement!("git.repo.branch_protection.set", "repo.administer", OP_PAT),
    requirement!("git.checks.report", "ci.checks.report", CI_ONLY),
    requirement!("git.search.code", "repo.pull", OP_AGENT_PAT),
    requirement!("git.repo.view", "repo.pull", OP_AGENT_PAT),
    requirement!("git.commits.log", "repo.pull", OP_AGENT_PAT),
    requirement!("git.commit.diff", "repo.pull", OP_AGENT_PAT),
    requirement!("git.prs.list", "repo.pull", OP_AGENT_PAT),
    requirement!("git.prs.mine", "repo.pull", OP_AGENT_PAT),
    requirement!("git.pr.commits", "repo.pull", OP_AGENT_PAT),
    requirement!("git.pr.diff", "repo.pull", OP_AGENT_PAT),
    requirement!("git.file.lines", "repo.pull", OP_AGENT_PAT),
    requirement!("git.pr.threads.list", "repo.pull", OP_AGENT_PAT),
    requirement!("git.pr.thread.create", "pull_request.review", OP_AGENT_PAT),
    requirement!("git.pr.comment.create", "pull_request.review", OP_AGENT_PAT),
    requirement!("git.pr.thread.resolve", "pull_request.review", OP_AGENT_PAT),
    requirement!("git.pr.review.start", "pull_request.review", OP_AGENT_PAT),
    requirement!("git.pr.review.comment", "pull_request.review", OP_AGENT_PAT),
    requirement!("git.pr.review.submit", "pull_request.review", OP_AGENT_PAT),
    requirement!("git.pr.review.discard", "pull_request.review", OP_AGENT_PAT),
    requirement!("git.refs.list", "repo.pull", OP_AGENT_PAT),
    requirement!("git.tree.view", "repo.pull", OP_AGENT_PAT),
    requirement!("git.blob.raw", "repo.pull", OP_AGENT_PAT),
    requirement!("git.blob.download", "repo.pull", OP_AGENT_PAT),
    requirement!("git.wire.upload_pack", "repo.pull", OP_AGENT_PAT_DEPLOY),
    requirement!("git.wire.receive_pack", "repo.push", OP_AGENT_PAT_DEPLOY),
];

pub fn action_requirement(action: &str) -> Option<&'static ActionRequirement> {
    ACTION_REQUIREMENTS
        .iter()
        .find(|rule| rule.action == action)
}

fn purpose_class(purpose: &CredentialPurpose) -> Option<AcceptedPurpose> {
    match purpose {
        CredentialPurpose::HumanSession => Some(AcceptedPurpose::HumanSession),
        CredentialPurpose::OperatorBootstrap => Some(AcceptedPurpose::OperatorBootstrap),
        CredentialPurpose::AgentRun { .. } => Some(AcceptedPurpose::AgentRun),
        CredentialPurpose::Pat => Some(AcceptedPurpose::Pat),
        CredentialPurpose::CiJob { .. } => Some(AcceptedPurpose::CiJob),
        CredentialPurpose::DeployKey => Some(AcceptedPurpose::DeployKey),
        CredentialPurpose::PerJob { .. } => None,
    }
}

pub fn human_session_authority() -> Vec<String> {
    ACTION_REQUIREMENTS
        .iter()
        .filter(|rule| {
            rule.accepted_purposes
                .contains(&AcceptedPurpose::HumanSession)
        })
        .map(|rule| rule.required_capability.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn authorize_edge_action(
    verb_policy: &dyn Authorizer,
    identity: &RequestIdentity,
    action: &str,
) -> bool {
    if !verb_policy.authorize(&identity.principal, action) {
        return false;
    }
    let capability: &VerifiedCapabilityContext = identity.capability();
    if capability.audience != CredentialAudience::Edge {
        return false;
    }
    let Some(rule) = action_requirement(action) else {
        return false;
    };
    let Some(class) = purpose_class(&capability.purpose) else {
        return false;
    };
    if !rule.accepted_purposes.contains(&class) {
        return false;
    }
    match capability.purpose {
        CredentialPurpose::OperatorBootstrap => {
            capability.effective_authority.holds("edge.operator")
        }
        _ => capability
            .effective_authority
            .holds(rule.required_capability),
    }
}

pub struct AuthenticatedActionPolicy {
    allowed: BTreeSet<&'static str>,
}

impl AuthenticatedActionPolicy {
    pub fn new(actions: impl IntoIterator<Item = &'static str>) -> AuthenticatedActionPolicy {
        AuthenticatedActionPolicy {
            allowed: actions.into_iter().collect(),
        }
    }

    pub fn mounted() -> AuthenticatedActionPolicy {
        AuthenticatedActionPolicy::new(ACTION_REQUIREMENTS.iter().map(|rule| rule.action))
    }
}

impl Authorizer for AuthenticatedActionPolicy {
    fn authorize(&self, principal: &Principal, action: &str) -> bool {
        if principal.principal_id.0.is_empty() || principal.tenant.0.is_empty() {
            return false;
        }
        self.allowed.contains(action)
    }
}

#[cfg(any(test, feature = "test-support"))]
pub struct AllowAll;

#[cfg(any(test, feature = "test-support"))]
impl Authorizer for AllowAll {
    fn authorize(&self, _principal: &Principal, _action: &str) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_identity_service::{
        Authority, CredentialContext, DpopState, VerifiedCapabilityContext,
    };
    use myelin_storage::TenantScope;
    use myelin_substrate::DenyAll;
    use myelin_tenancy::{Region, TenantId};

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn identity(
        purpose: CredentialPurpose,
        audience: CredentialAudience,
        grants: &[&str],
    ) -> RequestIdentity {
        let principal = principal();
        let scope = TenantScope::from_verified_token(&principal, Region("eu-west".into()));
        RequestIdentity {
            principal,
            scope,
            credential: CredentialContext::Capability(VerifiedCapabilityContext {
                purpose,
                audience,
                jti: "jti-test".into(),
                effective_authority: Authority::of(grants.iter().copied()),
                expires_at_unix: i64::MAX,
                dpop: DpopState::Unbound,
            }),
        }
    }

    #[test]
    fn allow_all_admits_and_deny_all_refuses() {
        let p = principal();
        assert!(AllowAll.authorize(&p, "edge.whoami"));
        assert!(!DenyAll.authorize(&p, "edge.whoami"));
    }

    #[test]
    fn mounted_policy_admits_every_mounted_action_for_an_authenticated_principal() {
        let policy = AuthenticatedActionPolicy::mounted();
        let p = principal();
        for action in MOUNTED_EDGE_ACTIONS {
            assert!(
                policy.authorize(&p, action),
                "the mounted policy must admit the registered action `{action}` - otherwise a \
                 production route regressed to 403"
            );
        }
    }

    #[test]
    fn mounted_policy_denies_an_unknown_action() {
        let policy = AuthenticatedActionPolicy::mounted();
        let p = principal();
        for unknown in [
            "edge.does.not.exist",
            "git.repo.delete",
            "git.unmapped:/api/git/future",
            "",
            "edge.whoami2",
        ] {
            assert!(
                !policy.authorize(&p, unknown),
                "an unknown/unregistered action `{unknown}` must be DENIED (deny-by-default)"
            );
        }
    }

    #[test]
    fn mounted_policy_denies_a_degenerate_unauthenticated_principal_shape() {
        let policy = AuthenticatedActionPolicy::mounted();
        let empty_id = Principal::stub(
            PrincipalId("".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        let empty_tenant = Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("".into()),
        );
        assert!(
            !policy.authorize(&empty_id, "edge.whoami"),
            "an empty principal id must be denied even on a mounted action (fail-closed)"
        );
        assert!(
            !policy.authorize(&empty_tenant, "edge.whoami"),
            "an empty tenant must be denied even on a mounted action (fail-closed)"
        );
    }

    #[test]
    fn explicit_allowlist_constructor_is_deny_by_default_outside_it() {
        let policy = AuthenticatedActionPolicy::new(["edge.whoami"]);
        let p = principal();
        assert!(policy.authorize(&p, "edge.whoami"));
        assert!(!policy.authorize(&p, "edge.events.subscribe"));
    }

    #[test]
    fn capability_catalogue_exactly_covers_the_compatibility_action_list() {
        let mapped: BTreeSet<&str> = ACTION_REQUIREMENTS.iter().map(|rule| rule.action).collect();
        let mounted: BTreeSet<&str> = MOUNTED_EDGE_ACTIONS.iter().copied().collect();
        assert_eq!(
            mapped.len(),
            ACTION_REQUIREMENTS.len(),
            "duplicate action rule"
        );
        assert_eq!(
            mapped, mounted,
            "mounted actions and capability rules drifted"
        );
    }

    #[test]
    fn human_session_authority_covers_ui_actions_but_never_ci_attestation() {
        let authority = Authority::of(human_session_authority());
        assert!(authority.holds("edge.identity.read"));
        assert!(authority.holds("repo.pull"));
        assert!(authority.holds("repo.push"));
        assert!(authority.holds("issue.view"));
        assert!(!authority.holds("ci.checks.report"));
        assert!(!authority.holds("edge.operator"));
    }

    #[test]
    fn signed_audience_purpose_and_narrow_authority_are_all_enforced() {
        let agent_pull = identity(
            CredentialPurpose::AgentRun {
                run_id: "run-1".into(),
                delegation_snapshot: Some(7),
            },
            CredentialAudience::Edge,
            &["repo.pull"],
        );
        assert!(authorize_edge_action(
            &AllowAll,
            &agent_pull,
            "git.pr.commits"
        ));
        assert!(!authorize_edge_action(
            &AllowAll,
            &agent_pull,
            "git.pr.merge"
        ));
        assert!(!authorize_edge_action(
            &AllowAll,
            &agent_pull,
            "git.repo.create"
        ));

        let wrong_audience = identity(
            CredentialPurpose::Pat,
            CredentialAudience::Mcp,
            &["repo.pull"],
        );
        assert!(!authorize_edge_action(
            &AllowAll,
            &wrong_audience,
            "git.pr.commits"
        ));

        let empty_operator = identity(
            CredentialPurpose::OperatorBootstrap,
            CredentialAudience::Edge,
            &[],
        );
        assert!(!authorize_edge_action(
            &AllowAll,
            &empty_operator,
            "edge.whoami"
        ));
    }

    #[test]
    fn operator_override_never_becomes_ci_attestation_or_agent_supercap() {
        let operator = identity(
            CredentialPurpose::OperatorBootstrap,
            CredentialAudience::Edge,
            &["edge.operator", "ci.checks.report"],
        );
        assert!(authorize_edge_action(
            &AllowAll,
            &operator,
            "git.repo.create"
        ));
        assert!(!authorize_edge_action(
            &AllowAll,
            &operator,
            "git.checks.report"
        ));

        let ci = identity(
            CredentialPurpose::CiJob {
                run_id: "ci-run-1".into(),
            },
            CredentialAudience::Edge,
            &["ci.checks.report"],
        );
        assert!(authorize_edge_action(&AllowAll, &ci, "git.checks.report"));

        let legacy_agent_run = identity(
            CredentialPurpose::AgentRun {
                run_id: "run-2".into(),
                delegation_snapshot: Some(9),
            },
            CredentialAudience::Edge,
            &["agent:run"],
        );
        assert!(!authorize_edge_action(
            &AllowAll,
            &legacy_agent_run,
            "git.pr.merge"
        ));
    }

    #[test]
    fn issues_actions_require_their_exact_signed_capability() {
        let view = identity(
            CredentialPurpose::AgentRun {
                run_id: "run-issues-view".into(),
                delegation_snapshot: Some(9),
            },
            CredentialAudience::Edge,
            &["issue.view"],
        );
        assert!(authorize_edge_action(&AllowAll, &view, "issues.list"));
        assert!(authorize_edge_action(
            &AllowAll,
            &view,
            "issues.authorization_status"
        ));
        assert!(authorize_edge_action(&AllowAll, &view, "issues.view"));
        assert!(!authorize_edge_action(&AllowAll, &view, "issues.create"));
        assert!(!authorize_edge_action(&AllowAll, &view, "issues.close"));

        let transition = identity(
            CredentialPurpose::AgentRun {
                run_id: "run-issues-close".into(),
                delegation_snapshot: Some(10),
            },
            CredentialAudience::Edge,
            &["issue.transition"],
        );
        assert!(authorize_edge_action(
            &AllowAll,
            &transition,
            "issues.close"
        ));
        assert!(!authorize_edge_action(
            &AllowAll,
            &transition,
            "issues.view"
        ));

        let create = identity(
            CredentialPurpose::Pat,
            CredentialAudience::Edge,
            &["issue.create"],
        );
        assert!(authorize_edge_action(&AllowAll, &create, "issues.create"));
        assert!(!authorize_edge_action(
            &AllowAll,
            &create,
            "issues.authorization_status"
        ));
        assert!(!authorize_edge_action(&AllowAll, &create, "issues.close"));
    }

    #[test]
    fn ci_run_reads_require_exact_authority_and_never_accept_a_job_token() {
        let viewer = identity(
            CredentialPurpose::Pat,
            CredentialAudience::Edge,
            &["run.view"],
        );
        assert!(authorize_edge_action(&AllowAll, &viewer, "ci.runs.list"));
        assert!(authorize_edge_action(&AllowAll, &viewer, "ci.run.view"));
        assert!(authorize_edge_action(&AllowAll, &viewer, "ci.run.log.read"));
        assert!(!authorize_edge_action(
            &AllowAll,
            &viewer,
            "git.checks.report"
        ));

        let repo_only = identity(
            CredentialPurpose::Pat,
            CredentialAudience::Edge,
            &["repo.pull"],
        );
        assert!(!authorize_edge_action(
            &AllowAll,
            &repo_only,
            "ci.runs.list"
        ));
        let ci_job = identity(
            CredentialPurpose::CiJob {
                run_id: "ci-run-read".into(),
            },
            CredentialAudience::Edge,
            &["run.view"],
        );
        assert!(!authorize_edge_action(&AllowAll, &ci_job, "ci.run.view"));
        assert!(!authorize_edge_action(
            &AllowAll,
            &ci_job,
            "ci.run.log.read"
        ));
    }
}
