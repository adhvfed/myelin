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

use myelin_agent::{EffectApi, EffectResult, EventId, ProposedEffect, RunCtx};
use myelin_git::pr_store::MergeAttempt;
use myelin_identity::Principal;
use serde_json::Value;

use crate::git_durable::DurableGitBackend;

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
    ) -> GitEffectApi {
        GitEffectApi {
            backend,
            tenant: tenant.into(),
            region: region.into(),
            principal,
        }
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
                let verdict = str_arg(args, "verdict").unwrap_or("comment");
                match self.backend.submit_review(t, r, repo, number, verdict, &self.principal) {
                    Ok(rec) => applied(run, tool, &format!("git.pr.review:#{}:{}", rec.number, verdict)),
                    Err(e) => EffectResult::Denied(e.to_string()),
                }
            }
            "git.endorse_fork_ci" => {
                let (repo, number) = match repo_and_number(args) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                match self.backend.endorse_fork_ci(t, r, repo, number, args) {
                    Ok(rec) => applied(run, tool, &format!("git.pr.endorse:#{}:{}", rec.number, rec.endorsed_contexts.len())),
                    Err(e) => EffectResult::Denied(e.to_string()),
                }
            }
            "git.merge" => {
                let (repo, number) = match repo_and_number(args) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
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
}

impl EffectApi for GitEffectApi {
    fn apply(&self, run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        match parse_proposed(&effect.0) {
            Some((tool, args)) => self.apply_tool(run, &tool, &args),
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
}
