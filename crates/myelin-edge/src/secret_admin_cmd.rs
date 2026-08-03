//! Testable body for the authenticated `edge secret ...` operator command family.
//!
//! Secret material crosses exactly one CLI boundary: UTF-8 bytes are read from STDIN into a
//! [`Zeroizing<String>`], moved into [`SecretMaterial`], and consumed by [`SecretAdmin`]. It is never
//! accepted in argv/environment, formatted, logged, returned, or retained by an error. Authentication
//! uses the same capability authenticator as the serving edge; only a signed `OperatorBootstrap`
//! credential carrying `edge.operator` reaches the independent `ci_project.administer` ReBAC gate.

use std::io::Read;
use std::sync::Arc;

use myelin_ci_controlplane::{
    DurableCiSecretStore, SecretAdmin, SecretAdminError, SecretBindingScope, SecretMaterial,
    SecretMetadata,
};
use myelin_identity::{Consistency, ConsistencyMode, Credential, IdentityService, Zookie};
use myelin_identity_service::{CapabilityAuthenticator, CredentialAudience, CredentialPurpose};
use myelin_tenancy::TenantId;
use zeroize::Zeroizing;

/// One secret target. Required fields are validated before authentication or STDIN is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecretTarget<'a> {
    pub tenant: &'a str,
    pub project: &'a str,
    pub name: &'a str,
}

/// The typed operation behind the `edge secret` subcommand family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretCommand<'a> {
    Create(SecretTarget<'a>),
    Update(SecretTarget<'a>),
    Rotate(SecretTarget<'a>),
    Delete(SecretTarget<'a>),
    List {
        tenant: &'a str,
        project: Option<&'a str>,
    },
    GrantBinding {
        target: SecretTarget<'a>,
        scope: &'a str,
    },
    RevokeBinding {
        target: SecretTarget<'a>,
        scope: &'a str,
    },
}

/// Material-free success output. No variant has a field capable of carrying secret material.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretCommandOutput {
    Changed {
        operation: &'static str,
        metadata: SecretMetadata,
    },
    Acknowledged {
        operation: &'static str,
        project_id: String,
        name: String,
    },
    Listed(Vec<SecretMetadata>),
}

impl SecretCommandOutput {
    /// Render operator-safe metadata. Secret material is structurally absent from the output type.
    pub fn render(&self) -> String {
        match self {
            Self::Changed {
                operation,
                metadata,
            } => format!(
                "ok {operation} project={} name={} version={} created_at={}",
                metadata.project_id,
                metadata.name,
                metadata.version,
                metadata.created_at.to_rfc3339()
            ),
            Self::Acknowledged {
                operation,
                project_id,
                name,
            } => format!("ok {operation} project={project_id} name={name}"),
            Self::Listed(rows) => {
                let mut rendered = format!("ok list count={}", rows.len());
                for row in rows {
                    rendered.push_str(&format!(
                        "\nproject={} name={} version={} created_at={}",
                        row.project_id,
                        row.name,
                        row.version,
                        row.created_at.to_rfc3339()
                    ));
                }
                rendered
            }
        }
    }
}

/// A PII- and secret-safe operator failure. Authentication and authorization are intentionally
/// uniform; no verifier, ReBAC, store, IO, token, or material detail crosses this boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretCommandError {
    BadParam(&'static str),
    Unauthorized,
    Forbidden,
    Conflict,
    NotFound,
    Unavailable,
    Input,
}

impl SecretCommandError {
    pub fn is_usage(&self) -> bool {
        matches!(self, Self::BadParam(_))
    }

    pub fn exit_code(&self) -> i32 {
        if self.is_usage() {
            2
        } else {
            1
        }
    }
}

impl core::fmt::Display for SecretCommandError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadParam(message) => write!(formatter, "secret parameter error: {message}"),
            Self::Unauthorized => formatter.write_str("authentication required"),
            Self::Forbidden => formatter.write_str("forbidden"),
            Self::Conflict => formatter.write_str("secret already exists"),
            Self::NotFound => formatter.write_str("secret does not exist"),
            Self::Unavailable => formatter.write_str("secret administration is unavailable"),
            Self::Input => formatter.write_str("secret material could not be read from stdin"),
        }
    }
}

impl std::error::Error for SecretCommandError {}

/// Authenticate an operator token and execute one secret administration operation.
///
/// Material-bearing operations read exactly one bounded UTF-8 value from `input`. The owned input
/// allocation is moved into `SecretMaterial`, whose redacted/zeroizing contract covers every
/// success and error path inside `SecretAdmin`.
pub fn execute_secret_command<I: IdentityService>(
    authenticator: &CapabilityAuthenticator,
    identity: Arc<I>,
    store: Arc<DurableCiSecretStore>,
    credential: Option<Credential>,
    command: SecretCommand<'_>,
    input: &mut impl Read,
) -> Result<SecretCommandOutput, SecretCommandError> {
    validate_command(&command)?;

    let credential = credential.ok_or(SecretCommandError::Unauthorized)?;
    let request_identity = authenticator
        .authenticate_identity(&credential, None)
        .map_err(|_| SecretCommandError::Unauthorized)?;
    let capability = request_identity.capability();
    if capability.audience != CredentialAudience::Edge
        || !matches!(&capability.purpose, CredentialPurpose::OperatorBootstrap)
        || !capability.effective_authority.holds("edge.operator")
    {
        return Err(SecretCommandError::Forbidden);
    }

    let admin = SecretAdmin::new(
        store,
        identity,
        request_identity.principal,
        strong_consistency(),
    );

    match command {
        SecretCommand::Create(target) => {
            let material = secret_material(input)?;
            admin
                .create_secret(
                    &TenantId::from_token(target.tenant),
                    target.project,
                    target.name,
                    material,
                )
                .map(|metadata| SecretCommandOutput::Changed {
                    operation: "create",
                    metadata,
                })
                .map_err(map_admin_error)
        }
        SecretCommand::Update(target) => {
            let material = secret_material(input)?;
            admin
                .update_secret(
                    &TenantId::from_token(target.tenant),
                    target.project,
                    target.name,
                    material,
                )
                .map(|metadata| SecretCommandOutput::Changed {
                    operation: "update",
                    metadata,
                })
                .map_err(map_admin_error)
        }
        SecretCommand::Rotate(target) => {
            let material = secret_material(input)?;
            admin
                .rotate_secret(
                    &TenantId::from_token(target.tenant),
                    target.project,
                    target.name,
                    material,
                )
                .map(|metadata| SecretCommandOutput::Changed {
                    operation: "rotate",
                    metadata,
                })
                .map_err(map_admin_error)
        }
        SecretCommand::Delete(target) => admin
            .delete_secret(
                &TenantId::from_token(target.tenant),
                target.project,
                target.name,
            )
            .map(|()| acknowledged("delete", target))
            .map_err(map_admin_error),
        SecretCommand::List { tenant, project } => admin
            .list_secrets(&TenantId::from_token(tenant), project)
            .map(SecretCommandOutput::Listed)
            .map_err(map_admin_error),
        SecretCommand::GrantBinding { target, scope } => {
            let scope = binding_scope(scope)?;
            admin
                .grant_binding(
                    &TenantId::from_token(target.tenant),
                    target.project,
                    &scope,
                    target.name,
                )
                .map(|()| acknowledged("grant-binding", target))
                .map_err(map_admin_error)
        }
        SecretCommand::RevokeBinding { target, scope } => {
            let scope = binding_scope(scope)?;
            admin
                .revoke_binding(
                    &TenantId::from_token(target.tenant),
                    target.project,
                    &scope,
                    target.name,
                )
                .map(|()| acknowledged("revoke-binding", target))
                .map_err(map_admin_error)
        }
    }
}

fn validate_command(command: &SecretCommand<'_>) -> Result<(), SecretCommandError> {
    match command {
        SecretCommand::Create(target)
        | SecretCommand::Update(target)
        | SecretCommand::Rotate(target)
        | SecretCommand::Delete(target)
        | SecretCommand::GrantBinding { target, .. }
        | SecretCommand::RevokeBinding { target, .. } => validate_target(*target),
        SecretCommand::List { tenant, project } => {
            required(tenant, "--tenant must be non-empty")?;
            if let Some(project) = project {
                required(project, "--project must be non-empty")?;
            }
            Ok(())
        }
    }
}

fn validate_target(target: SecretTarget<'_>) -> Result<(), SecretCommandError> {
    required(target.tenant, "--tenant must be non-empty")?;
    required(target.project, "--project must be non-empty")?;
    required(target.name, "--name must be non-empty")
}

fn required(value: &str, message: &'static str) -> Result<(), SecretCommandError> {
    if value.trim().is_empty() {
        Err(SecretCommandError::BadParam(message))
    } else {
        Ok(())
    }
}

fn binding_scope(scope: &str) -> Result<SecretBindingScope, SecretCommandError> {
    if scope == "project" {
        Ok(SecretBindingScope::Project)
    } else if let Some(job_id) = scope.strip_prefix("job:") {
        required(job_id, "--scope job id must be non-empty")?;
        Ok(SecretBindingScope::Job {
            job_id: job_id.to_owned(),
        })
    } else {
        Err(SecretCommandError::BadParam(
            "--scope must be `project` or `job:<uuid>`",
        ))
    }
}

fn secret_material(input: &mut impl Read) -> Result<SecretMaterial, SecretCommandError> {
    let mut material = read_material(input)?;
    // Move the one allocation into SecretMaterial. `material` is now an empty Zeroizing buffer;
    // SecretMaterial owns and zeroizes the original allocation after SecretAdmin consumes it.
    Ok(SecretMaterial::from(std::mem::take(&mut *material)))
}

fn read_material(input: &mut impl Read) -> Result<Zeroizing<String>, SecretCommandError> {
    const READ_CAPACITY: usize =
        myelin_ci_controlplane::secret_admin::MAX_SECRET_MATERIAL_BYTES + 1;
    let material = Zeroizing::new(String::with_capacity(READ_CAPACITY));
    read_material_into(input, material)
}

fn read_material_into(
    input: &mut impl Read,
    mut material: Zeroizing<String>,
) -> Result<Zeroizing<String>, SecretCommandError> {
    const READ_CAPACITY: usize =
        myelin_ci_controlplane::secret_admin::MAX_SECRET_MATERIAL_BYTES + 1;
    debug_assert!(material.is_empty());
    debug_assert!(material.capacity() >= READ_CAPACITY);
    let initial_capacity = material.capacity();
    input
        .take(READ_CAPACITY as u64)
        .read_to_string(&mut material)
        .map_err(|_| SecretCommandError::Input)?;
    debug_assert_eq!(material.capacity(), initial_capacity);
    if material.len() > myelin_ci_controlplane::secret_admin::MAX_SECRET_MATERIAL_BYTES {
        return Err(SecretCommandError::BadParam(
            "stdin material exceeds the 65536-byte limit",
        ));
    }
    Ok(material)
}

fn acknowledged(operation: &'static str, target: SecretTarget<'_>) -> SecretCommandOutput {
    SecretCommandOutput::Acknowledged {
        operation,
        project_id: target.project.to_owned(),
        name: target.name.to_owned(),
    }
}

fn map_admin_error(error: SecretAdminError) -> SecretCommandError {
    match error {
        SecretAdminError::Unauthorized => SecretCommandError::Forbidden,
        SecretAdminError::InvalidScope => SecretCommandError::BadParam(
            "tenant, project, name, material, or binding scope is invalid",
        ),
        SecretAdminError::AlreadyExists => SecretCommandError::Conflict,
        SecretAdminError::NotFound => SecretCommandError::NotFound,
        SecretAdminError::StoreUnavailable => SecretCommandError::Unavailable,
    }
}

fn strong_consistency() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_ci_controlplane::{DurableSecretCapability, SecretBindingScope, SecretCapability};
    use myelin_events::{OutboxStore, Timestamp};
    use myelin_identity::{
        DataRole, ObjectId, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RelName,
        RelationTuple, TupleDelta,
    };
    use myelin_identity_service::{
        CapabilityMintSpec, CellTokenAuthority, PasetoCapabilityVerifier, PrincipalStore,
        RevocationStore, StoreBackedCheck, TupleStore,
    };
    use myelin_storage::{KmsEngine, TenantScope};
    use myelin_tenancy::{ArtifactRef, Region};
    use std::io::{Cursor, Error as IoError};
    use zeroize::Zeroize;

    const TENANT: &str = "tenant-a";
    const OTHER_TENANT: &str = "tenant-b";
    const PROJECT: &str = "11111111-1111-4111-8111-111111111111";
    const JOB: &str = "22222222-2222-4222-8222-222222222222";
    const ADMIN: &str = "u:operator";
    const REGION: &str = "eu-test";

    struct Harness {
        authn: CapabilityAuthenticator,
        identity: Arc<StoreBackedCheck>,
        store: Arc<DurableCiSecretStore>,
        token: String,
        principal: Principal,
    }

    impl Harness {
        fn new(admin: bool) -> Self {
            let kms = Arc::new(KmsEngine::new());
            let cell = CellTokenAuthority::from_seed(&[7; 32], &[9; 32]).unwrap();
            let principal = Principal::new(
                TenantId::from_token(TENANT),
                Region::new(REGION),
                PrincipalId(ADMIN.into()),
                PrincipalKind::Human,
                DataRole::Controller,
                PrincipalStatus::Active,
            );
            let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
            let principals = PrincipalStore::new(kms.clone());
            principals
                .put_principal(
                    &scope,
                    principal.principal_id.clone(),
                    principal.kind.clone(),
                    principal.data_role,
                    principal.status,
                    None,
                )
                .unwrap();
            principals
                .link_credential(&scope, "agent", "operator-subject", &principal.principal_id)
                .unwrap();
            let token = cell.mint(&CapabilityMintSpec {
                tenant: TENANT.into(),
                region: REGION.into(),
                subject_key: "operator-subject".into(),
                jti: "secret-command-test-jti".into(),
                exp_unix: 4_102_444_800,
                authority: vec!["edge.operator".into()],
                dpop_jkt: None,
                purpose: CredentialPurpose::OperatorBootstrap,
                audience: CredentialAudience::Edge,
            });
            let authn = CapabilityAuthenticator::with_verifier(
                principals,
                Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
                RevocationStore::new(),
            );

            let tuples = TupleStore::new(OutboxStore::new());
            let identity = Arc::new(StoreBackedCheck::new(tuples));
            for admission in identity.admit_git_fragment() {
                assert!(matches!(
                    admission,
                    myelin_identity::FragmentAdmit::Admitted { .. }
                ));
            }
            for admission in identity.admit_ci_fragment() {
                assert!(matches!(
                    admission,
                    myelin_identity::FragmentAdmit::Admitted { .. }
                ));
            }
            if admin {
                identity
                    .tuples()
                    .write_tuples(
                        &scope,
                        &principal,
                        &[TupleDelta::Add(RelationTuple {
                            object: ObjectId(format!("ci_project:{PROJECT}")),
                            relation: RelName("admin".into()),
                            subject: principal.principal_id.clone(),
                            caveat: None,
                        })],
                        None,
                        None,
                        Timestamp("2026-08-03T00:00:00Z".into()),
                    )
                    .unwrap();
            }

            Self {
                authn,
                identity,
                store: Arc::new(DurableCiSecretStore::in_memory(kms, Region::new(REGION))),
                token,
                principal,
            }
        }

        fn credential(&self) -> Credential {
            Credential {
                scheme: "agent".into(),
                material: self.token.clone(),
            }
        }

        fn run(
            &self,
            command: SecretCommand<'_>,
            input: &mut impl Read,
        ) -> Result<SecretCommandOutput, SecretCommandError> {
            execute_secret_command(
                &self.authn,
                self.identity.clone(),
                self.store.clone(),
                Some(self.credential()),
                command,
                input,
            )
        }

        fn grant_secret_read(&self, handle: &str, subject: &Principal) {
            let scope =
                TenantScope::from_verified_token(&self.principal, self.principal.region.clone());
            self.identity
                .tuples()
                .write_tuples(
                    &scope,
                    &self.principal,
                    &[TupleDelta::Add(RelationTuple {
                        object: ObjectId(handle.to_owned()),
                        relation: RelName("direct_reader".into()),
                        subject: subject.principal_id.clone(),
                        caveat: None,
                    })],
                    None,
                    None,
                    Timestamp("2026-08-03T00:00:01Z".into()),
                )
                .unwrap();
        }
    }

    fn target<'a>(tenant: &'a str, project: &'a str, name: &'a str) -> SecretTarget<'a> {
        SecretTarget {
            tenant,
            project,
            name,
        }
    }

    fn no_input() -> Cursor<Vec<u8>> {
        Cursor::new(Vec::new())
    }

    fn secret_handle(tenant: &str, project: &str, name: &str) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"myelin-ci-managed-secret-id:v1");
        for component in [tenant.as_bytes(), project.as_bytes(), name.as_bytes()] {
            hasher.update(&(component.len() as u32).to_be_bytes());
            hasher.update(component);
        }
        format!("myelin://{tenant}/ci/secret/{}", hasher.finalize().to_hex())
    }

    #[test]
    fn secret_command_rejects_empty_tenant_project_and_name() {
        let harness = Harness::new(true);
        for invalid in [
            target("", PROJECT, "DEPLOY_KEY"),
            target(TENANT, "", "DEPLOY_KEY"),
            target(TENANT, PROJECT, ""),
        ] {
            let error = harness
                .run(SecretCommand::Create(invalid), &mut Cursor::new("unused"))
                .unwrap_err();
            assert!(matches!(error, SecretCommandError::BadParam(_)));
        }
    }

    #[test]
    fn secret_command_uses_authenticated_principal_and_exact_stdin_material() {
        let harness = Harness::new(true);
        let plaintext = "exact-stdin-material\nwith-second-line";
        let output = harness
            .run(
                SecretCommand::Create(target(TENANT, PROJECT, "DEPLOY_KEY")),
                &mut Cursor::new(plaintext),
            )
            .unwrap();
        let rendered = output.render();
        assert!(rendered.contains("ok create"));
        assert!(rendered.contains("name=DEPLOY_KEY"));
        assert!(!rendered.contains(plaintext));

        harness
            .run(
                SecretCommand::GrantBinding {
                    target: target(TENANT, PROJECT, "DEPLOY_KEY"),
                    scope: "project",
                },
                &mut no_input(),
            )
            .unwrap();
        let job = Principal::new(
            TenantId::from_token(TENANT),
            Region::new(REGION),
            PrincipalId("svc:ci:test-job".into()),
            PrincipalKind::Service,
            DataRole::Processor,
            PrincipalStatus::Active,
        );
        let handle = secret_handle(TENANT, PROJECT, "DEPLOY_KEY");
        harness.grant_secret_read(&handle, &job);
        let resolved = DurableSecretCapability::new(
            harness.store.clone(),
            harness.identity.clone(),
            job,
            PROJECT.into(),
            JOB.into(),
            strong_consistency(),
        )
        .resolve_handle(
            &TenantId::from_token(TENANT),
            &ArtifactRef(handle.clone()),
            "DEPLOY_KEY",
            &handle,
        )
        .expect("the command must pass the exact stdin material to SecretAdmin");
        assert_eq!(resolved.as_str(), plaintext);
    }

    #[test]
    fn secret_command_without_project_administer_is_uniform_forbidden() {
        let harness = Harness::new(false);
        let plaintext = "denied-material-must-not-escape";
        let error = harness
            .run(
                SecretCommand::Create(target(TENANT, PROJECT, "DEPLOY_KEY")),
                &mut Cursor::new(plaintext),
            )
            .unwrap_err();
        assert_eq!(error, SecretCommandError::Forbidden);
        let rendered = format!("{error:?} {error}");
        assert_eq!(error.to_string(), "forbidden");
        assert!(!rendered.contains(plaintext));
    }

    #[test]
    fn secret_command_cross_tenant_is_uniform_forbidden() {
        let harness = Harness::new(true);
        let plaintext = "cross-tenant-material-must-not-escape";
        let error = harness
            .run(
                SecretCommand::Create(target(OTHER_TENANT, PROJECT, "DEPLOY_KEY")),
                &mut Cursor::new(plaintext),
            )
            .unwrap_err();
        assert_eq!(error, SecretCommandError::Forbidden);
        assert_eq!(error.to_string(), "forbidden");
        assert!(!format!("{error:?} {error}").contains(plaintext));
    }

    #[test]
    fn secret_command_missing_and_invalid_tokens_are_uniform_unauthorized() {
        let harness = Harness::new(true);
        let command = SecretCommand::List {
            tenant: TENANT,
            project: Some(PROJECT),
        };
        let missing = execute_secret_command(
            &harness.authn,
            harness.identity.clone(),
            harness.store.clone(),
            None,
            command,
            &mut no_input(),
        )
        .unwrap_err();
        let invalid = execute_secret_command(
            &harness.authn,
            harness.identity.clone(),
            harness.store.clone(),
            Some(Credential {
                scheme: "agent".into(),
                material: "invalid-token".into(),
            }),
            command,
            &mut no_input(),
        )
        .unwrap_err();
        let empty = execute_secret_command(
            &harness.authn,
            harness.identity.clone(),
            harness.store.clone(),
            Some(Credential {
                scheme: "agent".into(),
                material: String::new(),
            }),
            command,
            &mut no_input(),
        )
        .unwrap_err();

        assert_eq!(missing, SecretCommandError::Unauthorized);
        assert_eq!(missing, invalid);
        assert_eq!(missing, empty);
        assert_eq!(missing.to_string(), "authentication required");
        assert_eq!(missing.exit_code(), 1);
        assert_eq!(missing.exit_code(), invalid.exit_code());
    }

    #[test]
    fn secret_command_list_renders_names_and_metadata_never_material() {
        let harness = Harness::new(true);
        for (name, plaintext) in [
            ("ALPHA", "alpha-list-material-must-not-escape"),
            ("BETA", "beta-list-material-must-not-escape"),
        ] {
            harness
                .run(
                    SecretCommand::Create(target(TENANT, PROJECT, name)),
                    &mut Cursor::new(plaintext),
                )
                .unwrap();
        }
        let output = harness
            .run(
                SecretCommand::List {
                    tenant: TENANT,
                    project: Some(PROJECT),
                },
                &mut no_input(),
            )
            .unwrap();
        let rendered = output.render();
        assert!(rendered.contains("name=ALPHA"));
        assert!(rendered.contains("name=BETA"));
        assert!(rendered.contains("version=1"));
        assert!(!rendered.contains("alpha-list-material-must-not-escape"));
        assert!(!rendered.contains("beta-list-material-must-not-escape"));
        assert!(!format!("{output:?}").contains("material-must-not-escape"));
    }

    struct MaterialThenError {
        emitted: bool,
    }

    impl Read for MaterialThenError {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.emitted {
                return Err(IoError::other("reader detail"));
            }
            self.emitted = true;
            let material = b"partial-secret";
            buffer[..material.len()].copy_from_slice(material);
            Ok(material.len())
        }
    }

    #[test]
    fn secret_command_material_read_is_fixed_capacity_bounded_and_zeroizing() {
        fn assert_zeroizing_string(_: &Zeroizing<String>) {}

        let read_capacity = myelin_ci_controlplane::secret_admin::MAX_SECRET_MATERIAL_BYTES + 1;
        let material = Zeroizing::new(String::with_capacity(read_capacity));
        let initial_pointer = material.as_ptr();
        let initial_capacity = material.capacity();
        let bounded_material = "x".repeat(read_capacity - 1);
        let mut material =
            read_material_into(&mut Cursor::new(bounded_material), material).unwrap();
        assert_zeroizing_string(&material);
        assert_eq!(material.as_ptr(), initial_pointer);
        assert_eq!(material.capacity(), initial_capacity);
        material.zeroize();
        assert!(material.is_empty());

        let oversized = "x".repeat(read_capacity);
        let error = read_material(&mut Cursor::new(oversized)).unwrap_err();
        assert_eq!(
            error,
            SecretCommandError::BadParam("stdin material exceeds the 65536-byte limit")
        );
    }

    #[test]
    fn secret_command_material_errors_and_outputs_cannot_carry_material() {
        let harness = Harness::new(true);

        let error = harness
            .run(
                SecretCommand::Create(target(TENANT, PROJECT, "DEPLOY_KEY")),
                &mut MaterialThenError { emitted: false },
            )
            .unwrap_err();
        assert_eq!(error, SecretCommandError::Input);
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("partial-secret"));
        assert!(!rendered.contains("reader detail"));

        let output = SecretCommandOutput::Acknowledged {
            operation: "grant-binding",
            project_id: PROJECT.into(),
            name: "DEPLOY_KEY".into(),
        };
        assert!(!format!("{output:?} {}", output.render()).contains("zeroize-me"));
        let _typed_scope = SecretBindingScope::Job { job_id: JOB.into() };
    }
}
