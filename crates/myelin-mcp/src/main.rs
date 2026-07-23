//! Production composition root for the governed MCP stdio server and its operator-only HITL tools.
//! Every serving path is durable-by-default and fail-loud: real PASETO authentication, PostgreSQL
//! identity/revocation/delegation/HITL stores, live Git ReBAC, and the durable outbox.

use std::fs::OpenOptions;
use std::io::{self, BufRead, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use myelin_config::Mode;
use myelin_edge::repo_authz::RepoPermission;
use myelin_edge::{
    recover_placed_git_at_boot, CheckBackedRepoAuthorizer, DurableGitBackend, GitDatabaseProviders,
    GitEffectApi, RepoAuthorizer, TupleRepoBootstrap,
};
use myelin_events::{IdMinter, OutboxStore, Timestamp, UlidMinter};
use myelin_git::core::RepoLoc;
use myelin_git::gix_backend::validate_repo_slug;
use myelin_identity::{
    Credential, DataRole, DelegationCaveats, FailStaticBound, FragmentAdmit, Principal,
    PrincipalId, PrincipalKind, PrincipalStatus, RunId,
};
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, CredentialAudience,
    CredentialPurpose, DelegationPolicySource, PasetoCapabilityVerifier, PrincipalStore,
    RevocationStore, StoreBackedCheck,
};
use myelin_mcp::{
    git_merge_repo_from_effect_key, AuditPhase, GateApproverPolicy, GovernanceAudit,
    GovernanceAuditRecord, GovernedRouter, McpServer, OutboxGovernanceAudit, RunPrincipal,
    ToolRegistry, MAX_FRAME_BYTES,
};
use myelin_storage::hitl_gate_durable::HitlVerdictStore;
use myelin_storage::{
    all_durable_migrations, seal_key_from_env, DurableCellRootBacking,
    DurableDelegationPolicyBacking, DurableKmsBacking, DurablePrincipalBacking,
    DurableRevocationBacking, HotTables, KmsEngine, PgBootstrap, PgOutboxBacking,
    SubstrateProvider,
};
use myelin_substrate::Thresholds;
use myelin_tenancy::Region;
use zeroize::Zeroize;

const MAX_CREDENTIAL_BYTES: u64 = 64 * 1024;
const MAX_OPERATOR_ARGS: usize = 64;
const MAX_BOOTSTRAP_PRINCIPAL_BYTES: usize = 255;
const MCP_SCHEME_ENV: &str = "MYELIN_MCP_CREDENTIAL_SCHEME";
const MCP_CREDENTIAL_FILE_ENV: &str = "MYELIN_MCP_CREDENTIAL_FILE";
const MCP_TENANT_ENV: &str = "MYELIN_MCP_TENANT";
const MCP_REGION_ENV: &str = "MYELIN_MCP_REGION";
const MCP_HITL_DECIDE_CAP: &str = "mcp.hitl.decide";

struct Core {
    provider: SubstrateProvider,
    kms: Arc<KmsEngine>,
    cell: Arc<CellTokenAuthority>,
    cell_id: String,
    handle: tokio::runtime::Handle,
}

struct GitMergeApproverPolicy {
    authorizer: Arc<dyn RepoAuthorizer>,
    principals: PrincipalStore,
    candidate_ids: Vec<PrincipalId>,
    scope: myelin_storage::TenantScope,
}

impl GateApproverPolicy for GitMergeApproverPolicy {
    fn eligible_approvers(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<Vec<PrincipalId>, String> {
        if tool != "git.merge" {
            return Err(format!(
                "no object-scoped HITL policy is registered for `{tool}`"
            ));
        }
        let repo = args
            .get("repo")
            .and_then(serde_json::Value::as_str)
            .filter(|repo| !repo.is_empty() && repo.len() <= 255)
            .ok_or_else(|| "git.merge requires a bounded string `repo`".to_string())?;
        validate_repo_slug(repo).map_err(|_| "git.merge repository slug is invalid".to_string())?;
        let loc = RepoLoc::new(
            self.scope.tenant().0.as_str(),
            self.scope.region().0.as_str(),
            repo,
        );
        let eligible = self
            .candidate_ids
            .iter()
            .filter_map(|candidate_id| {
                let row = self.principals.get_principal(&self.scope, candidate_id)?;
                if row.kind != PrincipalKind::Human || row.status != PrincipalStatus::Active {
                    return None;
                }
                let candidate = Principal::new(
                    row.tenant,
                    row.region,
                    row.principal_id,
                    row.kind,
                    row.data_role,
                    row.status,
                );
                self.authorizer
                    .authorize_repo_permission(&candidate, &loc, RepoPermission::ProtectedPush)
                    .then_some(candidate.principal_id)
            })
            .collect::<Vec<_>>();
        if eligible.is_empty() {
            Err(
                "no configured active Human has live protected-push authority on this repository"
                    .into(),
            )
        } else {
            Ok(eligible)
        }
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    myelin_events::install_payload_free_panic_hook("mcp");
    if let Err(error) = dispatch().await {
        eprintln!("myelin-mcp: {error}");
        std::process::exit(1);
    }
}

async fn dispatch() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = parse_command(&args)?;
    preflight_command(&command)?;
    match command {
        Command::Serve => serve(compose_core().await?).await,
        Command::Approve(gate_id) => decide(compose_core().await?, &gate_id, true).await,
        Command::Reject(gate_id) => decide(compose_core().await?, &gate_id, false).await,
        Command::Bootstrap(parsed) => bootstrap(compose_core().await?, parsed).await,
    }
}

enum Command {
    Serve,
    Approve(String),
    Reject(String),
    Bootstrap(BootstrapArgs),
}

/// Pure/configuration preflight that must finish before database connection, migrations, or key
/// loading. Authentication itself remains after key load, but malformed scope config and unsafe
/// credential files cannot trigger stateful composition work.
fn preflight_command(command: &Command) -> Result<(), String> {
    let tenant = required_env(MCP_TENANT_ENV)?;
    let region = required_env(MCP_REGION_ENV)?;
    validate_partition_token(MCP_TENANT_ENV, &tenant)?;
    validate_partition_token(MCP_REGION_ENV, &region)?;
    let scheme = required_env(MCP_SCHEME_ENV)?;
    validate_bootstrap_scheme(&scheme)?;
    if !matches!(command, Command::Bootstrap(_)) {
        let path = PathBuf::from(required_env(MCP_CREDENTIAL_FILE_ENV)?);
        let mut checked_material = read_secret_file(&path)?;
        checked_material.zeroize();
    }
    Ok(())
}

fn validate_partition_token(name: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 255
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        Err(format!(
            "{name} must be a non-empty bounded ASCII partition token"
        ))
    } else {
        Ok(())
    }
}

fn parse_command(args: &[String]) -> Result<Command, String> {
    if args.len() > MAX_OPERATOR_ARGS {
        return Err(format!(
            "refusing more than {MAX_OPERATOR_ARGS} command arguments"
        ));
    }
    match args.first().map(String::as_str) {
        None => Ok(Command::Serve),
        Some("serve") if args.len() == 1 => Ok(Command::Serve),
        Some("serve") => Err("serve accepts no arguments".into()),
        Some("approve") => Ok(Command::Approve(
            parse_decision_args(&args[1..])?.to_string(),
        )),
        Some("reject") => Ok(Command::Reject(
            parse_decision_args(&args[1..])?.to_string(),
        )),
        Some("bootstrap") => Ok(Command::Bootstrap(parse_bootstrap_args(&args[1..])?)),
        Some(other) => Err(format!(
            "unknown subcommand `{other}` (expected serve, approve, reject, or bootstrap)"
        )),
    }
}

async fn compose_core() -> Result<Core, String> {
    let bootstrap = PgBootstrap::from_env(Mode::RequireEnv)
        .await
        .map_err(|error| format!("database bootstrap refused: {error}"))?;
    bootstrap
        .migrate_foundation()
        .await
        .map_err(|error| format!("foundation migration failed: {error}"))?;
    bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .map_err(|error| format!("durable migration aggregate failed: {error}"))?;
    bootstrap
        .migrate(
            &myelin_git::pg_pr_store::git_pr_migrations(),
            &myelin_git::pg_pr_store::git_pr_hot_tables(),
        )
        .await
        .map_err(|error| format!("Git PR lifecycle migration failed: {error}"))?;
    bootstrap
        .verify_index_ready("git_pr_head_repo_idx")
        .await
        .map_err(|error| format!("Git PR provenance index is not ready: {error}"))?;
    bootstrap
        .verify_index_ready("git_pr_command_operation_scope_uidx")
        .await
        .map_err(|error| format!("Git PR operation-scope index is not ready: {error}"))?;
    let provider = bootstrap
        .into_runtime()
        .await
        .map_err(|error| format!("runtime database handoff refused: {error}"))?;
    let seal = seal_key_from_env().map_err(|error| format!("seal key refused: {error}"))?;
    let cell_id = required_env("MYELIN_CELL_ID")?;
    let kms = Arc::new(
        DurableKmsBacking::new(provider.db_pool().clone(), cell_id.clone())
            .load_or_generate(&seal)
            .await
            .map_err(|error| format!("durable KMS refused: {error}"))?,
    );
    let material = DurableCellRootBacking::new(provider.db_pool().clone(), cell_id.clone())
        .load_or_generate(&seal)
        .await
        .map_err(|error| format!("durable cell token root refused: {error}"))?;
    let cell = Arc::new(
        CellTokenAuthority::from_material(&material)
            .map_err(|error| format!("durable cell token root is invalid: {error:?}"))?,
    );
    Ok(Core {
        provider,
        kms,
        cell,
        cell_id,
        handle: tokio::runtime::Handle::current(),
    })
}

fn principal_store(core: &Core) -> PrincipalStore {
    PrincipalStore::with_pg(
        core.kms.clone(),
        DurablePrincipalBacking::new(core.provider.clone()),
        core.handle.clone(),
    )
}

fn revocations(core: &Core) -> RevocationStore {
    RevocationStore::with_pg(
        DurableRevocationBacking::new(core.provider.clone()),
        core.handle.clone(),
    )
}

fn authenticate_from_secure_file(
    core: &Core,
) -> Result<myelin_identity_service::RequestIdentity, String> {
    let path = PathBuf::from(required_env(MCP_CREDENTIAL_FILE_ENV)?);
    let scheme = required_env(MCP_SCHEME_ENV)?;
    let mut material = read_secret_file(&path)?;
    let auth = CapabilityAuthenticator::with_verifier(
        principal_store(core),
        Arc::new(PasetoCapabilityVerifier::new(core.cell.trust_anchor())),
        revocations(core),
    );
    let credential = Credential {
        scheme,
        material: std::mem::take(&mut material),
    };
    let mut credential = credential;
    let result = auth
        .authenticate_identity(&credential, None)
        .map_err(|error| format!("credential authentication refused: {error:?}"));
    credential.material.zeroize();
    drop(credential);
    let identity = result?;
    validate_mcp_operator_context(
        &identity.capability().purpose,
        &identity.capability().audience,
    )?;
    let tenant = required_env(MCP_TENANT_ENV)?;
    let region = required_env(MCP_REGION_ENV)?;
    if identity.scope.tenant().0 != tenant || identity.scope.region().0 != region {
        return Err("configured tenant/region does not match the verified credential scope".into());
    }
    if core.provider.config().region != region {
        return Err("configured MCP region does not match the runtime database region".into());
    }
    Ok(identity)
}

fn validate_mcp_operator_context(
    purpose: &CredentialPurpose,
    audience: &CredentialAudience,
) -> Result<(), String> {
    if audience != &CredentialAudience::Mcp {
        return Err("signed credential audience is not `mcp`".into());
    }
    if purpose != &CredentialPurpose::OperatorBootstrap {
        return Err("signed MCP credential purpose is not `operator_bootstrap`".into());
    }
    Ok(())
}

fn read_secret_file(path: &Path) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot stat credential file {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("credential file must be a regular, non-symlink file".into());
    }
    if metadata.permissions().mode() & 0o777 != 0o600 {
        return Err("credential file permissions must be exactly 0600".into());
    }
    // SAFETY: `geteuid` has no pointer arguments or memory-safety preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid {
        return Err("credential file must be owned by the effective process user".into());
    }
    if metadata.nlink() != 1 {
        return Err("credential file must have exactly one hard link".into());
    }
    if metadata.len() == 0 || metadata.len() > MAX_CREDENTIAL_BYTES {
        return Err(format!(
            "credential file must contain 1..={MAX_CREDENTIAL_BYTES} bytes"
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| format!("cannot securely open credential file: {error}"))?;
    let after = file
        .metadata()
        .map_err(|error| format!("cannot re-stat opened credential file: {error}"))?;
    if after.permissions().mode() & 0o777 != 0o600
        || after.uid() != effective_uid
        || after.nlink() != 1
        || after.len() == 0
        || after.len() > MAX_CREDENTIAL_BYTES
        || after.dev() != metadata.dev()
        || after.ino() != metadata.ino()
    {
        return Err("credential file changed while it was being opened".into());
    }
    let mut bytes = Vec::with_capacity(after.len() as usize);
    if let Err(error) = file.take(MAX_CREDENTIAL_BYTES + 1).read_to_end(&mut bytes) {
        bytes.zeroize();
        return Err(format!("cannot read credential file: {error}"));
    }
    if bytes.len() as u64 > MAX_CREDENTIAL_BYTES {
        bytes.zeroize();
        return Err("credential file exceeded its size bound while reading".into());
    }
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
    }
    let result = match String::from_utf8(bytes) {
        Ok(value) => value,
        Err(error) => {
            let mut rejected = error.into_bytes();
            rejected.zeroize();
            return Err("credential file is not valid UTF-8".into());
        }
    };
    if result.is_empty() {
        Err("credential file is empty".into())
    } else {
        Ok(result)
    }
}

async fn serve(core: Core) -> Result<(), String> {
    let trigger_identity = authenticate_from_secure_file(&core)?;
    if trigger_identity.principal.kind != PrincipalKind::Human
        || trigger_identity.principal.status != PrincipalStatus::Active
    {
        return Err("the MCP trigger credential must resolve to an active Human principal".into());
    }
    let agent_id = PrincipalId(required_env("MYELIN_MCP_AGENT_ID")?);
    let agent_row = principal_store(&core)
        .get_principal(&trigger_identity.scope, &agent_id)
        .ok_or_else(|| "configured MCP agent does not exist in the verified scope".to_string())?;
    if !matches!(agent_row.kind, PrincipalKind::Agent { .. })
        || agent_row.status != PrincipalStatus::Active
    {
        return Err("configured MCP agent must be an active Agent principal".into());
    }
    let agent = Principal::new(
        agent_row.tenant,
        agent_row.region,
        agent_row.principal_id,
        agent_row.kind,
        agent_row.data_role,
        agent_row.status,
    );
    let run_id = fresh_run_id();
    let resolved =
        DelegationPolicySource::with_pg(DurableDelegationPolicyBacking::new(core.provider.clone()))
            .resolve_for_run(
                &trigger_identity.scope,
                &agent,
                &trigger_identity.principal,
                &run_id,
            )
            .await
            .map_err(|error| format!("durable delegation policy refused the run: {error}"))?;
    let resolved = resolved.attenuate(&trigger_identity.capability().effective_authority);
    let caveats = DelegationCaveats(
        resolved
            .input()
            .delegation
            .grants()
            .map(str::to_string)
            .collect(),
    );

    let check = StoreBackedCheck::with_pg(
        core.provider.clone(),
        core.kms.clone(),
        core.cell.clone(),
        core.handle.clone(),
    );
    for result in check.admit_git_fragment() {
        if let FragmentAdmit::Rejected { reason } = result {
            return Err(format!("Git ReBAC fragment refused to admit: {reason}"));
        }
    }
    let thresholds = Thresholds::load_canonical()
        .map_err(|error| format!("canonical thresholds refused: {error}"))?;
    let repo_authz = Arc::new(
        CheckBackedRepoAuthorizer::try_new(
            check.clone(),
            thresholds.revocation.sla_mins * 60,
            &thresholds.fail_static,
        )
        .map_err(|error| format!("Git ReBAC authorizer refused: {error:?}"))?,
    );
    let git_root = PathBuf::from(required_env("MYELIN_GIT_ROOT")?);
    if !git_root.is_absolute() || git_root == Path::new("/") {
        return Err("MYELIN_GIT_ROOT must be an absolute, non-root path".into());
    }
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        core.provider.db_pool().clone(),
        core.handle.clone(),
    )));
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(UlidMinter::new());
    let git_check_admission_provider = core
        .provider
        .auxiliary_runtime_lane(4)
        .await
        .map_err(|error| format!("protected-push admission lane refused: {error}"))?;
    let backend = Arc::new(
        DurableGitBackend::rooted(
            git_root,
            String::new(),
            GitDatabaseProviders::new(core.provider.clone(), git_check_admission_provider),
            core.kms.clone(),
            core.handle.clone(),
            outbox.clone(),
            minter.clone(),
        )
        .map_err(|error| format!("PostgreSQL Git PR store refused: {error}"))?
        .with_repo_authorizer(repo_authz.clone())
        .with_repo_bootstrap(Arc::new(TupleRepoBootstrap::new(check.tuples().clone()))),
    );
    let recovery = recover_placed_git_at_boot(&backend, &core.provider, &core.cell_id)
        .await
        .map_err(|error| format!("durable Git boot recovery failed: {error}"))?;
    eprintln!(
        "myelin-mcp: Git recovery complete (tenants={}, repos={}, refs={}, merges={})",
        recovery.tenants_recovered,
        recovery.repos_reconciled,
        recovery.refs_reapplied,
        recovery.merges_recovered
    );
    let boundary = Arc::new(RunTokenAuthorizer::new(
        Arc::new(PasetoCapabilityVerifier::new(core.cell.trust_anchor())),
        check.revocations().clone(),
    ));
    let effect = GitEffectApi::new(
        backend,
        trigger_identity.scope.tenant().0.clone(),
        trigger_identity.scope.region().0.clone(),
        agent.clone(),
        boundary,
    );
    let approver_ids = configured_approver_ids(&agent_id)?;
    let approver_policy = Arc::new(GitMergeApproverPolicy {
        authorizer: repo_authz,
        principals: principal_store(&core),
        candidate_ids: approver_ids,
        scope: trigger_identity.scope.clone(),
    });
    let ttl = FailStaticBound {
        static_max_secs: thresholds.fail_static.agent_token_ttl_secs,
    };
    let trigger_jti = trigger_identity.capability().jti.clone();
    let trigger_expiry = trigger_identity.capability().expires_at_unix;
    let principal = RunPrincipal {
        scope: trigger_identity.scope.clone(),
        agent_id: agent_id.clone(),
        agent,
        trigger_actor: trigger_identity.principal,
        trigger_credential_jti: trigger_jti,
        trigger_expires_at_unix: trigger_expiry,
        run_id,
        resolved_policy: resolved,
        caveats,
        kind: myelin_identity_service::MachineKind::Agent,
        ttl,
    };
    let router = GovernedRouter::with_approver_policy(
        check.run_token_minter().clone(),
        principal,
        Box::new(effect),
        HitlVerdictStore::with_pg(core.provider.clone()),
        approver_policy,
        Arc::new(OutboxGovernanceAudit::new(outbox, minter)),
    );
    run_stdio_signal_aware(McpServer::with_router(ToolRegistry::with_git(), router)).await
}

fn configured_human_approvers(
    core: &Core,
    scope: &myelin_storage::TenantScope,
    requester: &PrincipalId,
) -> Result<Vec<Principal>, String> {
    let raw = required_env("MYELIN_MCP_APPROVERS")?;
    let store = principal_store(core);
    let mut result = Vec::new();
    for value in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let id = PrincipalId(value.to_string());
        if &id == requester {
            continue;
        }
        let row = store
            .get_principal(scope, &id)
            .ok_or_else(|| format!("configured approver `{value}` does not exist"))?;
        if row.kind != PrincipalKind::Human || row.status != PrincipalStatus::Active {
            return Err(format!(
                "configured approver `{value}` is not an active Human"
            ));
        }
        let principal = Principal::new(
            row.tenant,
            row.region,
            row.principal_id,
            row.kind,
            row.data_role,
            row.status,
        );
        if !result
            .iter()
            .any(|candidate: &Principal| candidate.principal_id == id)
        {
            result.push(principal);
        }
    }
    if result.is_empty() {
        return Err("MYELIN_MCP_APPROVERS must contain at least one distinct active Human".into());
    }
    Ok(result)
}

fn configured_approver_ids(requester: &PrincipalId) -> Result<Vec<PrincipalId>, String> {
    let raw = required_env("MYELIN_MCP_APPROVERS")?;
    let mut result = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| *value != requester.0)
        .map(|value| PrincipalId(value.to_string()))
        .collect::<Vec<_>>();
    result.sort_by(|left, right| left.0.cmp(&right.0));
    result.dedup();
    if result.is_empty()
        || result
            .iter()
            .any(|id| validate_bootstrap_principal(&id.0).is_err())
    {
        return Err(
            "MYELIN_MCP_APPROVERS must contain bounded distinct principal tokens other than the requester"
                .into(),
        );
    }
    Ok(result)
}

fn governance_audit(core: &Core) -> OutboxGovernanceAudit {
    OutboxGovernanceAudit::new(
        OutboxStore::durable(Arc::new(PgOutboxBacking::new(
            core.provider.db_pool().clone(),
            core.handle.clone(),
        ))),
        Arc::new(UlidMinter::new()),
    )
}

async fn decide(core: Core, gate_id: &str, approve: bool) -> Result<(), String> {
    validate_gate_id(gate_id)?;
    let identity = authenticate_from_secure_file(&core)?;
    if identity.principal.kind != PrincipalKind::Human
        || identity.principal.status != PrincipalStatus::Active
    {
        return Err("HITL decisions require an active Human MCP credential".into());
    }
    if !identity
        .capability()
        .effective_authority
        .holds(MCP_HITL_DECIDE_CAP)
    {
        return Err(format!(
            "HITL decision credential lacks `{MCP_HITL_DECIDE_CAP}`"
        ));
    }
    let now = timestamp_now();
    let now_unix = unix_now()?;
    let mut verdicts = HitlVerdictStore::with_pg(core.provider.clone());
    let audit = governance_audit(&core);
    let observed_gate = verdicts
        .fetch(&identity.scope, gate_id)
        .ok_or_else(|| "gate is absent from the verified tenant/region scope".to_string())?;
    git_merge_repo_from_effect_key(&observed_gate.effect_id)
        .ok_or_else(|| "gate is not a bounded canonical git.merge effect".to_string())?;
    if let Some(expired_gate) = verdicts.expire_if_due(&identity.scope, gate_id, now_unix) {
        let expiry_actor = Principal::new(
            identity.scope.tenant().clone(),
            identity.scope.region().clone(),
            PrincipalId("service:mcp-hitl-expiry".into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let expired_run_id = RunId(expired_gate.run_id);
        audit
            .record(GovernanceAuditRecord {
                scope: &identity.scope,
                actor: &expiry_actor,
                run_id: &expired_run_id,
                gate_id: Some(gate_id),
                tool: "git.merge",
                jti: "system:hitl-expiry",
                phase: AuditPhase::Expired,
                outcome: None,
                now: &now,
            })
            .map_err(|_| {
                "HITL expiry committed but its terminal audit did not; outcome is indeterminate"
                    .to_string()
            })?;
        return Err("gate decision refused because its approval window expired".into());
    }
    let gate = verdicts
        .fetch(&identity.scope, gate_id)
        .ok_or_else(|| "gate is absent from the verified tenant/region scope".to_string())?;
    let repo = git_merge_repo_from_effect_key(&gate.effect_id)
        .ok_or_else(|| "gate is not a bounded canonical git.merge effect".to_string())?;
    let configured = configured_human_approvers(
        &core,
        &identity.scope,
        &PrincipalId(gate.requested_by.clone()),
    )?;
    if !configured
        .iter()
        .any(|candidate| candidate.principal_id == identity.principal.principal_id)
    {
        return Err("authenticated Human is not a configured approver for this gate".into());
    }
    let check = StoreBackedCheck::with_pg(
        core.provider.clone(),
        core.kms.clone(),
        core.cell.clone(),
        core.handle.clone(),
    );
    for result in check.admit_git_fragment() {
        if let FragmentAdmit::Rejected { reason } = result {
            return Err(format!("Git ReBAC fragment refused to admit: {reason}"));
        }
    }
    let thresholds = Thresholds::load_canonical()
        .map_err(|error| format!("canonical thresholds refused: {error}"))?;
    let object_authz = CheckBackedRepoAuthorizer::try_new(
        check,
        thresholds.revocation.sla_mins * 60,
        &thresholds.fail_static,
    )
    .map_err(|error| format!("Git ReBAC authorizer refused: {error:?}"))?;
    if !object_authz.authorize_repo_permission(
        &identity.principal,
        &RepoLoc::new(
            identity.scope.tenant().0.as_str(),
            identity.scope.region().0.as_str(),
            repo,
        ),
        RepoPermission::ProtectedPush,
    ) {
        return Err(
            "authenticated Human lacks live protected-push authority on the gated repository"
                .into(),
        );
    }
    let run_id = RunId(gate.run_id.clone());
    // The durable intent precedes the decision, while the terminal audit follows it. These are not
    // one PostgreSQL transaction today: a missing pre-intent prevents the decision; a missing
    // terminal fact returns an explicit indeterminate error after the decision instead of claiming
    // success. The router still requires and one-shot consumes the exact durable approval.
    audit
        .record(GovernanceAuditRecord {
            scope: &identity.scope,
            actor: &identity.principal,
            run_id: &run_id,
            gate_id: Some(gate_id),
            tool: "git.merge",
            jti: &identity.capability().jti,
            phase: AuditPhase::Attempt,
            outcome: None,
            now: &now,
        })
        .map_err(|_| "durable pre-decision governance audit is unavailable".to_string())?;
    if approve {
        verdicts
            .approve_at(
                &identity.scope,
                gate_id,
                &identity.principal.principal_id.0,
                identity.principal.kind.clone(),
                now_unix,
            )
            .map_err(|error| format!("approval refused: {error:?}"))?;
    } else {
        verdicts
            .reject_at(
                &identity.scope,
                gate_id,
                &identity.principal.principal_id.0,
                identity.principal.kind.clone(),
                now_unix,
            )
            .map_err(|error| format!("rejection refused: {error:?}"))?;
    }
    audit
        .record(GovernanceAuditRecord {
            scope: &identity.scope,
            actor: &identity.principal,
            run_id: &run_id,
            gate_id: Some(gate_id),
            tool: "git.merge",
            jti: &identity.capability().jti,
            phase: if approve {
                AuditPhase::Approved
            } else {
                AuditPhase::Rejected
            },
            outcome: None,
            now: &now,
        })
        .map_err(|_| {
            "HITL decision committed but its governance audit did not; outcome is indeterminate"
                .to_string()
        })?;
    eprintln!("myelin-mcp: gate decision committed");
    Ok(())
}

async fn bootstrap(core: Core, parsed: BootstrapArgs) -> Result<(), String> {
    let principal_id = parsed.principal.as_str();
    validate_bootstrap_principal(principal_id)?;
    validate_bootstrap_semantics(&parsed)?;
    let ttl_secs = parsed.ttl_secs;
    let requested: std::collections::BTreeSet<&str> =
        parsed.capabilities.iter().map(String::as_str).collect();
    let tenant = required_env(MCP_TENANT_ENV)?;
    let region = required_env(MCP_REGION_ENV)?;
    if core.provider.config().region != region {
        return Err("configured MCP region does not match the runtime database region".into());
    }
    let scheme = required_env(MCP_SCHEME_ENV)?;
    validate_bootstrap_scheme(&scheme)?;
    let provisional = Principal::new(
        myelin_tenancy::TenantId(tenant.clone()),
        Region(region.clone()),
        PrincipalId("mcp-bootstrap-operator".into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let scope =
        myelin_storage::TenantScope::from_verified_token(&provisional, Region(region.clone()));
    let store = principal_store(&core);
    store
        .provision_principal_credential(
            &scope,
            myelin_identity_service::PrincipalCredentialProvision::new(
                PrincipalId(principal_id.to_string()),
                PrincipalKind::Human,
                DataRole::Controller,
                PrincipalStatus::Active,
                &scheme,
                principal_id,
            )
            .map_err(|error| {
                format!("invalid principal/credential provisioning request: {error}")
            })?,
        )
        .map_err(|error| format!("atomic principal/credential provisioning failed: {error}"))?;
    let now = unix_now()?;
    let token = core.cell.mint(&CapabilityMintSpec {
        tenant,
        region,
        subject_key: principal_id.to_string(),
        jti: format!("mcp-bootstrap-{principal_id}-{}", fresh_nonce()),
        exp_unix: now.saturating_add(ttl_secs),
        authority: requested.into_iter().map(str::to_string).collect(),
        dpop_jkt: None,
        purpose: CredentialPurpose::OperatorBootstrap,
        audience: CredentialAudience::Mcp,
    });
    println!("{token}");
    Ok(())
}

async fn run_stdio_signal_aware(server: McpServer) -> Result<(), String> {
    enum Input {
        Frame(String),
        Oversized,
        InvalidUtf8,
        Eof,
        Error(String),
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Input>(8);
    std::thread::Builder::new()
        .name("myelin-mcp-stdin".into())
        .spawn(move || {
            let stdin = io::stdin();
            let mut input = io::BufReader::new(stdin.lock());
            loop {
                let mut bytes = Vec::with_capacity(4096);
                match input
                    .by_ref()
                    .take((MAX_FRAME_BYTES + 1) as u64)
                    .read_until(b'\n', &mut bytes)
                {
                    Ok(0) => {
                        let _ = tx.blocking_send(Input::Eof);
                        break;
                    }
                    Ok(_) if bytes.len() > MAX_FRAME_BYTES => {
                        if bytes.last() != Some(&b'\n') {
                            if let Err(error) = drain_through_newline_bounded(&mut input) {
                                let _ = tx.blocking_send(Input::Error(error.to_string()));
                                break;
                            }
                        }
                        if tx.blocking_send(Input::Oversized).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {
                        while matches!(bytes.last(), Some(b'\n' | b'\r')) {
                            bytes.pop();
                        }
                        let item = match String::from_utf8(bytes) {
                            Ok(frame) => Input::Frame(frame),
                            Err(_) => Input::InvalidUtf8,
                        };
                        if tx.blocking_send(item).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = tx.blocking_send(Input::Error(error.to_string()));
                        break;
                    }
                }
            }
        })
        .map_err(|error| format!("cannot start bounded stdin pump: {error}"))?;
    let mut stdout = io::stdout().lock();
    let result = loop {
        tokio::select! {
            signal = shutdown_signal() => {
                break signal;
            }
            item = rx.recv() => match item {
                Some(Input::Frame(frame)) => {
                    if let Some(response) = server.handle_line(&frame) {
                        if let Err(error) = writeln!(stdout, "{response}") {
                            break Err(format!("stdout failed: {error}"));
                        }
                        if let Err(error) = stdout.flush() {
                            break Err(format!("stdout failed: {error}"));
                        }
                    }
                    if server.router().is_some_and(GovernedRouter::is_fatal) {
                        break Err("governed MCP session reached an indeterminate mutation outcome".into());
                    }
                }
                Some(Input::Oversized) => {
                    let response = serde_json::json!({
                        "jsonrpc":"2.0", "id": serde_json::Value::Null,
                        "error":{"code":-32600,"message":format!("JSON-RPC frame exceeds {MAX_FRAME_BYTES} bytes")}
                    });
                    if let Err(error) = writeln!(stdout, "{response}") {
                        break Err(format!("stdout failed: {error}"));
                    }
                    if let Err(error) = stdout.flush() {
                        break Err(format!("stdout failed: {error}"));
                    }
                }
                Some(Input::InvalidUtf8) => {
                    let response = serde_json::json!({
                        "jsonrpc":"2.0", "id": serde_json::Value::Null,
                        "error":{"code":-32600,"message":"JSON-RPC frame is not valid UTF-8"}
                    });
                    if let Err(error) = writeln!(stdout, "{response}") {
                        break Err(format!("stdout failed: {error}"));
                    }
                    if let Err(error) = stdout.flush() {
                        break Err(format!("stdout failed: {error}"));
                    }
                }
                Some(Input::Error(error)) => break Err(format!("stdin failed: {error}")),
                Some(Input::Eof) | None => break Ok(()),
            }
        }
    };
    server.teardown();
    result
}

/// Wait for terminal or process-manager shutdown. Either signal exits through `server.teardown()`
/// above so session-scoped run-token authority is revoked before the stdio process returns.
async fn shutdown_signal() -> Result<(), String> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|error| format!("failed to install SIGTERM handler: {error}"))?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.map_err(|error| format!("failed while waiting for SIGINT: {error}"))
            }
            signal = terminate.recv() => {
                signal
                    .map(|_| ())
                    .ok_or_else(|| "SIGTERM stream closed unexpectedly".to_string())
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .map_err(|error| format!("failed while waiting for shutdown signal: {error}"))
    }
}

fn drain_through_newline_bounded(reader: &mut impl BufRead) -> io::Result<()> {
    loop {
        let buffered = reader.fill_buf()?;
        if buffered.is_empty() {
            return Ok(());
        }
        let consumed = buffered
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffered.len(), |position| position + 1);
        let reached_newline = buffered.get(consumed.saturating_sub(1)) == Some(&b'\n');
        reader.consume(consumed);
        if reached_newline {
            return Ok(());
        }
    }
}

fn required_env(name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("required environment variable {name} is missing"))
}

fn parse_decision_args(args: &[String]) -> Result<&str, String> {
    match args {
        [flag, gate_id] if flag == "--gate-id" && !gate_id.starts_with("--") => {
            validate_gate_id(gate_id)?;
            Ok(gate_id)
        }
        _ => Err("decision accepts exactly `--gate-id <opaque-id>`".into()),
    }
}

struct BootstrapArgs {
    principal: String,
    ttl_secs: i64,
    capabilities: Vec<String>,
}

fn parse_bootstrap_args(args: &[String]) -> Result<BootstrapArgs, String> {
    let mut principal = None;
    let mut ttl_secs = None;
    let mut capabilities = Vec::new();
    let mut acknowledged = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--principal" => {
                if principal.is_some() {
                    return Err("duplicate --principal is refused".into());
                }
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.starts_with("--"))
                    .ok_or_else(|| "--principal requires one value".to_string())?;
                principal = Some(value.clone());
                index += 2;
            }
            "--ttl-secs" => {
                if ttl_secs.is_some() {
                    return Err("duplicate --ttl-secs is refused".into());
                }
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.starts_with("--"))
                    .ok_or_else(|| "--ttl-secs requires one value".to_string())?;
                ttl_secs = Some(
                    value
                        .parse::<i64>()
                        .map_err(|_| "--ttl-secs must be an integer".to_string())?,
                );
                index += 2;
            }
            "--cap" => {
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.starts_with("--"))
                    .ok_or_else(|| "--cap requires one value".to_string())?;
                capabilities.push(value.clone());
                index += 2;
            }
            "--ack-db-seal-operator-trust" => {
                if acknowledged {
                    return Err("duplicate --ack-db-seal-operator-trust is refused".into());
                }
                acknowledged = true;
                index += 1;
            }
            unknown => return Err(format!("unknown bootstrap argument `{unknown}`")),
        }
    }
    if !acknowledged {
        return Err(
            "bootstrap requires --ack-db-seal-operator-trust: possession of DB credentials plus the KMS seal key is the offline operator authority"
                .into(),
        );
    }
    let principal = principal.ok_or_else(|| "bootstrap requires --principal".to_string())?;
    validate_bootstrap_principal(&principal)?;
    let parsed = BootstrapArgs {
        principal,
        ttl_secs: ttl_secs.unwrap_or(3600),
        capabilities,
    };
    validate_bootstrap_semantics(&parsed)?;
    Ok(parsed)
}

fn validate_gate_id(gate_id: &str) -> Result<(), String> {
    if gate_id.len() > 256 || !gate_id.starts_with("gate:") {
        Err("gate id is malformed or exceeds 256 bytes".into())
    } else {
        Ok(())
    }
}

fn validate_bootstrap_scheme(scheme: &str) -> Result<(), String> {
    if scheme == "agent" {
        Ok(())
    } else {
        Err("MCP bootstrap currently requires the `agent` credential scheme".into())
    }
}

fn validate_bootstrap_semantics(parsed: &BootstrapArgs) -> Result<(), String> {
    if !(60..=86_400).contains(&parsed.ttl_secs) {
        return Err("--ttl-secs must be within 60..=86400".into());
    }
    let mut allowed: std::collections::BTreeSet<&str> = myelin_git::api::agent_tools()
        .into_iter()
        .flat_map(|tool| tool.required_caps.iter().copied())
        .collect();
    allowed.insert(MCP_HITL_DECIDE_CAP);
    if parsed.capabilities.is_empty()
        || parsed
            .capabilities
            .iter()
            .any(|cap| !allowed.contains(cap.as_str()))
    {
        return Err(format!(
            "bootstrap requires explicit catalogue-bounded --cap values; allowed: {}",
            allowed.iter().copied().collect::<Vec<_>>().join(",")
        ));
    }
    Ok(())
}

fn validate_bootstrap_principal(principal: &str) -> Result<(), String> {
    if principal.is_empty()
        || principal.len() > MAX_BOOTSTRAP_PRINCIPAL_BYTES
        || !principal.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(format!(
            "--principal must be a non-empty printable ASCII token of at most {MAX_BOOTSTRAP_PRINCIPAL_BYTES} bytes"
        ));
    }
    Ok(())
}

fn fresh_run_id() -> RunId {
    RunId(format!("mcp-run-{}", UlidMinter::new().mint().0))
}

fn fresh_nonce() -> String {
    UlidMinter::new().mint().0
}

fn unix_now() -> Result<i64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .map_err(|_| "system clock is before the Unix epoch".to_string())
}

fn timestamp_now() -> Timestamp {
    Timestamp(
        chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now())
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn operator_context_requires_exact_mcp_audience_and_bootstrap_purpose() {
        assert!(validate_mcp_operator_context(
            &CredentialPurpose::OperatorBootstrap,
            &CredentialAudience::Mcp,
        )
        .is_ok());
        assert!(validate_mcp_operator_context(
            &CredentialPurpose::AgentRun {
                run_id: "run:wrong-purpose".into(),
                delegation_snapshot: Some(1),
            },
            &CredentialAudience::Mcp,
        )
        .unwrap_err()
        .contains("purpose"));
        assert!(validate_mcp_operator_context(
            &CredentialPurpose::OperatorBootstrap,
            &CredentialAudience::Edge,
        )
        .unwrap_err()
        .contains("audience"));
    }

    #[test]
    fn operator_cli_rejects_unknown_duplicate_and_missing_arguments() {
        assert_eq!(
            parse_decision_args(&args(&["--gate-id", "gate:abc"])).unwrap(),
            "gate:abc"
        );
        for invalid in [
            args(&[]),
            args(&["--gate-id"]),
            args(&["--gate-id", "wrong-prefix"]),
            args(&["--gate-id", &format!("gate:{}", "x".repeat(257))]),
            args(&["--gate-id", "gate:a", "--gate-id", "gate:b"]),
            args(&["--unknown", "gate:a"]),
        ] {
            assert!(parse_decision_args(&invalid).is_err());
        }

        let valid = parse_bootstrap_args(&args(&[
            "--principal",
            "human:operator",
            "--cap",
            "repo.push",
            "--cap",
            MCP_HITL_DECIDE_CAP,
            "--ack-db-seal-operator-trust",
        ]))
        .unwrap();
        assert_eq!(valid.principal, "human:operator");
        assert_eq!(valid.capabilities.len(), 2);
        for invalid in [
            args(&["--principal", "p"]),
            args(&[
                "--principal",
                "p",
                "--principal",
                "q",
                "--ack-db-seal-operator-trust",
            ]),
            args(&[
                "--principal",
                "p",
                "--unknown",
                "x",
                "--ack-db-seal-operator-trust",
            ]),
            args(&["--principal", "p", "--cap", "--ack-db-seal-operator-trust"]),
            args(&[
                "--principal",
                "p",
                "--ttl-secs",
                "1",
                "--cap",
                "repo.push",
                "--ack-db-seal-operator-trust",
            ]),
            args(&[
                "--principal",
                "p",
                "--cap",
                "root.everything",
                "--ack-db-seal-operator-trust",
            ]),
        ] {
            assert!(parse_bootstrap_args(&invalid).is_err());
        }
        assert!(matches!(
            parse_command(&args(&["approve", "--gate-id", "gate:abc"])),
            Ok(Command::Approve(gate_id)) if gate_id == "gate:abc"
        ));
        assert!(parse_command(&args(&["approve", "--gate-id", "bad"])).is_err());
        assert!(validate_bootstrap_scheme("bearer").is_err());
        assert!(validate_partition_token(MCP_TENANT_ENV, "tenant_01:cell-a").is_ok());
        for invalid_scope in ["", "tenant name", "tenant/escape", &"x".repeat(256)] {
            assert!(validate_partition_token(MCP_TENANT_ENV, invalid_scope).is_err());
        }

        for invalid_principal in [
            "",
            "human operator",
            "human:\noperator",
            &"p".repeat(MAX_BOOTSTRAP_PRINCIPAL_BYTES + 1),
        ] {
            let invalid = args(&[
                "--principal",
                invalid_principal,
                "--ack-db-seal-operator-trust",
            ]);
            assert!(parse_bootstrap_args(&invalid).is_err());
        }
    }

    #[test]
    fn secure_credential_file_requires_single_owned_0600_inode_and_utf8() {
        let base = std::env::temp_dir().join(format!("myelin-mcp-secret-test-{}", fresh_nonce()));
        let link = base.with_extension("hardlink");
        let symlink = base.with_extension("symlink");
        let oversized = base.with_extension("oversized");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&base)
            .unwrap();
        file.write_all(b"secret-token\n").unwrap();
        file.flush().unwrap();
        drop(file);
        assert_eq!(read_secret_file(&base).unwrap(), "secret-token");

        std::os::unix::fs::symlink(&base, &symlink).unwrap();
        assert!(read_secret_file(&symlink)
            .unwrap_err()
            .contains("non-symlink"));
        std::fs::remove_file(&symlink).unwrap();

        std::fs::hard_link(&base, &link).unwrap();
        assert!(read_secret_file(&base).unwrap_err().contains("hard link"));
        std::fs::remove_file(&link).unwrap();

        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_secret_file(&base).unwrap_err().contains("0600"));
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&base)
            .unwrap();
        file.write_all(&[0xff]).unwrap();
        file.flush().unwrap();
        drop(file);
        assert!(read_secret_file(&base).unwrap_err().contains("UTF-8"));
        let mut oversized_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&oversized)
            .unwrap();
        oversized_file
            .write_all(&vec![b'x'; MAX_CREDENTIAL_BYTES as usize + 1])
            .unwrap();
        oversized_file.flush().unwrap();
        drop(oversized_file);
        assert!(read_secret_file(&oversized).unwrap_err().contains("1..="));
        std::fs::remove_file(&oversized).unwrap();
        std::fs::remove_file(&base).unwrap();
    }

    #[test]
    fn production_run_ids_use_distinct_canonical_ulids() {
        let first = fresh_run_id();
        let second = fresh_run_id();
        assert_ne!(first, second);
        for run in [first, second] {
            let ulid = run.0.strip_prefix("mcp-run-").unwrap();
            assert_eq!(ulid.len(), 26);
            assert!(ulid.bytes().all(|byte| byte.is_ascii_alphanumeric()));
        }
    }

    #[test]
    fn gate_open_refreshes_candidate_status_instead_of_using_startup_snapshots() {
        let kms = Arc::new(KmsEngine::new());
        let principals = PrincipalStore::new(kms);
        let tenant = myelin_tenancy::TenantId("acme".into());
        let region = Region("eu-west".into());
        let scope_principal = Principal::new(
            tenant,
            region.clone(),
            PrincipalId("human:trigger".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let scope = myelin_storage::TenantScope::from_verified_token(&scope_principal, region);
        let candidate = PrincipalId("human:lead".into());
        principals
            .put_principal(
                &scope,
                candidate.clone(),
                PrincipalKind::Human,
                DataRole::Controller,
                PrincipalStatus::Active,
                None,
            )
            .unwrap();
        let policy = GitMergeApproverPolicy {
            authorizer: Arc::new(myelin_edge::AllowAllRepos),
            principals: principals.clone(),
            candidate_ids: vec![candidate.clone()],
            scope: scope.clone(),
        };
        assert_eq!(
            policy
                .eligible_approvers("git.merge", &serde_json::json!({"repo":"alpha"}))
                .unwrap(),
            vec![candidate.clone()]
        );

        principals
            .put_principal(
                &scope,
                candidate,
                PrincipalKind::Human,
                DataRole::Controller,
                PrincipalStatus::Suspended,
                None,
            )
            .unwrap();
        assert!(policy
            .eligible_approvers("git.merge", &serde_json::json!({"repo":"alpha"}))
            .unwrap_err()
            .contains("no configured active Human"));
    }
}
