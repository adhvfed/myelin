//! # The authorization seam at the edge (re-authorize every call)
//!
//! The gateway re-authorizes EVERY dispatched call through the substrate
//! [`Authorizer`](myelin_substrate::Authorizer) seam (the same trait the internal RPC surface uses) —
//! "internal = safe" is never presumed, and a denial is fail-closed (a 403).
//!
//! ## R2.6 — the production action gate is a real policy, not a `true`-for-everything fixture
//!
//! The action-level seam is VERB-only (`authorize(principal, action)` — no object). Post-R2.1/R2.1a
//! the per-OBJECT enforcement is the repo-authz layer (the depth-bounded Zanzibar `check` behind
//! [`crate::repo_authz_live::CheckBackedRepoAuthorizer`], consulted on the git wire AND the JSON
//! product API), so this gate's honest role is COARSE: *may an authenticated, tenant-scoped
//! principal invoke this action verb at all.* The production authorizer is
//! [`AuthenticatedActionPolicy`] — an explicit ALLOWLIST of the mounted edge action verbs
//! ([`MOUNTED_EDGE_ACTIONS`]), deny-by-default for any action string it does not recognize and for
//! any principal shape that is not an authenticated tenant member. Layering (who enforces what):
//!  - **Authentication + tenant scope** are enforced UPSTREAM by the gateway lifecycle
//!    (`authenticate` → `resolve_scope`, the cardinal IDOR rule) before this seam is consulted —
//!    this policy still refuses a degenerate empty principal/tenant as defense-in-depth.
//!  - **Per-object authorization** (which repo, which PR) is the R2.1/R2.1a object-authz layer —
//!    this action gate is the coarse pre-filter, never the object gate.
//!  - **Finer action-role RBAC** ("only CI may `git.checks.report`", "only a repo admin may
//!    `git.repo.branch_protection.set`") is a named FUTURE fragment: today those actions are
//!    admitted at the verb level and gated at the object level (`repo_admin` check / the R0.2
//!    branch-protection gate); a role-aware body drops into this same seam without reshaping it.
//!
//! [`AllowAll`] — the old M0 happy-path fixture that admitted every principal/action — is now a
//! `#[cfg(any(test, feature = "test-support"))]` TEST DOUBLE (the same posture as the identity/
//! events/storage in-memory doubles): the production edge can no longer construct it, and the
//! `no-permissive-authorizer-in-prod` scanner (myelin-lints) fires on any `Arc::new(AllowAll)` /
//! `Arc::new(AllowAllRepos)` construction outside a test gate so it cannot return.

use myelin_identity::Principal;
use myelin_identity_service::{
    CredentialAudience, CredentialPurpose, RequestIdentity, VerifiedCapabilityContext,
};
use myelin_substrate::Authorizer;
use std::collections::BTreeSet;

/// Compatibility projection of action verbs mounted on the production edge. The canonical source
/// used by production authorization is [`ACTION_REQUIREMENTS`]; a bidirectional equality test keeps
/// this older exported list synchronized. One entry per action
/// string the production route registrations declare:
///  - `edge.*` — the whoami proof routes + the SSE subscribe route (`main.rs`).
///  - `git.*` (JSON product API) — the actions [`crate::register_git_durable`] binds over Git's
///    catalogue (13 catalogue entries) + the three GT-004 browse reads it adds.
///  - `git.wire.*` — the smart-HTTP wire actions [`crate::register_git_wire`] binds.
///
/// **Drift discipline:** a route registered with an action NOT in this list is DENIED (403) by the
/// policy — loudly visible, never a silent widening. The `edge_action_policy_integration` test
/// drives every mounted route over real HTTP against this policy and fails on any action-gate 403,
/// so the list cannot silently drift from the register fns. (The `git.unmapped:<path>` fail-honest
/// placeholder actions the register fns synthesize for a future unmapped catalogue entry are
/// deliberately NOT allowlisted: an unmapped entry has no real handler contract yet, and
/// deny-at-the-action-gate is the fail-closed posture; mapping the entry adds its real action here.)
pub const MOUNTED_EDGE_ACTIONS: &[&str] = &[
    // -- the edge's own routes (main.rs) --
    "edge.whoami",
    "edge.events.subscribe",
    // -- the durable Issues product API (register_issues) --
    "issues.list",
    "issues.create",
    "issues.authorization_status",
    "issues.view",
    "issues.close",
    // -- the git JSON product API (register_git_durable over Git's catalogue) --
    "git.repos.list",
    "git.repo.create",
    "git.pr.view",
    "git.pr.checks",
    "git.blob.view",
    "git.blob.commit",
    "git.pr.open",
    "git.pr.review",
    "git.pr.endorse_fork_ci",
    "git.pr.merge",
    "git.repo.branch_protection.set",
    "git.checks.report",
    "git.search.code",
    // -- the GT-004 browse reads register_git_durable adds beyond the catalogue --
    "git.repo.view",
    "git.commits.log",
    "git.commit.diff",
    // -- the R3.1 PR-list reads register_git_durable adds beyond the catalogue --
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
    // -- the git smart-HTTP wire (register_git_wire) --
    "git.wire.upload_pack",
    "git.wire.receive_pack",
];

/// Purpose classes accepted by an Edge action requirement. The signed purpose, never an unsigned
/// request header or the resolved principal kind, selects one of these classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcceptedPurpose {
    OperatorBootstrap,
    AgentRun,
    Pat,
    CiJob,
    DeployKey,
}

const OP_AGENT_PAT: &[AcceptedPurpose] = &[
    AcceptedPurpose::OperatorBootstrap,
    AcceptedPurpose::AgentRun,
    AcceptedPurpose::Pat,
];
const OP_PAT: &[AcceptedPurpose] = &[AcceptedPurpose::OperatorBootstrap, AcceptedPurpose::Pat];
const OP_AGENT_PAT_DEPLOY: &[AcceptedPurpose] = &[
    AcceptedPurpose::OperatorBootstrap,
    AcceptedPurpose::AgentRun,
    AcceptedPurpose::Pat,
    AcceptedPurpose::DeployKey,
];
const CI_ONLY: &[AcceptedPurpose] = &[AcceptedPurpose::CiJob];

/// One auditable action-to-capability rule. Action strings route requests; capability strings are
/// signed authority vocabulary. They are deliberately distinct and never inferred from each other.
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

/// Canonical capability catalogue for every mounted Edge route. A missing entry denies before
/// dispatch. `repo.create` is an explicit tenant-level capability; check reporting is restricted to
/// a dedicated CI purpose/capability. Operator bootstrap is a signed-purpose-limited override and
/// must itself carry `edge.operator`; `agent:run` is never an override or special capability.
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
    requirement!("git.repos.list", "repo.pull", OP_AGENT_PAT),
    requirement!("git.repo.create", "repo.create", OP_PAT),
    requirement!("git.pr.view", "repo.pull", OP_AGENT_PAT),
    requirement!("git.pr.checks", "repo.pull", OP_AGENT_PAT),
    requirement!("git.blob.view", "repo.pull", OP_AGENT_PAT),
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
        CredentialPurpose::OperatorBootstrap => Some(AcceptedPurpose::OperatorBootstrap),
        CredentialPurpose::AgentRun { .. } => Some(AcceptedPurpose::AgentRun),
        CredentialPurpose::Pat => Some(AcceptedPurpose::Pat),
        CredentialPurpose::CiJob { .. } => Some(AcceptedPurpose::CiJob),
        CredentialPurpose::DeployKey => Some(AcceptedPurpose::DeployKey),
        CredentialPurpose::PerJob { .. } => None,
    }
}

/// Final coarse Edge action boundary. Both the injected verb policy and this signed capability
/// requirement must allow. Object-addressed handlers then apply their independent ReBAC guard.
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

/// **The production action-level authorizer (R2.6).** Authorizes an authenticated, tenant-scoped
/// principal for exactly the allowlisted edge action verbs — deny-by-default for everything else.
///
/// This is a real, auditable policy (not the `true`-for-everything [`AllowAll`] fixture it
/// replaces): the allowlist is explicit and centralized ([`MOUNTED_EDGE_ACTIONS`]), an unknown/
/// unregistered action string is DENIED, and a degenerate principal (empty id or tenant — a shape
/// `authenticate` never produces, refused anyway as defense-in-depth) is DENIED. See the module
/// docs for the layering: authn + tenant scope upstream, per-object authz in the R2.1 layer,
/// finer action-role RBAC a named future fragment of THIS seam.
pub struct AuthenticatedActionPolicy {
    /// The admitted action verbs (deny-by-default outside this set).
    allowed: BTreeSet<&'static str>,
}

impl AuthenticatedActionPolicy {
    /// Build the policy over an explicit action allowlist (deny-by-default outside it).
    pub fn new(actions: impl IntoIterator<Item = &'static str>) -> AuthenticatedActionPolicy {
        AuthenticatedActionPolicy {
            allowed: actions.into_iter().collect(),
        }
    }

    /// The production policy: exactly the canonical capability catalogue's actions.
    pub fn mounted() -> AuthenticatedActionPolicy {
        AuthenticatedActionPolicy::new(ACTION_REQUIREMENTS.iter().map(|rule| rule.action))
    }
}

impl Authorizer for AuthenticatedActionPolicy {
    fn authorize(&self, principal: &Principal, action: &str) -> bool {
        // Defense-in-depth: the gateway authenticates + tenant-scopes BEFORE this seam, so a
        // well-formed principal always has a non-empty id + tenant — refuse a degenerate shape
        // anyway (fail-closed, never "empty means admin").
        if principal.principal_id.0.is_empty() || principal.tenant.0.is_empty() {
            return false;
        }
        // Deny-by-default: only an explicitly allowlisted action verb is admitted.
        self.allowed.contains(action)
    }
}

/// A seam fixture that admits every principal/action — **a TEST DOUBLE only (R2.6)**. The
/// production edge injects [`AuthenticatedActionPolicy`]; this fixture exists so the transport/
/// harness tests can exercise auth/scope/dispatch without composing the action policy, exactly like
/// the substrate's `AllowPrincipal`/`DenyAll` fixtures. Gated `#[cfg(any(test, feature =
/// "test-support"))]` (the same posture as the in-memory store doubles) so the production graph
/// cannot construct it; the `no-permissive-authorizer-in-prod` scanner enforces the absence.
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
                "the mounted policy must admit the registered action `{action}` — otherwise a \
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
            "git.repo.delete", // a plausible FUTURE verb — not mounted, must be denied
            "git.unmapped:/api/git/future", // the fail-honest placeholder — deliberately denied
            "",                // the degenerate empty action
            "edge.whoami2",    // a near-miss must not prefix-match
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
        assert!(!authorize_edge_action(&AllowAll, &create, "issues.close"));
    }
}
