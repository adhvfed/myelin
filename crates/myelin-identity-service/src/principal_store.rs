use myelin_gdpr::PersonalData;
use myelin_identity::{PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{
    KeyClass, KmsEngine, KmsError, OltpHolderRegistration, OltpStoreHolder, PiiKeyRef, TenantQuery,
    TenantScope, TenantTable,
};
use myelin_tenancy::{Region, TenantId};
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

pub const S1_TABLE: &str = "principal";

pub const S1_HOLDER: &str = "identity_principal";

#[derive(PersonalData, Clone, Debug, PartialEq, Eq)]
pub struct PrincipalProfile {
    #[personal_data(
        category = ContactInfo,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id",
    )]
    pub email: String,
    #[personal_data(
        category = ContactInfo,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id",
    )]
    pub display_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrincipalRow {
    pub tenant: TenantId,
    pub region: Region,
    pub principal_id: PrincipalId,
    pub kind: PrincipalKind,
    pub profile_ref: Option<ProfileRef>,
    pub data_role: myelin_identity::DataRole,
    pub status: PrincipalStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileRef {
    pub key_ref: PiiKeyRef,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EncryptedProfile {
    nonce: [u8; myelin_storage::NONCE_LEN],
    ciphertext: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrincipalError {
    InvalidProvisioning,
    CrossTenant {
        detail: String,
    },
    Kms(String),
    CorruptProfile,
    UnknownPrincipal {
        principal_id: String,
    },
    Storage(String),
}

impl core::fmt::Display for PrincipalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PrincipalError::InvalidProvisioning => f.write_str(
                "principal credential provisioning requires non-empty opaque identifiers and a valid scheme",
            ),
            PrincipalError::CrossTenant { detail } => write!(
                f,
                "principal write rejected a cross-tenant row: {detail} (there is no cross-tenant \
                 principal and no cross-tenant query path, identity §1/§2)"
            ),
            PrincipalError::Kms(why) => write!(
                f,
                "principal profile KMS error (the read/write did NOT succeed - never \
                 plaintext-without-key): {why}"
            ),
            PrincipalError::CorruptProfile => write!(
                f,
                "principal profile decrypted to a non-conforming shape (a wrong-key/corrupt open \
                 - refused, never silently coerced)"
            ),
            PrincipalError::UnknownPrincipal { principal_id } => write!(
                f,
                "credential link rejected: principal `{principal_id}` does not exist in the verified \
                 (tenant, region) partition (a dangling SSO/SCIM link is refused)"
            ),
            PrincipalError::Storage(why) => write!(
                f,
                "principal store durable backing error (the read/write did NOT succeed - never a \
                 silent partial write): {why}"
            ),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PrincipalCredentialProvision {
    principal_id: PrincipalId,
    kind: PrincipalKind,
    data_role: myelin_identity::DataRole,
    status: PrincipalStatus,
    scheme: String,
    subject_key: String,
}

impl PrincipalCredentialProvision {
    pub fn new(
        principal_id: PrincipalId,
        kind: PrincipalKind,
        data_role: myelin_identity::DataRole,
        status: PrincipalStatus,
        scheme: impl Into<String>,
        subject_key: impl Into<String>,
    ) -> Result<Self, PrincipalError> {
        let scheme = scheme.into();
        let subject_key = subject_key.into();
        if principal_id.0.trim().is_empty()
            || scheme.trim().is_empty()
            || subject_key.trim().is_empty()
            || scheme.contains('\x1f')
            || subject_key.contains('\x1f')
        {
            return Err(PrincipalError::InvalidProvisioning);
        }
        Ok(Self {
            principal_id,
            kind,
            data_role,
            status,
            scheme,
            subject_key,
        })
    }
}

impl core::fmt::Debug for PrincipalCredentialProvision {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PrincipalCredentialProvision")
            .field("principal_id", &self.principal_id)
            .field("kind", &self.kind)
            .field("data_role", &self.data_role)
            .field("status", &self.status)
            .field("scheme", &self.scheme)
            .field("subject_key", &"<redacted>")
            .finish()
    }
}

impl std::error::Error for PrincipalError {}

impl From<KmsError> for PrincipalError {
    fn from(e: KmsError) -> PrincipalError {
        PrincipalError::Kms(e.to_string())
    }
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
struct Inner {
    partitions: HashMap<(String, String), HashMap<String, PrincipalRow>>,
    profiles: HashMap<(String, String), HashMap<String, EncryptedProfile>>,
    credential_links: HashMap<(String, String), HashMap<String, String>>,
}

#[derive(Clone)]
pub struct PrincipalStore {
    backend: PrincipalBackend,
    kms: Arc<KmsEngine>,
    holder: OltpStoreHolder,
}

#[derive(Clone)]
enum PrincipalBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<Inner>>),
    Pg(PgPrincipalBacking),
}

#[derive(Clone)]
struct PgPrincipalBacking {
    backing: Arc<myelin_storage::DurablePrincipalBacking>,
    rt: tokio::runtime::Handle,
}

impl PrincipalStore {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(kms: Arc<KmsEngine>) -> PrincipalStore {
        let holder = OltpStoreHolder::new(S1_HOLDER);
        let _receipt = holder.register();
        PrincipalStore {
            backend: PrincipalBackend::Memory(Arc::new(Mutex::new(Inner::default()))),
            kms,
            holder,
        }
    }

    pub fn with_pg(
        kms: Arc<KmsEngine>,
        backing: myelin_storage::DurablePrincipalBacking,
        rt: tokio::runtime::Handle,
    ) -> PrincipalStore {
        let holder = OltpStoreHolder::new(S1_HOLDER);
        let _receipt = holder.register();
        PrincipalStore {
            backend: PrincipalBackend::Pg(PgPrincipalBacking {
                backing: Arc::new(backing),
                rt,
            }),
            kms,
            holder,
        }
    }

    pub fn holder(&self) -> &OltpStoreHolder {
        &self.holder
    }

    pub fn register_holder(&self) -> OltpHolderRegistration {
        self.holder.register()
    }

    pub fn subject_dek_class(principal_id: &PrincipalId) -> KeyClass {
        KeyClass::Subject(principal_id.0.clone())
    }

    pub fn tenant_dek_class() -> KeyClass {
        KeyClass::Tenant
    }

    pub fn put_principal(
        &self,
        scope: &TenantScope,
        principal_id: PrincipalId,
        kind: PrincipalKind,
        data_role: myelin_identity::DataRole,
        status: PrincipalStatus,
        profile: Option<&PrincipalProfile>,
    ) -> Result<PrincipalRow, PrincipalError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S1_TABLE));
        #[cfg(any(test, feature = "test-support"))]
        let part_key = Self::part_key(scope);

        let (profile_ref, sealed) = match profile {
            Some(p) => {
                let (key_ref, enc) = self.seal_profile(scope, &principal_id, p)?;
                (Some(ProfileRef { key_ref }), Some(enc))
            }
            None => (None, None),
        };

        let row = PrincipalRow {
            tenant: scope.tenant().clone(),
            region: scope.region().clone(),
            principal_id: principal_id.clone(),
            kind,
            profile_ref,
            data_role,
            status,
        };

        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PrincipalBackend::Memory(inner_arc) => {
                let mut inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                inner
                    .partitions
                    .entry(part_key.clone())
                    .or_default()
                    .insert(principal_id.0.clone(), row.clone());
                if let Some(enc) = sealed {
                    inner
                        .profiles
                        .entry(part_key)
                        .or_default()
                        .insert(principal_id.0.clone(), enc);
                }
                Ok(row)
            }
            PrincipalBackend::Pg(pg) => {
                let blob = match (&row.profile_ref, &sealed) {
                    (Some(pr), Some(enc)) => Some(myelin_storage::DurableProfileBlob {
                        key_ref: pr.key_ref.to_uri(),
                        nonce: enc.nonce.to_vec(),
                        ciphertext: enc.ciphertext.clone(),
                    }),
                    _ => None,
                };
                let drow = myelin_storage::DurablePrincipalRow {
                    principal_id: principal_id.0.clone(),
                    kind: serde_json::to_string(&row.kind).expect("principal.kind serializes"),
                    data_role: serde_json::to_string(&row.data_role)
                        .expect("principal.data_role serializes"),
                    status: serde_json::to_string(&row.status)
                        .expect("principal.status serializes"),
                    profile: blob,
                };
                pg.block(pg.backing.put_principal(&scope.tenant().0, drow))
                    .map_err(|e| PrincipalError::Storage(e.to_string()))?;
                Ok(row)
            }
        }
    }

    pub fn provision_principal_credential(
        &self,
        scope: &TenantScope,
        provision: PrincipalCredentialProvision,
    ) -> Result<PrincipalRow, PrincipalError> {
        let PrincipalCredentialProvision {
            principal_id,
            kind,
            data_role,
            status,
            scheme,
            subject_key,
        } = provision;
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S1_TABLE));
        let row = PrincipalRow {
            tenant: scope.tenant().clone(),
            region: scope.region().clone(),
            principal_id: principal_id.clone(),
            kind,
            profile_ref: None,
            data_role,
            status,
        };
        let link_key = Self::link_key(&scheme, &subject_key);
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PrincipalBackend::Memory(inner_arc) => {
                let part_key = Self::part_key(scope);
                let mut inner = inner_arc.lock().unwrap_or_else(|error| error.into_inner());
                inner
                    .partitions
                    .entry(part_key.clone())
                    .or_default()
                    .insert(principal_id.0.clone(), row.clone());
                inner
                    .credential_links
                    .entry(part_key)
                    .or_default()
                    .insert(link_key, principal_id.0.clone());
                Ok(row)
            }
            PrincipalBackend::Pg(pg) => {
                let durable = myelin_storage::DurablePrincipalRow {
                    principal_id: principal_id.0.clone(),
                    kind: serde_json::to_string(&row.kind).expect("principal.kind serializes"),
                    data_role: serde_json::to_string(&row.data_role)
                        .expect("principal.data_role serializes"),
                    status: serde_json::to_string(&row.status)
                        .expect("principal.status serializes"),
                    profile: None,
                };
                pg.block(pg.backing.put_principal_and_link_credential(
                    &scope.tenant().0,
                    durable,
                    &link_key,
                ))
                .map_err(|error| PrincipalError::Storage(error.to_string()))?;
                Ok(row)
            }
        }
    }

    fn seal_profile(
        &self,
        scope: &TenantScope,
        principal_id: &PrincipalId,
        profile: &PrincipalProfile,
    ) -> Result<(PiiKeyRef, EncryptedProfile), PrincipalError> {
        let kek_id = myelin_storage::KekId::new(scope.tenant().clone(), scope.region().clone());
        self.kms.ensure_kek(&kek_id);
        let key_ref = self.kms.ensure_dek(
            scope.tenant(),
            scope.region(),
            Self::subject_dek_class(principal_id),
        )?;
        let dek = self.kms.resolve_dek(&key_ref, scope.region())?;
        let (nonce, ciphertext) = dek.seal(&Self::profile_bytes(profile));
        Ok((key_ref, EncryptedProfile { nonce, ciphertext }))
    }

    pub fn try_get_principal(
        &self,
        scope: &TenantScope,
        principal_id: &PrincipalId,
    ) -> Result<Option<PrincipalRow>, PrincipalError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S1_TABLE));
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PrincipalBackend::Memory(inner_arc) => {
                let inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                Ok(inner
                    .partitions
                    .get(&Self::part_key(scope))
                    .and_then(|p| p.get(&principal_id.0).cloned()))
            }
            PrincipalBackend::Pg(pg) => pg
                .block(pg.backing.get_principal(&scope.tenant().0, &principal_id.0))
                .map(|row| row.map(|drow| Self::durable_to_row(scope, drow)))
                .map_err(|e| PrincipalError::Storage(e.to_string())),
        }
    }

    pub fn get_principal(
        &self,
        scope: &TenantScope,
        principal_id: &PrincipalId,
    ) -> Option<PrincipalRow> {
        self.try_get_principal(scope, principal_id)
            .unwrap_or_else(|e| panic!("principal store: principal read failed loud: {e}"))
    }

    pub fn get_profile(
        &self,
        scope: &TenantScope,
        principal_id: &PrincipalId,
    ) -> Result<Option<PrincipalProfile>, PrincipalError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S1_TABLE));
        #[cfg(any(test, feature = "test-support"))]
        let part_key = Self::part_key(scope);
        let (key_ref, enc) = match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PrincipalBackend::Memory(inner_arc) => {
                let inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                let row = match inner
                    .partitions
                    .get(&part_key)
                    .and_then(|p| p.get(&principal_id.0))
                {
                    Some(r) => r.clone(),
                    None => return Ok(None),
                };
                let key_ref = match row.profile_ref {
                    Some(pr) => pr.key_ref,
                    None => return Ok(None),
                };
                let enc = match inner
                    .profiles
                    .get(&part_key)
                    .and_then(|p| p.get(&principal_id.0))
                {
                    Some(e) => e.clone(),
                    None => return Ok(None),
                };
                (key_ref, enc)
            }
            PrincipalBackend::Pg(pg) => {
                let drow = match pg
                    .block(pg.backing.get_principal(&scope.tenant().0, &principal_id.0))
                    .map_err(|e| PrincipalError::Storage(e.to_string()))?
                {
                    Some(d) => d,
                    None => return Ok(None),
                };
                let blob = match drow.profile {
                    Some(b) => b,
                    None => return Ok(None),
                };
                let key_ref = PiiKeyRef::parse(&blob.key_ref)
                    .ok_or_else(|| PrincipalError::Storage("malformed profile key_ref".into()))?;
                let mut nonce = [0u8; myelin_storage::NONCE_LEN];
                if blob.nonce.len() != myelin_storage::NONCE_LEN {
                    return Err(PrincipalError::CorruptProfile);
                }
                nonce.copy_from_slice(&blob.nonce);
                (
                    key_ref,
                    EncryptedProfile {
                        nonce,
                        ciphertext: blob.ciphertext,
                    },
                )
            }
        };
        let dek = self.kms.resolve_dek(&key_ref, scope.region())?;
        let plain = dek
            .open(&enc.nonce, &enc.ciphertext)
            .ok_or(PrincipalError::CorruptProfile)?;
        Self::profile_from_bytes(&plain).map(Some)
    }

    pub fn profile_shred_key(
        &self,
        scope: &TenantScope,
        principal_id: &PrincipalId,
    ) -> Option<PiiKeyRef> {
        self.try_profile_shred_key(scope, principal_id)
            .unwrap_or_else(|e| panic!("principal store: erasure-key lookup failed loud: {e}"))
    }

    pub fn try_profile_shred_key(
        &self,
        scope: &TenantScope,
        principal_id: &PrincipalId,
    ) -> Result<Option<PiiKeyRef>, PrincipalError> {
        self.try_get_principal(scope, principal_id)
            .map(|row| row.and_then(|r| r.profile_ref.map(|pr| pr.key_ref)))
    }

    pub fn try_principals_in(
        &self,
        scope: &TenantScope,
    ) -> Result<Vec<PrincipalRow>, PrincipalError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S1_TABLE));
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PrincipalBackend::Memory(inner_arc) => {
                let inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                Ok(inner
                    .partitions
                    .get(&Self::part_key(scope))
                    .map(|p| p.values().cloned().collect())
                    .unwrap_or_default())
            }
            PrincipalBackend::Pg(pg) => pg
                .block(pg.backing.principals_in(&scope.tenant().0))
                .map(|rows| {
                    rows.into_iter()
                        .map(|drow| Self::durable_to_row(scope, drow))
                        .collect()
                })
                .map_err(|e| PrincipalError::Storage(e.to_string())),
        }
    }

    pub fn principals_in(&self, scope: &TenantScope) -> Vec<PrincipalRow> {
        self.try_principals_in(scope)
            .unwrap_or_else(|e| panic!("principal store: principal scan failed loud: {e}"))
    }

    pub fn link_credential(
        &self,
        scope: &TenantScope,
        scheme: &str,
        subject_key: &str,
        principal_id: &PrincipalId,
    ) -> Result<(), PrincipalError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S1_TABLE));
        #[cfg(any(test, feature = "test-support"))]
        let part_key = Self::part_key(scope);
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PrincipalBackend::Memory(inner_arc) => {
                let mut inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                let exists = inner
                    .partitions
                    .get(&part_key)
                    .is_some_and(|p| p.contains_key(&principal_id.0));
                if !exists {
                    return Err(PrincipalError::UnknownPrincipal {
                        principal_id: principal_id.0.clone(),
                    });
                }
                inner
                    .credential_links
                    .entry(part_key)
                    .or_default()
                    .insert(Self::link_key(scheme, subject_key), principal_id.0.clone());
                Ok(())
            }
            PrincipalBackend::Pg(pg) => {
                let linked = pg
                    .block(pg.backing.link_credential(
                        &scope.tenant().0,
                        &Self::link_key(scheme, subject_key),
                        &principal_id.0,
                    ))
                    .map_err(|e| PrincipalError::Storage(e.to_string()))?;
                if !linked {
                    return Err(PrincipalError::UnknownPrincipal {
                        principal_id: principal_id.0.clone(),
                    });
                }
                Ok(())
            }
        }
    }

    pub fn try_resolve_credential(
        &self,
        scope: &TenantScope,
        scheme: &str,
        subject_key: &str,
    ) -> Result<Option<PrincipalRow>, PrincipalError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S1_TABLE));
        #[cfg(any(test, feature = "test-support"))]
        let part_key = Self::part_key(scope);
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PrincipalBackend::Memory(inner_arc) => {
                let inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                let Some(principal_id) = inner
                    .credential_links
                    .get(&part_key)
                    .and_then(|m| m.get(&Self::link_key(scheme, subject_key)))
                else {
                    return Ok(None);
                };
                Ok(inner
                    .partitions
                    .get(&part_key)
                    .and_then(|p| p.get(principal_id).cloned()))
            }
            PrincipalBackend::Pg(pg) => pg
                .block(
                    pg.backing.resolve_credential(
                        &scope.tenant().0,
                        &Self::link_key(scheme, subject_key),
                    ),
                )
                .map(|row| row.map(|drow| Self::durable_to_row(scope, drow)))
                .map_err(|e| PrincipalError::Storage(e.to_string())),
        }
    }

    pub fn resolve_credential(
        &self,
        scope: &TenantScope,
        scheme: &str,
        subject_key: &str,
    ) -> Option<PrincipalRow> {
        self.try_resolve_credential(scope, scheme, subject_key)
            .unwrap_or_else(|e| panic!("principal store: credential lookup failed loud: {e}"))
    }

    fn link_key(scheme: &str, subject_key: &str) -> String {
        format!("{scheme}\x1f{subject_key}")
    }

    fn profile_bytes(profile: &PrincipalProfile) -> Vec<u8> {
        let mut bytes = Vec::new();
        for field in [&profile.email, &profile.display_name] {
            let len = field.len() as u32;
            bytes.extend_from_slice(&len.to_le_bytes());
            bytes.extend_from_slice(field.as_bytes());
        }
        bytes
    }

    fn profile_from_bytes(bytes: &[u8]) -> Result<PrincipalProfile, PrincipalError> {
        let mut cursor = 0usize;
        let mut read_field = || -> Option<String> {
            if cursor + 4 > bytes.len() {
                return None;
            }
            let len = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().ok()?) as usize;
            cursor += 4;
            if cursor + len > bytes.len() {
                return None;
            }
            let s = String::from_utf8(bytes[cursor..cursor + len].to_vec()).ok()?;
            cursor += len;
            Some(s)
        };
        let email = read_field().ok_or(PrincipalError::CorruptProfile)?;
        let display_name = read_field().ok_or(PrincipalError::CorruptProfile)?;
        if cursor != bytes.len() {
            return Err(PrincipalError::CorruptProfile);
        }
        Ok(PrincipalProfile {
            email,
            display_name,
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    fn part_key(scope: &TenantScope) -> (String, String) {
        (scope.tenant().0.clone(), scope.region().0.clone())
    }

    #[cfg(test)]
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        match &self.backend {
            PrincipalBackend::Memory(arc) => arc.lock().unwrap_or_else(|e| e.into_inner()),
            PrincipalBackend::Pg(_) => {
                panic!("lock() is the in-memory test-double accessor; the Pg backend has no map")
            }
        }
    }

    fn durable_to_row(
        scope: &TenantScope,
        drow: myelin_storage::DurablePrincipalRow,
    ) -> PrincipalRow {
        let profile_ref = drow
            .profile
            .as_ref()
            .and_then(|b| PiiKeyRef::parse(&b.key_ref))
            .map(|key_ref| ProfileRef { key_ref });
        PrincipalRow {
            tenant: scope.tenant().clone(),
            region: scope.region().clone(),
            principal_id: PrincipalId(drow.principal_id),
            kind: serde_json::from_str(&drow.kind).expect("principal.kind round-trips"),
            profile_ref,
            data_role: serde_json::from_str(&drow.data_role)
                .expect("principal.data_role round-trips"),
            status: serde_json::from_str(&drow.status).expect("principal.status round-trips"),
        }
    }
}

impl PgPrincipalBacking {
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, Principal};

    fn kms() -> Arc<KmsEngine> {
        Arc::new(KmsEngine::new())
    }

    fn scope(tenant: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn scope_region(tenant: &str, region: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region(region.into()))
    }

    fn profile(email_addr: &str, name: &str) -> PrincipalProfile {
        let email = email_addr.to_string();
        let display_name = name.to_string();
        PrincipalProfile {
            email,
            display_name,
        }
    }

    #[test]
    fn s1_row_round_trips_under_rls() {
        let store = PrincipalStore::new(kms());
        let s = scope("acme");
        let written = store
            .put_principal(
                &s,
                PrincipalId("p:alice".into()),
                PrincipalKind::Human,
                DataRole::Processor,
                PrincipalStatus::Active,
                Some(&profile("alice@acme.test", "Alice")),
            )
            .expect("write");
        assert_eq!(written.principal_id, PrincipalId("p:alice".into()));
        assert!(
            written.profile_ref.is_some(),
            "a profiled principal has an erasable profile_ref"
        );

        let read = store
            .try_get_principal(&s, &PrincipalId("p:alice".into()))
            .expect("principal directory read succeeds")
            .expect("the row round-trips under the same scope");
        assert_eq!(
            read, written,
            "the S1 row round-trips byte-for-byte under RLS"
        );
        assert_eq!(read.kind, PrincipalKind::Human);
        assert_eq!(read.status, PrincipalStatus::Active);
        assert_eq!(
            store.try_principals_in(&s).expect("directory scan succeeds"),
            vec![written.clone()]
        );
        assert_eq!(
            store
                .try_profile_shred_key(&s, &PrincipalId("p:alice".into()))
                .expect("erasure-key read succeeds"),
            written.profile_ref.map(|profile| profile.key_ref)
        );
    }

    #[test]
    fn cross_tenant_read_returns_nothing() {
        let store = PrincipalStore::new(kms());
        let acme = scope("acme");
        let globex = scope("globex");
        store
            .put_principal(
                &acme,
                PrincipalId("p:alice".into()),
                PrincipalKind::Human,
                DataRole::Processor,
                PrincipalStatus::Active,
                Some(&profile("alice@acme.test", "Alice")),
            )
            .expect("acme write");

        assert!(
            store
                .get_principal(&globex, &PrincipalId("p:alice".into()))
                .is_none(),
            "no cross-tenant read path: globex cannot see acme's principal"
        );
        assert!(
            store.principals_in(&globex).is_empty(),
            "globex's partition is empty"
        );
        assert_eq!(store.principals_in(&acme).len(), 1);
    }

    #[test]
    fn cross_region_read_returns_nothing() {
        let store = PrincipalStore::new(kms());
        let eu = scope_region("acme", "eu-west");
        let us = scope_region("acme", "us-east");
        store
            .put_principal(
                &eu,
                PrincipalId("p:alice".into()),
                PrincipalKind::Human,
                DataRole::Processor,
                PrincipalStatus::Active,
                None,
            )
            .expect("eu write");
        assert!(
            store
                .get_principal(&us, &PrincipalId("p:alice".into()))
                .is_none(),
            "residency partition: the us-east partition cannot see the eu-west principal"
        );
        assert_eq!(store.principals_in(&eu).len(), 1);
    }

    #[test]
    fn profile_pii_is_encrypted_under_the_per_subject_sub_key() {
        let store = PrincipalStore::new(kms());
        let s = scope("acme");
        store
            .put_principal(
                &s,
                PrincipalId("p:alice".into()),
                PrincipalKind::Human,
                DataRole::Processor,
                PrincipalStatus::Active,
                Some(&profile("alice@acme.test", "Alice")),
            )
            .expect("write");

        let got = store
            .get_profile(&s, &PrincipalId("p:alice".into()))
            .expect("profile read succeeds")
            .expect("a profile exists");
        assert_eq!(
            got,
            profile("alice@acme.test", "Alice"),
            "the profile decrypts correctly"
        );

        let key_ref = store
            .profile_shred_key(&s, &PrincipalId("p:alice".into()))
            .expect("a profiled principal has a shred key");
        assert_eq!(
            key_ref.class,
            KeyClass::Subject("p:alice".into()),
            "profile PII is keyed under the PER-SUBJECT DEK (GD-4), not the per-tenant DEK"
        );
        assert_ne!(
            key_ref.class,
            PrincipalStore::tenant_dek_class(),
            "the per-subject key is DISTINCT from the per-tenant key (the GD-4 boundary)"
        );
    }

    #[test]
    fn per_subject_key_boundary_a_does_not_open_b() {
        let store = PrincipalStore::new(kms());
        let s = scope("acme");
        store
            .put_principal(
                &s,
                PrincipalId("p:alice".into()),
                PrincipalKind::Human,
                DataRole::Processor,
                PrincipalStatus::Active,
                Some(&profile("alice@acme.test", "Alice")),
            )
            .unwrap();
        let alice_ref = store
            .profile_shred_key(&s, &PrincipalId("p:alice".into()))
            .unwrap();
        store
            .put_principal(
                &s,
                PrincipalId("p:bob".into()),
                PrincipalKind::Human,
                DataRole::Processor,
                PrincipalStatus::Active,
                Some(&profile("bob@acme.test", "Bob")),
            )
            .unwrap();
        let bob_ref = store
            .profile_shred_key(&s, &PrincipalId("p:bob".into()))
            .unwrap();

        assert_ne!(
            alice_ref.class, bob_ref.class,
            "distinct subjects get distinct per-subject DEKs"
        );

        let inner = store.lock();
        let part = (s.tenant().0.clone(), s.region().0.clone());
        let alice_ct = inner
            .profiles
            .get(&part)
            .unwrap()
            .get("p:alice")
            .unwrap()
            .clone();
        drop(inner);
        let bob_dek = store.kms.resolve_dek(&bob_ref, s.region()).unwrap();
        assert!(
            bob_dek
                .open(&alice_ct.nonce, &alice_ct.ciphertext)
                .is_none(),
            "bob's per-subject DEK must NOT open alice's profile ciphertext (the GD-4 boundary)"
        );
    }

    #[test]
    fn principal_id_is_opaque_stable_while_profile_ref_is_separable() {
        let store = PrincipalStore::new(kms());
        let s = scope("acme");
        let machine = store
            .put_principal(
                &s,
                PrincipalId("svc:deploy".into()),
                PrincipalKind::Service,
                DataRole::Controller,
                PrincipalStatus::Active,
                None,
            )
            .unwrap();
        assert_eq!(machine.principal_id, PrincipalId("svc:deploy".into()));
        assert!(
            machine.profile_ref.is_none(),
            "a no-PII principal has no erasable profile_ref"
        );
        assert!(
            store
                .get_profile(&s, &PrincipalId("svc:deploy".into()))
                .unwrap()
                .is_none(),
            "no profile to read for a machine principal"
        );

        let human = store
            .put_principal(
                &s,
                PrincipalId("p:alice".into()),
                PrincipalKind::Human,
                DataRole::Processor,
                PrincipalStatus::Active,
                Some(&profile("alice@acme.test", "Alice")),
            )
            .unwrap();
        let ref1 = human.profile_ref.clone().expect("a profile_ref");
        let rewritten = store
            .put_principal(
                &s,
                PrincipalId("p:alice".into()),
                PrincipalKind::Human,
                DataRole::Processor,
                PrincipalStatus::Active,
                Some(&profile("alice2@acme.test", "Alice A.")),
            )
            .unwrap();
        assert_eq!(
            rewritten.principal_id, human.principal_id,
            "the opaque principal_id is STABLE across a profile update (immutable attribution)"
        );
        assert_eq!(
            rewritten.profile_ref.unwrap().key_ref.class,
            ref1.key_ref.class,
            "the erasable profile_ref points at the same stable per-subject key (the §X-7 split)"
        );
        assert_eq!(
            store
                .get_profile(&s, &PrincipalId("p:alice".into()))
                .unwrap()
                .unwrap(),
            profile("alice2@acme.test", "Alice A."),
            "the profile update is durable"
        );
    }

    #[test]
    fn crypto_shredded_profile_read_fails_loud_not_open() {
        let store = PrincipalStore::new(kms());
        let s = scope("acme");
        store
            .put_principal(
                &s,
                PrincipalId("p:alice".into()),
                PrincipalKind::Human,
                DataRole::Processor,
                PrincipalStatus::Active,
                Some(&profile("alice@acme.test", "Alice")),
            )
            .unwrap();
        let key_ref = store
            .profile_shred_key(&s, &PrincipalId("p:alice".into()))
            .unwrap();
        let dek_id = myelin_storage::DekId::new(key_ref.tenant.clone(), key_ref.class.clone());
        assert!(
            store.kms.destroy_dek(&dek_id),
            "the per-subject DEK is destroyed (crypto-shred)"
        );

        let r = store.get_profile(&s, &PrincipalId("p:alice".into()));
        assert!(
            matches!(r, Err(PrincipalError::Kms(_))),
            "a crypto-shredded profile read fails loud (KmsError), never plaintext-without-key"
        );
        assert!(
            store
                .get_principal(&s, &PrincipalId("p:alice".into()))
                .is_some(),
            "the opaque principal_id row survives the profile shred (immutable attribution)"
        );
    }

    #[test]
    fn s1_store_registers_as_a_personal_data_holder() {
        let store = PrincipalStore::new(kms());
        assert_eq!(
            store.holder().store,
            S1_HOLDER,
            "the S1 store registered under its holder name"
        );
        let receipt = store.register_holder();
        assert_eq!(receipt.store, S1_HOLDER);
    }

    #[test]
    fn orgs_teams_projects_are_principal_rows() {
        let store = PrincipalStore::new(kms());
        let s = scope("acme");
        store
            .put_principal(
                &s,
                PrincipalId("org:acme".into()),
                PrincipalKind::Service,
                DataRole::Controller,
                PrincipalStatus::Active,
                None,
            )
            .unwrap();
        store
            .put_principal(
                &s,
                PrincipalId("p:alice".into()),
                PrincipalKind::Human,
                DataRole::Processor,
                PrincipalStatus::Active,
                Some(&profile("alice@acme.test", "Alice")),
            )
            .unwrap();
        let rows = store.principals_in(&s);
        assert_eq!(
            rows.len(),
            2,
            "the org-principal and the human principal coexist in S1"
        );
        assert!(rows
            .iter()
            .any(|r| r.principal_id == PrincipalId("org:acme".into())));
    }

    #[test]
    fn corrupt_or_truncated_profile_is_refused() {
        assert_eq!(
            PrincipalStore::profile_from_bytes(&[0u8, 0, 0]),
            Err(PrincipalError::CorruptProfile),
            "a 3-byte buffer (< one length header) is refused"
        );
        let two_empty = vec![0u8, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(
            PrincipalStore::profile_from_bytes(&two_empty),
            Ok(PrincipalProfile {
                email: String::new(),
                display_name: String::new()
            }),
            "a buffer ending exactly at the last header boundary parses (the == boundary succeeds)"
        );
        let overrun = vec![10u8, 0, 0, 0, b'a', b'b'];
        assert_eq!(
            PrincipalStore::profile_from_bytes(&overrun),
            Err(PrincipalError::CorruptProfile),
            "a field length running past the buffer is refused"
        );
        let mut trailing = PrincipalStore::profile_bytes(&profile("a@b.test", "Ab"));
        trailing.push(0xFF);
        assert_eq!(
            PrincipalStore::profile_from_bytes(&trailing),
            Err(PrincipalError::CorruptProfile),
            "trailing bytes after the two fields are a non-conforming shape (refused)"
        );
        let bytes = PrincipalStore::profile_bytes(&profile("a@b.test", "Ab"));
        assert_eq!(
            PrincipalStore::profile_from_bytes(&bytes),
            Ok(profile("a@b.test", "Ab")),
            "the canonical profile bytes round-trip"
        );
    }

    #[test]
    fn s1_profile_compiles_with_personal_data_tags() {
        let email = "alice@acme.test".to_string();
        let display_name = "Alice".to_string();
        let p = PrincipalProfile {
            email,
            display_name,
        };
        assert_eq!(p.email, "alice@acme.test");
        assert_eq!(p.display_name, "Alice");
    }

    #[test]
    fn bootstrap_provisioning_commits_principal_and_credential_as_one_operation() {
        let store = PrincipalStore::new(kms());
        let scope = scope("acme");
        let principal_id = PrincipalId("human:mcp-operator".into());
        let provision = PrincipalCredentialProvision::new(
            principal_id.clone(),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
            "agent",
            "human:mcp-operator",
        )
        .unwrap();
        store
            .provision_principal_credential(&scope, provision)
            .unwrap();
        assert_eq!(
            store
                .try_resolve_credential(&scope, "agent", "human:mcp-operator")
                .expect("credential directory read succeeds")
                .unwrap()
                .principal_id,
            principal_id
        );
        assert!(
            store
                .try_resolve_credential(&scope, "agent", "missing")
                .expect("an absent link is not a read fault")
                .is_none(),
            "a genuine absence remains distinguishable from storage failure"
        );
    }

    #[test]
    fn provisioning_request_validates_link_components_and_redacts_subject() {
        let provision = PrincipalCredentialProvision::new(
            PrincipalId("human:operator".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
            "oidc",
            "external|sensitive-subject",
        )
        .unwrap();
        let debug = format!("{provision:?}");
        assert!(!debug.contains("external|sensitive-subject"));
        assert!(debug.contains("subject_key: \"<redacted>\""));

        assert_eq!(
            PrincipalCredentialProvision::new(
                PrincipalId("human:operator".into()),
                PrincipalKind::Human,
                DataRole::Controller,
                PrincipalStatus::Active,
                "oidc\x1fconfused",
                "subject",
            ),
            Err(PrincipalError::InvalidProvisioning)
        );
        assert_eq!(
            PrincipalCredentialProvision::new(
                PrincipalId("human:operator".into()),
                PrincipalKind::Human,
                DataRole::Controller,
                PrincipalStatus::Active,
                "oidc",
                " ",
            ),
            Err(PrincipalError::InvalidProvisioning)
        );
    }
}
