//! # `git_effect` — the concrete `EffectApi` body that lands a governed git tool on the GT-003 backend.
//!
//! This is the GT-005 connective tissue: the MR-021 [`myelin_mcp::GovernedRouter`] routes every MCP
//! `tools/call` through `mint_run_token → EffectApi::apply` (revocation-consulted, HITL-gated), but the
//! concrete `EffectApi` BODY is injected by the composition root. [`GitEffectApi`] is that body for the
//! git tools (`git.open_pr` / `git.submit_review` / `git.merge` / `git.endorse_fork_ci`): it parses the
//! brain's [`ProposedEffect`], performs the matching DURABLE [`DurableGitBackend`] operation under the
//! run's VERIFIED `(tenant, region)` scope + acting principal, and returns `Applied | Denied`.
//!
//! ## What is real vs reflected (honesty)
//! - **Real:** the durable git mutation (a PR row persists, a merge advances the base ref on disk) runs
//!   here under the run-token's RunCtx — the SAME GT-003 operation the HTTP edge handler invokes.
//! - **Reflected, never bypassed:** the merge gate is the repo-owned branch-protection policy evaluated
//!   inside [`DurableGitBackend::merge`]. A blocked merge comes back as [`EffectResult::Denied`] carrying
//!   the gate reason — the MCP tool REFLECTS the server gate; it cannot weaken or skip it.
//! - **The tenant is the run's, never the args.** The `(tenant, region)` + principal are fixed at
//!   construction from the run's verified scope (the MR-021 `RunPrincipal`); the args carry only the
//!   `repo`/`number`/proposal, never a tenant (the IDOR floor).
//!
//! The args envelope the brain proposes is `tool:<name>|args:<json>` (the opaque-string carrier the MCP
//! [`myelin_mcp::governance::proposed_effect_for`] packs); a malformed/unknown proposal is a loud
//! `Denied`, never a silent no-op or a panic.

use std::sync::Arc;

use myelin_agent::{EffectApi, EffectAuthority, EffectResult, EventId, ProposedEffect, RunCtx};
use myelin_git::core::RepoLoc;
use myelin_git::pr_store::MergeAttempt;
use myelin_identity::Principal;
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_storage::TenantScope;
use myelin_tenancy::Region;
use serde_json::Value;

use crate::git_durable::DurableGitBackend;
use crate::repo_authz::RepoPermission;

/// **The concrete git `EffectApi` body (GT-005).** Binds the durable GT-003 backend to a single run's
/// verified scope + acting principal. Injected into the MR-021 `GovernedRouter` by the composition root
/// so a governed `tools/call` lands the real git effect (or reflects the server gate's refusal).
pub struct GitEffectApi {
    backend: Arc<DurableGitBackend>,
    /// The verified tenant the run acts under (from the run's `RunPrincipal` scope — never the args).
    tenant: String,
    /// The verified residency region (from the run's scope).
    region: String,
    /// The acting principal (the run's agent/operator — authored into the durable record's pseudonym).
    principal: Principal,
    /// The signed-token + S7 verifier run again immediately before the durable mutation.
    authority: Arc<RunTokenAuthorizer>,
}

impl GitEffectApi {
    /// Build the git effect body over the durable backend + the run's VERIFIED `(tenant, region)` +
    /// acting principal. The scope is the run's (the MR-021 `RunPrincipal`); it is NEVER read from a
    /// tool argument (the IDOR floor).
    pub fn new(
        backend: Arc<DurableGitBackend>,
        tenant: impl Into<String>,
        region: impl Into<String>,
        principal: Principal,
        authority: Arc<RunTokenAuthorizer>,
    ) -> GitEffectApi {
        GitEffectApi {
            backend,
            tenant: tenant.into(),
            region: region.into(),
            principal,
            authority,
        }
    }

    fn authorize_effect(
        &self,
        authority: &EffectAuthority,
        proposed_tool: &str,
    ) -> Result<(), String> {
        if authority.tool != proposed_tool {
            return Err(format!(
                "run-token authority is bound to `{}`, not proposed tool `{proposed_tool}`",
                authority.tool
            ));
        }
        if authority.principal_id != self.principal.principal_id {
            return Err(
                "run-token authority principal does not match the mutation adapter principal"
                    .into(),
            );
        }
        if self.principal.tenant.0 != self.tenant || self.principal.region.0 != self.region {
            return Err(
                "git effect adapter scope does not match its authenticated principal — denied"
                    .into(),
            );
        }
        let def = myelin_git::api::agent_tools()
            .into_iter()
            .find(|def| def.name == proposed_tool)
            .ok_or_else(|| format!("unknown git tool `{proposed_tool}` at authority boundary"))?;
        let required_caps: Vec<String> = def
            .required_caps
            .iter()
            .map(|cap| (*cap).to_string())
            .collect();
        let scope = TenantScope::from_verified_token(&self.principal, Region(self.region.clone()));
        self.authority
            .authorize(
                &scope,
                &self.principal.principal_id,
                &authority.run_token,
                &required_caps,
            )
            .map(|_| ())
    }

    /// Apply one parsed git tool. Split out so it is unit-testable without the RunCtx wrapper.
    fn apply_tool(&self, run: &RunCtx, tool: &str, args: &Value) -> EffectResult {
        let (t, r) = (self.tenant.as_str(), self.region.as_str());
        match tool {
            "git.open_pr" => {
                let repo = match str_arg(args, "repo") {
                    Some(s) => s,
                    None => return deny_missing("repo"),
                };
                if let Err(denied) = self.authorize_repo(repo, RepoPermission::Push) {
                    return denied;
                }
                match self.backend.open_pr(t, r, repo, args, &self.principal) {
                    Ok(rec) => applied(run, tool, &format!("git.pr.open:#{}", rec.number)),
                    Err(e) => EffectResult::Denied(e.to_string()),
                }
            }
            "git.submit_review" => {
                let (repo, number) = match repo_and_number(args) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                // The compiled permission is
                // `pull_request.review = reviewer ∪ parent_repo->push`. The filesystem PR store
                // does not yet materialize the PR parent/reviewer tuples, so the sound available
                // reduction is the parent repo's Push rung. This intentionally over-denies a
                // reviewer-only grant; it never admits an ungranted reviewer.
                if let Err(denied) = self.authorize_repo(repo, RepoPermission::Push) {
                    return denied;
                }
                let verdict = str_arg(args, "verdict").unwrap_or("comment");
                match self
                    .backend
                    .submit_review(t, r, repo, number, verdict, &self.principal)
                {
                    Ok(rec) => applied(
                        run,
                        tool,
                        &format!("git.pr.review:#{}:{}", rec.number, verdict),
                    ),
                    Err(e) => EffectResult::Denied(e.to_string()),
                }
            }
            "git.endorse_fork_ci" => {
                let (repo, number) = match repo_and_number(args) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                if let Err(denied) = self.authorize_repo(repo, RepoPermission::ApproveUntrustedCi) {
                    return denied;
                }
                match self.backend.endorse_fork_ci(t, r, repo, number, args) {
                    Ok(rec) => applied(
                        run,
                        tool,
                        &format!(
                            "git.pr.endorse:#{}:{}",
                            rec.number,
                            rec.endorsed_contexts.len()
                        ),
                    ),
                    Err(e) => EffectResult::Denied(e.to_string()),
                }
            }
            "git.merge" => {
                let (repo, number) = match repo_and_number(args) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                if let Err(denied) = self.authorize_repo(repo, RepoPermission::ProtectedPush) {
                    return denied;
                }
                match self.backend.merge(t, r, repo, number, &self.principal) {
                    Ok(MergeAttempt::Merged { base_ref, new_oid, .. }) => {
                        applied(run, tool, &format!("git.pr.merge:#{number}:{base_ref}@{new_oid}"))
                    }
                    // The merge gate BLOCKED — the repo-owned branch-protection policy was not met. The
                    // tool REFLECTS the server gate's refusal (never a bypass): a loud Denied with the
                    // gate reason, no ref advanced.
                    Ok(MergeAttempt::Blocked(eval)) => EffectResult::Denied(format!(
                        "merge blocked by policy (required-set admitted: {}, ruleset satisfied: {}) \
                         — the gate is server-enforced; the tool cannot bypass it",
                        eval.gate.is_admitted(),
                        eval.ruleset.is_satisfied()
                    )),
                    Ok(MergeAttempt::RefRefused(why)) => {
                        EffectResult::Denied(format!("merge ref advance refused: {why:?}"))
                    }
                    Ok(MergeAttempt::InvalidHead(why)) => {
                        EffectResult::Denied(format!("invalid merge head: {why}"))
                    }
                    Err(e) => EffectResult::Denied(e.to_string()),
                }
            }
            other => EffectResult::Denied(format!(
                "git tool `{other}` is registered but not yet wired through GitEffectApi (GT-005b) \
                 — denied, never a silent no-op"
            )),
        }
    }

    /// The object leg of the final mutation boundary. The router's signed capability check and this
    /// live ReBAC decision are independent conjuncts: neither can substitute for the other.
    fn authorize_repo(&self, repo: &str, permission: RepoPermission) -> Result<(), EffectResult> {
        let loc = RepoLoc::new(&self.tenant, &self.region, repo);
        if self.backend.repo_authorizer().authorize_repo_permission(
            &self.principal,
            &loc,
            permission,
        ) {
            Ok(())
        } else {
            Err(EffectResult::Denied(format!(
                "object authorization denied `{permission:?}` on repository `{repo}`"
            )))
        }
    }
}

impl EffectApi for GitEffectApi {
    fn apply(&self, _run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
        EffectResult::Denied(
            "git mutation requires the signed run-token authority entry — direct EffectApi::apply is denied"
                .into(),
        )
    }

    fn apply_authorized(
        &self,
        run: &RunCtx,
        authority: &EffectAuthority,
        effect: ProposedEffect,
    ) -> EffectResult {
        match parse_proposed(&effect.0) {
            Some((tool, args)) => match self.authorize_effect(authority, &tool) {
                Ok(()) => self.apply_tool(run, &tool, &args),
                Err(reason) => EffectResult::Denied(reason),
            },
            None => EffectResult::Denied(format!(
                "malformed proposed effect `{}` (expected `tool:<name>|args:<json>`)",
                effect.0
            )),
        }
    }
}

/// Parse the MCP opaque-string `ProposedEffect` (`tool:<name>|args:<json>`) into `(tool, args)`. A shape
/// that does not match is `None` (a loud Denied at the call site) — never a panic.
fn parse_proposed(s: &str) -> Option<(String, Value)> {
    let rest = s.strip_prefix("tool:")?;
    let (tool, args_str) = rest.split_once("|args:")?;
    let args = serde_json::from_str(args_str).unwrap_or(Value::Null);
    Some((tool.to_string(), args))
}

/// The `Applied` outcome whose event id carries the RunCtx (the run-token jti + principal + tool) — so
/// the audit trail attributes the durable git effect to the minted run token (NOT a bare PAT).
fn applied(run: &RunCtx, _tool: &str, action: &str) -> EffectResult {
    EffectResult::Applied(EventId(format!("{action}|{}", run.0)))
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn deny_missing(field: &str) -> EffectResult {
    EffectResult::Denied(format!("git tool argument `{field}` is required"))
}

/// Extract `(repo, number)` from the tool args (the per-PR target). A missing/non-numeric value is a
/// loud Denied, never a panic.
fn repo_and_number(args: &Value) -> Result<(&str, u64), EffectResult> {
    let repo = str_arg(args, "repo").ok_or_else(|| deny_missing("repo"))?;
    let number = args
        .get("number")
        .and_then(Value::as_u64)
        .ok_or_else(|| deny_missing("number"))?;
    Ok((repo, number))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind, RunToken, RuntimeRef};
    use myelin_identity_service::machine_auth::StructuralTokenVerifier;
    use myelin_identity_service::revocation::RevocationStore;
    use myelin_tenancy::TenantId;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parse_proposed_round_trips_the_mcp_carrier() {
        // The exact shape myelin_mcp::governance::proposed_effect_for builds.
        let s = r#"tool:git.open_pr|args:{"repo":"alpha","number":1}"#;
        let (tool, args) = parse_proposed(s).unwrap();
        assert_eq!(tool, "git.open_pr");
        assert_eq!(args["repo"], "alpha");
        // A malformed carrier is None (→ a loud Denied at the call site), never a panic.
        assert!(parse_proposed("garbage").is_none());
    }

    #[test]
    fn direct_and_mismatched_tool_invocations_cannot_reach_the_git_mutation() {
        let mut root = std::env::temp_dir();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        root.push(format!("myelin-git-effect-authority-{nonce}"));
        let backend = Arc::new(DurableGitBackend::rooted_inmem_for_test(&root));
        backend
            .create_repo("acme", "eu-west", "alpha")
            .expect("repo");
        let mut principal = Principal::stub(
            PrincipalId("agent:claude".into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt-local".into()),
                on_behalf_of: None,
            },
            TenantId("acme".into()),
        );
        principal.region = Region("eu-west".into());
        let authorizer = Arc::new(RunTokenAuthorizer::new(
            Arc::new(StructuralTokenVerifier::new()),
            RevocationStore::new(),
        ));
        let api = GitEffectApi::new(backend.clone(), "acme", "eu-west", principal, authorizer);
        let effect = ProposedEffect(
            r#"tool:git.open_pr|args:{"repo":"alpha","title":"blocked","head_oid":"deadbeef","base_ref":"refs/heads/main"}"#.into(),
        );

        let direct = api.apply(&RunCtx("direct".into()), effect.clone());
        assert!(matches!(direct, EffectResult::Denied(reason) if reason.contains("direct")));
        assert!(backend
            .get_pr("acme", "eu-west", "alpha", 1)
            .unwrap()
            .is_none());

        let mismatched = api.apply_authorized(
            &RunCtx("mismatch".into()),
            &EffectAuthority {
                run_token: RunToken {
                    token: "not-trusted".into(),
                    jti: "not-trusted".into(),
                },
                principal_id: PrincipalId("agent:claude".into()),
                tool: "git.submit_review".into(),
            },
            effect,
        );
        assert!(matches!(
            mismatched,
            EffectResult::Denied(reason) if reason.contains("not proposed tool")
        ));
        let principal_mismatch = api.apply_authorized(
            &RunCtx("principal-mismatch".into()),
            &EffectAuthority {
                run_token: RunToken {
                    token: "not-trusted".into(),
                    jti: "not-trusted".into(),
                },
                principal_id: PrincipalId("agent:other".into()),
                tool: "git.open_pr".into(),
            },
            ProposedEffect(
                r#"tool:git.open_pr|args:{"repo":"alpha","title":"blocked","head_oid":"deadbeef","base_ref":"refs/heads/main"}"#.into(),
            ),
        );
        assert!(matches!(
            principal_mismatch,
            EffectResult::Denied(reason) if reason.contains("adapter principal")
        ));
        assert!(backend
            .get_pr("acme", "eu-west", "alpha", 1)
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
