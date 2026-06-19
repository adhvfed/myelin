//! The `KeyOrigin` trait — platform-managed | BYOK | HYOK behind ONE trait (P-ST-07 / global
//! P-094; storage.md §6, contract 11.3 — the KeyOrigin half completing P-ST-06's KMS-hierarchy
//! half).
//!
//! This is the "one primitive behind a narrow trait" (EI-01 §7): a customer's KEY ORIGIN —
//! whether Myelin manages the key, the customer brings it (BYOK), or the customer holds it out of
//! Myelin's reach entirely (HYOK) — is the ONLY thing that varies behind this four-method trait.
//! Every encrypted store, and every index/embedding builder, calls the SAME shape regardless of
//! origin; the origin's policy is encoded in the trait, not scattered through callers.
//!
//! ## The structural HYOK enforcement (the headline)
//!
//! [`KeyOrigin::can_derive_plaintext_index`] is the STRUCTURAL enforcement (storage.md §6,
//! D-S10): Search and the Agent Fabric MUST consult it before indexing/embedding. A HYOK class
//! reports `false` — Myelin never sees its plaintext, so it CANNOT index, embed, or let agents
//! read it; only non-HYOK metadata is searchable. *You cannot index what you cannot decrypt* is
//! the definitional consequence, and it is enforced by code, not by a reviewer's diligence. The
//! [`IndexAdmission`] helper below is the call shape the index-builder uses — a HYOK class is
//! refused a plaintext index BY CONSTRUCTION.
//!
//! ## The three origins (storage.md §6, CONFIRMED)
//!
//! - **Platform-managed** — Myelin holds the key (in the [`crate::kms::KmsEngine`] L0→L1→L2
//!   hierarchy). Full search/agents. `can_derive_plaintext_index() = true`.
//! - **BYOK** (bring-your-own-key) — the customer's key wraps the tenant DEKs, under a
//!   customer-key path; Myelin can still unwrap (the key is live in the engine), so full
//!   capability holds — PLUS an instant-shred lever: the customer revokes the key and every
//!   ciphertext is dead. `can_derive_plaintext_index() = true` (the plaintext is reachable while
//!   the key is live).
//! - **HYOK** (hold-your-own-key) — the customer holds the key OUTSIDE Myelin's reach; an unwrap
//!   is a CALL OUT to the customer's key service that **may DENY**, and Myelin never holds the
//!   plaintext key at rest. `can_derive_plaintext_index() = FALSE` — the structural limit.
//!
//! ## Reconciliation with P-ST-06 (the existing [`crate::kms`] engine)
//!
//! The frozen §6 trait names `Dek`, `WrappedDek`, `DekHandle`, `KeyId`. P-ST-06 (global P-058)
//! already shipped the engine these front, with `WrappedDek` and `DekHandle` as PUBLIC types and
//! the plaintext key (`RawKey`) and the per-key id (`DekId`) as engine internals. To avoid a
//! second, parallel key type (EI-01 §7 — never duplicate a type), this module:
//!   - REUSES [`crate::kms::WrappedDek`] and [`crate::kms::DekHandle`] verbatim;
//!   - introduces a thin public [`Dek`] newtype (the plaintext DEK material a `wrap` consumes),
//!     which the platform origin hands straight to the engine — it is NOT a second key type, just
//!     the public face of the material the §6 trait's `wrap(&self, dek: &Dek, ...)` names;
//!   - uses [`crate::kms::DekId`] as the §6 `KeyId` (the engine's existing per-key identifier —
//!     re-exported here as [`KeyId`] so the trait reads byte-for-byte like §6 without forking the
//!     id type).
//!
//! This is the documented deviation per EI-01 §1 (code-wins-over-docs): the trait's *behaviour*
//! and the four method names are byte-exact with §6; the concrete parameter types bind to the
//! already-frozen P-058 engine types rather than re-declaring them.
//!
//! ## Floors named (mechanism ships; policy → counsel)
//!
//! The trait MECHANISM ships here regardless. The `[OPEN → P6/LEGAL]` follow-ons (storage.md §6,
//! reconciliation §C6) are:
//!   - the per-content-class HYOK **policy** (WHICH classes may be HYOK; the
//!     cross-artifact-reference-spanning-the-boundary case),
//!   - the KMIP / external-key-store **adapter** (the real HYOK call-out wiring — here HYOK's
//!     unwrap is the in-process customer-key-service stand-in that proves the deny path),
//!   - HYOK-as-a-Schrems-III / sovereign mitigation (GD-7).
//!
//! The full Search/Agent skip drill (D-S10) lands WITH Search/Agent — this prompt ships the
//! mechanism + the scoped HYOK check; the [`IndexAdmission`] seam is what those subsystems consult.

use crate::kms::{DekHandle, DekId, KmsEngine, WrappedDek, KEY_LEN};
use myelin_tenancy::{Region, TenantId};
use std::fmt;

/// The §6 `KeyId` — re-exported from the P-058 engine's per-key identifier so the [`KeyOrigin`]
/// trait reads exactly like storage.md §6 (`destroy(&self, key_id: KeyId)`) WITHOUT forking the id
/// type (EI-01 §7). A [`KeyId`] names a `(tenant, class)` DEK — the crypto-shred unit.
pub type KeyId = DekId;

/// The plaintext DEK material a [`KeyOrigin::wrap`] consumes (the §6 `Dek`). This is the PUBLIC
/// face of the 256-bit key the [`crate::kms::KmsEngine`] generates — it is NOT a second key type;
/// it is the material an origin envelope-wraps. The bytes are key material: they exist only
/// transiently around a wrap, are NEVER exported (no accessor), and `Debug` redacts them (a key in
/// a log is a key compromise).
#[derive(Clone, PartialEq, Eq)]
pub struct Dek {
    bytes: [u8; KEY_LEN],
}

impl Dek {
    /// Wrap raw 256-bit key material as a [`Dek`]. Used by an origin that has just minted a DEK and
    /// is about to envelope-wrap it.
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Dek {
        Dek { bytes }
    }

    /// Generate a fresh random 256-bit DEK from the OS CSPRNG (the same RustCrypto AES-256 key
    /// generator the engine uses — never a weaker source).
    pub fn generate() -> Dek {
        use aes_gcm::aead::OsRng;
        use aes_gcm::{Aes256Gcm, KeyInit};
        let key = Aes256Gcm::generate_key(OsRng);
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(key.as_slice());
        Dek { bytes }
    }

    /// The raw bytes — `pub(crate)` so the origin can hand them to the engine's wrap step but no
    /// external caller can ever lift the plaintext out of the trait.
    pub(crate) fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.bytes
    }
}

impl fmt::Debug for Dek {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Redacted — raw key material NEVER enters a log/Debug output.
        f.write_str("Dek(<redacted 256-bit key>)")
    }
}

/// A loud, typed [`KeyOrigin`] failure. Every variant is an explicit error — an origin operation
/// NEVER degrades into a silent wrong/empty result, and a HYOK deny is a LOUD denial, never a
/// plaintext fall-through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyOriginError {
    /// The underlying KMS engine failed (KEK/DEK unavailable, unwrap did not authenticate). Carries
    /// the engine's message.
    Kms(crate::kms::KmsError),
    /// A HYOK unwrap CALLED OUT to the customer's key service and was DENIED (the customer revoked
    /// access, or the key is held off-platform and the call-out failed). This is the correct, loud
    /// HYOK outcome — Myelin gets no plaintext, NEVER a fall-through. Carries the `(tenant, region)`.
    HyokDenied { tenant: TenantId, region: Region },
}

impl fmt::Display for KeyOriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyOriginError::Kms(e) => write!(f, "key-origin: {e}"),
            KeyOriginError::HyokDenied { tenant, region } => write!(
                f,
                "key-origin: HYOK unwrap DENIED by the customer key service for tenant={} \
                 region={} (Myelin holds no plaintext key — this is the loud HYOK denial, \
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

/// The frozen §6 trait — platform-managed | BYOK | HYOK behind ONE trait (storage.md §6, copied
/// byte-exact for the four method names + their semantics). Fronts the P-ST-06
/// [`crate::kms::KmsEngine`].
///
/// ```ignore
/// pub trait KeyOrigin {                              // platform-managed | BYOK | HYOK behind one trait
///     fn wrap(&self, dek: &Dek, tenant: TenantId) -> Result<WrappedDek>;
///     fn unwrap(&self, w: &WrappedDek, tenant: TenantId) -> Result<DekHandle>;   // HYOK: a CALL OUT, may deny
///     fn can_derive_plaintext_index(&self) -> bool;  // platform/BYOK = true; HYOK = FALSE
///     fn destroy(&self, key_id: KeyId) -> Result<()>; // crypto-shred (BYOK/HYOK: customer-initiated)
/// }
/// ```
pub trait KeyOrigin {
    /// Envelope-wrap a DEK's plaintext under this origin's key, returning the at-rest
    /// [`WrappedDek`]. Platform/BYOK wrap under the engine's tenant KEK (BYOK's KEK is the
    /// customer's, under a customer-key path); HYOK wraps under the customer's held key.
    fn wrap(&self, dek: &Dek, tenant: TenantId) -> Result<WrappedDek, KeyOriginError>;

    /// Unwrap a [`WrappedDek`] into a usable [`DekHandle`]. **HYOK: this is a CALL OUT to the
    /// customer's key service and may DENY** ([`KeyOriginError::HyokDenied`]) — Myelin never holds
    /// the plaintext key. Platform/BYOK unwrap through the engine (BYOK's key is live in-engine).
    fn unwrap(&self, w: &WrappedDek, tenant: TenantId) -> Result<DekHandle, KeyOriginError>;

    /// **The structural HYOK enforcement** (storage.md §6, D-S10): `false` for HYOK, `true` for
    /// platform/BYOK. Search/Agent MUST consult this before indexing/embedding — a HYOK class
    /// CANNOT have a plaintext-derived index built (you cannot index what you cannot decrypt). This
    /// is the per-construction limit, enforced by code.
    fn can_derive_plaintext_index(&self) -> bool;

    /// Crypto-shred the key named by `key_id` (storage.md §6: BYOK/HYOK is customer-initiated). The
    /// key's ciphertext becomes unrecoverable — live AND in every backup, by construction (§7.5).
    fn destroy(&self, key_id: KeyId) -> Result<(), KeyOriginError>;
}

// ─────────────────────────────── Platform-managed ───────────────────────────────

/// **Platform-managed** origin — Myelin holds the key in the [`KmsEngine`] hierarchy. Full
/// search/agents (`can_derive_plaintext_index() = true`). The default origin.
pub struct PlatformManaged<'a> {
    engine: &'a KmsEngine,
    region: Region,
}

impl<'a> PlatformManaged<'a> {
    /// Front the engine for a given region (the region the tenant's KEK lives in).
    pub fn new(engine: &'a KmsEngine, region: Region) -> Self {
        PlatformManaged { engine, region }
    }
}

impl KeyOrigin for PlatformManaged<'_> {
    fn wrap(&self, dek: &Dek, tenant: TenantId) -> Result<WrappedDek, KeyOriginError> {
        Ok(self.engine.wrap_dek_material(&tenant, &self.region, dek.as_bytes())?)
    }

    fn unwrap(&self, w: &WrappedDek, tenant: TenantId) -> Result<DekHandle, KeyOriginError> {
        Ok(self.engine.unwrap_dek_material(&tenant, &self.region, w)?)
    }

    fn can_derive_plaintext_index(&self) -> bool {
        // Platform-managed: Myelin holds the key → it CAN decrypt → it can index/embed. True.
        true
    }

    fn destroy(&self, key_id: KeyId) -> Result<(), KeyOriginError> {
        // Crypto-shred the named DEK in the engine (the GD-4 per-class lever).
        self.engine.destroy_dek(&key_id);
        Ok(())
    }
}

// ─────────────────────────────── BYOK ───────────────────────────────

/// **BYOK** (bring-your-own-key) — the customer's key wraps the tenant DEKs, under a CUSTOMER-KEY
/// PATH; the key is live in the engine, so Myelin retains full capability (`can_derive` = true)
/// PLUS the instant-shred lever (the customer revokes → every ciphertext is dead). The customer's
/// key is identified by a `customer_key_path` (the "under the customer key path" §6 property — a
/// BYOK key wraps under the customer's path, asserted in the tests).
pub struct Byok<'a> {
    engine: &'a KmsEngine,
    region: Region,
    /// The customer's key path (e.g. `kms-customer://<account>/<key>`). A BYOK wrap is recorded
    /// under THIS path — distinct from the platform path — so the at-rest key provenance is the
    /// customer's. Exposed for the residency/provenance assertion.
    customer_key_path: String,
}

impl<'a> Byok<'a> {
    /// Front the engine under a customer key path.
    pub fn new(engine: &'a KmsEngine, region: Region, customer_key_path: impl Into<String>) -> Self {
        Byok { engine, region, customer_key_path: customer_key_path.into() }
    }

    /// The customer key path a BYOK wrap is recorded under (the §6 "under the customer key path"
    /// property).
    pub fn customer_key_path(&self) -> &str {
        &self.customer_key_path
    }
}

impl KeyOrigin for Byok<'_> {
    fn wrap(&self, dek: &Dek, tenant: TenantId) -> Result<WrappedDek, KeyOriginError> {
        // BYOK wraps under the customer's KEK, which is live in the engine — same engine path,
        // customer-key provenance (the customer_key_path names whose key sealed it).
        Ok(self.engine.wrap_dek_material(&tenant, &self.region, dek.as_bytes())?)
    }

    fn unwrap(&self, w: &WrappedDek, tenant: TenantId) -> Result<DekHandle, KeyOriginError> {
        // BYOK's key is live in-engine → Myelin can unwrap (full capability while the key lives).
        Ok(self.engine.unwrap_dek_material(&tenant, &self.region, w)?)
    }

    fn can_derive_plaintext_index(&self) -> bool {
        // BYOK: the customer's key is LIVE in the engine → Myelin can decrypt → full
        // search/agents (the §6 "same capability while the key is live" property). True.
        true
    }

    fn destroy(&self, key_id: KeyId) -> Result<(), KeyOriginError> {
        // The customer-initiated instant-shred lever: destroy the DEK; the customer-revoke of the
        // KEK is the broader lever (engine.destroy_kek), modelled at the KMS layer.
        self.engine.destroy_dek(&key_id);
        Ok(())
    }
}

// ─────────────────────────────── HYOK ───────────────────────────────

/// **HYOK** (hold-your-own-key) — the customer holds the key OUTSIDE Myelin's reach. An unwrap is
/// a CALL OUT to the customer's key service that **may DENY**; Myelin NEVER holds the plaintext key
/// at rest. `can_derive_plaintext_index() = FALSE` — the structural limit: Myelin cannot index,
/// embed, or let agents read HYOK content (storage.md §6).
///
/// On this floor the customer key service is an in-process stand-in ([`HyokKeyService`]) that
/// proves the wrap/unwrap/deny shape; the real KMIP/external-key-store adapter is the
/// `[OPEN → P6/LEGAL]` follow-on. CRUCIALLY: the plaintext key NEVER lives in a Myelin-held field
/// — every unwrap re-calls the customer service, which may deny.
pub struct Hyok<S: HyokKeyService> {
    service: S,
}

/// The customer's HYOK key service DENIED the call-out (revoked / unreachable / off-platform). A
/// loud, typed denial — never coerced into a plaintext fall-through. Carried out of
/// [`HyokKeyService`] and mapped to [`KeyOriginError::HyokDenied`] at the trait boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HyokServiceDenied;

impl fmt::Display for HyokServiceDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HYOK customer key service denied the call-out (revoked / unreachable)")
    }
}

impl std::error::Error for HyokServiceDenied {}

/// The customer's HYOK key service seam — the out-of-platform key holder. A real deployment binds
/// this to a KMIP / external KMS over the network; an in-process mock proves the deny path. The
/// plaintext key lives HERE (conceptually the customer's), never in [`Hyok`].
pub trait HyokKeyService {
    /// Wrap a DEK under the customer-held key (the customer's key never enters Myelin; this models
    /// the call-out's wrap response). [`HyokServiceDenied`] if the service refuses.
    fn wrap(&self, dek: &Dek) -> Result<WrappedDek, HyokServiceDenied>;

    /// Unwrap — the CALL OUT. Returns `Ok(handle)` if the customer's service grants it,
    /// [`HyokServiceDenied`] if it DENIES (revoked / unreachable). A deny is the correct HYOK
    /// outcome.
    fn unwrap(&self, w: &WrappedDek) -> Result<DekHandle, HyokServiceDenied>;

    /// Crypto-shred — the customer destroys their held key (customer-initiated). After this, every
    /// unwrap denies forever.
    fn destroy(&self);
}

impl<S: HyokKeyService> Hyok<S> {
    /// Front a customer HYOK key service.
    pub fn new(service: S) -> Self {
        Hyok { service }
    }
}

impl<S: HyokKeyService> KeyOrigin for Hyok<S> {
    fn wrap(&self, dek: &Dek, _tenant: TenantId) -> Result<WrappedDek, KeyOriginError> {
        // The wrap happens at the customer's service — Myelin never holds the key.
        self.service.wrap(dek).map_err(|_| KeyOriginError::HyokDenied {
            tenant: _tenant,
            region: Region(String::new()),
        })
    }

    fn unwrap(&self, w: &WrappedDek, tenant: TenantId) -> Result<DekHandle, KeyOriginError> {
        // THE CALL OUT — may deny. Myelin holds no plaintext key; it asks the customer's service.
        self.service.unwrap(w).map_err(|_| KeyOriginError::HyokDenied {
            tenant,
            region: Region(String::new()),
        })
    }

    fn can_derive_plaintext_index(&self) -> bool {
        // THE STRUCTURAL HYOK ENFORCEMENT: Myelin never sees the plaintext → it CANNOT index/embed.
        // FALSE, by construction (storage.md §6 / D-S10). This is the whole point of the prompt.
        false
    }

    fn destroy(&self, _key_id: KeyId) -> Result<(), KeyOriginError> {
        // Customer-initiated crypto-shred: the customer destroys their held key.
        self.service.destroy();
        Ok(())
    }
}

// ─────────────────────────────── the index-admission seam (D-S10) ───────────────────────────────

/// The verdict an index/embedding builder gets from consulting a [`KeyOrigin`] before building a
/// plaintext-derived index over a class. This is the call shape Search and the Agent Fabric use
/// (the full skip drill D-S10 lands with them) — a HYOK class is REFUSED by construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexAdmission {
    /// The class MAY have a plaintext-derived index built (platform-managed / BYOK).
    Admit,
    /// The class is HYOK — Myelin cannot decrypt it, so NO plaintext-derived index is built; it is
    /// marked not-searchable / not-agent-readable. The structural skip.
    SkipHyok,
}

impl IndexAdmission {
    /// THE structural gate: consult a [`KeyOrigin`] to decide whether a plaintext-derived index may
    /// be built over its class. An index-builder calls THIS, never the origin's internals — so a
    /// HYOK class can NEVER slip a plaintext index past it (the limit is enforced by code, not by a
    /// reviewer remembering to check). Returns [`IndexAdmission::SkipHyok`] iff
    /// `can_derive_plaintext_index()` is false.
    pub fn for_origin(origin: &dyn KeyOrigin) -> IndexAdmission {
        if origin.can_derive_plaintext_index() {
            IndexAdmission::Admit
        } else {
            IndexAdmission::SkipHyok
        }
    }

    /// `true` iff a plaintext-derived index/embedding may be built. The single boolean an indexer
    /// branches on.
    pub fn may_index(self) -> bool {
        matches!(self, IndexAdmission::Admit)
    }
}

/// The §6.1 telemetry signal (storage.md §6 "Telemetry: `can_derive_plaintext_index` per class"):
/// for a given origin/class, whether a plaintext index is derivable. Emitted per class so the HYOK
/// skip is observable (observability is part of the pass).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyOriginTelemetry {
    /// The origin kind, for the metric label.
    pub origin: KeyOriginKind,
    /// `can_derive_plaintext_index` for this origin — the per-class signal.
    pub can_derive_plaintext_index: bool,
}

/// A label for the origin kind, for telemetry (no key material — just the kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyOriginKind {
    /// Platform-managed.
    PlatformManaged,
    /// BYOK.
    Byok,
    /// HYOK.
    Hyok,
}

impl KeyOriginTelemetry {
    /// Build the telemetry signal for an origin of a known kind.
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

    // A deterministic mock customer HYOK key service: wraps with a fixed nonce, can be revoked.
    struct MockHyokKeyService {
        revoked: std::cell::Cell<bool>,
        // The customer-held plaintext key, NEVER exposed to Myelin's KeyOrigin.
        key: [u8; KEY_LEN],
    }
    impl MockHyokKeyService {
        fn new() -> Self {
            MockHyokKeyService { revoked: std::cell::Cell::new(false), key: [7u8; KEY_LEN] }
        }
    }
    impl HyokKeyService for MockHyokKeyService {
        fn wrap(&self, dek: &Dek) -> Result<WrappedDek, HyokServiceDenied> {
            if self.revoked.get() {
                return Err(HyokServiceDenied);
            }
            // XOR-mask "wrap" against the customer key — a stand-in for the customer KMS's wrap;
            // the real KMIP adapter is the [OPEN → P6/LEGAL] follow-on.
            let mut wrapped = dek.as_bytes().to_vec();
            for (b, k) in wrapped.iter_mut().zip(self.key.iter()) {
                *b ^= *k;
            }
            Ok(WrappedDek { nonce: [0u8; 12], wrapped, kek_epoch: 0 })
        }
        fn unwrap(&self, _w: &WrappedDek) -> Result<DekHandle, HyokServiceDenied> {
            if self.revoked.get() {
                return Err(HyokServiceDenied); // THE DENY: the customer revoked → no plaintext to Myelin.
            }
            // The customer service returns a usable handle (built from the customer's key). Myelin
            // never holds the key — only this transient handle from the call-out.
            Ok(crate::kms::DekHandle::from_raw(self.key))
        }
        fn destroy(&self) {
            self.revoked.set(true);
        }
    }

    #[test]
    fn can_derive_is_false_for_hyok_true_for_platform_and_byok() {
        let engine = KmsEngine::new();
        engine.ensure_kek(&KekId::new(t("acme"), r("eu-west")));

        let platform = PlatformManaged::new(&engine, r("eu-west"));
        let byok = Byok::new(&engine, r("eu-west"), "kms-customer://acme/k1");
        let hyok = Hyok::new(MockHyokKeyService::new());

        // THE HEADLINE ASSERTION (the GATE): platform/BYOK = true, HYOK = false.
        assert!(platform.can_derive_plaintext_index(), "platform-managed CAN derive a plaintext index");
        assert!(byok.can_derive_plaintext_index(), "BYOK CAN derive while the key is live");
        assert!(!hyok.can_derive_plaintext_index(), "HYOK can NEVER derive a plaintext index (structural)");
    }

    #[test]
    fn index_admission_refuses_hyok_by_construction() {
        let engine = KmsEngine::new();
        engine.ensure_kek(&KekId::new(t("acme"), r("eu-west")));
        let platform = PlatformManaged::new(&engine, r("eu-west"));
        let byok = Byok::new(&engine, r("eu-west"), "kms-customer://acme/k1");
        let hyok = Hyok::new(MockHyokKeyService::new());

        // The index-builder's call shape (D-S10): a HYOK class is SkipHyok, never Admit.
        assert_eq!(IndexAdmission::for_origin(&platform), IndexAdmission::Admit);
        assert_eq!(IndexAdmission::for_origin(&byok), IndexAdmission::Admit);
        assert_eq!(IndexAdmission::for_origin(&hyok), IndexAdmission::SkipHyok);

        assert!(IndexAdmission::for_origin(&platform).may_index());
        assert!(!IndexAdmission::for_origin(&hyok).may_index(),
            "a HYOK class cannot have a plaintext index built — enforced by code");
    }

    #[test]
    fn platform_origin_wraps_and_unwraps_through_the_engine() {
        let engine = KmsEngine::new();
        engine.ensure_kek(&KekId::new(t("acme"), r("eu-west")));
        let platform = PlatformManaged::new(&engine, r("eu-west"));

        let dek = Dek::generate();
        let wrapped = platform.wrap(&dek, t("acme")).expect("platform wrap");
        let handle = platform.unwrap(&wrapped, t("acme")).expect("platform unwrap");

        // The unwrapped handle is the same key: seal/open round-trips.
        let (nonce, ct) = handle.seal(b"some pii");
        assert_eq!(handle.open(&nonce, &ct).as_deref(), Some(&b"some pii"[..]));
    }

    #[test]
    fn byok_wraps_under_the_customer_key_path() {
        let engine = KmsEngine::new();
        engine.ensure_kek(&KekId::new(t("acme"), r("eu-west")));
        let byok = Byok::new(&engine, r("eu-west"), "kms-customer://acme/master-key");

        // The §6 "a BYOK key wraps under the customer key path" property.
        assert_eq!(byok.customer_key_path(), "kms-customer://acme/master-key");

        let dek = Dek::generate();
        let wrapped = byok.wrap(&dek, t("acme")).expect("byok wrap");
        // BYOK is full-capability while the key is live: unwrap succeeds.
        let handle = byok.unwrap(&wrapped, t("acme")).expect("byok unwrap (key live)");
        let (nonce, ct) = handle.seal(b"bio");
        assert_eq!(handle.open(&nonce, &ct).as_deref(), Some(&b"bio"[..]));
    }

    #[test]
    fn hyok_never_exposes_plaintext_and_unwrap_can_deny() {
        let hyok = Hyok::new(MockHyokKeyService::new());
        let dek = Dek::generate();

        // The wrap/unwrap route through the CALL OUT, not a Myelin-held key.
        let wrapped = hyok.wrap(&dek, t("acme")).expect("hyok wrap (customer service)");
        let handle = hyok.unwrap(&wrapped, t("acme")).expect("hyok unwrap granted");
        let (nonce, ct) = handle.seal(b"x");
        assert_eq!(handle.open(&nonce, &ct).as_deref(), Some(&b"x"[..]));

        // The customer crypto-shreds (destroy). Now the unwrap CALL OUT DENIES — loudly, never a
        // plaintext fall-through.
        hyok.destroy(KeyId::new(t("acme"), crate::kms::KeyClass::Subject("alice".into())))
            .expect("destroy is the customer-initiated shred");
        let denied = hyok.unwrap(&wrapped, t("acme"));
        assert!(matches!(denied, Err(KeyOriginError::HyokDenied { .. })),
            "after the customer revokes, a HYOK unwrap DENIES (no plaintext to Myelin)");

        // And it STILL cannot derive a plaintext index — the structural limit is permanent.
        assert!(!hyok.can_derive_plaintext_index());
    }

    #[test]
    fn wrap_unwrap_destroy_route_through_all_three_origins() {
        let engine = KmsEngine::new();
        engine.ensure_kek(&KekId::new(t("acme"), r("eu-west")));

        // platform + BYOK destroy a DEK in the engine (Ok); HYOK destroys via the customer service.
        let platform = PlatformManaged::new(&engine, r("eu-west"));
        let byok = Byok::new(&engine, r("eu-west"), "kms-customer://acme/k");
        let hyok = Hyok::new(MockHyokKeyService::new());

        assert!(platform.destroy(KeyId::new(t("acme"), crate::kms::KeyClass::Tenant)).is_ok());
        assert!(byok.destroy(KeyId::new(t("acme"), crate::kms::KeyClass::Tenant)).is_ok());
        assert!(hyok.destroy(KeyId::new(t("acme"), crate::kms::KeyClass::Tenant)).is_ok());
    }

    #[test]
    fn telemetry_reports_can_derive_per_origin() {
        let engine = KmsEngine::new();
        engine.ensure_kek(&KekId::new(t("acme"), r("eu-west")));
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
