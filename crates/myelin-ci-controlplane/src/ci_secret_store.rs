//! Durable, tenant-DEK-sealed CI secret material and the production broker capability.
//!
//! `secret_binding` remains names-and-handles only. The material lives in `ci_secret` exclusively as
//! Storage's existing [`myelin_storage::EncryptedColumn`] envelope. Reads and writes use the shared
//! tenant-scoped transaction convention, while the synchronous broker trait bridges onto the service
//! runtime in the same way as the other durable stores.

use std::sync::Arc;

use myelin_gdpr::ErasureMethod;
use myelin_identity::{
    AuthzError, Consistency, DataRole, Decision, IdentityService, Permission, Principal,
    PrincipalId, PrincipalKind, PrincipalStatus,
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

#[derive(Clone)]
struct StoredSecret {
    key_ref: PiiKeyRef,
    nonce: [u8; NONCE_LEN],
    ciphertext: Vec<u8>,
}

#[derive(Clone)]
enum SecretStoreBackend {
    Pg {
        pool: sqlx::PgPool,
        runtime: tokio::runtime::Handle,
    },
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<std::sync::Mutex<std::collections::BTreeMap<(String, String), StoredSecret>>>),
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
                std::collections::BTreeMap::new(),
            ))),
            kms,
            region,
        }
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
            .encrypt(
                tenant,
                None,
                &ErasureMethod::CryptoShred("tenant_dek".into()),
                material.as_bytes(),
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
    ) -> Result<Option<String>, CiSecretStoreError> {
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
                .decrypt(&column)
                .map_err(|_| CiSecretStoreError::CorruptCiphertext)?,
        );
        let material = std::str::from_utf8(plaintext.as_slice())
            .map_err(|_| CiSecretStoreError::CorruptCiphertext)?
            .to_owned();
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
            SecretStoreBackend::Memory(rows) => {
                rows.lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
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
            SecretStoreBackend::Memory(rows) => Ok(rows
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&(tenant.as_str().to_owned(), secret_id.to_owned()))
                .cloned()),
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
    consistency: Consistency,
}

impl<I: IdentityService> DurableSecretCapability<I> {
    pub fn new(
        store: Arc<DurableCiSecretStore>,
        identity: Arc<I>,
        subject: Principal,
        consistency: Consistency,
    ) -> Self {
        Self {
            store,
            identity,
            subject,
            consistency,
        }
    }
}

impl<I: IdentityService> SecretCapability for DurableSecretCapability<I> {
    fn resolve_handle(
        &self,
        tenant: &TenantId,
        authorized_object: &ArtifactRef,
        handle: &str,
    ) -> Option<String> {
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
        if context.tenant_id != tenant.as_str() {
            return Err(SecretLaunchError::Authorization(AuthzError::FailClosed(
                "CI secret resolution tenant does not match the authorized claim".into(),
            )));
        }
        let subject = Principal::new(
            tenant.clone(),
            Region::new(context.region),
            PrincipalId(context.principal_id),
            PrincipalKind::Service,
            DataRole::Processor,
            PrincipalStatus::Active,
        );
        let capability = DurableSecretCapability::new(
            store.clone(),
            identity.clone(),
            subject.clone(),
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
        let identity = Arc::new(ScopedIdentity::default().grant("tenant-a", "ci-job", &object));
        let capability = DurableSecretCapability::new(
            store,
            identity.clone(),
            subject("tenant-a", "ci-job"),
            consistency(),
        );

        assert_eq!(
            capability.resolve_handle(&tenant, &ArtifactRef(object.clone()), &object),
            Some(material.to_owned())
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
            consistency(),
        );

        assert_eq!(
            capability.resolve_handle(
                &requesting_tenant,
                &ArtifactRef(requesting_object.clone()),
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
            consistency(),
        );

        assert_eq!(
            capability.resolve_handle(&tenant("tenant-a"), &ArtifactRef(local_object), &foreign,),
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
        let capability = DurableSecretCapability::new(
            store,
            identity.clone(),
            subject("tenant-a", "ci-job"),
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
        let SecretStoreBackend::Memory(rows) = &store.backend else {
            unreachable!("unit fixture is memory-backed")
        };
        rows.lock()
            .unwrap()
            .get_mut(&("tenant-a".into(), "deploy".into()))
            .unwrap()
            .ciphertext[0] ^= 0xff;
        let identity = Arc::new(ScopedIdentity::default().grant("tenant-a", "ci-job", &object));
        let capability = DurableSecretCapability::new(
            store,
            identity.clone(),
            subject("tenant-a", "ci-job"),
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
}
