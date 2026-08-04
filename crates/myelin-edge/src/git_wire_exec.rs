use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    GitWireSpec, IdemToken, MeterTarget, ResourceLimits, RunTokenCredential, RunnerHooks,
    SandboxBackend, SandboxLaunch,
};
use myelin_git::core::{GitCoreError, RoutedGitCore, WireExecutor, WireInvocation, WireOutput};
use myelin_git::gix_backend::{GixCore, RootedResolver};
use myelin_git::live_check::{perm, strong_at};
use myelin_identity::{
    DataRole, Decision, DelegationCaveats, FailStaticBound, IdentityService, Permission, Principal,
    PrincipalId, PrincipalKind, PrincipalStatus, RunId, RunToken, Zookie,
};
use myelin_identity_service::delegation::DelegationInput;
use myelin_identity_service::machine_auth::{
    scheme, Authority, CredentialAudience, CredentialPurpose, MachineKind,
};
use myelin_identity_service::StoreBackedCheck;
use myelin_storage::TenantScope;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

pub struct GitWireCredentialRequest<'a> {
    pub tenant: &'a str,
    pub region: &'a str,
    pub repo: &'a str,
    pub operation: &'a str,
    pub invocation_id: &'a str,
    pub ttl_secs: u64,
}

pub trait GitWireCredentialIssuer: Send + Sync {
    fn mint(&self, request: &GitWireCredentialRequest<'_>) -> Result<RunTokenCredential, String>;

    fn verify(&self, credential: &RunTokenCredential) -> Result<(), String>;
}

pub trait GitWireCredentialIssuerFactory: Send + Sync {
    fn bind(&self, principal: &Principal) -> Arc<dyn GitWireCredentialIssuer>;
}

#[derive(Debug)]
struct UnavailableGitWireCredentialIssuer;

impl GitWireCredentialIssuer for UnavailableGitWireCredentialIssuer {
    fn mint(&self, _request: &GitWireCredentialRequest<'_>) -> Result<RunTokenCredential, String> {
        Err("no live Identity Git-wire credential issuer is configured".into())
    }

    fn verify(&self, _credential: &RunTokenCredential) -> Result<(), String> {
        Err("no live Identity Git-wire credential verifier is configured".into())
    }
}

fn unavailable_git_wire_credential_issuer() -> Arc<dyn GitWireCredentialIssuer> {
    Arc::new(UnavailableGitWireCredentialIssuer)
}

#[derive(Debug)]
struct UnavailableGitWireCredentialIssuerFactory;

impl GitWireCredentialIssuerFactory for UnavailableGitWireCredentialIssuerFactory {
    fn bind(&self, _principal: &Principal) -> Arc<dyn GitWireCredentialIssuer> {
        Arc::new(UnavailableGitWireCredentialIssuer)
    }
}

pub(crate) fn unavailable_git_wire_credential_issuer_factory(
) -> Arc<dyn GitWireCredentialIssuerFactory> {
    Arc::new(UnavailableGitWireCredentialIssuerFactory)
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct TestGitWireCredentialIssuer;

#[cfg(any(test, feature = "test-support"))]
impl GitWireCredentialIssuer for TestGitWireCredentialIssuer {
    fn mint(&self, request: &GitWireCredentialRequest<'_>) -> Result<RunTokenCredential, String> {
        RunTokenCredential::new(
            format!("test-git-wire-bearer:{}", request.invocation_id),
            format!("test-git-wire-jti:{}", request.invocation_id),
            request.ttl_secs,
        )
        .map_err(|error| error.to_string())
    }

    fn verify(&self, credential: &RunTokenCredential) -> Result<(), String> {
        if credential
            .expose_bearer()
            .starts_with("test-git-wire-bearer:")
            && credential.jti.starts_with("test-git-wire-jti:")
        {
            Ok(())
        } else {
            Err("test Git-wire credential verification refused".into())
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn test_git_wire_credential_issuer() -> Arc<dyn GitWireCredentialIssuer> {
    Arc::new(TestGitWireCredentialIssuer)
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct TestGitWireCredentialIssuerFactory;

#[cfg(any(test, feature = "test-support"))]
impl GitWireCredentialIssuerFactory for TestGitWireCredentialIssuerFactory {
    fn bind(&self, _principal: &Principal) -> Arc<dyn GitWireCredentialIssuer> {
        test_git_wire_credential_issuer()
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn test_git_wire_credential_issuer_factory() -> Arc<dyn GitWireCredentialIssuerFactory> {
    Arc::new(TestGitWireCredentialIssuerFactory)
}

#[derive(Clone)]
pub struct IdentityGitWireCredentialIssuerFactory {
    identity: StoreBackedCheck,
}

impl IdentityGitWireCredentialIssuerFactory {
    pub fn new(identity: StoreBackedCheck) -> Self {
        Self { identity }
    }
}

impl GitWireCredentialIssuerFactory for IdentityGitWireCredentialIssuerFactory {
    fn bind(&self, principal: &Principal) -> Arc<dyn GitWireCredentialIssuer> {
        Arc::new(IdentityGitWireCredentialIssuer {
            identity: self.identity.clone(),
            principal: principal.clone(),
        })
    }
}

#[derive(Clone)]
struct IdentityGitWireCredentialIssuer {
    identity: StoreBackedCheck,
    principal: Principal,
}

impl GitWireCredentialIssuer for IdentityGitWireCredentialIssuer {
    fn mint(&self, request: &GitWireCredentialRequest<'_>) -> Result<RunTokenCredential, String> {
        if self.principal.status != PrincipalStatus::Active
            || self.principal.tenant.0 != request.tenant
            || self.principal.region.0 != request.region
        {
            return Err("verified principal does not match the Git-wire scope".into());
        }
        let required = match request.operation {
            "upload-pack" => perm::PULL,
            "receive-pack-ingest" => perm::PUSH,
            _ => return Err("unsupported Git-wire credential operation".into()),
        };
        if !matches!(
            self.identity.check(
                &self.principal,
                &Permission(required.into()),
                &crate::repo_authz_live::repo_object_ref(request.repo),
                &strong_at(Zookie(String::new())),
                None,
            ),
            Ok(Decision::Allow)
        ) {
            return Err("Identity refused the Git-wire repo permission".into());
        }

        let scope =
            TenantScope::from_verified_token(&self.principal, self.principal.region.clone());
        let service_id = PrincipalId(format!("git-wire:{}", request.invocation_id));
        let service = Principal::new(
            self.principal.tenant.clone(),
            self.principal.region.clone(),
            service_id.clone(),
            PrincipalKind::Service,
            DataRole::Processor,
            PrincipalStatus::Active,
        );
        let empty = Authority::default();
        let input = DelegationInput {
            agent_policy: empty.clone(),
            delegation: empty.clone(),
            tenant_policy: empty.clone(),
            trigger_actor_held: empty,
        };
        let minted = self
            .identity
            .mint_run_token_in(
                &scope,
                &service_id,
                &RunId(format!("git-wire:{}", request.invocation_id)),
                &service,
                &self.principal,
                &input,
                &DelegationCaveats(Vec::new()),
                MachineKind::PerJob,
                &FailStaticBound {
                    static_max_secs: request.ttl_secs,
                },
                &system_now_timestamp(),
            )
            .map_err(|_| "Identity refused the Git-wire run-token mint".to_string())?;
        let (bearer, jti) = minted.into_parts();
        RunTokenCredential::new(bearer, jti, request.ttl_secs)
            .map_err(|_| "Identity returned an invalid Git-wire credential".to_string())
    }

    fn verify(&self, credential: &RunTokenCredential) -> Result<(), String> {
        let token = RunToken {
            token: credential.expose_bearer().to_owned(),
            jti: credential.jti.clone(),
        };
        let verified = self
            .identity
            .introspect_run_token(scheme::PER_JOB, &token)
            .map_err(|_| "Identity rejected the Git-wire credential signature".to_string())?;
        let expected_prefix = "git-wire:";
        let purpose_matches_subject = matches!(
            &verified.purpose,
            CredentialPurpose::PerJob { run_id } if run_id == &verified.subject_key
        );
        if verified.jti != credential.jti
            || verified.tenant != self.principal.tenant
            || verified.region != self.principal.region
            || !verified.subject_key.starts_with(expected_prefix)
            || !purpose_matches_subject
            || verified.audience != CredentialAudience::Edge
            || !verified.authority.is_empty()
        {
            return Err("Identity rejected the Git-wire credential binding".into());
        }
        let verified_principal = Principal::new(
            verified.tenant,
            verified.region,
            PrincipalId(verified.subject_key),
            PrincipalKind::Service,
            DataRole::Processor,
            PrincipalStatus::Active,
        );
        let scope = TenantScope::from_verified_token(
            &verified_principal,
            verified_principal.region.clone(),
        );
        if !self
            .identity
            .run_token_minter()
            .is_live(&scope, &token, &system_now_timestamp())
        {
            return Err("Identity rejected the Git-wire credential lifecycle".into());
        }
        Ok(())
    }
}

fn system_now_timestamp() -> myelin_events::Timestamp {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let instant = chrono::DateTime::from_timestamp(seconds, 0).unwrap_or_default();
    myelin_events::Timestamp(instant.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

pub struct GitWireExecutor {
    backend: GvisorBackend,
    root: PathBuf,
    limits: ResourceLimits,
    hooks: RunnerHooks,
    credential_issuer: Arc<dyn GitWireCredentialIssuer>,
    shutdown: Arc<AtomicBool>,
    seq: AtomicU64,
}

impl GitWireExecutor {
    pub fn new(
        root: impl Into<PathBuf>,
        limits: ResourceLimits,
        hooks: RunnerHooks,
        credential_issuer: Arc<dyn GitWireCredentialIssuer>,
    ) -> Self {
        let verifier = credential_issuer.clone();
        let hooks = hooks.with_attribute_guard(move |spec| {
            verifier
                .verify(&spec.run_token)
                .map_err(myelin_ci_sandbox::HookError)
        });
        Self {
            backend: GvisorBackend::git_wire_only(),
            root: root.into(),
            limits,
            hooks,
            credential_issuer,
            shutdown: Arc::new(AtomicBool::new(false)),
            seq: AtomicU64::new(0),
        }
    }

    pub fn with_shutdown_signal(mut self, shutdown: Arc<AtomicBool>) -> Self {
        self.shutdown = shutdown;
        self
    }

    pub fn serving_default(root: impl Into<PathBuf>) -> Self {
        Self::new(
            root,
            Self::default_limits(),
            Self::serving_hooks(),
            unavailable_git_wire_credential_issuer(),
        )
    }

    pub fn default_limits() -> ResourceLimits {
        ResourceLimits {
            cpu_millis: 2000,
            mem_bytes: 512 * 1024 * 1024,
            disk_bytes: 512 * 1024 * 1024,
            tmpfs_bytes: 512 * 1024 * 1024,
            pids_max: 256,
            timeout_secs: 120,
        }
    }

    pub fn serving_hooks() -> RunnerHooks {
        RunnerHooks::new(
            myelin_ci_sandbox::CompletionSettlementOwner::Hook,
            Box::new(|spec| {
                Ok(myelin_ci_sandbox::ReserveHandle(
                    spec.meter_to.reserve_id.clone(),
                ))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        )
    }

    fn wire_env() -> Vec<String> {
        vec![
            "HOME=/tmp".to_string(),
            "GIT_EXEC_PATH=/usr/lib/git-core".to_string(),
            "GIT_CONFIG_NOSYSTEM=1".to_string(),
            "GIT_CONFIG_COUNT=1".to_string(),
            "GIT_CONFIG_KEY_0=safe.directory".to_string(),
            "GIT_CONFIG_VALUE_0=*".to_string(),
        ]
    }

    fn next_tokens(
        &self,
        repo: &myelin_git::core::RepoLoc,
        operation: &str,
    ) -> Result<(RunTokenCredential, MeterTarget, IdemToken), GitCoreError> {
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let tag = format!("git-wire-{}-{n}", std::process::id());
        let credential = self
            .credential_issuer
            .mint(&GitWireCredentialRequest {
                tenant: &repo.tenant,
                region: &repo.region,
                repo: &repo.repo,
                operation,
                invocation_id: &tag,
                ttl_secs: 120,
            })
            .map_err(|error| {
                GitCoreError::Wire(format!("Git-wire credential mint refused: {error}"))
            })?;
        Ok((
            credential,
            MeterTarget {
                reserve_id: format!("{tag}-reserve"),
            },
            IdemToken(tag),
        ))
    }
}

impl GitWireExecutor {
    pub fn ingest_pack(
        &self,
        repo: &myelin_git::core::RepoLoc,
        pack: Vec<u8>,
    ) -> Result<Vec<u8>, GitCoreError> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(GitCoreError::Wire(
                "sandboxed receive-pack ingest cancelled by process shutdown".into(),
            ));
        }
        let (rt, mt, it) = self.next_tokens(repo, "receive-pack-ingest")?;
        let spec = GitWireSpec::for_repo(
            &self.root,
            &repo.tenant,
            &repo.region,
            &repo.repo,
            Vec::new(),
            pack,
            Self::wire_env(),
            None,
            self.limits,
            rt,
            mt,
            it,
        )
        .map_err(|e| GitCoreError::Wire(e.to_string()))?;

        let SandboxLaunch {
            handle,
            result,
            output_complete,
        } = self
            .backend
            .launch_git_receive_pack_until_cancelled(&spec, &self.hooks, &self.shutdown)
            .map_err(|e| GitCoreError::Wire(e.to_string()))?;
        let _ = self.backend.kill(&handle);

        if !output_complete {
            return Err(GitCoreError::Wire(
                "sandboxed receive-pack ingest returned incomplete output".into(),
            ));
        }
        if self.shutdown.load(Ordering::Acquire) {
            return Err(GitCoreError::Wire(
                "sandboxed receive-pack ingest cancelled by process shutdown".into(),
            ));
        }
        if result.timed_out {
            return Err(GitCoreError::Wire(format!(
                "sandboxed receive-pack ingest timed out ({}s ceiling)",
                self.limits.timeout_secs
            )));
        }
        match result.exit_code {
            Some(0) => Ok(result.stdout),
            other => Err(GitCoreError::Wire(format!(
                "sandboxed receive-pack ingest (git index-pack) exited {other:?}: {}",
                String::from_utf8_lossy(&result.stderr)
            ))),
        }
    }
}

impl WireExecutor for GitWireExecutor {
    fn run(&self, inv: &WireInvocation) -> Result<WireOutput, GitCoreError> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(GitCoreError::Wire(format!(
                "sandboxed `git {}` cancelled by process shutdown",
                inv.argv.join(" ")
            )));
        }
        let operation = inv.argv.first().map(String::as_str).unwrap_or("git-wire");
        let (rt, mt, it) = self.next_tokens(&inv.repo, operation)?;
        let spec = GitWireSpec::for_repo(
            &self.root,
            &inv.repo.tenant,
            &inv.repo.region,
            &inv.repo.repo,
            inv.argv.clone(),
            inv.stdin.clone(),
            Self::wire_env(),
            None,
            self.limits,
            rt,
            mt,
            it,
        )
        .map_err(|e| GitCoreError::Wire(e.to_string()))?;

        let SandboxLaunch {
            handle,
            result,
            output_complete,
        } = self
            .backend
            .launch_git_wire_until_cancelled(&spec, &self.hooks, &self.shutdown)
            .map_err(|e| GitCoreError::Wire(e.to_string()))?;

        let _ = self.backend.kill(&handle);

        if !output_complete {
            return Err(GitCoreError::Wire(format!(
                "sandboxed `git {}` returned incomplete output",
                inv.argv.join(" ")
            )));
        }
        if self.shutdown.load(Ordering::Acquire) {
            return Err(GitCoreError::Wire(format!(
                "sandboxed `git {}` cancelled by process shutdown",
                inv.argv.join(" ")
            )));
        }
        if result.timed_out {
            return Err(GitCoreError::Wire(format!(
                "sandboxed `git {}` timed out (wall-clock {}s ceiling) - refused",
                inv.argv.join(" "),
                self.limits.timeout_secs
            )));
        }
        match result.exit_code {
            Some(0) => Ok(WireOutput {
                stdout: result.stdout,
                status: 0,
            }),
            other => Err(GitCoreError::Wire(format!(
                "sandboxed `git {}` exited {:?}: {}",
                inv.argv.join(" "),
                other,
                String::from_utf8_lossy(&result.stderr)
            ))),
        }
    }
}

pub fn production_git_core(
    root: impl Into<PathBuf>,
    limits: ResourceLimits,
    hooks: RunnerHooks,
) -> RoutedGitCore<GitWireExecutor, GixCore<RootedResolver>> {
    let root = root.into();
    let exec = GitWireExecutor::new(
        root.clone(),
        limits,
        hooks,
        unavailable_git_wire_credential_issuer(),
    );
    let read = GixCore::new(RootedResolver::new(root));
    RoutedGitCore::new(exec, read)
}

pub fn production_git_core_with_issuer(
    root: impl Into<PathBuf>,
    limits: ResourceLimits,
    hooks: RunnerHooks,
    credential_issuer: Arc<dyn GitWireCredentialIssuer>,
) -> RoutedGitCore<GitWireExecutor, GixCore<RootedResolver>> {
    let root = root.into();
    let exec = GitWireExecutor::new(root.clone(), limits, hooks, credential_issuer);
    let read = GixCore::new(RootedResolver::new(root));
    RoutedGitCore::new(exec, read)
}

pub fn production_git_core_with_shutdown(
    root: impl Into<PathBuf>,
    limits: ResourceLimits,
    hooks: RunnerHooks,
    shutdown: Arc<AtomicBool>,
) -> RoutedGitCore<GitWireExecutor, GixCore<RootedResolver>> {
    let root = root.into();
    let exec = GitWireExecutor::new(
        root.clone(),
        limits,
        hooks,
        unavailable_git_wire_credential_issuer(),
    )
    .with_shutdown_signal(shutdown);
    let read = GixCore::new(RootedResolver::new(root));
    RoutedGitCore::new(exec, read)
}

pub fn production_git_core_with_shutdown_and_issuer(
    root: impl Into<PathBuf>,
    limits: ResourceLimits,
    hooks: RunnerHooks,
    shutdown: Arc<AtomicBool>,
    credential_issuer: Arc<dyn GitWireCredentialIssuer>,
) -> RoutedGitCore<GitWireExecutor, GixCore<RootedResolver>> {
    let root = root.into();
    let exec = GitWireExecutor::new(root.clone(), limits, hooks, credential_issuer)
        .with_shutdown_signal(shutdown);
    let read = GixCore::new(RootedResolver::new(root));
    RoutedGitCore::new(exec, read)
}

pub fn production_git_core_default(
    root: impl Into<PathBuf>,
) -> RoutedGitCore<GitWireExecutor, GixCore<RootedResolver>> {
    production_git_core(
        root,
        GitWireExecutor::default_limits(),
        GitWireExecutor::serving_hooks(),
    )
}

pub fn production_git_core_default_with_shutdown(
    root: impl Into<PathBuf>,
    shutdown: Arc<AtomicBool>,
) -> RoutedGitCore<GitWireExecutor, GixCore<RootedResolver>> {
    production_git_core_with_shutdown(
        root,
        GitWireExecutor::default_limits(),
        GitWireExecutor::serving_hooks(),
        shutdown,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{OutboxStore, Timestamp};
    use myelin_git::core::RepoLoc;
    use myelin_identity::{ObjectId, RelName, RelationTuple, TupleDelta};
    use myelin_identity_service::TupleStore;
    use myelin_tenancy::{Region, TenantId};

    fn wire_principal() -> Principal {
        Principal::new(
            TenantId("acme".into()),
            Region("eu-west".into()),
            PrincipalId("p:wire-user".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn identity_wire_factory() -> IdentityGitWireCredentialIssuerFactory {
        let tuples = TupleStore::new(OutboxStore::new());
        let identity = StoreBackedCheck::new(tuples.clone());
        for result in identity.admit_git_fragment() {
            assert!(matches!(
                result,
                myelin_identity::FragmentAdmit::Admitted { .. }
            ));
        }
        let principal = wire_principal();
        let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
        tuples
            .write_tuples(
                &scope,
                &principal,
                &[TupleDelta::Add(RelationTuple {
                    object: ObjectId("repo:widgets".into()),
                    relation: RelName("reader".into()),
                    subject: principal.principal_id.clone(),
                    caveat: None,
                })],
                None,
                None,
                Timestamp("2026-07-22T00:00:00Z".into()),
            )
            .unwrap();
        IdentityGitWireCredentialIssuerFactory::new(identity)
    }

    #[test]
    fn pre_cancelled_executor_never_reaches_runsc() {
        let shutdown = Arc::new(AtomicBool::new(true));
        let executor =
            GitWireExecutor::serving_default("/absent/git-root").with_shutdown_signal(shutdown);
        let error = executor
            .run(&WireInvocation {
                repo: RepoLoc::new("acme", "eu-west", "widgets"),
                argv: vec!["upload-pack".into(), "--stateless-rpc".into()],
                stdin: Vec::new(),
            })
            .expect_err("shutdown must refuse before filesystem or runsc access");
        assert!(error.to_string().contains("cancelled by process shutdown"));
    }

    #[test]
    fn production_default_refuses_without_a_live_credential_issuer() {
        let executor = GitWireExecutor::serving_default("/absent/git-root");
        let error = executor
            .run(&WireInvocation {
                repo: RepoLoc::new("acme", "eu-west", "widgets"),
                argv: vec!["upload-pack".into(), "--stateless-rpc".into()],
                stdin: Vec::new(),
            })
            .expect_err(
                "an unbound production executor must fail before filesystem or runsc access",
            );
        assert!(error
            .to_string()
            .contains("no live Identity Git-wire credential issuer is configured"));
    }

    #[test]
    fn identity_issuer_mints_and_final_boundary_verifies_a_zero_authority_token() {
        let factory = identity_wire_factory();
        let principal = wire_principal();
        let issuer = factory.bind(&principal);
        let credential = issuer
            .mint(&GitWireCredentialRequest {
                tenant: "acme",
                region: "eu-west",
                repo: "widgets",
                operation: "upload-pack",
                invocation_id: "invocation-1",
                ttl_secs: 120,
            })
            .expect("the strongly authorized repo read mints");
        issuer
            .verify(&credential)
            .expect("the signed token is live at the final boundary");

        let forged = RunTokenCredential::new(
            "forged-bearer",
            credential.jti.clone(),
            credential.ttl_secs(),
        )
        .unwrap();
        assert!(issuer.verify(&forged).is_err());
    }

    #[test]
    fn identity_issuer_refuses_scope_permission_and_operation_drift() {
        let factory = identity_wire_factory();
        let principal = wire_principal();
        let issuer = factory.bind(&principal);
        for (tenant, repo, operation) in [
            ("other", "widgets", "upload-pack"),
            ("acme", "ungranted", "upload-pack"),
            ("acme", "widgets", "maintenance"),
        ] {
            assert!(issuer
                .mint(&GitWireCredentialRequest {
                    tenant,
                    region: "eu-west",
                    repo,
                    operation,
                    invocation_id: "denied",
                    ttl_secs: 120,
                })
                .is_err());
        }

        let ungranted_principal = Principal::new(
            TenantId("acme".into()),
            Region("eu-west".into()),
            PrincipalId("p:ungranted".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        assert!(factory
            .bind(&ungranted_principal)
            .mint(&GitWireCredentialRequest {
                tenant: "acme",
                region: "eu-west",
                repo: "widgets",
                operation: "upload-pack",
                invocation_id: "denied-principal",
                ttl_secs: 120,
            })
            .is_err());
    }
}
