use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use aes_gcm::aead::{Aead, OsRng, Payload};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};

use myelin_tenancy::{Region, TenantId};

use crate::kms_durable::DurableKms;

pub const KEY_LEN: usize = 32;

pub const NONCE_LEN: usize = 12;

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum KeyClass {
    Tenant,
    Subject(String),
    Blob,
}

impl KeyClass {
    pub fn as_token(&self) -> String {
        match self {
            KeyClass::Tenant => "tenant".to_string(),
            KeyClass::Subject(id) => format!("subject:{id}"),
            KeyClass::Blob => "blob".to_string(),
        }
    }

    pub fn parse_token(s: &str) -> Option<KeyClass> {
        match s {
            "tenant" => Some(KeyClass::Tenant),
            "blob" => Some(KeyClass::Blob),
            other => other.strip_prefix("subject:").and_then(|id| {
                if id.is_empty() {
                    None
                } else {
                    Some(KeyClass::Subject(id.to_string()))
                }
            }),
        }
    }
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct PiiKeyRef {
    pub tenant: TenantId,
    pub dek_epoch: u64,
    pub class: KeyClass,
}

impl PiiKeyRef {
    pub fn new(tenant: TenantId, dek_epoch: u64, class: KeyClass) -> PiiKeyRef {
        PiiKeyRef {
            tenant,
            dek_epoch,
            class,
        }
    }

    pub fn to_uri(&self) -> String {
        format!(
            "kms://{}/{}/{}",
            self.tenant.as_str(),
            self.dek_epoch,
            self.class.as_token()
        )
    }

    pub fn parse(uri: &str) -> Option<PiiKeyRef> {
        let rest = uri.strip_prefix("kms://")?;
        let mut parts = rest.split('/');
        let tenant = parts.next()?;
        let epoch = parts.next()?;
        let class = parts.next()?;
        if tenant.is_empty() || parts.next().is_some() {
            return None;
        }
        let dek_epoch: u64 = epoch.parse().ok()?;
        let class = KeyClass::parse_token(class)?;
        let parsed = PiiKeyRef {
            tenant: TenantId(tenant.to_string()),
            dek_epoch,
            class,
        };
        (parsed.to_uri() == uri).then_some(parsed)
    }
}

impl fmt::Display for PiiKeyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_uri())
    }
}

#[derive(Clone, PartialEq, Eq)]
struct RawKey([u8; KEY_LEN]);

impl RawKey {
    fn generate() -> RawKey {
        let key = Aes256Gcm::generate_key(OsRng);
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(key.as_slice());
        RawKey(bytes)
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.0))
    }
}

impl fmt::Debug for RawKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RawKey(<redacted 256-bit key>)")
    }
}

#[derive(Debug)]
pub struct CellRoot {
    root: RawKey,
}

impl Default for CellRoot {
    fn default() -> Self {
        Self::generate()
    }
}

impl CellRoot {
    pub fn generate() -> CellRoot {
        CellRoot {
            root: RawKey::generate(),
        }
    }

    fn wrap_kek(&self, kek_plain: &RawKey) -> WrappedKey {
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let ct = self
            .root
            .cipher()
            .encrypt(&nonce, kek_plain.0.as_slice())
            .expect("AES-256-GCM wrap KEK under root");
        let mut n = [0u8; NONCE_LEN];
        n.copy_from_slice(nonce.as_slice());
        WrappedKey {
            nonce: n,
            wrapped: ct,
        }
    }

    fn unwrap_kek(&self, w: &WrappedKey) -> Option<RawKey> {
        let plain = self
            .root
            .cipher()
            .decrypt(Nonce::from_slice(&w.nonce), w.wrapped.as_slice())
            .ok()?;
        if plain.len() != KEY_LEN {
            return None;
        }
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(&plain);
        Some(RawKey(bytes))
    }

    pub fn seal(&self, seal_key: &SealKey) -> SealedRoot {
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let ct = seal_key
            .cipher()
            .encrypt(&nonce, self.root.0.as_slice())
            .expect("AES-256-GCM seal cell root under the seal key");
        let mut n = [0u8; NONCE_LEN];
        n.copy_from_slice(nonce.as_slice());
        SealedRoot {
            nonce: n,
            ciphertext: ct,
        }
    }

    pub fn unseal(seal_key: &SealKey, sealed: &SealedRoot) -> Option<CellRoot> {
        let plain = seal_key
            .cipher()
            .decrypt(
                Nonce::from_slice(&sealed.nonce),
                sealed.ciphertext.as_slice(),
            )
            .ok()?;
        if plain.len() != KEY_LEN {
            return None;
        }
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(&plain);
        Some(CellRoot {
            root: RawKey(bytes),
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SealKey(RawKey);

impl SealKey {
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> SealKey {
        SealKey(RawKey(bytes))
    }

    pub fn from_encoded(s: &str) -> Result<SealKey, SealKeyError> {
        let decoded = hex::decode(s.trim()).map_err(|e| SealKeyError::Decode(e.to_string()))?;
        if decoded.len() != KEY_LEN {
            return Err(SealKeyError::WrongLength(decoded.len()));
        }
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(&decoded);
        Ok(SealKey(RawKey(bytes)))
    }

    pub fn derive_service_key(&self, context: &str) -> zeroize::Zeroizing<[u8; KEY_LEN]> {
        zeroize::Zeroizing::new(blake3::derive_key(context, &self.0 .0))
    }

    fn cipher(&self) -> Aes256Gcm {
        self.0.cipher()
    }

    pub fn seal_bytes(&self, plaintext: &[u8]) -> ([u8; NONCE_LEN], Vec<u8>) {
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let ct = self
            .cipher()
            .encrypt(&nonce, plaintext)
            .expect("AES-256-GCM seal arbitrary bytes under the seal key");
        let mut n = [0u8; NONCE_LEN];
        n.copy_from_slice(nonce.as_slice());
        (n, ct)
    }

    pub fn open_bytes(&self, nonce: &[u8; NONCE_LEN], ciphertext: &[u8]) -> Option<Vec<u8>> {
        self.cipher()
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .ok()
    }
}

impl fmt::Debug for SealKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SealKey(<redacted seal key>)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SealKeyError {
    Decode(String),
    WrongLength(usize),
}

impl fmt::Display for SealKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SealKeyError::Decode(e) => write!(
                f,
                "KMS seal key is not valid hex (a 256-bit key as 64 hex chars is required): {e}"
            ),
            SealKeyError::WrongLength(n) => write!(
                f,
                "KMS seal key decoded to {n} bytes; a 256-bit (32-byte) key is required"
            ),
        }
    }
}

impl std::error::Error for SealKeyError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedRoot {
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WrappedKey {
    nonce: [u8; NONCE_LEN],
    wrapped: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KekId {
    pub tenant: TenantId,
    pub region: Region,
}

impl KekId {
    pub fn new(tenant: TenantId, region: Region) -> KekId {
        KekId { tenant, region }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DekId {
    pub tenant: TenantId,
    pub class: KeyClass,
}

impl DekId {
    pub fn new(tenant: TenantId, class: KeyClass) -> DekId {
        DekId { tenant, class }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrappedDek {
    pub nonce: [u8; NONCE_LEN],
    pub wrapped: Vec<u8>,
    pub kek_epoch: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct DekHandle {
    key: RawKey,
}

impl DekHandle {
    pub fn from_raw(bytes: [u8; KEY_LEN]) -> DekHandle {
        DekHandle { key: RawKey(bytes) }
    }

    pub fn seal(&self, plaintext: &[u8]) -> ([u8; NONCE_LEN], Vec<u8>) {
        self.seal_with_aad(plaintext, &[])
    }

    pub fn seal_with_aad(&self, plaintext: &[u8], aad: &[u8]) -> ([u8; NONCE_LEN], Vec<u8>) {
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let ct = self
            .key
            .cipher()
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .expect("AES-256-GCM seal");
        let mut n = [0u8; NONCE_LEN];
        n.copy_from_slice(nonce.as_slice());
        (n, ct)
    }

    pub fn open(&self, nonce: &[u8; NONCE_LEN], ciphertext: &[u8]) -> Option<Vec<u8>> {
        self.open_with_aad(nonce, ciphertext, &[])
    }

    pub fn open_with_aad(
        &self,
        nonce: &[u8; NONCE_LEN],
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Option<Vec<u8>> {
        self.key
            .cipher()
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .ok()
    }
}

impl fmt::Debug for DekHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DekHandle(<redacted resolved DEK>)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KmsError {
    KekUnavailable(KekId),
    DekUnavailable(DekId),
    UnwrapFailed(DekId),
    Durability(String),
}

impl fmt::Display for KmsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KmsError::KekUnavailable(id) => write!(
                f,
                "KMS: no KEK for tenant={} region={} (never created, or crypto-shredded - \
                 a DEK under a destroyed KEK is unrecoverable; this is the loud failure, NOT \
                 a plaintext fall-through)",
                id.tenant.as_str(),
                id.region.as_str()
            ),
            KmsError::DekUnavailable(id) => write!(
                f,
                "KMS: no DEK for tenant={} class={} (never created, or per-subject \
                 crypto-shredded - the subject ciphertext is unrecoverable)",
                id.tenant.as_str(),
                id.class.as_token()
            ),
            KmsError::UnwrapFailed(id) => write!(
                f,
                "KMS: wrapped DEK for tenant={} class={} failed to authenticate under its KEK \
                 (tamper / wrong KEK) - refused, NEVER a silent wrong-key unwrap",
                id.tenant.as_str(),
                id.class.as_token()
            ),
            KmsError::Durability(e) => write!(
                f,
                "KMS: durable write-through FAILED - the key operation was rolled back and refused \
                 (a key that does not survive a restart is never handed out; SI-006): {e}"
            ),
        }
    }
}

impl std::error::Error for KmsError {}

struct StoredKek {
    wrapped: WrappedKey,
    epoch: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportedKek {
    pub nonce: [u8; NONCE_LEN],
    pub wrapped: Vec<u8>,
    pub epoch: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KmsDurableSnapshot {
    pub sealed_root: SealedRoot,
    pub keks: Vec<(KekId, ExportedKek)>,
    pub deks: Vec<(DekId, WrappedDek, u64)>,
}

pub(crate) struct KmsCore {
    root: CellRoot,
    keks: Mutex<BTreeMap<KekId, StoredKek>>,
    deks: Mutex<BTreeMap<DekId, (WrappedDek, u64)>>,
}

impl KmsCore {
    #[cfg(any(test, feature = "test-support"))]
    pub fn fresh() -> KmsCore {
        KmsCore::from_root(CellRoot::generate())
    }

    pub fn from_root(root: CellRoot) -> KmsCore {
        KmsCore {
            root,
            keks: Mutex::new(BTreeMap::new()),
            deks: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn install_wrapped_kek(
        &self,
        id: KekId,
        nonce: [u8; NONCE_LEN],
        wrapped: Vec<u8>,
        epoch: u64,
    ) {
        let mut keks = self.keks.lock().expect("KMS keks poisoned");
        keks.insert(
            id,
            StoredKek {
                wrapped: WrappedKey { nonce, wrapped },
                epoch,
            },
        );
    }

    pub fn install_wrapped_dek(&self, id: DekId, dek: WrappedDek, dek_epoch: u64) {
        let mut deks = self.deks.lock().expect("KMS deks poisoned");
        deks.insert(id, (dek, dek_epoch));
    }

    pub fn export_sealed_root(&self, seal_key: &SealKey) -> SealedRoot {
        self.root.seal(seal_key)
    }

    pub fn export_kek(&self, id: &KekId) -> Option<ExportedKek> {
        let keks = self.keks.lock().expect("KMS keks poisoned");
        keks.get(id).map(|sk| ExportedKek {
            nonce: sk.wrapped.nonce,
            wrapped: sk.wrapped.wrapped.clone(),
            epoch: sk.epoch,
        })
    }

    pub fn export_dek(&self, id: &DekId) -> Option<(WrappedDek, u64)> {
        let deks = self.deks.lock().expect("KMS deks poisoned");
        deks.get(id).map(|(w, e)| (w.clone(), *e))
    }

    pub fn export_deks(&self) -> Vec<(DekId, WrappedDek, u64)> {
        let deks = self.deks.lock().expect("KMS deks poisoned");
        deks.iter()
            .map(|(id, (w, e))| (id.clone(), w.clone(), *e))
            .collect()
    }

    pub fn ensure_kek(&self, id: &KekId) -> u64 {
        self.ensure_kek_tracked(id).0
    }

    pub fn ensure_kek_tracked(&self, id: &KekId) -> (u64, bool) {
        let mut keks = self.keks.lock().expect("KMS keks poisoned");
        if let Some(existing) = keks.get(id) {
            return (existing.epoch, false);
        }
        let wrapped = self.root.wrap_kek(&RawKey::generate());
        keks.insert(id.clone(), StoredKek { wrapped, epoch: 0 });
        (0, true)
    }

    fn open_kek(&self, id: &KekId) -> Result<RawKey, KmsError> {
        let keks = self.keks.lock().expect("KMS keks poisoned");
        let kek = keks
            .get(id)
            .ok_or_else(|| KmsError::KekUnavailable(id.clone()))?;
        self.root
            .unwrap_kek(&kek.wrapped)
            .ok_or_else(|| KmsError::KekUnavailable(id.clone()))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn ensure_dek(
        &self,
        tenant: &TenantId,
        region: &Region,
        class: KeyClass,
    ) -> Result<PiiKeyRef, KmsError> {
        self.ensure_dek_tracked(tenant, region, class)
            .map(|(k, _)| k)
    }

    pub fn ensure_dek_tracked(
        &self,
        tenant: &TenantId,
        region: &Region,
        class: KeyClass,
    ) -> Result<(PiiKeyRef, bool), KmsError> {
        let kek_id = KekId::new(tenant.clone(), region.clone());
        let dek_id = DekId::new(tenant.clone(), class.clone());
        {
            let deks = self.deks.lock().expect("KMS deks poisoned");
            if let Some((_, dek_epoch)) = deks.get(&dek_id) {
                return Ok((PiiKeyRef::new(tenant.clone(), *dek_epoch, class), false));
            }
        }
        let dek_plain = RawKey::generate();
        let wrapped = self.wrap_dek(&kek_id, &dek_plain)?;
        let mut deks = self.deks.lock().expect("KMS deks poisoned");
        if let Some((_, dek_epoch)) = deks.get(&dek_id) {
            return Ok((PiiKeyRef::new(tenant.clone(), *dek_epoch, class), false));
        }
        let dek_epoch = 0u64;
        deks.insert(dek_id, (wrapped, dek_epoch));
        Ok((PiiKeyRef::new(tenant.clone(), dek_epoch, class), true))
    }

    fn wrap_dek(&self, kek_id: &KekId, dek_plain: &RawKey) -> Result<WrappedDek, KmsError> {
        let kek = self.open_kek(kek_id)?;
        let kek_epoch = {
            let keks = self.keks.lock().expect("KMS keks poisoned");
            keks.get(kek_id)
                .map(|k| k.epoch)
                .ok_or_else(|| KmsError::KekUnavailable(kek_id.clone()))?
        };
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let wrapped = kek
            .cipher()
            .encrypt(&nonce, dek_plain.0.as_slice())
            .expect("AES-256-GCM wrap");
        let mut n = [0u8; NONCE_LEN];
        n.copy_from_slice(nonce.as_slice());
        Ok(WrappedDek {
            nonce: n,
            wrapped,
            kek_epoch,
        })
    }

    pub fn resolve_dek(&self, key_ref: &PiiKeyRef, region: &Region) -> Result<DekHandle, KmsError> {
        let kek_id = KekId::new(key_ref.tenant.clone(), region.clone());
        let dek_id = DekId::new(key_ref.tenant.clone(), key_ref.class.clone());

        let deks = self.deks.lock().expect("KMS deks poisoned");
        let (wrapped, _epoch) = deks
            .get(&dek_id)
            .ok_or_else(|| KmsError::DekUnavailable(dek_id.clone()))?;
        let wrapped = wrapped.clone();
        drop(deks);

        let kek = self.open_kek(&kek_id)?;
        let plain = kek
            .cipher()
            .decrypt(
                Nonce::from_slice(&wrapped.nonce),
                wrapped.wrapped.as_slice(),
            )
            .map_err(|_| KmsError::UnwrapFailed(dek_id.clone()))?;
        if plain.len() != KEY_LEN {
            return Err(KmsError::UnwrapFailed(dek_id));
        }
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(&plain);
        Ok(DekHandle { key: RawKey(bytes) })
    }

    pub fn rotate_kek(&self, id: &KekId) -> Result<u64, KmsError> {
        let dek_ids: Vec<DekId> = {
            let deks = self.deks.lock().expect("KMS deks poisoned");
            deks.keys()
                .filter(|d| d.tenant == id.tenant)
                .cloned()
                .collect()
        };
        let mut plains: Vec<(DekId, RawKey)> = Vec::with_capacity(dek_ids.len());
        for dek_id in &dek_ids {
            let key_ref = PiiKeyRef::new(id.tenant.clone(), 0, dek_id.class.clone());
            let handle = self.resolve_dek(&key_ref, &id.region)?;
            plains.push((dek_id.clone(), handle.key));
        }

        let new_wrapped = self.root.wrap_kek(&RawKey::generate());
        let new_epoch = {
            let mut keks = self.keks.lock().expect("KMS keks poisoned");
            let kek = keks
                .get_mut(id)
                .ok_or_else(|| KmsError::KekUnavailable(id.clone()))?;
            kek.wrapped = new_wrapped;
            kek.epoch += 1;
            kek.epoch
        };

        for (dek_id, plain) in plains {
            let wrapped = self.wrap_dek(id, &plain)?;
            let mut deks = self.deks.lock().expect("KMS deks poisoned");
            if let Some((slot, dek_epoch)) = deks.get_mut(&dek_id) {
                *slot = wrapped;
                *dek_epoch += 1;
            }
        }
        Ok(new_epoch)
    }

    pub fn destroy_kek(&self, id: &KekId) -> bool {
        let mut keks = self.keks.lock().expect("KMS keks poisoned");
        keks.remove(id).is_some()
    }

    pub fn destroy_dek(&self, id: &DekId) -> bool {
        let mut deks = self.deks.lock().expect("KMS deks poisoned");
        deks.remove(id).is_some()
    }

    pub fn backup_snapshot(&self) -> Vec<(DekId, WrappedDek)> {
        let keks = self.keks.lock().expect("KMS keks poisoned");
        let deks = self.deks.lock().expect("KMS deks poisoned");
        deks.iter()
            .filter(|(dek_id, _)| keks.keys().any(|k| k.tenant == dek_id.tenant))
            .map(|(dek_id, (wrapped, _epoch))| (dek_id.clone(), wrapped.clone()))
            .collect()
    }

    pub fn backup_snapshot_durable(&self, seal_key: &SealKey) -> KmsDurableSnapshot {
        let keks = self.keks.lock().expect("KMS keks poisoned");
        let deks = self.deks.lock().expect("KMS deks poisoned");
        let kek_list: Vec<(KekId, ExportedKek)> = keks
            .iter()
            .map(|(id, sk)| {
                (
                    id.clone(),
                    ExportedKek {
                        nonce: sk.wrapped.nonce,
                        wrapped: sk.wrapped.wrapped.clone(),
                        epoch: sk.epoch,
                    },
                )
            })
            .collect();
        let dek_list: Vec<(DekId, WrappedDek, u64)> = deks
            .iter()
            .filter(|(dek_id, _)| keks.keys().any(|k| k.tenant == dek_id.tenant))
            .map(|(dek_id, (wrapped, epoch))| (dek_id.clone(), wrapped.clone(), *epoch))
            .collect();
        KmsDurableSnapshot {
            sealed_root: self.root.seal(seal_key),
            keks: kek_list,
            deks: dek_list,
        }
    }

    pub fn wrap_dek_material(
        &self,
        tenant: &TenantId,
        region: &Region,
        material: &[u8; KEY_LEN],
    ) -> Result<WrappedDek, KmsError> {
        let kek_id = KekId::new(tenant.clone(), region.clone());
        self.ensure_kek(&kek_id);
        self.wrap_dek(&kek_id, &RawKey(*material))
    }

    pub fn counts(&self) -> (usize, usize) {
        let keks = self.keks.lock().map(|k| k.len()).unwrap_or(0);
        let deks = self.deks.lock().map(|d| d.len()).unwrap_or(0);
        (keks, deks)
    }

    pub fn unwrap_dek_material(
        &self,
        tenant: &TenantId,
        region: &Region,
        w: &WrappedDek,
    ) -> Result<DekHandle, KmsError> {
        let kek_id = KekId::new(tenant.clone(), region.clone());
        let kek = self.open_kek(&kek_id)?;
        let plain = kek
            .cipher()
            .decrypt(Nonce::from_slice(&w.nonce), w.wrapped.as_slice())
            .map_err(|_| KmsError::UnwrapFailed(DekId::new(tenant.clone(), KeyClass::Tenant)))?;
        if plain.len() != KEY_LEN {
            return Err(KmsError::UnwrapFailed(DekId::new(
                tenant.clone(),
                KeyClass::Tenant,
            )));
        }
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(&plain);
        Ok(DekHandle { key: RawKey(bytes) })
    }
}

enum KmsBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(KmsCore),
    Durable(DurableKms),
}

pub struct KmsEngine {
    backend: KmsBackend,
}

#[cfg(any(test, feature = "test-support"))]
impl Default for KmsEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl KmsEngine {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> KmsEngine {
        KmsEngine {
            backend: KmsBackend::Memory(KmsCore::fresh()),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn from_root(root: CellRoot) -> KmsEngine {
        KmsEngine {
            backend: KmsBackend::Memory(KmsCore::from_root(root)),
        }
    }

    pub(crate) fn durable(backend: DurableKms) -> KmsEngine {
        KmsEngine {
            backend: KmsBackend::Durable(backend),
        }
    }

    pub(crate) fn core(&self) -> &KmsCore {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core,
            KmsBackend::Durable(d) => d.core(),
        }
    }

    pub fn install_wrapped_kek(
        &self,
        id: KekId,
        nonce: [u8; NONCE_LEN],
        wrapped: Vec<u8>,
        epoch: u64,
    ) {
        self.core().install_wrapped_kek(id, nonce, wrapped, epoch);
    }

    pub fn install_wrapped_dek(&self, id: DekId, dek: WrappedDek, dek_epoch: u64) {
        self.core().install_wrapped_dek(id, dek, dek_epoch);
    }

    pub fn export_sealed_root(&self, seal_key: &SealKey) -> SealedRoot {
        self.core().export_sealed_root(seal_key)
    }

    pub fn export_kek(&self, id: &KekId) -> Option<ExportedKek> {
        self.core().export_kek(id)
    }

    pub fn export_dek(&self, id: &DekId) -> Option<(WrappedDek, u64)> {
        self.core().export_dek(id)
    }

    pub fn export_deks(&self) -> Vec<(DekId, WrappedDek, u64)> {
        self.core().export_deks()
    }

    pub fn ensure_kek(&self, id: &KekId) -> u64 {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core.ensure_kek(id),
            KmsBackend::Durable(d) => d.ensure_kek(id),
        }
    }

    pub fn ensure_dek(
        &self,
        tenant: &TenantId,
        region: &Region,
        class: KeyClass,
    ) -> Result<PiiKeyRef, KmsError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core.ensure_dek(tenant, region, class),
            KmsBackend::Durable(d) => d.ensure_dek(tenant, region, class),
        }
    }

    pub fn resolve_dek(&self, key_ref: &PiiKeyRef, region: &Region) -> Result<DekHandle, KmsError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core.resolve_dek(key_ref, region),
            KmsBackend::Durable(durable) => durable.resolve_dek(key_ref, region),
        }
    }

    pub fn rotate_kek(&self, id: &KekId) -> Result<u64, KmsError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core.rotate_kek(id),
            KmsBackend::Durable(d) => d.rotate_kek(id),
        }
    }

    pub fn destroy_kek(&self, id: &KekId) -> bool {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core.destroy_kek(id),
            KmsBackend::Durable(d) => d.destroy_kek(id),
        }
    }

    pub fn destroy_dek(&self, id: &DekId) -> bool {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core.destroy_dek(id),
            KmsBackend::Durable(d) => d.destroy_dek(id),
        }
    }

    pub fn backup_snapshot(&self) -> Vec<(DekId, WrappedDek)> {
        self.core().backup_snapshot()
    }

    pub fn backup_snapshot_durable(&self, seal_key: &SealKey) -> KmsDurableSnapshot {
        self.core().backup_snapshot_durable(seal_key)
    }

    pub fn wrap_dek_material(
        &self,
        tenant: &TenantId,
        region: &Region,
        material: &[u8; KEY_LEN],
    ) -> Result<WrappedDek, KmsError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core.wrap_dek_material(tenant, region, material),
            KmsBackend::Durable(d) => d.wrap_dek_material(tenant, region, material),
        }
    }

    pub fn unwrap_dek_material(
        &self,
        tenant: &TenantId,
        region: &Region,
        w: &WrappedDek,
    ) -> Result<DekHandle, KmsError> {
        self.core().unwrap_dek_material(tenant, region, w)
    }
}

impl fmt::Debug for KmsEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (keks, deks) = self.core().counts();
        f.debug_struct("KmsEngine")
            .field("keks", &keks)
            .field("deks", &deks)
            .finish_non_exhaustive()
    }
}

pub trait KmsAdapter: Send + Sync {
    fn resolve_dek(&self, key_ref: &PiiKeyRef, region: &Region) -> Result<DekHandle, KmsError>;
}

impl KmsAdapter for KmsEngine {
    fn resolve_dek(&self, key_ref: &PiiKeyRef, region: &Region) -> Result<DekHandle, KmsError> {
        KmsEngine::resolve_dek(self, key_ref, region)
    }
}

pub use crate::kms_failstatic::{KmsReadError, KmsReadPath, KmsReadResult, KmsReadiness};

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> TenantId {
        TenantId(s.to_string())
    }
    fn r(s: &str) -> Region {
        Region(s.to_string())
    }

    #[test]
    fn pii_key_ref_encodes_tenant_epoch_class_exactly() {
        let kr = PiiKeyRef::new(t("acme"), 0, KeyClass::Tenant);
        assert_eq!(kr.to_uri(), "kms://acme/0/tenant");
        let kr = PiiKeyRef::new(t("acme"), 3, KeyClass::Subject("u-42".into()));
        assert_eq!(kr.to_uri(), "kms://acme/3/subject:u-42");
        let kr = PiiKeyRef::new(t("acme"), 7, KeyClass::Blob);
        assert_eq!(kr.to_uri(), "kms://acme/7/blob");
    }

    #[test]
    fn pii_key_ref_round_trips_through_parse() {
        for uri in [
            "kms://acme/0/tenant",
            "kms://acme/12/subject:u-99",
            "kms://acme/5/blob",
        ] {
            let kr = PiiKeyRef::parse(uri).expect("parses the canonical grammar");
            assert_eq!(kr.to_uri(), uri, "round-trip is byte-identical");
        }
    }

    #[test]
    fn pii_key_ref_rejects_malformed_uris_loudly() {
        assert!(
            PiiKeyRef::parse("https://acme/0/tenant").is_none(),
            "wrong scheme"
        );
        assert!(PiiKeyRef::parse("kms://acme/0").is_none(), "missing class");
        assert!(
            PiiKeyRef::parse("kms://acme/notanint/tenant").is_none(),
            "non-int epoch"
        );
        assert!(
            PiiKeyRef::parse("kms:///0/tenant").is_none(),
            "empty tenant"
        );
        assert!(
            PiiKeyRef::parse("kms://acme/0/bogus").is_none(),
            "unknown class"
        );
        assert!(
            PiiKeyRef::parse("kms://acme/0/subject:").is_none(),
            "empty subject id"
        );
        assert!(
            PiiKeyRef::parse("kms://acme/0/subject:u42/extra").is_none(),
            "a key ref has exactly three path segments"
        );
        assert!(
            PiiKeyRef::parse("kms://acme/00/tenant").is_none(),
            "the epoch spelling is canonical"
        );
    }

    #[test]
    fn subject_dek_uri_with_colon_in_class_parses_the_full_id() {
        let kr = PiiKeyRef::parse("kms://acme/4/subject:alice:bob").expect("parses");
        assert_eq!(kr.class, KeyClass::Subject("alice:bob".into()));
        assert_eq!(kr.dek_epoch, 4);
    }

    #[test]
    fn wrap_unwrap_round_trips_a_dek_under_a_kek() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        let kr = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("ensure dek");

        let dek = kms.resolve_dek(&kr, &region).expect("resolve");
        let (nonce, ct) = dek.seal(b"some encrypted column value");
        let pt = dek.open(&nonce, &ct).expect("authenticated open");
        assert_eq!(pt, b"some encrypted column value");

        let dek2 = kms.resolve_dek(&kr, &region).expect("resolve again");
        assert_eq!(
            dek2.open(&nonce, &ct).expect("open"),
            b"some encrypted column value"
        );
    }

    #[test]
    fn per_subject_dek_is_distinct_from_the_tenant_dek() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        let tk = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("tenant dek");
        let sk = kms
            .ensure_dek(&tenant, &region, KeyClass::Subject("u-1".into()))
            .expect("subject dek");
        assert_ne!(tk, sk, "different key refs");

        let tdek = kms.resolve_dek(&tk, &region).expect("resolve tenant");
        let sdek = kms.resolve_dek(&sk, &region).expect("resolve subject");
        let (nonce, ct) = tdek.seal(b"bulk");
        assert!(
            sdek.open(&nonce, &ct).is_none(),
            "subject DEK must not open tenant ciphertext"
        );
    }

    #[test]
    fn destroy_kek_renders_every_dek_under_it_unrecoverable() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        let kek_id = KekId::new(tenant.clone(), region.clone());
        kms.ensure_kek(&kek_id);
        let tk = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("tenant dek");
        let sk = kms
            .ensure_dek(&tenant, &region, KeyClass::Subject("u-1".into()))
            .expect("subject dek");

        assert!(kms.resolve_dek(&tk, &region).is_ok());
        assert!(kms.resolve_dek(&sk, &region).is_ok());

        assert!(kms.destroy_kek(&kek_id), "a KEK was present to destroy");

        assert_eq!(
            kms.resolve_dek(&tk, &region),
            Err(KmsError::KekUnavailable(kek_id.clone()))
        );
        assert_eq!(
            kms.resolve_dek(&sk, &region),
            Err(KmsError::KekUnavailable(kek_id))
        );
    }

    #[test]
    fn destroy_subject_dek_leaves_the_tenant_and_other_subjects_intact() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        let tk = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("tenant");
        let s1 = kms
            .ensure_dek(&tenant, &region, KeyClass::Subject("u-1".into()))
            .expect("s1");
        let s2 = kms
            .ensure_dek(&tenant, &region, KeyClass::Subject("u-2".into()))
            .expect("s2");

        let s1_id = DekId::new(tenant.clone(), KeyClass::Subject("u-1".into()));
        assert!(kms.destroy_dek(&s1_id), "subject DEK present to destroy");

        assert_eq!(
            kms.resolve_dek(&s1, &region),
            Err(KmsError::DekUnavailable(s1_id))
        );
        assert!(
            kms.resolve_dek(&tk, &region).is_ok(),
            "tenant DEK untouched"
        );
        assert!(
            kms.resolve_dek(&s2, &region).is_ok(),
            "other subject untouched"
        );
    }

    #[test]
    fn rotate_re_wraps_without_re_encrypting_the_payload() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        let kek_id = KekId::new(tenant.clone(), region.clone());
        kms.ensure_kek(&kek_id);
        let kr = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("dek");

        let dek = kms.resolve_dek(&kr, &region).expect("resolve");
        let (nonce, ct) = dek.seal(b"a value sealed before rotation");

        let new_epoch = kms.rotate_kek(&kek_id).expect("rotate");
        assert_eq!(new_epoch, 1, "forward-only epoch bump");

        let kr2 = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("dek post-rotate");
        assert_eq!(kr2.dek_epoch, 1, "the dek epoch bumped on re-wrap");

        let dek2 = kms.resolve_dek(&kr2, &region).expect("resolve post-rotate");
        assert_eq!(
            dek2.open(&nonce, &ct).expect("still opens"),
            b"a value sealed before rotation",
            "rotation re-wraps the DEK; the payload is NOT re-encrypted"
        );
    }

    #[test]
    fn resolve_with_no_kek_fails_loudly_never_plaintext() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        let kr = PiiKeyRef::new(tenant.clone(), 0, KeyClass::Tenant);
        assert!(matches!(
            kms.resolve_dek(&kr, &region),
            Err(KmsError::DekUnavailable(_))
        ));
    }

    #[test]
    fn ensure_dek_without_a_kek_fails_loudly() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        let err = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect_err("no kek");
        assert_eq!(err, KmsError::KekUnavailable(KekId::new(tenant, region)));
    }

    #[test]
    fn backup_snapshot_excludes_a_crypto_shredded_tenant() {
        let kms = KmsEngine::new();
        let (live, region) = (t("live-co"), r("eu-west"));
        let (dead, _) = (t("offboarded-co"), r("eu-west"));
        kms.ensure_kek(&KekId::new(live.clone(), region.clone()));
        kms.ensure_kek(&KekId::new(dead.clone(), region.clone()));
        kms.ensure_dek(&live, &region, KeyClass::Tenant)
            .expect("live dek");
        kms.ensure_dek(&dead, &region, KeyClass::Tenant)
            .expect("dead dek");

        assert!(kms.destroy_kek(&KekId::new(dead.clone(), region.clone())));

        let snap = kms.backup_snapshot();
        assert!(
            snap.iter().any(|(d, _)| d.tenant == live),
            "live tenant DEK is backed up"
        );
        assert!(
            !snap.iter().any(|(d, _)| d.tenant == dead),
            "a crypto-shredded tenant DEK is EXCLUDED from backup (stays dead across restore)"
        );
    }

    #[test]
    fn pii_key_ref_display_equals_the_uri() {
        let kr = PiiKeyRef::new(t("acme"), 2, KeyClass::Subject("u-7".into()));
        assert_eq!(format!("{kr}"), "kms://acme/2/subject:u-7");
        assert_eq!(format!("{kr}"), kr.to_uri());
    }

    #[test]
    fn kms_error_display_names_the_loud_failure() {
        let e = KmsError::KekUnavailable(KekId::new(t("acme"), r("eu-west")));
        let m = e.to_string();
        assert!(m.contains("acme") && m.contains("crypto-shred"), "got: {m}");
        let e = KmsError::DekUnavailable(DekId::new(t("acme"), KeyClass::Subject("u".into())));
        assert!(
            e.to_string().contains("unrecoverable"),
            "names the unrecoverable outcome"
        );
        let e = KmsError::UnwrapFailed(DekId::new(t("acme"), KeyClass::Tenant));
        assert!(
            e.to_string().contains("authenticate"),
            "names the auth failure"
        );
    }

    #[test]
    fn raw_key_debug_redacts_the_key_bytes() {
        let root = CellRoot::generate();
        let dbg = format!("{root:?}");
        assert!(dbg.contains("redacted"), "RawKey Debug is redacted: {dbg}");
        assert!(
            dbg.contains("CellRoot"),
            "the CellRoot wrapper is named: {dbg}"
        );
    }

    #[test]
    fn seal_unseal_round_trips_the_root_and_never_rests_plaintext() {
        let root = CellRoot::generate();
        let seal = SealKey::from_bytes([3u8; KEY_LEN]);
        let sealed = root.seal(&seal);
        assert_ne!(
            sealed.ciphertext.as_slice(),
            root.root.0.as_slice(),
            "the sealed root is ciphertext, never the plaintext root"
        );
        let recovered =
            CellRoot::unseal(&seal, &sealed).expect("unseal under the correct seal key");
        assert_eq!(
            recovered.root.0, root.root.0,
            "unseal recovers the exact 256-bit root"
        );
    }

    #[test]
    fn unseal_with_a_wrong_seal_key_fails_never_a_silent_root() {
        let root = CellRoot::generate();
        let sealed = root.seal(&SealKey::from_bytes([1u8; KEY_LEN]));
        assert!(
            CellRoot::unseal(&SealKey::from_bytes([2u8; KEY_LEN]), &sealed).is_none(),
            "a wrong seal key must NOT unseal the root"
        );
    }

    #[test]
    fn a_kek_wrapped_under_the_root_survives_a_seal_unseal_cycle() {
        let root = CellRoot::generate();
        let kek_plain = RawKey::generate();
        let wrapped = root.wrap_kek(&kek_plain);
        let seal = SealKey::from_bytes([9u8; KEY_LEN]);
        let recovered = CellRoot::unseal(&seal, &root.seal(&seal)).expect("unseal");
        let unwrapped = recovered
            .unwrap_kek(&wrapped)
            .expect("the KEK unwraps under the recovered root");
        assert_eq!(
            unwrapped.0, kek_plain.0,
            "the KEK plaintext survived the seal/unseal cycle"
        );
    }

    #[test]
    fn seal_key_from_encoded_decodes_hex_and_rejects_garbage() {
        let hexkey = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let k = SealKey::from_encoded(hexkey).expect("valid 32-byte hex seal key");
        let root = CellRoot::generate();
        assert!(CellRoot::unseal(&k, &root.seal(&k)).is_some());
        assert!(matches!(
            SealKey::from_encoded("nothex!!"),
            Err(SealKeyError::Decode(_))
        ));
        assert!(matches!(
            SealKey::from_encoded("00112233"),
            Err(SealKeyError::WrongLength(4))
        ));
    }

    #[test]
    fn seal_key_debug_is_redacted() {
        let seal = SealKey::from_bytes([5u8; KEY_LEN]);
        assert_eq!(format!("{seal:?}"), "SealKey(<redacted seal key>)");
        assert!(!format!("{seal:?}").contains('5'));
    }

    #[test]
    fn seal_key_service_derivation_is_stable_and_domain_separated() {
        let seal = SealKey::from_bytes([5u8; KEY_LEN]);
        let cursor = seal.derive_service_key("myelin test cursor v1");
        assert_eq!(
            *cursor,
            *seal.derive_service_key("myelin test cursor v1"),
            "all instances sharing the cell seal root derive the same service key"
        );
        assert_ne!(
            *cursor,
            *seal.derive_service_key("myelin test other purpose v1"),
            "one derived service key cannot be replayed into another domain"
        );
        assert_ne!(*cursor, [5u8; KEY_LEN], "the seal root is never exported");
    }

    #[test]
    fn backup_snapshot_durable_carries_root_keks_deks_and_rebuilds_a_working_engine() {
        let kms = KmsEngine::new();
        let (live, region) = (t("live-co"), r("eu-west"));
        let dead = t("offboarded-co");
        kms.ensure_kek(&KekId::new(live.clone(), region.clone()));
        kms.ensure_kek(&KekId::new(dead.clone(), region.clone()));
        let kr = kms
            .ensure_dek(&live, &region, KeyClass::Tenant)
            .expect("live dek");
        kms.ensure_dek(&dead, &region, KeyClass::Tenant)
            .expect("dead dek");
        let (nonce, ct) = kms.resolve_dek(&kr, &region).expect("resolve").seal(b"col");

        assert!(kms.destroy_kek(&KekId::new(dead.clone(), region.clone())));
        let seal = SealKey::from_bytes([4u8; KEY_LEN]);
        let snap = kms.backup_snapshot_durable(&seal);

        assert!(snap.keks.iter().any(|(id, _)| id.tenant == live));
        assert!(
            !snap.keks.iter().any(|(id, _)| id.tenant == dead),
            "a crypto-shredded KEK is EXCLUDED from the durable snapshot"
        );
        assert!(snap.deks.iter().any(|(id, ..)| id.tenant == live));
        assert!(
            !snap.deks.iter().any(|(id, ..)| id.tenant == dead),
            "a crypto-shredded tenant's DEK is EXCLUDED (stays dead across restore)"
        );

        let engine2 = KmsEngine::from_root(
            CellRoot::unseal(&seal, &snap.sealed_root).expect("unseal the snapshot root"),
        );
        for (id, k) in snap.keks {
            engine2.install_wrapped_kek(id, k.nonce, k.wrapped, k.epoch);
        }
        for (id, w, e) in snap.deks {
            engine2.install_wrapped_dek(id, w, e);
        }
        let pt = engine2
            .resolve_dek(&kr, &region)
            .expect("the live DEK resolves after a from-snapshot rebuild")
            .open(&nonce, &ct)
            .expect("and decrypts the pre-snapshot ciphertext");
        assert_eq!(pt, b"col", "decrypt across a from-snapshot rebuild");
    }

    #[test]
    fn debug_redacts_all_key_material() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        let kr = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("dek");
        let dek = kms.resolve_dek(&kr, &region).expect("resolve");
        assert!(format!("{kms:?}").contains("KmsEngine"));
        assert!(
            !format!("{dek:?}").contains("["),
            "DekHandle redacts its bytes"
        );
        assert!(format!("{dek:?}").contains("redacted"));
    }
}
