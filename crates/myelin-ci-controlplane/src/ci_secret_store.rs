//! Durable, tenant-DEK-sealed CI secret material and the production broker capability.
//!
//! `secret_binding` remains names-and-handles only. The material lives in `ci_secret` exclusively as
//! Storage's existing [`myelin_storage::EncryptedColumn`] envelope. Reads and writes use the shared
//! tenant-scoped transaction convention, while the synchronous broker trait bridges onto the service
//! runtime in the same way as the other durable stores.

use std::sync::Arc;

use myelin_gdpr::ErasureMethod;
use myelin_identity::{
    AuthzError, Consistency, Decision, IdentityService, Permission, Principal,
};
use myelin_storage::{
    with_tenant_tx, ColumnCryptor, EncryptedColumn, KekId, KeyClass, KmsEngine, PiiKeyRef,
    NONCE_LEN,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use sqlx::Row;
use zeroize::Zeroizing;

use crate::secret_broker::{parse_canonical_secret_handle, strict_secret_segment};
use crate::{
    CiJobSecretResolver, SecretBroker, SecretCapability, SecretLaunchError, SECRET_READ_PERMISSION,
};

const INSERT_SECRET: &str = "INSERT INTO ci_secret \
    (tenant_id, region, secret_id, name, pii_key_ref, nonce, ciphertext, version) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, 1) \
    ON CONFLICT (tenant_id, secret_id) DO UPDATE SET \
      name = EXCLUDED.name, pii_key_ref = EXCLUDED.pii_key_ref, nonce = EXCLUDED.nonce, \
      ciphertext = EXCLUDED.ciphertext, version = ci_secret.version + 1, updated_at = now()";

const SELECT_SECRET: &str = "SELECT pii_key_ref, nonce, ciphertext \
    FROM ci_secret \
    WHERE tenant_id = $1 AND region = $2 AND secret_id = $3";

const SELECT_SECRET_BINDING: &str = "SELECT EXISTS (SELECT 1 FROM secret_binding \
    WHERE tenant_id = $1 AND region = $2 AND project_id = $3::uuid AND name = $4 \
      AND value_ref = $5 AND scope IN ('project', $6)) AS bound";

const SECRET_AAD_DOMAIN: &[u8] = b"myelin-ci-secret-row:v1";

/// Unambiguous, domain-separated AES-GCM AAD for one durable secret row. Length prefixes prevent
/// tuple-boundary ambiguity; region is included so moving an envelope across any row-scope
/// dimension fails authentication.
fn secret_row_aad(tenant: &TenantId, region: &Region, secret_id: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(
        SECRET_AAD_DOMAIN.len() + tenant.as_str().len() + region.as_str().len() + secret_id.len() + 12,
    );
    aad.extend_from_slice(SECRET_AAD_DOMAIN);
    for component in [tenant.as_str().as_bytes(), region.as_str().as_bytes(), secret_id.as_bytes()] {
        aad.extend_from_slice(&(component.len() as u32).to_be_bytes());
        aad.extend_from_slice(component);
    }
    aad
}

#[derive(Clone)]
struct StoredSecret {
    key_ref: PiiKeyRef,
    nonce: [u8; NONCE_LEN],
    ciphertext: Vec<u8>,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
struct MemorySecretStore {
    rows: std::collections::BTreeMap<(String, String), StoredSecret>,
    bindings: std::collections::BTreeSet<(String, String, String, String, String)>,
}

#[derive(Clone)]
enum SecretStoreBackend {
    Pg {
        pool: sqlx::PgPool,
        runtime: tokio::runtime::Handle,
    },
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<std::sync::Mutex<MemorySecretStore>>),
}

/// A material-free secret-store failure. No variant carries plaintext, ciphertext, or key material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiSecretStoreError {
    InvalidScope,
    Encrypt,
    Database,
    CorruptCiphertext,
}

impl std::fmt::Display for CiSecretStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidScope => "CI secret scope is invalid",
            Self::Encrypt => "CI secret sealing failed",
            Self::Database => "CI secret durable store is unavailable",
            Self::CorruptCiphertext => "CI secret ciphertext could not be authenticated",
        })
    }
}

impl std::error::Error for CiSecretStoreError {}

/// The durable `ci_secret` store. Production uses PostgreSQL; the in-memory backend is compiled only
/// for unit tests and retains the exact same encrypted-row representation and tenant-DEK cipher.
#[derive(Clone)]
pub struct DurableCiSecretStore {
    backend: SecretStoreBackend,
    kms: Arc<KmsEngine>,
    region: Region,
}

impl DurableCiSecretStore {
    pub fn with_pg(
        pool: sqlx::PgPool,
        kms: Arc<KmsEngine>,
        region: Region,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            backend: SecretStoreBackend::Pg { pool, runtime },
            kms,
            region,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn in_memory(kms: Arc<KmsEngine>, region: Region) -> Self {
        Self {
            backend: SecretStoreBackend::Memory(Arc::new(std::sync::Mutex::new(
                MemorySecretStore::default(),
            ))),
            kms,
            region,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn bind_secret_for_project(
        &self,
        tenant: &TenantId,
        project_id: &str,
        name: &str,
        handle: &str,
    ) {
        let SecretStoreBackend::Memory(state) = &self.backend else {
            panic!("test binding helper requires the in-memory backend")
        };
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .bindings
            .insert((
                tenant.as_str().to_owned(),
                project_id.to_owned(),
                name.to_owned(),
                "project".to_owned(),
                handle.to_owned(),
            ));
    }

    /// Minimal write/seal primitive: provision the tenant hierarchy, encrypt with the existing
    /// tenant-DEK column cipher, then persist only the envelope (`pii_key_ref`, nonce, ciphertext).
    pub fn seal_secret(
        &self,
        tenant: &TenantId,
        secret_id: &str,
        name: &str,
        material: &str,
    ) -> Result<(), CiSecretStoreError> {
        if !strict_secret_segment(tenant.as_str())
            || !strict_secret_segment(secret_id)
            || name.is_empty()
            || name.len() > 128
            || name.chars().any(char::is_control)
            || material.is_empty()
        {
            return Err(CiSecretStoreError::InvalidScope);
        }

        self.kms
            .ensure_kek(&KekId::new(tenant.clone(), self.region.clone()));
        let encrypted = ColumnCryptor::new(&self.kms, self.region.clone())
            .encrypt_with_aad(
                tenant,
                None,
                &ErasureMethod::CryptoShred("tenant_dek".into()),
                material.as_bytes(),
                &secret_row_aad(tenant, &self.region, secret_id),
            )
            .map_err(|_| CiSecretStoreError::Encrypt)?;

        self.store_encrypted(tenant, secret_id, name, encrypted)
    }

    /// Open one exact `(tenant, secret_id)` row. Any absence, malformed envelope, wrong tenant key,
    /// AEAD authentication failure, or invalid UTF-8 is a material-free error/absence for the
    /// capability to convert into a terminal withhold.
    fn resolve_secret(
        &self,
        tenant: &TenantId,
        secret_id: &str,
    ) -> Result<Option<Zeroizing<String>>, CiSecretStoreError> {
        if !strict_secret_segment(tenant.as_str()) || !strict_secret_segment(secret_id) {
            return Err(CiSecretStoreError::InvalidScope);
        }
        let Some(stored) = self.load_encrypted(tenant, secret_id)? else {
            return Ok(None);
        };
        if stored.key_ref.tenant != *tenant || stored.key_ref.class != KeyClass::Tenant {
            return Err(CiSecretStoreError::CorruptCiphertext);
        }
        let column = EncryptedColumn {
            key_ref: stored.key_ref,
            nonce: stored.nonce,
            ciphertext: stored.ciphertext,
        };
        let plaintext = Zeroizing::new(
            ColumnCryptor::new(&self.kms, self.region.clone())
                .decrypt_with_aad(
                    &column,
                    &secret_row_aad(tenant, &self.region, secret_id),
                )
                .map_err(|_| CiSecretStoreError::CorruptCiphertext)?,
        );
        let material = Zeroizing::new(
            std::str::from_utf8(plaintext.as_slice())
                .map_err(|_| CiSecretStoreError::CorruptCiphertext)?
                .to_owned(),
        );
        Ok(Some(material))
    }

    fn store_encrypted(
        &self,
        tenant: &TenantId,
        secret_id: &str,
        name: &str,
        encrypted: EncryptedColumn,
    ) -> Result<(), CiSecretStoreError> {
        let stored = StoredSecret {
            key_ref: encrypted.key_ref,
            nonce: encrypted.nonce,
            ciphertext: encrypted.ciphertext,
        };
        match &self.backend {
            SecretStoreBackend::Pg { pool, runtime } => {
                let pool = pool.clone();
                let tenant_id = tenant.as_str().to_owned();
                let region = self.region.as_str().to_owned();
                let secret_id = secret_id.to_owned();
                let name = name.to_owned();
                let key_ref = stored.key_ref.to_uri();
                let nonce = stored.nonce.to_vec();
                let ciphertext = stored.ciphertext;
                let row_tenant = tenant_id.clone();
                let row_region = region.clone();
                bridge(runtime, async move {
                    with_tenant_tx(&pool, &tenant_id, &region, |connection| {
                        Box::pin(async move {
                            sqlx::query(INSERT_SECRET)
                                .bind(row_tenant)
                                .bind(row_region)
                                .bind(secret_id)
                                .bind(name)
                                .bind(key_ref)
                                .bind(nonce)
                                .bind(ciphertext)
                                .execute(&mut *connection)
                                .await
                                .map_err(|error| {
                                    myelin_storage::PgError::Query(error.to_string())
                                })?;
                            Ok(())
                        })
                    })
                    .await
                })
                .map_err(|_| CiSecretStoreError::Database)
            }
            #[cfg(any(test, feature = "test-support"))]
            SecretStoreBackend::Memory(state) => {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .rows
                    .insert((tenant.as_str().to_owned(), secret_id.to_owned()), stored);
                Ok(())
            }
        }
    }

    fn load_encrypted(
        &self,
        tenant: &TenantId,
        secret_id: &str,
    ) -> Result<Option<StoredSecret>, CiSecretStoreError> {
        match &self.backend {
            SecretStoreBackend::Pg { pool, runtime } => {
                let pool = pool.clone();
                let tenant_id = tenant.as_str().to_owned();
                let region = self.region.as_str().to_owned();
                let secret_id = secret_id.to_owned();
                let row_tenant = tenant_id.clone();
                let row_region = region.clone();
                bridge(runtime, async move {
                    with_tenant_tx(&pool, &tenant_id, &region, |connection| {
                        Box::pin(async move {
                            let row = sqlx::query(SELECT_SECRET)
                                .bind(row_tenant)
                                .bind(row_region)
                                .bind(secret_id)
                                .fetch_optional(&mut *connection)
                                .await
                                .map_err(|error| {
                                    myelin_storage::PgError::Query(error.to_string())
                                })?;
                            Ok(row.map(|row| {
                                (
                                    row.get::<String, _>("pii_key_ref"),
                                    row.get::<Vec<u8>, _>("nonce"),
                                    row.get::<Vec<u8>, _>("ciphertext"),
                                )
                            }))
                        })
                    })
                    .await
                })
                .map_err(|_| CiSecretStoreError::Database)?
                .map(parse_stored_secret)
                .transpose()
            }
            #[cfg(any(test, feature = "test-support"))]
            SecretStoreBackend::Memory(state) => Ok(state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .rows
                .get(&(tenant.as_str().to_owned(), secret_id.to_owned()))
                .cloned()),
        }
    }

    fn is_bound_to_request(
        &self,
        tenant: &TenantId,
        project_id: &str,
        job_id: &str,
        name: &str,
        handle: &str,
    ) -> Result<bool, CiSecretStoreError> {
        let job_scope = format!("job:{job_id}");
        match &self.backend {
            SecretStoreBackend::Pg { pool, runtime } => {
                let pool = pool.clone();
                let tenant_id = tenant.as_str().to_owned();
                let region = self.region.as_str().to_owned();
                let project_id = project_id.to_owned();
                let name = name.to_owned();
                let handle = handle.to_owned();
                let row_tenant = tenant_id.clone();
                let row_region = region.clone();
                bridge(runtime, async move {
                    with_tenant_tx(&pool, &tenant_id, &region, |connection| {
                        Box::pin(async move {
                            sqlx::query(SELECT_SECRET_BINDING)
                                .bind(row_tenant)
                                .bind(row_region)
                                .bind(project_id)
                                .bind(name)
                                .bind(handle)
                                .bind(job_scope)
                                .fetch_one(&mut *connection)
                                .await
                                .map(|row| row.get::<bool, _>("bound"))
                                .map_err(|error| {
                                    myelin_storage::PgError::Query(error.to_string())
                                })
                        })
                    })
                    .await
                })
                .map_err(|_| CiSecretStoreError::Database)
            }
            #[cfg(any(test, feature = "test-support"))]
            SecretStoreBackend::Memory(state) => {
                let state = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                Ok(["project", job_scope.as_str()].iter().any(|scope| {
                    state.bindings.contains(&(
                        tenant.as_str().to_owned(),
                        project_id.to_owned(),
                        name.to_owned(),
                        (*scope).to_owned(),
                        handle.to_owned(),
                    ))
                }))
            }
        }
    }
}

fn parse_stored_secret(
    (key_ref, nonce, ciphertext): (String, Vec<u8>, Vec<u8>),
) -> Result<StoredSecret, CiSecretStoreError> {
    let key_ref = PiiKeyRef::parse(&key_ref).ok_or(CiSecretStoreError::CorruptCiphertext)?;
    let nonce: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| CiSecretStoreError::CorruptCiphertext)?;
    if ciphertext.len() < 16 {
        return Err(CiSecretStoreError::CorruptCiphertext);
    }
    Ok(StoredSecret {
        key_ref,
        nonce,
        ciphertext,
    })
}

fn bridge<F: std::future::Future>(runtime: &tokio::runtime::Handle, future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| runtime.block_on(future)),
        Err(_) => runtime.block_on(future),
    }
}

/// A request-bound production capability. It deliberately repeats the broker's ReBAC check and
/// tenant/object/handle equivalence before touching storage, so direct trait use cannot turn the
/// durable store into a confused deputy.
pub struct DurableSecretCapability<I: IdentityService> {
    store: Arc<DurableCiSecretStore>,
    identity: Arc<I>,
    subject: Principal,
    project_id: String,
    job_id: String,
    consistency: Consistency,
}

impl<I: IdentityService> DurableSecretCapability<I> {
    pub fn new(
        store: Arc<DurableCiSecretStore>,
        identity: Arc<I>,
        subject: Principal,
        project_id: String,
        job_id: String,
        consistency: Consistency,
    ) -> Self {
        Self {
            store,
            identity,
            subject,
            project_id,
            job_id,
            consistency,
        }
    }
}

impl<I: IdentityService> SecretCapability for DurableSecretCapability<I> {
    fn resolve_handle(
        &self,
        tenant: &TenantId,
        authorized_object: &ArtifactRef,
        binding_name: &str,
        handle: &str,
    ) -> Option<Zeroizing<String>> {
        let parsed = parse_canonical_secret_handle(handle)?;
        let authorized = parse_canonical_secret_handle(&authorized_object.0)?;
        if self.subject.tenant != *tenant
            || parsed.tenant != tenant.as_str()
            || authorized.tenant != tenant.as_str()
            || parsed != authorized
            || authorized_object.0 != handle
        {
            return None;
        }
        let decision = self
            .identity
            .check(
                &self.subject,
                &Permission(SECRET_READ_PERMISSION.to_owned()),
                authorized_object,
                &self.consistency,
                None,
            )
            .ok()?;
        if decision != Decision::Allow {
            return None;
        }
        if !self
            .store
            .is_bound_to_request(
                tenant,
                &self.project_id,
                &self.job_id,
                binding_name,
                handle,
            )
            .ok()?
        {
            return None;
        }
        self.store.resolve_secret(tenant, parsed.id).ok().flatten()
    }
}

/// Compose the production claim-time resolver over the durable encrypted store and the same
/// Identity engine used for `secret.read`. The capability is bound to the claim-derived subject for
/// each invocation, then the broker repeats the authorization at its normal boundary.
pub fn durable_ci_job_secret_resolver<I>(
    store: Arc<DurableCiSecretStore>,
    identity: Arc<I>,
    consistency: Consistency,
) -> CiJobSecretResolver
where
    I: IdentityService + Send + Sync + 'static,
{
    Arc::new(move |tenant, spec| {
        let context = match spec.run_token_authorization.as_ref() {
            Some(myelin_ci_sandbox::RunTokenAuthorizationContext::CiJob(context)) => {
                context.clone()
            }
            None => {
                return Err(SecretLaunchError::Authorization(AuthzError::FailClosed(
                    "CI secret resolution requires a claim authorization context".into(),
                )));
            }
        };
        let project_id = context.project_id.clone();
        let job_id = context.job_id.clone();
        let subject = crate::ci_manifest_job_runner::claim_secret_subject(tenant, &context)?;
        let capability = DurableSecretCapability::new(
            store.clone(),
            identity.clone(),
            subject.clone(),
            project_id,
            job_id,
            consistency.clone(),
        );
        SecretBroker::new(&capability, identity.as_ref()).resolve_for_launch(
            spec,
            &subject,
            |secret| ArtifactRef(secret.handle.clone()),
            &consistency,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SecretBroker, SecretLaunchError, WithholdReason};
    use myelin_ci_sandbox::{
        EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget, ResourceLimits,
        RunTokenCredential, SecretRef, TrustTier, WorkspaceSpec,
    };
    use myelin_identity::{
        AuthzError, CaveatContext, ConsistencyMode, Credential, DelegationCaveats, EffectivePolicy,
        FailStaticBound, FragmentAdmit, ListObjectsResult, NamespaceFragment, ObjectId, ObjectType,
        Precondition, PrincipalId, PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace,
        RunId, RunToken, SubjectTree, TupleDelta, Zookie,
    };
    use std::collections::BTreeSet;

    const PROJECT_A: &str = "11111111-1111-4111-8111-111111111111";
    const PROJECT_B: &str = "22222222-2222-4222-8222-222222222222";
    const JOB_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

    #[derive(Default)]
    struct ScopedIdentity {
        grants: BTreeSet<(String, String, String)>,
        checks: std::sync::atomic::AtomicUsize,
    }

    impl ScopedIdentity {
        fn grant(mut self, tenant: &str, subject: &str, object: &str) -> Self {
            self.grants
                .insert((tenant.to_owned(), subject.to_owned(), object.to_owned()));
            self
        }

        fn check_count(&self) -> usize {
            self.checks.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl IdentityService for ScopedIdentity {
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
            assert_eq!(permission.0, SECRET_READ_PERMISSION);
            self.checks
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(
                if self.grants.contains(&(
                    subject.tenant.as_str().to_owned(),
                    subject.principal_id.0.clone(),
                    object.0.clone(),
                )) {
                    Decision::Allow
                } else {
                    Decision::Deny
                },
            )
        }
        fn list_objects(
            &self,
            _subject: &Principal,
            _permission: &Permission,
            _ty: &ObjectType,
            _at: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("test stub"))
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
        fn delegation(
            &self,
            _agent: &Principal,
            _trigger: &Principal,
        ) -> IdResult<EffectivePolicy> {
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
            _agent: &PrincipalId,
            _run: &RunId,
            _caveats: &DelegationCaveats,
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

    fn subject(tenant: &str, id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.to_owned()),
            PrincipalKind::Service,
            TenantId::from_token(tenant),
        )
    }

    fn consistency() -> Consistency {
        Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        }
    }

    fn fixture_store() -> Arc<DurableCiSecretStore> {
        Arc::new(DurableCiSecretStore::in_memory(
            Arc::new(KmsEngine::new()),
            Region::new("eu-test"),
        ))
    }

    fn handle(tenant: &str, id: &str) -> String {
        format!("myelin://{tenant}/ci/secret/{id}")
    }

    fn one_secret_spec(tenant: &str, id: &str) -> JobSpec {
        JobSpec::new(
            JobKind::Ci,
            ImageRef::pinned(format!("registry.invalid/job@sha256:{}", "a".repeat(64))).unwrap(),
            vec!["/bin/true".into()],
            Vec::new(),
            vec![SecretRef {
                name: "DEPLOY_KEY".into(),
                handle: handle(tenant, id),
            }],
            EgressPolicy::deny_all(),
            ResourceLimits {
                cpu_millis: 1000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1024 * 1024 * 1024,
                tmpfs_bytes: 64 * 1024 * 1024,
                pids_max: 64,
                timeout_secs: 30,
            },
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            RunTokenCredential::new("ephemeral", "jti:secret-store", 60).unwrap(),
            MeterTarget {
                reserve_id: "reserve:secret-store".into(),
            },
            IdemToken("idem:secret-store".into()),
        )
        .unwrap()
    }

    #[test]
    fn stored_secret_resolves_decrypts_and_injects_for_authorized_tenant_subject() {
        let store = fixture_store();
        let tenant = tenant("tenant-a");
        let object = handle("tenant-a", "deploy");
        let material = "correct-horse-battery-staple";
        store
            .seal_secret(&tenant, "deploy", "DEPLOY_KEY", material)
            .unwrap();
        store.bind_secret_for_project(&tenant, PROJECT_A, "DEPLOY_KEY", &object);
        let identity = Arc::new(ScopedIdentity::default().grant("tenant-a", "ci-job", &object));
        let capability = DurableSecretCapability::new(
            store,
            identity.clone(),
            subject("tenant-a", "ci-job"),
            PROJECT_A.into(),
            JOB_A.into(),
            consistency(),
        );

        assert_eq!(
            capability
                .resolve_handle(&tenant, &ArtifactRef(object.clone()), "DEPLOY_KEY", &object)
                .as_deref()
                .map(String::as_str),
            Some(material)
        );
        let launched = SecretBroker::new(&capability, identity.as_ref())
            .resolve_for_launch(
                one_secret_spec("tenant-a", "deploy"),
                &subject("tenant-a", "ci-job"),
                |secret| ArtifactRef(secret.handle.clone()),
                &consistency(),
            )
            .expect("stored secret must reach the checked injection bundle");
        assert_eq!(launched.resolved_secret_count(), 1);
        assert!(launched.validate_secret_coverage().is_ok());
        assert!(!format!("{launched:?}").contains(material));
    }

    #[test]
    fn different_tenant_subject_is_denied_by_rebac_without_material() {
        let store = fixture_store();
        let owner = tenant("tenant-a");
        store
            .seal_secret(&owner, "deploy", "DEPLOY_KEY", "tenant-a-material")
            .unwrap();
        let identity = Arc::new(ScopedIdentity::default());
        let requesting_tenant = tenant("tenant-b");
        let requesting_object = handle("tenant-b", "deploy");
        let capability = DurableSecretCapability::new(
            store,
            identity.clone(),
            subject("tenant-b", "ci-job"),
            PROJECT_A.into(),
            JOB_A.into(),
            consistency(),
        );

        assert_eq!(
            capability.resolve_handle(
                &requesting_tenant,
                &ArtifactRef(requesting_object.clone()),
                "DEPLOY_KEY",
                &requesting_object,
            ),
            None
        );
        assert_eq!(
            identity.check_count(),
            1,
            "the foreign tenant subject reaches the direct-reader ReBAC gate and is denied"
        );
    }

    #[test]
    fn cross_tenant_handle_is_refused_against_authorized_object() {
        let store = fixture_store();
        let owner = tenant("tenant-b");
        let foreign = handle("tenant-b", "deploy");
        store
            .seal_secret(&owner, "deploy", "DEPLOY_KEY", "tenant-b-material")
            .unwrap();
        let local_object = handle("tenant-a", "deploy");
        let identity =
            Arc::new(ScopedIdentity::default().grant("tenant-a", "ci-job", &local_object));
        let capability = DurableSecretCapability::new(
            store,
            identity,
            subject("tenant-a", "ci-job"),
            PROJECT_A.into(),
            JOB_A.into(),
            consistency(),
        );

        assert_eq!(
            capability.resolve_handle(
                &tenant("tenant-a"),
                &ArtifactRef(local_object),
                "DEPLOY_KEY",
                &foreign,
            ),
            None
        );
    }

    #[test]
    fn stored_material_is_ciphertext_at_rest_and_never_contains_plaintext() {
        let store = fixture_store();
        let tenant = tenant("tenant-a");
        let material = "plaintext-must-never-rest-here";
        store
            .seal_secret(&tenant, "deploy", "DEPLOY_KEY", material)
            .unwrap();
        let stored = store.load_encrypted(&tenant, "deploy").unwrap().unwrap();

        assert_eq!(stored.key_ref.tenant, tenant);
        assert_eq!(stored.key_ref.class, KeyClass::Tenant);
        assert_ne!(stored.ciphertext, material.as_bytes());
        assert!(!stored
            .ciphertext
            .windows(material.len())
            .any(|window| window == material.as_bytes()));
    }

    #[test]
    fn missing_secret_becomes_observable_terminal_withhold() {
        let store = fixture_store();
        let object = handle("tenant-a", "missing");
        let identity = Arc::new(ScopedIdentity::default().grant("tenant-a", "ci-job", &object));
        store.bind_secret_for_project(&tenant("tenant-a"), PROJECT_A, "DEPLOY_KEY", &object);
        let capability = DurableSecretCapability::new(
            store,
            identity.clone(),
            subject("tenant-a", "ci-job"),
            PROJECT_A.into(),
            JOB_A.into(),
            consistency(),
        );

        let error = SecretBroker::new(&capability, identity.as_ref())
            .resolve_for_launch(
                one_secret_spec("tenant-a", "missing"),
                &subject("tenant-a", "ci-job"),
                |secret| ArtifactRef(secret.handle.clone()),
                &consistency(),
            )
            .expect_err("missing ciphertext must terminate the launch");
        assert!(matches!(
            error,
            SecretLaunchError::Withheld(ref withheld)
                if withheld.len() == 1 && withheld[0].reason == WithholdReason::NotGranted
        ));
        assert_eq!(
            error.to_string(),
            "secret launch withheld: DEPLOY_KEY=not_granted"
        );
    }

    #[test]
    fn decrypt_failure_withholds_without_panic_or_plaintext_leak() {
        let store = fixture_store();
        let tenant = tenant("tenant-a");
        let object = handle("tenant-a", "deploy");
        let material = "must-not-escape-on-auth-failure";
        store
            .seal_secret(&tenant, "deploy", "DEPLOY_KEY", material)
            .unwrap();
        store.bind_secret_for_project(&tenant, PROJECT_A, "DEPLOY_KEY", &object);
        let SecretStoreBackend::Memory(state) = &store.backend else {
            unreachable!("unit fixture is memory-backed")
        };
        state
            .lock()
            .unwrap()
            .rows
            .get_mut(&("tenant-a".into(), "deploy".into()))
            .unwrap()
            .ciphertext[0] ^= 0xff;
        let identity = Arc::new(ScopedIdentity::default().grant("tenant-a", "ci-job", &object));
        let capability = DurableSecretCapability::new(
            store,
            identity.clone(),
            subject("tenant-a", "ci-job"),
            PROJECT_A.into(),
            JOB_A.into(),
            consistency(),
        );

        let error = SecretBroker::new(&capability, identity.as_ref())
            .resolve_for_launch(
                one_secret_spec("tenant-a", "deploy"),
                &subject("tenant-a", "ci-job"),
                |secret| ArtifactRef(secret.handle.clone()),
                &consistency(),
            )
            .expect_err("tampered ciphertext must be withheld");
        let rendered = format!("{error:?} {error}");
        assert!(matches!(error, SecretLaunchError::Withheld(_)));
        assert!(!rendered.contains(material));
    }

    #[test]
    fn ciphertext_copied_to_a_different_secret_row_fails_aad_authentication() {
        let store = fixture_store();
        let tenant = tenant("tenant-a");
        store
            .seal_secret(&tenant, "prod", "PROD_KEY", "prod-material")
            .unwrap();
        store
            .seal_secret(&tenant, "dev", "DEV_KEY", "dev-material")
            .unwrap();
        let SecretStoreBackend::Memory(state) = &store.backend else {
            unreachable!("unit fixture is memory-backed")
        };
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prod = state
            .rows
            .get(&("tenant-a".into(), "prod".into()))
            .cloned()
            .unwrap();
        state.rows.insert(("tenant-a".into(), "dev".into()), prod);
        drop(state);

        assert_eq!(
            store.resolve_secret(&tenant, "dev"),
            Err(CiSecretStoreError::CorruptCiphertext),
            "prod's valid envelope must not authenticate as dev's row"
        );

        let dev_object = handle("tenant-a", "dev");
        store.bind_secret_for_project(&tenant, PROJECT_A, "DEPLOY_KEY", &dev_object);
        let identity = Arc::new(
            ScopedIdentity::default().grant("tenant-a", "ci-job", &dev_object),
        );
        let capability = DurableSecretCapability::new(
            store,
            identity.clone(),
            subject("tenant-a", "ci-job"),
            PROJECT_A.into(),
            JOB_A.into(),
            consistency(),
        );
        let error = SecretBroker::new(&capability, identity.as_ref())
            .resolve_for_launch(
                one_secret_spec("tenant-a", "dev"),
                &subject("tenant-a", "ci-job"),
                |secret| ArtifactRef(secret.handle.clone()),
                &consistency(),
            )
            .expect_err("AAD substitution must withhold the launch");
        assert!(matches!(error, SecretLaunchError::Withheld(_)));
        assert!(!format!("{error:?} {error}").contains("prod-material"));
    }

    #[test]
    fn project_a_job_cannot_resolve_project_b_secret_binding() {
        let store = fixture_store();
        let tenant = tenant("tenant-a");
        let object = handle("tenant-a", "deploy");
        store
            .seal_secret(&tenant, "deploy", "DEPLOY_KEY", "bound-material")
            .unwrap();
        store.bind_secret_for_project(&tenant, PROJECT_B, "DEPLOY_KEY", &object);
        let job_principal = format!("svc:ci:project:{PROJECT_A}:job:{JOB_A}");
        let job_subject = subject("tenant-a", &job_principal);
        let identity = Arc::new(
            ScopedIdentity::default().grant("tenant-a", &job_principal, &object),
        );
        let capability = DurableSecretCapability::new(
            store.clone(),
            identity.clone(),
            job_subject.clone(),
            PROJECT_A.into(),
            JOB_A.into(),
            consistency(),
        );

        let error = SecretBroker::new(&capability, identity.as_ref())
            .resolve_for_launch(
                one_secret_spec("tenant-a", "deploy"),
                &job_subject,
                |secret| ArtifactRef(secret.handle.clone()),
                &consistency(),
            )
            .expect_err("project-A must not consume project-B's binding");
        assert!(matches!(error, SecretLaunchError::Withheld(_)));

        store.bind_secret_for_project(&tenant, PROJECT_A, "DEPLOY_KEY", &object);
        let launched = SecretBroker::new(&capability, identity.as_ref())
            .resolve_for_launch(
                one_secret_spec("tenant-a", "deploy"),
                &job_subject,
                |secret| ArtifactRef(secret.handle.clone()),
                &consistency(),
            )
            .expect("the same job may consume its own project's binding");
        assert_eq!(launched.resolved_secret_count(), 1);
    }

    #[test]
    fn capability_plaintext_type_is_zeroizing_end_to_end() {
        fn assert_zeroizing(_: &Option<Zeroizing<String>>) {}

        let store = fixture_store();
        let tenant = tenant("tenant-a");
        let object = handle("tenant-a", "deploy");
        store
            .seal_secret(&tenant, "deploy", "DEPLOY_KEY", "ephemeral-material")
            .unwrap();
        store.bind_secret_for_project(&tenant, PROJECT_A, "DEPLOY_KEY", &object);
        let identity = Arc::new(ScopedIdentity::default().grant("tenant-a", "ci-job", &object));
        let capability = DurableSecretCapability::new(
            store,
            identity,
            subject("tenant-a", "ci-job"),
            PROJECT_A.into(),
            JOB_A.into(),
            consistency(),
        );
        let material = capability.resolve_handle(
            &tenant,
            &ArtifactRef(object.clone()),
            "DEPLOY_KEY",
            &object,
        );
        assert_zeroizing(&material);
        assert_eq!(material.as_deref().map(String::as_str), Some("ephemeral-material"));
    }
}
