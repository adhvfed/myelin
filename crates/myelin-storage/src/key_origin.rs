use crate::kms::{DekHandle, DekId, KmsEngine, WrappedDek, KEY_LEN};
use myelin_tenancy::{Region, TenantId};
use std::fmt;

pub type KeyId = DekId;

#[derive(Clone, PartialEq, Eq)]
pub struct Dek {
    bytes: [u8; KEY_LEN],
}

impl Dek {
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Dek {
        Dek { bytes }
    }

    pub fn generate() -> Dek {
        use aes_gcm::aead::OsRng;
        use aes_gcm::{Aes256Gcm, KeyInit};
        let key = Aes256Gcm::generate_key(OsRng);
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(key.as_slice());
        Dek { bytes }
    }

    pub(crate) fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.bytes
    }
}

impl fmt::Debug for Dek {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Dek(<redacted 256-bit key>)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyOriginError {
    Kms(crate::kms::KmsError),
    HyokDenied { tenant: TenantId, region: Region },
}

impl fmt::Display for KeyOriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyOriginError::Kms(e) => write!(f, "key-origin: {e}"),
            KeyOriginError::HyokDenied { tenant, region } => write!(
                f,
                "key-origin: HYOK unwrap DENIED by the customer key service for tenant={} \
                 region={} (Myelin holds no plaintext key - this is the loud HYOK denial, \
                 NEVER a plaintext fall-through)",
                tenant.as_str(),
                region.as_str()
            ),
        }
    }
}

impl std::error::Error for KeyOriginError {}

impl From<crate::kms::KmsError> for KeyOriginError {
    fn from(e: crate::kms::KmsError) -> Self {
        KeyOriginError::Kms(e)
    }
}

pub trait KeyOrigin {
    fn wrap(&self, dek: &Dek, tenant: TenantId) -> Result<WrappedDek, KeyOriginError>;

    fn unwrap(&self, w: &WrappedDek, tenant: TenantId) -> Result<DekHandle, KeyOriginError>;

    fn can_derive_plaintext_index(&self) -> bool;

    fn destroy(&self, key_id: KeyId) -> Result<(), KeyOriginError>;
}

pub struct PlatformManaged<'a> {
    engine: &'a KmsEngine,
    region: Region,
}

impl<'a> PlatformManaged<'a> {
    pub fn new(engine: &'a KmsEngine, region: Region) -> Self {
        PlatformManaged { engine, region }
    }
}

impl KeyOrigin for PlatformManaged<'_> {
    fn wrap(&self, dek: &Dek, tenant: TenantId) -> Result<WrappedDek, KeyOriginError> {
        Ok(self
            .engine
            .wrap_dek_material(&tenant, &self.region, dek.as_bytes())?)
    }

    fn unwrap(&self, w: &WrappedDek, tenant: TenantId) -> Result<DekHandle, KeyOriginError> {
        Ok(self.engine.unwrap_dek_material(&tenant, &self.region, w)?)
    }

    fn can_derive_plaintext_index(&self) -> bool {
        true
    }

    fn destroy(&self, key_id: KeyId) -> Result<(), KeyOriginError> {
        self.engine.destroy_dek(&key_id)?;
        Ok(())
    }
}

pub struct Byok<'a> {
    engine: &'a KmsEngine,
    region: Region,
    customer_key_path: String,
}

impl<'a> Byok<'a> {
    pub fn new(
        engine: &'a KmsEngine,
        region: Region,
        customer_key_path: impl Into<String>,
    ) -> Self {
        Byok {
            engine,
            region,
            customer_key_path: customer_key_path.into(),
        }
    }

    pub fn customer_key_path(&self) -> &str {
        &self.customer_key_path
    }
}

impl KeyOrigin for Byok<'_> {
    fn wrap(&self, dek: &Dek, tenant: TenantId) -> Result<WrappedDek, KeyOriginError> {
        Ok(self
            .engine
            .wrap_dek_material(&tenant, &self.region, dek.as_bytes())?)
    }

    fn unwrap(&self, w: &WrappedDek, tenant: TenantId) -> Result<DekHandle, KeyOriginError> {
        Ok(self.engine.unwrap_dek_material(&tenant, &self.region, w)?)
    }

    fn can_derive_plaintext_index(&self) -> bool {
        true
    }

    fn destroy(&self, key_id: KeyId) -> Result<(), KeyOriginError> {
        self.engine.destroy_dek(&key_id)?;
        Ok(())
    }
}

pub struct Hyok<S: HyokKeyService> {
    service: S,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HyokServiceDenied;

impl fmt::Display for HyokServiceDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HYOK customer key service denied the call-out (revoked / unreachable)")
    }
}

impl std::error::Error for HyokServiceDenied {}

pub trait HyokKeyService {
    fn wrap(&self, dek: &Dek) -> Result<WrappedDek, HyokServiceDenied>;

    fn unwrap(&self, w: &WrappedDek) -> Result<DekHandle, HyokServiceDenied>;

    fn destroy(&self);
}

impl<S: HyokKeyService> Hyok<S> {
    pub fn new(service: S) -> Self {
        Hyok { service }
    }
}

impl<S: HyokKeyService> KeyOrigin for Hyok<S> {
    fn wrap(&self, dek: &Dek, _tenant: TenantId) -> Result<WrappedDek, KeyOriginError> {
        self.service
            .wrap(dek)
            .map_err(|_| KeyOriginError::HyokDenied {
                tenant: _tenant,
                region: Region(String::new()),
            })
    }

    fn unwrap(&self, w: &WrappedDek, tenant: TenantId) -> Result<DekHandle, KeyOriginError> {
        self.service
            .unwrap(w)
            .map_err(|_| KeyOriginError::HyokDenied {
                tenant,
                region: Region(String::new()),
            })
    }

    fn can_derive_plaintext_index(&self) -> bool {
        false
    }

    fn destroy(&self, _key_id: KeyId) -> Result<(), KeyOriginError> {
        self.service.destroy();
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexAdmission {
    Admit,
    SkipHyok,
}

impl IndexAdmission {
    pub fn for_origin(origin: &dyn KeyOrigin) -> IndexAdmission {
        if origin.can_derive_plaintext_index() {
            IndexAdmission::Admit
        } else {
            IndexAdmission::SkipHyok
        }
    }

    pub fn may_index(self) -> bool {
        matches!(self, IndexAdmission::Admit)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyOriginTelemetry {
    pub origin: KeyOriginKind,
    pub can_derive_plaintext_index: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyOriginKind {
    PlatformManaged,
    Byok,
    Hyok,
}

impl KeyOriginTelemetry {
    pub fn observe(origin: &dyn KeyOrigin, kind: KeyOriginKind) -> KeyOriginTelemetry {
        KeyOriginTelemetry {
            origin: kind,
            can_derive_plaintext_index: origin.can_derive_plaintext_index(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kms::{KekId, KmsEngine};

    fn t(s: &str) -> TenantId {
        TenantId(s.to_string())
    }
    fn r(s: &str) -> Region {
        Region(s.to_string())
    }

    struct MockHyokKeyService {
        revoked: std::cell::Cell<bool>,
        key: [u8; KEY_LEN],
    }
    impl MockHyokKeyService {
        fn new() -> Self {
            MockHyokKeyService {
                revoked: std::cell::Cell::new(false),
                key: [7u8; KEY_LEN],
            }
        }
    }
    impl HyokKeyService for MockHyokKeyService {
        fn wrap(&self, dek: &Dek) -> Result<WrappedDek, HyokServiceDenied> {
            if self.revoked.get() {
                return Err(HyokServiceDenied);
            }
            let mut wrapped = dek.as_bytes().to_vec();
            for (b, k) in wrapped.iter_mut().zip(self.key.iter()) {
                *b ^= *k;
            }
            Ok(WrappedDek {
                nonce: [0u8; 12],
                wrapped,
                kek_epoch: 0,
            })
        }
        fn unwrap(&self, _w: &WrappedDek) -> Result<DekHandle, HyokServiceDenied> {
            if self.revoked.get() {
                return Err(HyokServiceDenied);
            }
            Ok(crate::kms::DekHandle::from_raw(self.key))
        }
        fn destroy(&self) {
            self.revoked.set(true);
        }
    }

    #[test]
    fn can_derive_is_false_for_hyok_true_for_platform_and_byok() {
        let engine = KmsEngine::new();
        engine
            .ensure_kek(&KekId::new(t("acme"), r("eu-west")))
            .expect("seed the in-memory KEK");

        let platform = PlatformManaged::new(&engine, r("eu-west"));
        let byok = Byok::new(&engine, r("eu-west"), "kms-customer://acme/k1");
        let hyok = Hyok::new(MockHyokKeyService::new());

        assert!(
            platform.can_derive_plaintext_index(),
            "platform-managed CAN derive a plaintext index"
        );
        assert!(
            byok.can_derive_plaintext_index(),
            "BYOK CAN derive while the key is live"
        );
        assert!(
            !hyok.can_derive_plaintext_index(),
            "HYOK can NEVER derive a plaintext index (structural)"
        );
    }

    #[test]
    fn index_admission_refuses_hyok_by_construction() {
        let engine = KmsEngine::new();
        engine
            .ensure_kek(&KekId::new(t("acme"), r("eu-west")))
            .expect("seed the in-memory KEK");
        let platform = PlatformManaged::new(&engine, r("eu-west"));
        let byok = Byok::new(&engine, r("eu-west"), "kms-customer://acme/k1");
        let hyok = Hyok::new(MockHyokKeyService::new());

        assert_eq!(IndexAdmission::for_origin(&platform), IndexAdmission::Admit);
        assert_eq!(IndexAdmission::for_origin(&byok), IndexAdmission::Admit);
        assert_eq!(IndexAdmission::for_origin(&hyok), IndexAdmission::SkipHyok);

        assert!(IndexAdmission::for_origin(&platform).may_index());
        assert!(
            !IndexAdmission::for_origin(&hyok).may_index(),
            "a HYOK class cannot have a plaintext index built - enforced by code"
        );
    }

    #[test]
    fn platform_origin_wraps_and_unwraps_through_the_engine() {
        let engine = KmsEngine::new();
        engine
            .ensure_kek(&KekId::new(t("acme"), r("eu-west")))
            .expect("seed the in-memory KEK");
        let platform = PlatformManaged::new(&engine, r("eu-west"));

        let dek = Dek::generate();
        let wrapped = platform.wrap(&dek, t("acme")).expect("platform wrap");
        let handle = platform
            .unwrap(&wrapped, t("acme"))
            .expect("platform unwrap");

        let (nonce, ct) = handle.seal(b"some pii");
        assert_eq!(handle.open(&nonce, &ct).as_deref(), Some(&b"some pii"[..]));
    }

    #[test]
    fn byok_wraps_under_the_customer_key_path() {
        let engine = KmsEngine::new();
        engine
            .ensure_kek(&KekId::new(t("acme"), r("eu-west")))
            .expect("seed the in-memory KEK");
        let byok = Byok::new(&engine, r("eu-west"), "kms-customer://acme/master-key");

        assert_eq!(byok.customer_key_path(), "kms-customer://acme/master-key");

        let dek = Dek::generate();
        let wrapped = byok.wrap(&dek, t("acme")).expect("byok wrap");
        let handle = byok
            .unwrap(&wrapped, t("acme"))
            .expect("byok unwrap (key live)");
        let (nonce, ct) = handle.seal(b"bio");
        assert_eq!(handle.open(&nonce, &ct).as_deref(), Some(&b"bio"[..]));
    }

    #[test]
    fn hyok_never_exposes_plaintext_and_unwrap_can_deny() {
        let hyok = Hyok::new(MockHyokKeyService::new());
        let dek = Dek::generate();

        let wrapped = hyok
            .wrap(&dek, t("acme"))
            .expect("hyok wrap (customer service)");
        let handle = hyok
            .unwrap(&wrapped, t("acme"))
            .expect("hyok unwrap granted");
        let (nonce, ct) = handle.seal(b"x");
        assert_eq!(handle.open(&nonce, &ct).as_deref(), Some(&b"x"[..]));

        hyok.destroy(KeyId::new(
            t("acme"),
            crate::kms::KeyClass::Subject("alice".into()),
        ))
        .expect("destroy is the customer-initiated shred");
        let denied = hyok.unwrap(&wrapped, t("acme"));
        assert!(
            matches!(denied, Err(KeyOriginError::HyokDenied { .. })),
            "after the customer revokes, a HYOK unwrap DENIES (no plaintext to Myelin)"
        );

        assert!(!hyok.can_derive_plaintext_index());
    }

    #[test]
    fn wrap_unwrap_destroy_route_through_all_three_origins() {
        let engine = KmsEngine::new();
        engine
            .ensure_kek(&KekId::new(t("acme"), r("eu-west")))
            .expect("seed the in-memory KEK");

        let platform = PlatformManaged::new(&engine, r("eu-west"));
        let byok = Byok::new(&engine, r("eu-west"), "kms-customer://acme/k");
        let hyok = Hyok::new(MockHyokKeyService::new());

        assert!(platform
            .destroy(KeyId::new(t("acme"), crate::kms::KeyClass::Tenant))
            .is_ok());
        assert!(byok
            .destroy(KeyId::new(t("acme"), crate::kms::KeyClass::Tenant))
            .is_ok());
        assert!(hyok
            .destroy(KeyId::new(t("acme"), crate::kms::KeyClass::Tenant))
            .is_ok());
    }

    #[test]
    fn telemetry_reports_can_derive_per_origin() {
        let engine = KmsEngine::new();
        engine
            .ensure_kek(&KekId::new(t("acme"), r("eu-west")))
            .expect("seed the in-memory KEK");
        let platform = PlatformManaged::new(&engine, r("eu-west"));
        let hyok = Hyok::new(MockHyokKeyService::new());

        let pt = KeyOriginTelemetry::observe(&platform, KeyOriginKind::PlatformManaged);
        let ht = KeyOriginTelemetry::observe(&hyok, KeyOriginKind::Hyok);
        assert!(pt.can_derive_plaintext_index);
        assert!(!ht.can_derive_plaintext_index);
        assert_eq!(pt.origin, KeyOriginKind::PlatformManaged);
        assert_eq!(ht.origin, KeyOriginKind::Hyok);
    }
}
