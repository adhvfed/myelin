use myelin_identity::Principal;
use myelin_identity_service::{
    CredentialAudience, CredentialPurpose, RequestIdentity, VerifiedCapabilityContext,
};
use myelin_substrate::Authorizer;
use std::collections::BTreeSet;

pub const MOUNTED_EDGE_ACTIONS: &[&str] = &[
    "edge.whoami",
    "edge.auth.device.approve",
    "edge.events.subscribe",
    "agent.tools.list",
    "agent.tool.view",
    "identity.agents.list",
    "identity.agent.create",
    "identity.agent.view",
    "identity.agent.suspend",
    "identity.agent.resume",
    "identity.agent.retire",
    "identity.agent.run.create",
    "identity.agent.run.close",
    "identity.agent.run.mcp",
    "identity.agent.approval.decide",
    "identity.triggers.list",
    "identity.trigger.create",
    "identity.trigger.pause",
    "identity.trigger.resume",
    "identity.trigger.disable",
    "identity.trigger.result.erase",
    "identity.trigger.firing.approve",
    "identity.trigger.firing.reject",
    "privacy.agent_data.read",
    "privacy.agent_data.erase",
    "identity.projects.list",
    "identity.project.create",
    "identity.project.view",
    "issues.list",
    "issues.create",
    "issues.import.dry_run",
    "issues.import.run",
    "issues.authorization_status",
    "issues.view",
    "issues.close",
    "issues.relations.list",
    "issues.relations.create",
    "issues.relations.remove",
    "ci.runs.list",
    "ci.run.view",
    "ci.run.log.read",
    "ci.run.log.watch",
    "notif.inbox.list",
    "notif.inbox.get",
    "notif.inbox.mark_read",
    "chat.conversations.list",
    "chat.conversation.create",
    "chat.messages.list",
    "chat.message.post",
    "knowledge.pages.list",
    "knowledge.page.create",
    "knowledge.page.view",
    "knowledge.page.save",
    "refs.backlinks.list",
    "refs.links.list",
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

const OP_PAT: &[AcceptedPurpose] = &[
    AcceptedPurpose::HumanSession,
    AcceptedPurpose::OperatorBootstrap,
    AcceptedPurpose::Pat,
];
const OP_PAT_DEPLOY: &[AcceptedPurpose] = &[
    AcceptedPurpose::HumanSession,
    AcceptedPurpose::OperatorBootstrap,
    AcceptedPurpose::Pat,
    AcceptedPurpose::DeployKey,
];
const CI_ONLY: &[AcceptedPurpose] = &[AcceptedPurpose::CiJob];
const HUMAN_OR_OPERATOR: &[AcceptedPurpose] = &[
    AcceptedPurpose::HumanSession,
    AcceptedPurpose::OperatorBootstrap,
];
const HUMAN_SESSION: &[AcceptedPurpose] = &[AcceptedPurpose::HumanSession];
const AGENT_RUN_ONLY: &[AcceptedPurpose] = &[AcceptedPurpose::AgentRun];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionRequirement {
    pub action: &'static str,
    /// `None` is reserved for a principal or credential's own scope-bound lifecycle. In that case
    /// the exact accepted credential purpose is the authority; no tenant policy grant is
    /// manufactured.
    pub required_capability: Option<&'static str>,
    pub accepted_purposes: &'static [AcceptedPurpose],
}

macro_rules! requirement {
    ($action:literal, $capability:literal, $purposes:expr) => {
        ActionRequirement {
            action: $action,
            required_capability: Some($capability),
            accepted_purposes: $purposes,
        }
    };
}

pub const ACTION_REQUIREMENTS: &[ActionRequirement] = &[
    requirement!("edge.whoami", "edge.identity.read", OP_PAT),
    requirement!(
        "edge.auth.device.approve",
        "edge.auth.delegate",
        HUMAN_OR_OPERATOR
    ),
    requirement!("edge.events.subscribe", "edge.events.subscribe", OP_PAT),
    requirement!("agent.tools.list", "agent.tools.read", OP_PAT),
    requirement!("agent.tool.view", "agent.tools.read", OP_PAT),
    requirement!("identity.agents.list", "agent.view", HUMAN_SESSION),
    requirement!("identity.agent.create", "agent.manage", HUMAN_SESSION),
    requirement!("identity.agent.view", "agent.view", HUMAN_SESSION),
    requirement!("identity.agent.suspend", "agent.manage", HUMAN_SESSION),
    requirement!("identity.agent.resume", "agent.manage", HUMAN_SESSION),
    requirement!("identity.agent.retire", "agent.manage", HUMAN_SESSION),
    requirement!("identity.agent.run.create", "agent.run", HUMAN_SESSION),
    requirement!(
        "identity.agent.approval.decide",
        "agent.manage",
        HUMAN_SESSION
    ),
    requirement!("identity.triggers.list", "agent.view", HUMAN_SESSION),
    requirement!("identity.trigger.create", "agent.manage", HUMAN_SESSION),
    requirement!("identity.trigger.pause", "agent.manage", HUMAN_SESSION),
    requirement!("identity.trigger.resume", "agent.manage", HUMAN_SESSION),
    requirement!("identity.trigger.disable", "agent.manage", HUMAN_SESSION),
    requirement!(
        "identity.trigger.result.erase",
        "agent.manage",
        HUMAN_SESSION
    ),
    requirement!(
        "identity.trigger.firing.approve",
        "agent.manage",
        HUMAN_SESSION
    ),
    requirement!(
        "identity.trigger.firing.reject",
        "agent.manage",
        HUMAN_SESSION
    ),
    ActionRequirement {
        action: "identity.agent.run.close",
        required_capability: None,
        accepted_purposes: AGENT_RUN_ONLY,
    },
    ActionRequirement {
        action: "identity.agent.run.mcp",
        required_capability: None,
        accepted_purposes: AGENT_RUN_ONLY,
    },
    ActionRequirement {
        action: "privacy.agent_data.read",
        required_capability: None,
        accepted_purposes: HUMAN_SESSION,
    },
    ActionRequirement {
        action: "privacy.agent_data.erase",
        required_capability: None,
        accepted_purposes: HUMAN_SESSION,
    },
    requirement!("identity.projects.list", "project.view", OP_PAT),
    requirement!("identity.project.create", "project.create", OP_PAT),
    requirement!("identity.project.view", "project.view", OP_PAT),
    requirement!("issues.list", "issue.view", OP_PAT),
    requirement!("issues.create", "issue.create", OP_PAT),
    requirement!("issues.import.dry_run", "issue.create", OP_PAT),
    requirement!("issues.import.run", "issue.create", OP_PAT),
    requirement!("issues.authorization_status", "issue.view", OP_PAT),
    requirement!("issues.view", "issue.view", OP_PAT),
    requirement!("issues.close", "issue.transition", OP_PAT),
    requirement!("issues.relations.list", "issue.view", OP_PAT),
    requirement!("issues.relations.create", "issue.transition", OP_PAT),
    requirement!("issues.relations.remove", "issue.transition", OP_PAT),
    requirement!("ci.runs.list", "run.view", OP_PAT),
    requirement!("ci.run.view", "run.view", OP_PAT),
    requirement!("ci.run.log.read", "run.view", OP_PAT),
    requirement!("ci.run.log.watch", "run.view", OP_PAT),
    requirement!("notif.inbox.list", "notification.read", OP_PAT),
    requirement!("notif.inbox.get", "notification.read", OP_PAT),
    requirement!("notif.inbox.mark_read", "notification.write", OP_PAT),
    requirement!("chat.conversations.list", "chat.read", OP_PAT),
    requirement!("chat.conversation.create", "chat.manage", OP_PAT),
    requirement!("chat.messages.list", "chat.read", OP_PAT),
    requirement!("chat.message.post", "chat.post", OP_PAT),
    requirement!("knowledge.pages.list", "knowledge.read", OP_PAT),
    requirement!("knowledge.page.create", "knowledge.edit", OP_PAT),
    requirement!("knowledge.page.view", "knowledge.read", OP_PAT),
    requirement!("knowledge.page.save", "knowledge.edit", OP_PAT),
    requirement!("refs.backlinks.list", "refs.read", OP_PAT),
    requirement!("refs.links.list", "refs.read", OP_PAT),
    requirement!("git.repos.list", "repo.pull", OP_PAT),
    requirement!("git.repo.create", "repo.create", OP_PAT),
    requirement!("git.pr.view", "repo.pull", OP_PAT),
    requirement!("git.pr.checks", "repo.pull", OP_PAT),
    requirement!("git.blob.view", "repo.pull", OP_PAT),
    requirement!("git.blame.view", "repo.pull", OP_PAT),
    requirement!("git.blob.commit", "repo.push", OP_PAT),
    requirement!("git.pr.open", "repo.push", OP_PAT),
    requirement!("git.pr.review", "pull_request.review", OP_PAT),
    requirement!(
        "git.pr.endorse_fork_ci",
        "repo.approve_untrusted_ci",
        OP_PAT
    ),
    requirement!("git.pr.merge", "pull_request.merge", OP_PAT),
    requirement!("git.repo.branch_protection.set", "repo.administer", OP_PAT),
    requirement!("git.checks.report", "ci.checks.report", CI_ONLY),
    requirement!("git.search.code", "repo.pull", OP_PAT),
    requirement!("git.repo.view", "repo.pull", OP_PAT),
    requirement!("git.commits.log", "repo.pull", OP_PAT),
    requirement!("git.commit.diff", "repo.pull", OP_PAT),
    requirement!("git.prs.list", "repo.pull", OP_PAT),
    requirement!("git.prs.mine", "repo.pull", OP_PAT),
    requirement!("git.pr.commits", "repo.pull", OP_PAT),
    requirement!("git.pr.diff", "repo.pull", OP_PAT),
    requirement!("git.file.lines", "repo.pull", OP_PAT),
    requirement!("git.pr.threads.list", "repo.pull", OP_PAT),
    requirement!("git.pr.thread.create", "pull_request.review", OP_PAT),
    requirement!("git.pr.comment.create", "pull_request.review", OP_PAT),
    requirement!("git.pr.thread.resolve", "pull_request.review", OP_PAT),
    requirement!("git.pr.review.start", "pull_request.review", OP_PAT),
    requirement!("git.pr.review.comment", "pull_request.review", OP_PAT),
    requirement!("git.pr.review.submit", "pull_request.review", OP_PAT),
    requirement!("git.pr.review.discard", "pull_request.review", OP_PAT),
    requirement!("git.refs.list", "repo.pull", OP_PAT),
    requirement!("git.tree.view", "repo.pull", OP_PAT),
    requirement!("git.blob.raw", "repo.pull", OP_PAT),
    requirement!("git.blob.download", "repo.pull", OP_PAT),
    requirement!("git.wire.upload_pack", "repo.pull", OP_PAT_DEPLOY),
    requirement!("git.wire.receive_pack", "repo.push", OP_PAT_DEPLOY),
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
        .filter_map(|rule| rule.required_capability.map(str::to_string))
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
    let expected_audience = match capability.purpose {
        CredentialPurpose::AgentRun { .. } => CredentialAudience::Mcp,
        _ => CredentialAudience::Edge,
    };
    if capability.audience != expected_audience {
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
        _ => rule
            .required_capability
            .is_none_or(|required| capability.effective_authority.holds(required)),
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
    fn an_agent_run_credential_opens_only_its_governed_mcp_lifecycle() {
        let agent = identity(
            CredentialPurpose::AgentRun {
                run_id: "run-close".into(),
                delegation_snapshot: Some(11),
            },
            CredentialAudience::Mcp,
            &["issue.view", "chat.post"],
        );
        assert!(authorize_edge_action(
            &AllowAll,
            &agent,
            "identity.agent.run.close"
        ));
        assert!(authorize_edge_action(
            &AllowAll,
            &agent,
            "identity.agent.run.mcp"
        ));
        assert!(!authorize_edge_action(&AllowAll, &agent, "issues.list"));
        assert!(!authorize_edge_action(
            &AllowAll,
            &agent,
            "chat.message.post"
        ));

        let legacy_edge_audience = identity(
            CredentialPurpose::AgentRun {
                run_id: "run-close".into(),
                delegation_snapshot: Some(11),
            },
            CredentialAudience::Edge,
            &[],
        );
        assert!(!authorize_edge_action(
            &AllowAll,
            &legacy_edge_audience,
            "identity.agent.run.mcp"
        ));

        let human = identity(
            CredentialPurpose::HumanSession,
            CredentialAudience::Edge,
            &["agent.run.close"],
        );
        assert!(!authorize_edge_action(
            &AllowAll,
            &human,
            "identity.agent.run.close"
        ));
        assert!(!authorize_edge_action(
            &AllowAll,
            &human,
            "identity.agent.run.mcp"
        ));
    }

    #[test]
    fn a_human_controls_their_own_agent_data_without_an_organization_grant() {
        let human = identity(
            CredentialPurpose::HumanSession,
            CredentialAudience::Edge,
            &[],
        );
        for action in ["privacy.agent_data.read", "privacy.agent_data.erase"] {
            assert!(authorize_edge_action(&AllowAll, &human, action));
        }

        let pat = identity(CredentialPurpose::Pat, CredentialAudience::Edge, &[]);
        let operator = identity(
            CredentialPurpose::OperatorBootstrap,
            CredentialAudience::Edge,
            &["edge.operator"],
        );
        let agent = identity(
            CredentialPurpose::AgentRun {
                run_id: "run-private".into(),
                delegation_snapshot: Some(1),
            },
            CredentialAudience::Mcp,
            &[],
        );
        for identity in [&pat, &operator, &agent] {
            assert!(!authorize_edge_action(
                &AllowAll,
                identity,
                "privacy.agent_data.erase"
            ));
        }
    }

    #[test]
    fn signed_audience_purpose_and_narrow_authority_are_all_enforced() {
        let agent_pull = identity(
            CredentialPurpose::AgentRun {
                run_id: "run-1".into(),
                delegation_snapshot: Some(7),
            },
            CredentialAudience::Mcp,
            &["repo.pull"],
        );
        assert!(!authorize_edge_action(
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
            CredentialPurpose::Pat,
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
        assert!(authorize_edge_action(
            &AllowAll,
            &view,
            "issues.relations.list"
        ));
        assert!(!authorize_edge_action(&AllowAll, &view, "issues.create"));
        assert!(!authorize_edge_action(
            &AllowAll,
            &view,
            "issues.import.dry_run"
        ));
        assert!(!authorize_edge_action(
            &AllowAll,
            &view,
            "issues.import.run"
        ));
        assert!(!authorize_edge_action(&AllowAll, &view, "issues.close"));
        assert!(!authorize_edge_action(
            &AllowAll,
            &view,
            "issues.relations.create"
        ));

        let transition = identity(
            CredentialPurpose::Pat,
            CredentialAudience::Edge,
            &["issue.transition"],
        );
        assert!(authorize_edge_action(
            &AllowAll,
            &transition,
            "issues.close"
        ));
        assert!(authorize_edge_action(
            &AllowAll,
            &transition,
            "issues.relations.create"
        ));
        assert!(authorize_edge_action(
            &AllowAll,
            &transition,
            "issues.relations.remove"
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
        assert!(authorize_edge_action(
            &AllowAll,
            &create,
            "issues.import.dry_run"
        ));
        assert!(authorize_edge_action(
            &AllowAll,
            &create,
            "issues.import.run"
        ));
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
