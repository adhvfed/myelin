//! Tenant-scoped, ReBAC-gated administration for durable CI secrets and their use bindings.

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use myelin_identity::{
    Consistency, Decision, IdentityService, ListObjectsResult, ObjectType, Permission, Principal,
};
use myelin_tenancy::{ArtifactRef, TenantId};
use zeroize::Zeroizing;

use crate::ci_secret_store::{CiSecretStoreError, DurableCiSecretStore, ManagedSecretRow};
use crate::rebac_fragment::{object_types, ADMINISTER};
use crate::secret_broker::strict_secret_segment;

/// The secret-management capability. It is deliberately the CI-project administration permission,
/// not the job-facing `secret.read` permission.
pub const SECRET_ADMIN_PERMISSION: &str = ADMINISTER;

const SECRET_ID_DOMAIN: &[u8] = b"myelin-ci-managed-secret-id:v1";
const MAX_SECRET_MATERIAL_BYTES: usize = 64 * 1024;

/// Secret input owned by the call. Debug is always redacted and the allocation is zeroized on drop,
/// including every success and error path after encryption.
pub struct SecretMaterial(Zeroizing<String>);

impl SecretMaterial {
    pub fn new(material: impl Into<String>) -> Self {
        Self(Zeroizing::new(material.into()))
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl std::fmt::Debug for SecretMaterial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretMaterial([REDACTED])")
    }
}

impl From<String> for SecretMaterial {
    fn from(material: String) -> Self {
        Self::new(material)
    }
}

impl From<&str> for SecretMaterial {
    fn from(material: &str) -> Self {
        Self::new(material)
    }
}

/// Material-free metadata returned by create, update, rotate, and list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretMetadata {
    pub project_id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub version: i64,
}

/// A use binding is either available to every job in the project or to one exact job. Job scope
/// validates the canonical UUID syntax but deliberately does not require a durable `ci_job` row:
/// bindings may be provisioned before a job exists, and use still requires both a live binding and
/// the exact job principal's independent `secret.read` grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretBindingScope {
    Project,
    Job { job_id: String },
}

impl SecretBindingScope {
    fn storage_scope(&self) -> Result<String, SecretAdminError> {
        match self {
            Self::Project => Ok("project".to_owned()),
            Self::Job { job_id } if valid_uuid(job_id) => Ok(format!("job:{job_id}")),
            Self::Job { .. } => Err(SecretAdminError::InvalidScope),
        }
    }
}

/// A material-free admin failure. Authorization failures intentionally collapse tenant mismatch,
/// Identity errors, and non-Allow decisions into the same fail-closed result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretAdminError {
    Unauthorized,
    InvalidScope,
    AlreadyExists,
    NotFound,
    StoreUnavailable,
}

impl std::fmt::Display for SecretAdminError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unauthorized => "CI secret administration is not authorized",
            Self::InvalidScope => "CI secret administration scope is invalid",
            Self::AlreadyExists => "CI secret already exists",
            Self::NotFound => "CI secret does not exist",
            Self::StoreUnavailable => "CI secret administration is unavailable",
        })
    }
}

impl std::error::Error for SecretAdminError {}

/// A caller-bound secret management service. All operations are tenant-first and authorize before
/// reading or mutating secret rows.
pub struct SecretAdmin<I: IdentityService> {
    store: Arc<DurableCiSecretStore>,
    identity: Arc<I>,
    caller: Principal,
    consistency: Consistency,
}

impl<I: IdentityService> SecretAdmin<I> {
    pub fn new(
        store: Arc<DurableCiSecretStore>,
        identity: Arc<I>,
        caller: Principal,
        consistency: Consistency,
    ) -> Self {
        Self {
            store,
            identity,
            caller,
            consistency,
        }
    }

    pub fn create_secret(
        &self,
        tenant: &TenantId,
        project_id: &str,
        name: &str,
        material: SecretMaterial,
    ) -> Result<SecretMetadata, SecretAdminError> {
        self.authorize_project(tenant, project_id)?;
        if !strict_secret_segment(tenant.as_str()) {
            return Err(SecretAdminError::InvalidScope);
        }
        validate_name_and_material(name, material.as_bytes())?;
        let secret_id = secret_id_for(tenant, project_id, name);
        self.store
            .create_managed_secret(tenant, project_id, &secret_id, name, material.as_bytes())
            .map(metadata_of)
            .map_err(map_store_error)
    }

    pub fn update_secret(
        &self,
        tenant: &TenantId,
        project_id: &str,
        name: &str,
        material: SecretMaterial,
    ) -> Result<SecretMetadata, SecretAdminError> {
        self.replace_secret(tenant, project_id, name, material)
    }

    pub fn rotate_secret(
        &self,
        tenant: &TenantId,
        project_id: &str,
        name: &str,
        material: SecretMaterial,
    ) -> Result<SecretMetadata, SecretAdminError> {
        self.replace_secret(tenant, project_id, name, material)
    }

    pub fn delete_secret(
        &self,
        tenant: &TenantId,
        project_id: &str,
        name: &str,
    ) -> Result<(), SecretAdminError> {
        self.authorize_project(tenant, project_id)?;
        validate_name(name)?;
        self.store
            .delete_managed_secret(tenant, project_id, name)
            .map_err(map_store_error)
    }

    pub fn list_secrets(
        &self,
        tenant: &TenantId,
        project_id: Option<&str>,
    ) -> Result<Vec<SecretMetadata>, SecretAdminError> {
        self.require_same_tenant(tenant)?;
        let authorized_projects = match project_id {
            Some(project_id) => {
                self.authorize_project(tenant, project_id)?;
                AuthorizedProjects::Only(BTreeSet::from([project_id.to_owned()]))
            }
            None => self.authorized_projects(tenant)?,
        };
        self.store
            .list_managed_secrets(tenant, project_id)
            .map_err(map_store_error)
            .map(|rows| {
                rows.into_iter()
                    .filter(|row| authorized_projects.contains(&row.project_id))
                    .map(metadata_of)
                    .collect()
            })
    }

    pub fn grant_binding(
        &self,
        tenant: &TenantId,
        project_id: &str,
        scope: &SecretBindingScope,
        secret_name: &str,
    ) -> Result<(), SecretAdminError> {
        self.authorize_project(tenant, project_id)?;
        validate_name(secret_name)?;
        let scope = scope.storage_scope()?;
        self.store
            .grant_managed_binding(tenant, project_id, secret_name, &scope)
            .map_err(map_store_error)
    }

    pub fn revoke_binding(
        &self,
        tenant: &TenantId,
        project_id: &str,
        scope: &SecretBindingScope,
        secret_name: &str,
    ) -> Result<(), SecretAdminError> {
        self.authorize_project(tenant, project_id)?;
        validate_name(secret_name)?;
        let scope = scope.storage_scope()?;
        self.store
            .revoke_managed_binding(tenant, project_id, secret_name, &scope)
            .map_err(map_store_error)
    }

    fn replace_secret(
        &self,
        tenant: &TenantId,
        project_id: &str,
        name: &str,
        material: SecretMaterial,
    ) -> Result<SecretMetadata, SecretAdminError> {
        self.authorize_project(tenant, project_id)?;
        validate_name_and_material(name, material.as_bytes())?;
        let secret_id = secret_id_for(tenant, project_id, name);
        self.store
            .replace_managed_secret(tenant, project_id, &secret_id, name, material.as_bytes())
            .map(metadata_of)
            .map_err(map_store_error)
    }

    fn require_same_tenant(&self, tenant: &TenantId) -> Result<(), SecretAdminError> {
        if self.caller.tenant == *tenant {
            Ok(())
        } else {
            Err(SecretAdminError::Unauthorized)
        }
    }

    fn authorize_project(
        &self,
        tenant: &TenantId,
        project_id: &str,
    ) -> Result<(), SecretAdminError> {
        self.require_same_tenant(tenant)?;
        if !valid_uuid(project_id) {
            return Err(SecretAdminError::InvalidScope);
        }
        let object = project_object(tenant, project_id);
        match self.identity.check(
            &self.caller,
            &Permission(SECRET_ADMIN_PERMISSION.to_owned()),
            &object,
            &self.consistency,
            None,
        ) {
            Ok(Decision::Allow) => Ok(()),
            Ok(Decision::Deny | Decision::Conditional) | Err(_) => {
                Err(SecretAdminError::Unauthorized)
            }
        }
    }

    fn authorized_projects(
        &self,
        _tenant: &TenantId,
    ) -> Result<AuthorizedProjects, SecretAdminError> {
        match self.identity.list_objects(
            &self.caller,
            &Permission(SECRET_ADMIN_PERMISSION.to_owned()),
            &ObjectType(object_types::CI_PROJECT.to_owned()),
            &self.consistency,
        ) {
            Ok(ListObjectsResult::Filter {
                set_expr: myelin_identity::SetExpr::All,
                ..
            }) => Ok(AuthorizedProjects::All),
            Ok(ListObjectsResult::Ids { ids, .. }) if !ids.is_empty() => Ok(
                AuthorizedProjects::Only(ids.into_iter().map(|id| project_id_of(&id.0)).collect()),
            ),
            Ok(ListObjectsResult::Ids { .. } | ListObjectsResult::Filter { .. }) | Err(_) => {
                Err(SecretAdminError::Unauthorized)
            }
        }
    }
}

enum AuthorizedProjects {
    All,
    Only(BTreeSet<String>),
}

impl AuthorizedProjects {
    fn contains(&self, project_id: &str) -> bool {
        match self {
            Self::All => true,
            Self::Only(projects) => projects.contains(project_id),
        }
    }
}

fn metadata_of(row: ManagedSecretRow) -> SecretMetadata {
    SecretMetadata {
        project_id: row.project_id,
        name: row.name,
        created_at: row.created_at,
        version: row.version,
    }
}

fn map_store_error(error: CiSecretStoreError) -> SecretAdminError {
    match error {
        CiSecretStoreError::InvalidScope => SecretAdminError::InvalidScope,
        CiSecretStoreError::AlreadyExists => SecretAdminError::AlreadyExists,
        CiSecretStoreError::NotFound => SecretAdminError::NotFound,
        CiSecretStoreError::Encrypt
        | CiSecretStoreError::Database
        | CiSecretStoreError::CorruptCiphertext => SecretAdminError::StoreUnavailable,
    }
}

fn project_object(tenant: &TenantId, project_id: &str) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/ci/ci_project/{project_id}",
        tenant.as_str()
    ))
}

fn project_id_of(object_id: &str) -> String {
    object_id
        .strip_prefix("ci_project:")
        .or_else(|| object_id.rsplit('/').next())
        .unwrap_or(object_id)
        .to_owned()
}

fn secret_id_for(tenant: &TenantId, project_id: &str, name: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SECRET_ID_DOMAIN);
    for component in [
        tenant.as_str().as_bytes(),
        project_id.as_bytes(),
        name.as_bytes(),
    ] {
        hasher.update(&(component.len() as u32).to_be_bytes());
        hasher.update(component);
    }
    hasher.finalize().to_hex().to_string()
}

fn validate_name(name: &str) -> Result<(), SecretAdminError> {
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        Err(SecretAdminError::InvalidScope)
    } else {
        Ok(())
    }
}

fn validate_name_and_material(name: &str, material: &[u8]) -> Result<(), SecretAdminError> {
    validate_name(name)?;
    if material.is_empty() || material.len() > MAX_SECRET_MATERIAL_BYTES {
        Err(SecretAdminError::InvalidScope)
    } else {
        Ok(())
    }
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{
        AuthzError, CaveatContext, ConsistencyMode, Credential, DelegationCaveats, EffectivePolicy,
        FailStaticBound, FragmentAdmit, NamespaceFragment, ObjectId, Precondition, PrincipalId,
        PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace, RunId, RunToken,
        SubjectTree, TupleDelta, Zookie,
    };
    use myelin_storage::KmsEngine;
    use myelin_tenancy::Region;
    use std::collections::HashSet;

    use crate::{DurableSecretCapability, SecretCapability, SECRET_READ_PERMISSION};

    const PROJECT: &str = "11111111-1111-4111-8111-111111111111";
    const OTHER_PROJECT: &str = "22222222-2222-4222-8222-222222222222";
    const JOB: &str = "33333333-3333-4333-8333-333333333333";
    const ADMIN_ID: &str = "u:secret-admin";

    #[derive(Default)]
    struct TestIdentity {
        grants: std::sync::Mutex<HashSet<(String, String, String)>>,
    }

    impl TestIdentity {
        fn grant(&self, principal: &str, permission: &str, object: &ArtifactRef) {
            self.grants
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert((
                    principal.to_owned(),
                    permission.to_owned(),
                    object.0.clone(),
                ));
        }
    }

    impl IdentityService for TestIdentity {
        fn authenticate(&self, _credential: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("test stub"))
        }

        fn check(
            &self,
            subject: &Principal,
            permission: &Permission,
            object: &ArtifactRef,
            _at: &Consistency,
            _caveat: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            if self
                .grants
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains(&(
                    subject.principal_id.0.clone(),
                    permission.0.clone(),
                    object.0.clone(),
                ))
            {
                Ok(Decision::Allow)
            } else {
                Ok(Decision::Deny)
            }
        }

        fn list_objects(
            &self,
            subject: &Principal,
            permission: &Permission,
            object_type: &ObjectType,
            _at: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            let ids = self
                .grants
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .filter(|(principal, granted_permission, _object)| {
                    principal == &subject.principal_id.0
                        && granted_permission == &permission.0
                        && object_type.0 == object_types::CI_PROJECT
                })
                .filter_map(|(_, _, object)| object.rsplit('/').next().map(str::to_owned))
                .map(ObjectId)
                .collect();
            Ok(ListObjectsResult::Ids {
                ids,
                zookie: Zookie("test".into()),
            })
        }

        fn list_subjects(
            &self,
            _object: &ObjectId,
            _permission: &Permission,
            _at: &Consistency,
        ) -> IdResult<SubjectTree> {
            Err(AuthzError::NotYetImplemented("test stub"))
        }

        fn explain(
            &self,
            _subject: &Principal,
            _permission: &Permission,
            _object: &ObjectId,
            _at: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("test stub"))
        }

        fn delegation(&self, _actor: &Principal, _target: &Principal) -> IdResult<EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("test stub"))
        }

        fn write_tuples(
            &self,
            _deltas: &[TupleDelta],
            _precondition: Option<&Precondition>,
        ) -> IdResult<Zookie> {
            Err(AuthzError::NotYetImplemented("test stub"))
        }

        fn mint_run_token(
            &self,
            _actor: &PrincipalId,
            _run: &RunId,
            _delegation: &DelegationCaveats,
            _ttl: &FailStaticBound,
        ) -> IdResult<RunToken> {
            Err(AuthzError::NotYetImplemented("test stub"))
        }

        fn revoke(&self, _target: &RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("test stub"))
        }

        fn resolve_pseudonym(
            &self,
            _subject: &PrincipalId,
            _tenant: &TenantId,
        ) -> IdResult<String> {
            Err(AuthzError::NotYetImplemented("test stub"))
        }

        fn erase(&self, _subject: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("test stub"))
        }

        fn admit_fragment(&self, _fragment: &NamespaceFragment) -> IdResult<FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("test stub"))
        }
    }

    fn tenant(value: &str) -> TenantId {
        TenantId::from_token(value)
    }

    fn principal(tenant: &str, id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.to_owned()),
            PrincipalKind::Human,
            TenantId::from_token(tenant),
        )
    }

    fn consistency() -> Consistency {
        Consistency {
            at_least: Zookie("test".into()),
            mode: ConsistencyMode::Strong,
        }
    }

    fn store() -> Arc<DurableCiSecretStore> {
        Arc::new(DurableCiSecretStore::in_memory(
            Arc::new(KmsEngine::new()),
            Region::new("eu-test"),
        ))
    }

    fn admin_fixture(
        tenant_id: &str,
        project_id: &str,
    ) -> (
        Arc<DurableCiSecretStore>,
        Arc<TestIdentity>,
        SecretAdmin<TestIdentity>,
    ) {
        let store = store();
        let identity = Arc::new(TestIdentity::default());
        let tenant = tenant(tenant_id);
        let caller = principal(tenant_id, ADMIN_ID);
        identity.grant(
            ADMIN_ID,
            SECRET_ADMIN_PERMISSION,
            &project_object(&tenant, project_id),
        );
        let admin = SecretAdmin::new(store.clone(), identity.clone(), caller, consistency());
        (store, identity, admin)
    }

    fn handle(tenant: &TenantId, project_id: &str, name: &str) -> String {
        format!(
            "myelin://{}/ci/secret/{}",
            tenant.as_str(),
            secret_id_for(tenant, project_id, name)
        )
    }

    fn resolve(
        store: Arc<DurableCiSecretStore>,
        identity: Arc<TestIdentity>,
        tenant: &TenantId,
        name: &str,
    ) -> Option<Zeroizing<String>> {
        let handle = handle(tenant, PROJECT, name);
        let job_principal = format!("svc:ci:project:{PROJECT}:job:{JOB}");
        let job_subject = principal(tenant.as_str(), &job_principal);
        identity.grant(
            &job_subject.principal_id.0,
            SECRET_READ_PERMISSION,
            &ArtifactRef(handle.clone()),
        );
        DurableSecretCapability::new(
            store,
            identity,
            job_subject,
            PROJECT.into(),
            JOB.into(),
            consistency(),
        )
        .resolve_handle(tenant, &ArtifactRef(handle.clone()), name, &handle)
    }

    #[test]
    fn secret_admin_create_grant_resolves_through_durable_capability() {
        let tenant = tenant("tenant-a");
        let (store, identity, admin) = admin_fixture(tenant.as_str(), PROJECT);
        let metadata = admin
            .create_secret(
                &tenant,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from("created-material"),
            )
            .expect("authorized create");
        assert_eq!(metadata.version, 1);
        admin
            .grant_binding(&tenant, PROJECT, &SecretBindingScope::Project, "DEPLOY_KEY")
            .expect("authorized binding grant");

        assert_eq!(
            resolve(store, identity, &tenant, "DEPLOY_KEY")
                .as_deref()
                .map(String::as_str),
            Some("created-material")
        );
    }

    #[test]
    fn secret_admin_list_is_metadata_only_and_duplicate_create_is_refused() {
        let tenant = tenant("tenant-a");
        let (_store, _identity, admin) = admin_fixture(tenant.as_str(), PROJECT);
        let material = "must-never-appear-in-list";
        admin
            .create_secret(&tenant, PROJECT, "ALPHA", SecretMaterial::from(material))
            .unwrap();
        admin
            .create_secret(
                &tenant,
                PROJECT,
                "BETA",
                SecretMaterial::from("different-material"),
            )
            .unwrap();
        assert_eq!(
            admin.create_secret(
                &tenant,
                PROJECT,
                "ALPHA",
                SecretMaterial::from("replacement-must-not-upsert"),
            ),
            Err(SecretAdminError::AlreadyExists)
        );

        let listed = admin.list_secrets(&tenant, None).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["ALPHA", "BETA"]
        );
        assert!(listed.iter().all(|row| row.version == 1));
        let rendered = format!("{listed:?}");
        assert!(!rendered.contains(material));
        assert!(!rendered.contains("different-material"));
        assert!(!rendered.contains("ciphertext"));
        assert!(!rendered.contains("material"));
    }

    #[test]
    fn secret_admin_cross_tenant_admin_is_refused_all_crud_and_list() {
        let owner = tenant("tenant-a");
        let (store, _owner_identity, owner_admin) = admin_fixture(owner.as_str(), PROJECT);
        owner_admin
            .create_secret(
                &owner,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from("owner-material"),
            )
            .unwrap();

        let foreign_identity = Arc::new(TestIdentity::default());
        let foreign_tenant = tenant("tenant-b");
        foreign_identity.grant(
            ADMIN_ID,
            SECRET_ADMIN_PERMISSION,
            &project_object(&foreign_tenant, PROJECT),
        );
        let foreign_admin = SecretAdmin::new(
            store,
            foreign_identity,
            principal(foreign_tenant.as_str(), ADMIN_ID),
            consistency(),
        );

        assert_eq!(
            foreign_admin.create_secret(
                &owner,
                OTHER_PROJECT,
                "FOREIGN",
                SecretMaterial::from("foreign-create-material"),
            ),
            Err(SecretAdminError::Unauthorized)
        );
        assert_eq!(
            foreign_admin.update_secret(
                &owner,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from("foreign-update-material"),
            ),
            Err(SecretAdminError::Unauthorized)
        );
        assert_eq!(
            foreign_admin.delete_secret(&owner, PROJECT, "DEPLOY_KEY"),
            Err(SecretAdminError::Unauthorized)
        );
        assert_eq!(
            foreign_admin.list_secrets(&owner, Some(PROJECT)),
            Err(SecretAdminError::Unauthorized)
        );
    }

    #[test]
    fn secret_admin_revoke_binding_terminally_withholds_and_delete_removes_bindings() {
        let tenant = tenant("tenant-a");
        let (store, identity, admin) = admin_fixture(tenant.as_str(), PROJECT);
        admin
            .create_secret(
                &tenant,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from("bound-material"),
            )
            .unwrap();
        admin
            .grant_binding(
                &tenant,
                PROJECT,
                &SecretBindingScope::Job { job_id: JOB.into() },
                "DEPLOY_KEY",
            )
            .unwrap();
        assert!(resolve(store.clone(), identity.clone(), &tenant, "DEPLOY_KEY").is_some());

        admin
            .revoke_binding(
                &tenant,
                PROJECT,
                &SecretBindingScope::Job { job_id: JOB.into() },
                "DEPLOY_KEY",
            )
            .unwrap();
        assert_eq!(
            resolve(store.clone(), identity.clone(), &tenant, "DEPLOY_KEY"),
            None
        );

        admin
            .grant_binding(&tenant, PROJECT, &SecretBindingScope::Project, "DEPLOY_KEY")
            .unwrap();
        admin.delete_secret(&tenant, PROJECT, "DEPLOY_KEY").unwrap();
        assert_eq!(resolve(store, identity, &tenant, "DEPLOY_KEY"), None);
        assert_eq!(
            admin.grant_binding(&tenant, PROJECT, &SecretBindingScope::Project, "DEPLOY_KEY",),
            Err(SecretAdminError::NotFound)
        );
    }

    #[test]
    fn secret_admin_grant_losing_delete_race_fails_without_orphan_or_recreate_resurrection() {
        let tenant = tenant("tenant-a");
        let (store, identity, admin) = admin_fixture(tenant.as_str(), PROJECT);
        admin
            .create_secret(
                &tenant,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from("pre-delete-material"),
            )
            .unwrap();
        let secret_id = secret_id_for(&tenant, PROJECT, "DEPLOY_KEY");

        // This is the losing grant-after-delete interleaving. The production INSERT..SELECT and FK
        // make the same interleaving fail closed even when delete commits between lookup and insert.
        admin.delete_secret(&tenant, PROJECT, "DEPLOY_KEY").unwrap();
        assert_eq!(
            store
                .insert_binding_for_secret_id_for_test(&tenant, PROJECT, "DEPLOY_KEY", &secret_id,),
            Err(CiSecretStoreError::NotFound),
            "the FK-equivalent insert phase refuses the secret id captured before delete"
        );
        assert_eq!(
            admin.grant_binding(&tenant, PROJECT, &SecretBindingScope::Project, "DEPLOY_KEY"),
            Err(SecretAdminError::NotFound)
        );
        assert_eq!(
            store.binding_count_for_secret_for_test(&tenant, &secret_id),
            0,
            "a losing concurrent grant cannot leave an orphan"
        );

        let recreated = admin
            .create_secret(
                &tenant,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from("post-delete-material"),
            )
            .unwrap();
        assert_eq!(recreated.version, 2);
        assert_eq!(
            resolve(store, identity, &tenant, "DEPLOY_KEY"),
            None,
            "the deterministic handle does not resurrect a pre-delete grant"
        );
    }

    #[test]
    fn secret_admin_delete_cascades_every_binding_for_secret_id() {
        let tenant = tenant("tenant-a");
        let (store, _identity, admin) = admin_fixture(tenant.as_str(), PROJECT);
        admin
            .create_secret(
                &tenant,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from("bound-material"),
            )
            .unwrap();
        admin
            .grant_binding(&tenant, PROJECT, &SecretBindingScope::Project, "DEPLOY_KEY")
            .unwrap();
        admin
            .grant_binding(
                &tenant,
                PROJECT,
                &SecretBindingScope::Job { job_id: JOB.into() },
                "DEPLOY_KEY",
            )
            .unwrap();
        let secret_id = secret_id_for(&tenant, PROJECT, "DEPLOY_KEY");
        store.insert_malformed_binding_for_secret_for_test(&tenant, PROJECT, &secret_id);
        assert_eq!(
            store.binding_count_for_secret_for_test(&tenant, &secret_id),
            3
        );

        admin.delete_secret(&tenant, PROJECT, "DEPLOY_KEY").unwrap();

        assert_eq!(
            store.binding_count_for_secret_for_test(&tenant, &secret_id),
            0,
            "cascade keys cleanup by tenant + secret_id, not mutable name/handle metadata"
        );
    }

    #[test]
    fn secret_admin_create_rejects_noncanonical_tenant_handles() {
        for malformed in ["bad/tenant".to_owned(), "t".repeat(129)] {
            let tenant = tenant(&malformed);
            let (_store, _identity, admin) = admin_fixture(tenant.as_str(), PROJECT);
            assert_eq!(
                admin.create_secret(
                    &tenant,
                    PROJECT,
                    "DEPLOY_KEY",
                    SecretMaterial::from("must-not-be-stored"),
                ),
                Err(SecretAdminError::InvalidScope)
            );
        }
    }

    #[test]
    fn secret_admin_version_is_monotonic_across_delete_recreate() {
        let tenant = tenant("tenant-a");
        let (_store, _identity, admin) = admin_fixture(tenant.as_str(), PROJECT);
        let created = admin
            .create_secret(
                &tenant,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from("version-one"),
            )
            .unwrap();
        let updated = admin
            .update_secret(
                &tenant,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from("version-two"),
            )
            .unwrap();
        admin.delete_secret(&tenant, PROJECT, "DEPLOY_KEY").unwrap();
        let recreated = admin
            .create_secret(
                &tenant,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from("version-three"),
            )
            .unwrap();

        assert_eq!(
            (created.version, updated.version, recreated.version),
            (1, 2, 3)
        );
    }

    #[test]
    fn secret_admin_update_and_rotate_replace_material_nonce_ciphertext_and_bump_version() {
        let tenant = tenant("tenant-a");
        let (store, identity, admin) = admin_fixture(tenant.as_str(), PROJECT);
        admin
            .create_secret(
                &tenant,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from("old-material"),
            )
            .unwrap();
        admin
            .grant_binding(&tenant, PROJECT, &SecretBindingScope::Project, "DEPLOY_KEY")
            .unwrap();
        let secret_id = secret_id_for(&tenant, PROJECT, "DEPLOY_KEY");
        let first_envelope = store
            .managed_envelope_for_test(&tenant, &secret_id)
            .unwrap();

        let updated = admin
            .update_secret(
                &tenant,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from("updated-material"),
            )
            .unwrap();
        let second_envelope = store
            .managed_envelope_for_test(&tenant, &secret_id)
            .unwrap();
        assert_eq!(updated.version, 2);
        assert_ne!(
            first_envelope.0, second_envelope.0,
            "update uses a fresh nonce"
        );
        assert_ne!(first_envelope.1, second_envelope.1);
        assert_eq!(
            resolve(store.clone(), identity.clone(), &tenant, "DEPLOY_KEY")
                .as_deref()
                .map(String::as_str),
            Some("updated-material")
        );

        let rotated = admin
            .rotate_secret(
                &tenant,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from("rotated-material"),
            )
            .unwrap();
        let third_envelope = store
            .managed_envelope_for_test(&tenant, &secret_id)
            .unwrap();
        assert_eq!(rotated.version, 3);
        assert_ne!(
            second_envelope.0, third_envelope.0,
            "rotate uses a fresh nonce"
        );
        assert_ne!(second_envelope.1, third_envelope.1);
        assert_eq!(
            resolve(store, identity, &tenant, "DEPLOY_KEY")
                .as_deref()
                .map(String::as_str),
            Some("rotated-material")
        );
    }

    #[test]
    fn secret_admin_material_is_zeroizing_redacted_and_ciphertext_only_at_rest() {
        fn assert_zeroizing_string(_: &Zeroizing<String>) {}

        let tenant = tenant("tenant-a");
        let (store, _identity, admin) = admin_fixture(tenant.as_str(), PROJECT);
        let plaintext = "plaintext-must-not-rest-or-render";
        let material = SecretMaterial::from(plaintext);
        assert_zeroizing_string(&material.0);
        assert_eq!(format!("{material:?}"), "SecretMaterial([REDACTED])");
        admin
            .create_secret(&tenant, PROJECT, "DEPLOY_KEY", material)
            .unwrap();

        let secret_id = secret_id_for(&tenant, PROJECT, "DEPLOY_KEY");
        let (_nonce, ciphertext) = store
            .managed_envelope_for_test(&tenant, &secret_id)
            .unwrap();
        assert_ne!(ciphertext, plaintext.as_bytes());
        assert!(!ciphertext
            .windows(plaintext.len())
            .any(|window| window == plaintext.as_bytes()));
    }

    #[test]
    fn secret_admin_without_administer_capability_is_refused_without_material_leak() {
        let tenant = tenant("tenant-a");
        let store = store();
        let identity = Arc::new(TestIdentity::default());
        let admin = SecretAdmin::new(
            store,
            identity,
            principal(tenant.as_str(), "u:not-admin"),
            consistency(),
        );
        let plaintext = "denied-material-must-not-escape";
        let error = admin
            .create_secret(
                &tenant,
                PROJECT,
                "DEPLOY_KEY",
                SecretMaterial::from(plaintext),
            )
            .expect_err("no project administer grant");

        assert_eq!(error, SecretAdminError::Unauthorized);
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains(plaintext));
        assert_ne!(SECRET_ADMIN_PERMISSION, SECRET_READ_PERMISSION);
    }
}
