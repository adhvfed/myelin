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
use myelin_substrate::Authorizer;
use std::collections::BTreeSet;

/// **The canonical allowlist of action verbs mounted on the production edge** — the single place
/// the composition root (`main.rs`) seeds [`AuthenticatedActionPolicy`] from. One entry per action
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
    // -- the git smart-HTTP wire (register_git_wire) --
    "git.wire.upload_pack",
    "git.wire.receive_pack",
];

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

    /// The production policy: exactly the mounted edge actions ([`MOUNTED_EDGE_ACTIONS`]) — what
    /// the composition root (`main.rs`) injects into `Gateway::builder`.
    pub fn mounted() -> AuthenticatedActionPolicy {
        AuthenticatedActionPolicy::new(MOUNTED_EDGE_ACTIONS.iter().copied())
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
    use myelin_substrate::DenyAll;
    use myelin_tenancy::TenantId;

    fn principal() -> Principal {
        Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, TenantId("acme".into()))
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
            "git.repo.delete",     // a plausible FUTURE verb — not mounted, must be denied
            "git.unmapped:/api/git/future", // the fail-honest placeholder — deliberately denied
            "",                    // the degenerate empty action
            "edge.whoami2",        // a near-miss must not prefix-match
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
}
