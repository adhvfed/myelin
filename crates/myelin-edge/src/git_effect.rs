use std::sync::Arc;

use myelin_agent::{
    EffectApi, EffectApproval, EffectAuthority, EffectResource, EffectResult, EventId,
    ProposedEffect, RunCtx,
};
use myelin_git::core::RepoLoc;
use myelin_git::durable::DurableError;
use myelin_git::pg_pr_store::PrOperationId;
use myelin_git::pr_store::MergeAttempt;
use myelin_identity::Principal;
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_storage::TenantScope;
use myelin_tenancy::Region;
use serde_json::Value;

use crate::agent_delegation::is_active_delegation;
use crate::effect_carrier::parse_proposed;
use crate::git_durable::{map_durable_err, AgentFileWrite, DurableGitBackend, RepoActorContext};
use crate::repo_authz::RepoPermission;

pub struct GitEffectApi {
    backend: Arc<DurableGitBackend>,
    tenant: String,
    region: String,
    principal: Principal,
    delegator: Principal,
    authority: Arc<RunTokenAuthorizer>,
}

impl GitEffectApi {
    pub fn new(
        backend: Arc<DurableGitBackend>,
        tenant: impl Into<String>,
        region: impl Into<String>,
        principal: Principal,
        delegator: Principal,
        authority: Arc<RunTokenAuthorizer>,
    ) -> GitEffectApi {
        GitEffectApi {
            backend,
            tenant: tenant.into(),
            region: region.into(),
            principal,
            delegator,
            authority,
        }
    }

    fn authorize_effect(
        &self,
        authority: &EffectAuthority,
        proposed_tool: &str,
        arguments: &Value,
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
                "git effect adapter scope does not match its authenticated principal - denied"
                    .into(),
            );
        }
        if !is_active_delegation(&self.principal, &self.delegator) {
            return Err("git effect delegation binding is not an active human relationship".into());
        }
        let def = myelin_git::api::agent_tools()
            .into_iter()
            .find(|def| def.name == proposed_tool)
            .ok_or_else(|| format!("unknown git tool `{proposed_tool}` at authority boundary"))?;
        if def.requires_approval && authority.approval != EffectApproval::HumanApproved {
            return Err(format!(
                "git tool `{proposed_tool}` requires a consumed human approval"
            ));
        }
        let required_caps: Vec<String> = def
            .required_caps
            .iter()
            .map(|cap| (*cap).to_string())
            .collect();
        let repository = str_arg(arguments, "repo").ok_or_else(|| {
            "git tool argument `repo` is required at the authority boundary".to_string()
        })?;
        let scope = TenantScope::from_verified_token(&self.principal, Region(self.region.clone()));
        self.authority
            .authorize_repository(
                &scope,
                &self.principal.principal_id,
                &authority.run_token,
                &required_caps,
                repository,
            )
            .map(|_| ())
    }

    fn apply_tool(
        &self,
        run: &RunCtx,
        tool: &str,
        args: &Value,
        operation_id: &PrOperationId,
    ) -> EffectResult {
        let (t, r) = (self.tenant.as_str(), self.region.as_str());
        match tool {
            "git.write_file" => {
                let required = |name| {
                    str_arg(args, name).ok_or_else(|| {
                        EffectResult::Denied(format!("git tool argument `{name}` is required"))
                    })
                };
                let (repo, gitref, path, contents, base_oid) = match (
                    required("repo"),
                    required("ref"),
                    required("path"),
                    required("contents"),
                    required("base_oid"),
                ) {
                    (Ok(repo), Ok(gitref), Ok(path), Ok(contents), Ok(base_oid)) => {
                        (repo, gitref, path, contents, base_oid)
                    }
                    (Err(error), _, _, _, _)
                    | (_, Err(error), _, _, _)
                    | (_, _, Err(error), _, _)
                    | (_, _, _, Err(error), _)
                    | (_, _, _, _, Err(error)) => return error,
                };
                if let Err(denied) = self.authorize_repo(repo, RepoPermission::Push) {
                    return denied;
                }
                let request = AgentFileWrite {
                    target: RepoActorContext::new(t, r, repo, &self.principal),
                    gitref,
                    path,
                    expected_base: base_oid,
                    contents,
                    start_ref: str_arg(args, "start_ref"),
                    operation_id,
                };
                match self.backend.write_file_with_operation(request) {
                    Ok(commit_oid) => applied_resource(
                        run,
                        &format!("git.file.write:{commit_oid}"),
                        format!("myelin://{t}/git/commit/{repo}:{commit_oid}"),
                        serde_json::json!({
                            "commit_oid": commit_oid,
                            "repo": repo,
                            "ref": gitref,
                            "path": path,
                        }),
                    ),
                    Err(error) => deny_durable_error(error),
                }
            }
            "git.open_pr" => {
                let repo = match str_arg(args, "repo") {
                    Some(s) => s,
                    None => return deny_missing("repo"),
                };
                if let Err(denied) = self.authorize_repo(repo, RepoPermission::Push) {
                    return denied;
                }
                match self.backend.open_pr_for_actor_with_operation(
                    t,
                    r,
                    repo,
                    args,
                    &self.principal,
                    &self.delegator,
                    operation_id,
                ) {
                    Ok(rec) => applied_resource(
                        run,
                        &format!("git.pr.open:#{}", rec.number),
                        format!("myelin://{t}/git/pr/{repo}:{}", rec.number),
                        serde_json::json!({
                            "number": rec.number,
                            "repo": repo,
                            "base_ref": rec.base_ref,
                            "head_ref": rec.head_ref,
                            "head_oid": rec.head_oid,
                        }),
                    ),
                    Err(error) => deny_durable_error(error),
                }
            }
            "git.submit_review" => {
                let (repo, number) = match repo_and_number(args) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                if !self
                    .backend
                    .authorize_pr_review(t, r, repo, number, &self.principal)
                {
                    return EffectResult::Denied("no review grant for this pull request".into());
                }
                let verdict = str_arg(args, "verdict").unwrap_or("comment");
                match self.backend.submit_review_with_operation(
                    t,
                    r,
                    repo,
                    number,
                    verdict,
                    &self.principal,
                    operation_id,
                ) {
                    Ok(rec) => applied(
                        run,
                        tool,
                        &format!("git.pr.review:#{}:{}", rec.number, verdict),
                    ),
                    Err(error) => deny_durable_error(error),
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
                match self.backend.endorse_fork_ci_with_operation(
                    t,
                    r,
                    repo,
                    number,
                    args,
                    &self.principal,
                    operation_id,
                ) {
                    Ok(rec) => applied(
                        run,
                        tool,
                        &format!(
                            "git.pr.endorse:#{}:{}",
                            rec.number,
                            rec.endorsed_contexts.len()
                        ),
                    ),
                    Err(error) => deny_durable_error(error),
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
                match self.backend.merge_human_approved_agent_with_operation(
                    RepoActorContext::new(t, r, repo, &self.principal).for_pr(number),
                    &self.delegator,
                    operation_id,
                ) {
                    Ok(MergeAttempt::Merged { base_ref, new_oid, .. }) => {
                        applied(run, tool, &format!("git.pr.merge:#{number}:{base_ref}@{new_oid}"))
                    }
                    Ok(MergeAttempt::Blocked(eval)) => EffectResult::Denied(format!(
                        "merge blocked by policy (required-set admitted: {}, ruleset satisfied: {}) \
                         - the gate is server-enforced; the tool cannot bypass it",
                        eval.gate.is_admitted(),
                        eval.ruleset.is_satisfied()
                    )),
                    Ok(MergeAttempt::RefRefused(why)) => {
                        EffectResult::Denied(format!("merge ref advance refused: {why:?}"))
                    }
                    Ok(MergeAttempt::InvalidHead(why)) => {
                        EffectResult::Denied(format!("invalid merge head: {why}"))
                    }
                    Err(error) => deny_durable_error(error),
                }
            }
            other => EffectResult::Denied(format!(
                "git tool `{other}` is registered but not yet wired through GitEffectApi (GT-005b) \
                 - denied, never a silent no-op"
            )),
        }
    }

    fn authorize_repo(&self, repo: &str, permission: RepoPermission) -> Result<(), EffectResult> {
        let loc = RepoLoc::new(&self.tenant, &self.region, repo);
        if self.backend.repo_authorizer().authorize_repo_permission(
            &self.delegator,
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
            "git mutation requires the signed run-token authority entry - direct EffectApi::apply is denied"
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
            Some((tool, args)) => match self.authorize_effect(authority, &tool, &args) {
                Ok(()) => match mcp_operation_id(
                    &self.tenant,
                    &authority.principal_id.0,
                    &authority.idempotency_key,
                ) {
                    Ok(operation_id) => self.apply_tool(run, &tool, &args, &operation_id),
                    Err(error) => deny_durable_error(error),
                },
                Err(reason) => EffectResult::Denied(reason),
            },
            None => EffectResult::Denied(format!(
                "malformed proposed effect `{}` (expected `tool:<name>|args:<json>`)",
                effect.0
            )),
        }
    }
}

fn mcp_operation_id(
    tenant: &str,
    principal_id: &str,
    idempotency_key: &str,
) -> Result<PrOperationId, myelin_git::durable::DurableError> {
    PrOperationId::derive(
        "myelin.git.mcp-effect-operation.v1",
        &[
            tenant.as_bytes(),
            principal_id.as_bytes(),
            idempotency_key.as_bytes(),
        ],
    )
}

fn applied(run: &RunCtx, _tool: &str, action: &str) -> EffectResult {
    EffectResult::Applied(EventId(format!("{action}|{}", run.0)))
}

fn applied_resource(run: &RunCtx, action: &str, artifact_ref: String, data: Value) -> EffectResult {
    EffectResult::AppliedResource {
        event_id: EventId(format!("{action}|{}", run.0)),
        resource: EffectResource::new(artifact_ref, data),
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn deny_missing(field: &str) -> EffectResult {
    EffectResult::Denied(format!("git tool argument `{field}` is required"))
}

fn deny_durable_error(error: DurableError) -> EffectResult {
    EffectResult::Denied(map_durable_err(error).client_message())
}

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
    fn durable_failures_cross_the_mcp_boundary_as_public_errors_only() {
        for private_failure in [
            DurableError::Io(
                "open /srv/myelin/private/repositories/acme/core.git: permission denied".into(),
            ),
            DurableError::Git("postgres relation agent_command_ledger is unavailable".into()),
        ] {
            assert_eq!(
                deny_durable_error(private_failure),
                EffectResult::Denied("internal error".into()),
            );
        }
        assert_eq!(
            deny_durable_error(DurableError::InvalidInput(
                "file edit path contains a reserved Git administrative component".into(),
            )),
            EffectResult::Denied(
                "file edit path contains a reserved Git administrative component".into(),
            ),
            "safe recovery guidance remains actionable"
        );
    }

    #[test]
    fn parse_proposed_round_trips_the_mcp_carrier() {
        let s = r#"tool:git.open_pr|args:{"repo":"alpha","number":1}"#;
        let (tool, args) = parse_proposed(s).unwrap();
        assert_eq!(tool, "git.open_pr");
        assert_eq!(args["repo"], "alpha");
        assert!(parse_proposed("garbage").is_none());
    }

    #[test]
    fn mcp_operation_identity_is_stable_bound_and_contains_no_raw_material() {
        let effect =
            ProposedEffect(r#"tool:git.open_pr|args:{"repo":"alpha","title":"private"}"#.into());
        let first = mcp_operation_id("acme", "agent:claude", "request-secret").unwrap();
        let retry = mcp_operation_id("acme", "agent:claude", "request-secret").unwrap();
        assert_eq!(first, retry);
        assert_eq!(first.digest().len(), 64);
        assert!(!first.digest().contains("request-secret"));
        assert!(!first.digest().contains("private"));
        assert_ne!(
            first,
            mcp_operation_id("acme", "agent:claude", "another-request").unwrap()
        );
        assert_ne!(
            first,
            mcp_operation_id("other", "agent:claude", "request-secret").unwrap()
        );
        let changed_effect =
            ProposedEffect(r#"tool:git.submit_review|args:{"repo":"alpha"}"#.into());
        assert_eq!(
            first,
            mcp_operation_id("acme", "agent:claude", "request-secret").unwrap(),
            "the command ledger, not a changed digest, detects key reuse across effects"
        );
        assert_ne!(effect, changed_effect);
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
        let read_principal = principal.clone();
        let authorizer = Arc::new(RunTokenAuthorizer::new(
            Arc::new(StructuralTokenVerifier::new()),
            RevocationStore::new(),
        ));
        let api = GitEffectApi::new(
            backend.clone(),
            "acme",
            "eu-west",
            principal,
            read_principal.clone(),
            authorizer,
        );
        let effect = ProposedEffect(
            r#"tool:git.open_pr|args:{"repo":"alpha","title":"blocked","head_oid":"deadbeef","base_ref":"refs/heads/main"}"#.into(),
        );

        let direct = api.apply(&RunCtx("direct".into()), effect.clone());
        assert!(matches!(direct, EffectResult::Denied(reason) if reason.contains("direct")));
        assert!(backend
            .get_pr("acme", "eu-west", "alpha", 1, &read_principal)
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
                idempotency_key: "mismatch-1".into(),
                approval: EffectApproval::NotRequired,
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
                idempotency_key: "principal-mismatch-1".into(),
                approval: EffectApproval::NotRequired,
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
            .get_pr("acme", "eu-west", "alpha", 1, &read_principal)
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
