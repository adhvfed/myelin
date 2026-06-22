//! OLTP + blob envelope encryption wired + the classify-driven per-subject/per-tenant key
//! choice (P-ST-08 / global P-095; contracts 11.1 at-rest-encryption half + 11.2 content-key-wrap
//! half + 11.4 the GD-4 classify-driven key-choice rule).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md`
//! §3.1 (the OLTP envelope-encryption seam — personal-data columns under the tenant DEK,
//! free-text/profile columns under a per-subject sub-key),
//! §5.1 (the GD-4 decision rule — `classify(field)` drives the key choice automatically),
//! §3.2 (the per-blob random content key wrapped by the tenant/per-subject DEK).
//! Contract-index rows 11.1 / 11.2 / 11.4 / 10.2.
//!
//! ## What this prompt closes (P-ST-08) — the two floors named upstream
//! 1. **The plaintext-at-rest floor named by P-ST-01** (`oltp.rs`): the personal-data columns of
//!    the OLTP tables now encrypt under the tenant DEK; free-text/profile columns under a
//!    per-SUBJECT sub-key. [`ColumnCryptor`] is the encrypted-column read/write path. After this,
//!    a tagged column is **ciphertext-at-rest** ([`ColumnCryptor::plaintext_at_rest_count`] is the
//!    `plaintext_at_rest_count == 0` telemetry the GATE asserts).
//! 2. **The content-key-wrap floor named by P-ST-03** (`blob.rs`): the [`crate::blob::FsBlobStore`]
//!    per-blob bytes now wrap under the tenant/per-subject DEK via [`DekContentWrap`] — a real
//!    [`crate::blob::ContentWrap`] implementation that REPLACES the [`crate::blob::IdentityWrap`]
//!    plaintext floor (a localised swap; the content address stays plaintext-derived, so nothing
//!    moves).
//!
//! ## The GD-4 classify-driven key choice (11.4 — the headline rule)
//! [`key_class_for`] is the GD-4 decision rule (§5.1) made executable: it reads a field's `erasure`
//! tag (contract 10.2's [`myelin_gdpr::ErasureMethod`]) and the GD-4 granularity intent and returns
//! the [`crate::kms::KeyClass`] the column/blob is keyed under:
//!   - `erasure = CryptoShred("subject_dek")` (or any `subject*` key-class ref) → **per-subject DEK**
//!     ([`KeyClass::Subject`]) — *data whose erasure unit is the individual subject is keyed
//!     per-subject* (one key-destroy = that person's Art. 17 erasure);
//!   - `erasure = CryptoShred("tenant_dek")` / `Pseudonymise` / `PurgeReindex` / `CarveOut` →
//!     **per-tenant DEK** ([`KeyClass::Tenant`]) — *data whose erasure is satisfied by
//!     pseudonymisation/tombstoning is keyed per-tenant*.
//!
//! A field tagged `personal-data, erasure=subject` is thus auto-wired to a per-subject DEK by the
//! harness, exactly the §5.1 rule (the erase ALGORITHM that DESTROYS the chosen key is P-ST-09).
//!
//! ## Floors named (stubbed / deferred + the filling prompt) — VISION §3, prompt DoD
//! - **The CI inline-PII log-segment extension (C1)** is the named **M4 follow-on (P-ST-27)**: the
//!   per-subject DEK class today covers free-text/profile/chat-body/agent-memory; CI log segments
//!   join the per-subject row where isolable in M4. Recorded HERE in writing.
//! - **The real key-resolution wiring through the [`crate::kms::KmsAdapter`] / [`KmsReadPath`]
//!   fail-static read path** is the SAME engine this module already resolves DEKs through ([`crate::kms::KmsEngine`],
//!   P-058) — this prompt uses it directly; the production HSM/Vault backing is the P-058 floor.
//! - **The HYOK structural skip on encryption** rides the [`crate::key_origin::KeyOrigin`] seam
//!   (P-094): an encrypted store consults [`crate::key_origin::IndexAdmission`] before deriving a
//!   plaintext index; encryption itself is origin-agnostic (it always seals). The full Search/Agent
//!   skip drill is D-S10 (with those subsystems).
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2; prompt TESTS field)
//! The classify→key-choice routing ([`key_class_for`]) is mandatory-core: the load-bearing decision
//! is *`erasure=subject` routes to a per-subject DEK and a bulk class to the tenant DEK*. The
//! ciphertext-at-rest property ([`ColumnCryptor::encrypt`] stores ciphertext, never plaintext) is
//! the at-rest-encryption invariant. The achieved score is stated in the P-095 report
//! (`cargo mutants -p myelin-storage -f crates/myelin-storage/src/encryption.rs`).

use myelin_gdpr::ErasureMethod;
use myelin_tenancy::{Region, TenantId};

use crate::blob::ContentWrap;
use crate::kms::{DekHandle, KeyClass, KmsEngine, KmsError, PiiKeyRef, NONCE_LEN};

/// The subject identifier a per-subject DEK is keyed under (the data subject whose Art. 17 erasure
/// is one key-destroy). Carried so the classify→key-choice rule can mint the right
/// [`KeyClass::Subject`] when a column/blob belongs to a known subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubjectId(pub String);

impl SubjectId {
    /// Build a subject id from any string-ish.
    pub fn new(id: impl Into<String>) -> SubjectId {
        SubjectId(id.into())
    }
    /// The underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// **The GD-4 classify-driven key-choice rule (contract 11.4, storage.md §5.1) made executable.**
///
/// Given a field's `erasure` tag (contract 10.2's [`ErasureMethod`]) and the subject the row
/// belongs to (when the data is subject-scoped), return the [`KeyClass`] the column/blob is keyed
/// under. This is *the* rule the harness applies automatically when it encrypts a tagged column:
///
/// - `erasure = CryptoShred(class_ref)` where `class_ref` names a **subject** key class
///   (`"subject_dek"`, `"subject:<id>"`, anything starting `subject`) → **per-subject DEK**
///   ([`KeyClass::Subject`]) IFF a `subject` is known; *data whose erasure unit is the individual
///   subject is keyed per-subject* (§5.1). If the tag says subject but no subject id is supplied,
///   that is a classification error ([`KeyChoiceError::SubjectClassMissingSubject`]) — NEVER a
///   silent downgrade to the tenant key (a downgrade would defeat the GD-4 individual-erasure
///   lever).
/// - `erasure = CryptoShred(class_ref)` where `class_ref` names the **tenant** key class
///   (`"tenant_dek"`, `"tenant"`) → **per-tenant DEK** ([`KeyClass::Tenant`]).
/// - `erasure = Pseudonymise | PurgeReindex | CarveOut` → **per-tenant DEK** ([`KeyClass::Tenant`])
///   — *data whose erasure is satisfied by pseudonymisation/tombstoning is keyed per-tenant* (§5.1).
///
/// The `<class>` payload string convention mirrors the [`PiiKeyRef`] `<class>` grammar
/// (`tenant` | `subject:<id>` | `blob`) so the GDPR `CryptoShred(key_class)` tag and the KMS key
/// class speak the same vocabulary (EI-01 §7 — one vocabulary, not two).
pub fn key_class_for(
    erasure: &ErasureMethod,
    subject: Option<&SubjectId>,
) -> Result<KeyClass, KeyChoiceError> {
    match erasure {
        // The crypto-shred classes name WHICH key-hierarchy class to destroy — the GD-4 lever.
        ErasureMethod::CryptoShred(class_ref) => {
            if names_subject_class(class_ref) {
                // erasure=subject → per-subject DEK. The subject MUST be known (no silent downgrade).
                match subject {
                    Some(s) => Ok(KeyClass::Subject(s.0.clone())),
                    None => Err(KeyChoiceError::SubjectClassMissingSubject(
                        class_ref.clone(),
                    )),
                }
            } else if names_tenant_class(class_ref) {
                Ok(KeyClass::Tenant)
            } else {
                // An unrecognised crypto-shred class ref is a loud classification error — never
                // silently coerced to a default key (a wrong key class is an erasure-reach bug).
                Err(KeyChoiceError::UnknownKeyClass(class_ref.clone()))
            }
        }
        // Pseudonymise / PurgeReindex / CarveOut: erasure is satisfied without a per-subject
        // key-destroy → bulk per-tenant DEK (§5.1).
        ErasureMethod::Pseudonymise | ErasureMethod::PurgeReindex | ErasureMethod::CarveOut => {
            Ok(KeyClass::Tenant)
        }
    }
}

/// Whether a `CryptoShred(<class>)` tag names the per-SUBJECT key class (the GD-4 individual lever).
/// The canonical refs are `subject_dek` and the `subject:<id>` / `subject` grammar.
fn names_subject_class(class_ref: &str) -> bool {
    class_ref == "subject_dek" || class_ref == "subject" || class_ref.starts_with("subject:")
}

/// Whether a `CryptoShred(<class>)` tag names the per-TENANT key class (the bulk lever).
fn names_tenant_class(class_ref: &str) -> bool {
    class_ref == "tenant_dek" || class_ref == "tenant"
}

/// A loud, typed failure of the classify→key-choice rule. A misclassified field is a LOUD error,
/// never a silent fall-through to a wrong (e.g. weaker-erasure) key class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyChoiceError {
    /// The `erasure` tag named the per-subject key class, but no subject id was supplied — the
    /// harness cannot pick a per-subject DEK without knowing the subject. NEVER downgraded to the
    /// tenant key (that would lose the GD-4 individual-erasure lever).
    SubjectClassMissingSubject(String),
    /// The `CryptoShred(<class>)` tag named a key class this rule does not recognise (not
    /// `subject*` and not `tenant*`). Refused loudly — a wrong key class is an erasure-reach bug.
    UnknownKeyClass(String),
    /// The underlying KMS could not provision/resolve the chosen DEK (KEK unavailable / shredded).
    Kms(KmsError),
}

impl core::fmt::Display for KeyChoiceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            KeyChoiceError::SubjectClassMissingSubject(c) => write!(
                f,
                "classify→key-choice: erasure tag names a per-subject key class ({c}) but no \
                 subject id was supplied — refused, NEVER downgraded to the tenant key (that \
                 would lose the GD-4 individual-erasure lever)"
            ),
            KeyChoiceError::UnknownKeyClass(c) => write!(
                f,
                "classify→key-choice: unrecognised CryptoShred key class ({c}) — refused, a \
                 wrong key class is an erasure-reach bug"
            ),
            KeyChoiceError::Kms(e) => write!(f, "classify→key-choice: {e}"),
        }
    }
}

impl std::error::Error for KeyChoiceError {}

impl From<KmsError> for KeyChoiceError {
    fn from(e: KmsError) -> Self {
        KeyChoiceError::Kms(e)
    }
}

// ─────────────────────────────── the OLTP encrypted-column path (11.1) ───────────────────────────

/// A column value stored at rest: the [`PiiKeyRef`] that names WHICH DEK sealed it (so rotation /
/// crypto-shred operate at the key layer while the ciphertext stays put), the AEAD nonce, and the
/// ciphertext+tag. **It carries NO plaintext** — the at-rest form of a personal-data column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedColumn {
    /// The `pii_key_ref` travelling with the ciphertext (`kms://<tenant>/<epoch>/<class>`).
    pub key_ref: PiiKeyRef,
    /// The per-seal AEAD nonce.
    pub nonce: [u8; NONCE_LEN],
    /// The AES-256-GCM ciphertext+tag of the column plaintext.
    pub ciphertext: Vec<u8>,
}

impl EncryptedColumn {
    /// Whether this stored value contains the given plaintext verbatim — the at-rest assertion the
    /// GATE uses to prove `plaintext_at_rest_count == 0` (a real ciphertext never contains the
    /// plaintext bytes for a non-trivial value).
    pub fn contains_plaintext(&self, plaintext: &[u8]) -> bool {
        // A windowed search: the stored ciphertext must not contain the plaintext byte-run.
        if plaintext.is_empty() {
            return false;
        }
        self.ciphertext
            .windows(plaintext.len())
            .any(|w| w == plaintext)
    }
}

/// **The OLTP encrypted-column read/write path (contract 11.1 at-rest half, storage.md §3.1).**
/// Closes the plaintext-at-rest floor P-ST-01 named: a personal-data column written through
/// [`ColumnCryptor::encrypt`] is sealed under the classify-chosen DEK (per-subject or per-tenant)
/// and stored as an [`EncryptedColumn`] (ciphertext-at-rest); [`ColumnCryptor::decrypt`] resolves
/// the DEK named by the column's `pii_key_ref` and opens it. It fronts the P-058
/// [`KmsEngine`] (never a parallel key store) so rotation/crypto-shred reach these columns by
/// construction.
pub struct ColumnCryptor<'a> {
    engine: &'a KmsEngine,
    region: Region,
    /// Count of personal-data columns observed plaintext-at-rest — the `plaintext_at_rest_count`
    /// telemetry the GATE asserts is **0** for a tagged column. It is incremented ONLY if a tagged
    /// column is ever stored without going through [`Self::encrypt`] (the [`Self::audit_plaintext`]
    /// path) — a defence-in-depth detector, not a normal path. An atomic so [`ColumnCryptor`] (and
    /// therefore [`DekContentWrap`]) is `Send + Sync` (the [`ContentWrap`] trait bound).
    plaintext_at_rest: std::sync::atomic::AtomicU64,
}

impl<'a> ColumnCryptor<'a> {
    /// Front the KMS engine for a region (the region the tenant's KEK lives in).
    pub fn new(engine: &'a KmsEngine, region: Region) -> ColumnCryptor<'a> {
        ColumnCryptor {
            engine,
            region,
            plaintext_at_rest: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Encrypt a personal-data column value, choosing the DEK by the field's `erasure` tag (the
    /// GD-4 classify-driven rule, [`key_class_for`]). Ensures the chosen DEK exists, seals the
    /// plaintext under it, and returns the [`EncryptedColumn`] stored at rest (ciphertext only).
    ///
    /// `subject` is the row's data subject when the field is subject-scoped (a free-text/profile
    /// column); `None` for a bulk/tenant column. A subject-class tag with no subject is a LOUD
    /// [`KeyChoiceError::SubjectClassMissingSubject`] — never a silent tenant-key downgrade.
    pub fn encrypt(
        &self,
        tenant: &TenantId,
        subject: Option<&SubjectId>,
        erasure: &ErasureMethod,
        plaintext: &[u8],
    ) -> Result<EncryptedColumn, KeyChoiceError> {
        // (1) GD-4: the erasure tag drives the key class (per-subject vs per-tenant).
        let class = key_class_for(erasure, subject)?;
        // (2) Ensure the chosen DEK exists, get its pii_key_ref (the epoch travels with the value).
        let key_ref = self
            .engine
            .ensure_dek(tenant, &self.region, class)
            .map_err(KeyChoiceError::Kms)?;
        // (3) Resolve the DEK and seal — store CIPHERTEXT, never plaintext.
        let dek = self
            .engine
            .resolve_dek(&key_ref, &self.region)
            .map_err(KeyChoiceError::Kms)?;
        let (nonce, ciphertext) = dek.seal(plaintext);
        Ok(EncryptedColumn {
            key_ref,
            nonce,
            ciphertext,
        })
    }

    /// Decrypt a stored [`EncryptedColumn`] back to its plaintext — resolve the DEK named by the
    /// column's `pii_key_ref` and open the ciphertext. A crypto-shredded key fails LOUDLY
    /// ([`KeyChoiceError::Kms`]) — the column is unrecoverable (the GD-4 erase lever working), NEVER
    /// a plaintext-without-key fall-through.
    pub fn decrypt(&self, column: &EncryptedColumn) -> Result<Vec<u8>, KeyChoiceError> {
        let dek: DekHandle = self
            .engine
            .resolve_dek(&column.key_ref, &self.region)
            .map_err(KeyChoiceError::Kms)?;
        dek.open(&column.nonce, &column.ciphertext)
            .ok_or(KeyChoiceError::Kms(KmsError::UnwrapFailed(
                crate::kms::DekId::new(column.key_ref.tenant.clone(), column.key_ref.class.clone()),
            )))
    }

    /// Defence-in-depth: record that a tagged column was observed plaintext-at-rest (it bypassed
    /// [`Self::encrypt`]). In a correct flow this is NEVER called; the GATE asserts the counter
    /// stays **0**. Exposed so an at-rest scanner / migration can flag a leak loudly.
    pub fn audit_plaintext(&self) {
        self.plaintext_at_rest
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// The `plaintext_at_rest_count` telemetry the GATE asserts is **0** for tagged columns.
    pub fn plaintext_at_rest_count(&self) -> u64 {
        self.plaintext_at_rest
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

// ─────────────────────────────── the blob content-key wrap (11.2) ───────────────────────────────

/// **The real per-blob content-key wrap (contract 11.2 content-key-wrap half, storage.md §3.2).**
/// A [`ContentWrap`] implementation that REPLACES the [`crate::blob::IdentityWrap`] plaintext floor
/// P-ST-03 named: blob bytes are sealed under the tenant/per-subject DEK before they rest. The
/// content address stays **plaintext-derived** (computed by [`crate::blob::FsBlobStore::put`] BEFORE
/// the wrap), so swapping this in moves no content address — exactly the localised swap the
/// `ContentWrap` seam was built for.
///
/// **The §3.2 stable-content-address property:** a per-blob random content key would normally wrap
/// under the DEK; on this floor we seal the bytes directly under the classify-chosen DEK (a
/// per-blob random key is the production refinement — the wrapping LAYER, not the address, which is
/// already plaintext-derived). The blob's key class is chosen by [`key_class_for`] exactly like a
/// column: a subject-scoped blob (a profile avatar) under the per-subject DEK, a bulk blob under
/// the tenant DEK.
pub struct DekContentWrap {
    /// The KMS engine, held as an [`Arc`] so the wrap is `'static` and can be installed into a
    /// [`crate::blob::FsBlobStore`] (whose `with_wrap` takes `Box<dyn ContentWrap>` = `'static`).
    /// It is the SAME P-058 engine the column path uses — rotation/crypto-shred reach blobs too.
    engine: std::sync::Arc<KmsEngine>,
    region: Region,
    erasure: ErasureMethod,
    subject: Option<SubjectId>,
}

impl DekContentWrap {
    /// Build a content wrap that seals blobs under the DEK chosen by `erasure` (+ `subject` when
    /// subject-scoped). For a per-tenant (bulk) blob class pass `ErasureMethod::PurgeReindex` (or
    /// `CryptoShred("tenant_dek")`) and `subject = None`; for a per-subject blob pass
    /// `CryptoShred("subject_dek")` + the subject. The engine is shared via [`Arc`] (the same engine
    /// the [`ColumnCryptor`] uses — never a parallel key store).
    pub fn new(
        engine: std::sync::Arc<KmsEngine>,
        region: Region,
        erasure: ErasureMethod,
        subject: Option<SubjectId>,
    ) -> DekContentWrap {
        DekContentWrap {
            engine,
            region,
            erasure,
            subject,
        }
    }

    /// Seal blob bytes under the classify-chosen DEK into a self-framed stored record (key_ref +
    /// nonce + ciphertext), or fail loudly. The framing lets `unwrap` resolve the exact DEK that
    /// sealed these bytes (rotation/shred operate at the key layer).
    fn seal(&self, tenant: &TenantId, plaintext: &[u8]) -> Result<Vec<u8>, KeyChoiceError> {
        let cryptor = ColumnCryptor::new(&self.engine, self.region.clone());
        let col = cryptor.encrypt(tenant, self.subject.as_ref(), &self.erasure, plaintext)?;
        Ok(frame(&col))
    }

    /// Open a sealed blob record back to plaintext, or fail loudly (a shredded key → unrecoverable).
    fn open(&self, stored: &[u8]) -> Result<Vec<u8>, KeyChoiceError> {
        let col = unframe(stored).ok_or(KeyChoiceError::UnknownKeyClass(
            "corrupt blob envelope frame".to_string(),
        ))?;
        let cryptor = ColumnCryptor::new(&self.engine, self.region.clone());
        cryptor.decrypt(&col)
    }
}

impl ContentWrap for DekContentWrap {
    /// Wrap (encrypt) blob plaintext under the classify-chosen DEK. The [`ContentWrap`] trait's
    /// `wrap` is infallible by signature, but a wrap MUST NEVER fall back to storing plaintext on a
    /// key error (that would re-open the plaintext-at-rest floor). On a key error this panics
    /// LOUDLY — a service must not persist an un-encryptable personal-data blob (fail-closed, never
    /// fail-open). The fallible [`Self::seal`] is the path a caller that wants the typed error uses.
    fn wrap(&self, tenant: &TenantId, plaintext: &[u8]) -> Vec<u8> {
        self.seal(tenant, plaintext).unwrap_or_else(|e| {
            panic!(
                "blob content-key wrap FAILED ({e}) — refusing to store an un-encryptable \
                 personal-data blob as plaintext (fail-closed, NEVER fail-open / plaintext-at-rest)"
            )
        })
    }

    /// Unwrap (decrypt) the stored ciphertext back to plaintext. Panics LOUDLY on a key error (a
    /// crypto-shredded blob is unrecoverable — the GD-4 erase lever — and that must surface, never
    /// a silent empty/garbage serve).
    fn unwrap(&self, _tenant: &TenantId, stored: &[u8]) -> Vec<u8> {
        self.open(stored).unwrap_or_else(|e| {
            panic!(
                "blob content-key UNWRAP failed ({e}) — the blob is unrecoverable (crypto-shred) \
                 or the envelope is corrupt; refusing a silent wrong-bytes serve"
            )
        })
    }
}

/// Frame an [`EncryptedColumn`] into a self-describing stored byte record:
/// `<key_ref_uri>\n<nonce:12><ciphertext>`. The `\n` separates the textual key ref from the binary
/// nonce+ciphertext (a key ref never contains `\n`). This is the at-rest blob envelope.
fn frame(col: &EncryptedColumn) -> Vec<u8> {
    let mut out = col.key_ref.to_uri().into_bytes();
    out.push(b'\n');
    out.extend_from_slice(&col.nonce);
    out.extend_from_slice(&col.ciphertext);
    out
}

/// Parse a framed blob envelope back to an [`EncryptedColumn`]. `None` on any malformation (a
/// corrupt frame is a loud failure, never a silent wrong-key open).
fn unframe(stored: &[u8]) -> Option<EncryptedColumn> {
    let nl = stored.iter().position(|&b| b == b'\n')?;
    let uri = std::str::from_utf8(&stored[..nl]).ok()?;
    let key_ref = PiiKeyRef::parse(uri)?;
    let rest = &stored[nl + 1..];
    if rest.len() < NONCE_LEN {
        return None;
    }
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&rest[..NONCE_LEN]);
    let ciphertext = rest[NONCE_LEN..].to_vec();
    Some(EncryptedColumn {
        key_ref,
        nonce,
        ciphertext,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::{BlobStore, ContentHash, FsBlobStore};
    use crate::kms::KekId;

    fn t(s: &str) -> TenantId {
        TenantId(s.to_string())
    }
    fn r() -> Region {
        Region("eu-west".to_string())
    }
    fn engine_for(tenant: &TenantId) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant.clone(), r()));
        kms
    }
    fn arc_engine_for(tenant: &TenantId) -> std::sync::Arc<KmsEngine> {
        std::sync::Arc::new(engine_for(tenant))
    }

    // ───────────── 11.4 — the GD-4 classify→key-choice rule ─────────────

    #[test]
    fn classify_erasure_subject_routes_to_a_per_subject_dek() {
        // erasure=CryptoShred("subject_dek") + a known subject → KeyClass::Subject(that id).
        let class = key_class_for(
            &ErasureMethod::CryptoShred("subject_dek".into()),
            Some(&SubjectId::new("u-42")),
        )
        .expect("subject class with a subject");
        assert_eq!(class, KeyClass::Subject("u-42".into()));
    }

    #[test]
    fn classify_erasure_subject_grammar_variants_route_to_subject() {
        // The `subject:<id>` / `subject` grammar all name the per-subject class.
        for tag in ["subject", "subject:alice", "subject_dek"] {
            let class = key_class_for(
                &ErasureMethod::CryptoShred(tag.into()),
                Some(&SubjectId::new("alice")),
            )
            .expect("subject-class variant");
            assert_eq!(class, KeyClass::Subject("alice".into()), "tag {tag}");
        }
    }

    #[test]
    fn classify_bulk_routes_to_the_tenant_dek() {
        // The tenant crypto-shred class → tenant DEK.
        assert_eq!(
            key_class_for(&ErasureMethod::CryptoShred("tenant_dek".into()), None).unwrap(),
            KeyClass::Tenant
        );
        // Pseudonymise / PurgeReindex / CarveOut → tenant DEK (erasure satisfied without key-destroy).
        for e in [
            ErasureMethod::Pseudonymise,
            ErasureMethod::PurgeReindex,
            ErasureMethod::CarveOut,
        ] {
            assert_eq!(key_class_for(&e, None).unwrap(), KeyClass::Tenant, "{e:?}");
            // ...and a bulk class with a subject present still routes to tenant (bulk is bulk).
            assert_eq!(
                key_class_for(&e, Some(&SubjectId::new("u-1"))).unwrap(),
                KeyClass::Tenant
            );
        }
    }

    #[test]
    fn classify_subject_tag_without_a_subject_is_a_loud_error_never_a_tenant_downgrade() {
        // The GD-4 lever depends on this: a subject-class tag with no subject MUST be a loud error,
        // NEVER silently downgraded to the tenant key (that would lose per-subject erasure).
        let err = key_class_for(&ErasureMethod::CryptoShred("subject_dek".into()), None)
            .expect_err("subject class with no subject is an error");
        assert_eq!(
            err,
            KeyChoiceError::SubjectClassMissingSubject("subject_dek".into())
        );
        assert!(err
            .to_string()
            .contains("NEVER downgraded to the tenant key"));
    }

    #[test]
    fn classify_unknown_crypto_shred_class_is_refused_loudly() {
        let err = key_class_for(&ErasureMethod::CryptoShred("mystery_dek".into()), None)
            .expect_err("unknown class is refused");
        assert_eq!(err, KeyChoiceError::UnknownKeyClass("mystery_dek".into()));
    }

    // ───────────── 11.1 — the OLTP encrypted-column path (plaintext floor closed) ─────────────

    #[test]
    fn personal_data_column_is_ciphertext_at_rest_subject_class() {
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        let cryptor = ColumnCryptor::new(&kms, r());

        let plaintext = b"alice@example.test"; // a free-text/profile PII column value.
        let col = cryptor
            .encrypt(
                &tenant,
                Some(&SubjectId::new("u-alice")),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                plaintext,
            )
            .expect("encrypt under the per-subject DEK");

        // The stored column is keyed under the per-SUBJECT DEK (GD-4).
        assert_eq!(col.key_ref.class, KeyClass::Subject("u-alice".into()));
        assert_eq!(col.key_ref.tenant, tenant);
        // CIPHERTEXT-AT-REST: the stored bytes do NOT contain the plaintext (the floor closed).
        assert!(
            !col.contains_plaintext(plaintext),
            "a tagged column must be ciphertext-at-rest (the plaintext-at-rest floor is closed)"
        );
        // The telemetry the GATE reads: 0 plaintext-at-rest for the tagged column.
        assert_eq!(cryptor.plaintext_at_rest_count(), 0);

        // ...and it round-trips back to the exact plaintext on decrypt.
        assert_eq!(cryptor.decrypt(&col).expect("decrypt"), plaintext);
    }

    #[test]
    fn bulk_column_is_ciphertext_under_the_tenant_dek() {
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        let cryptor = ColumnCryptor::new(&kms, r());

        let plaintext = b"PR-1234 metadata"; // a bulk tenant-content column.
        let col = cryptor
            .encrypt(&tenant, None, &ErasureMethod::PurgeReindex, plaintext)
            .expect("encrypt under the tenant DEK");
        assert_eq!(col.key_ref.class, KeyClass::Tenant);
        assert!(!col.contains_plaintext(plaintext));
        assert_eq!(cryptor.decrypt(&col).expect("decrypt"), plaintext);
    }

    #[test]
    fn a_subject_column_does_not_open_under_a_different_subjects_dek() {
        // The GD-4 individual-erasure lever depends on per-subject key isolation: u-1's column is
        // sealed under u-1's DEK; resolving u-2's DEK cannot open it.
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        let cryptor = ColumnCryptor::new(&kms, r());

        let c1 = cryptor
            .encrypt(
                &tenant,
                Some(&SubjectId::new("u-1")),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                b"u-1 bio",
            )
            .unwrap();
        // Re-tag the stored column to u-2's key ref (a forged/wrong key ref) and try to open.
        let forged = EncryptedColumn {
            key_ref: cryptor
                .engine
                .ensure_dek(&tenant, &r(), KeyClass::Subject("u-2".into()))
                .unwrap(),
            ..c1.clone()
        };
        assert!(
            cryptor.decrypt(&forged).is_err(),
            "u-1's ciphertext must NOT open under u-2's DEK (per-subject isolation)"
        );
    }

    #[test]
    fn crypto_shredding_the_subject_dek_makes_the_column_unrecoverable() {
        // The GD-4 erase lever (the algorithm is P-ST-09, but the property holds here): destroy the
        // per-subject DEK → the column is unrecoverable, a LOUD error, never plaintext.
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        let cryptor = ColumnCryptor::new(&kms, r());

        let col = cryptor
            .encrypt(
                &tenant,
                Some(&SubjectId::new("u-erase")),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                b"to be forgotten",
            )
            .unwrap();
        assert!(cryptor.decrypt(&col).is_ok(), "decrypts before the shred");

        // Crypto-shred u-erase's DEK (the GD-4 individual lever).
        assert!(kms.destroy_dek(&crate::kms::DekId::new(
            tenant.clone(),
            KeyClass::Subject("u-erase".into())
        )));

        // Now the column is unrecoverable — loud, never a plaintext fall-through.
        assert!(matches!(cryptor.decrypt(&col), Err(KeyChoiceError::Kms(_))));
    }

    // ───────────── 11.2 — the blob content-key wrap (content-key-wrap floor closed) ─────────────

    #[test]
    fn blob_content_key_wraps_under_the_tenant_dek_and_round_trips() {
        let tenant = t("acme");
        let kms = arc_engine_for(&tenant);
        // A bulk blob class → tenant DEK.
        let wrap = DekContentWrap::new(kms.clone(), r(), ErasureMethod::PurgeReindex, None);
        let store = FsBlobStore::with_wrap(Box::new(wrap));

        let plaintext = b"a repo object's bytes";
        let h = store
            .put(&tenant, plaintext)
            .expect("put through the DEK wrap");

        // The content address is the PLAINTEXT hash (stable across the real wrap — store ciphertext).
        assert_eq!(h, ContentHash::blake3(plaintext));
        // The stored bytes are NOT the plaintext (ciphertext-at-rest — the content-key-wrap floor
        // is closed).
        {
            let stored = store.head(&tenant, &h).expect("head").stored_len;
            // The framed envelope (key_ref + nonce + ciphertext+tag) is strictly larger than the
            // plaintext, and re-hash-on-read proves it decrypts back to the exact bytes.
            assert!(
                stored > plaintext.len(),
                "stored is the ciphertext envelope, not plaintext"
            );
        }
        // get unwraps (decrypts) + re-hash-verifies and returns the exact plaintext.
        assert_eq!(store.get(&tenant, &h).expect("get round-trips"), plaintext);
    }

    #[test]
    fn blob_content_key_wraps_under_a_per_subject_dek() {
        let tenant = t("acme");
        let kms = arc_engine_for(&tenant);
        // A subject-scoped blob (e.g. a profile avatar) → per-subject DEK.
        let wrap = DekContentWrap::new(
            kms.clone(),
            r(),
            ErasureMethod::CryptoShred("subject_dek".into()),
            Some(SubjectId::new("u-avatar")),
        );
        let store = FsBlobStore::with_wrap(Box::new(wrap));

        let plaintext = b"avatar png bytes";
        let h = store.put(&tenant, plaintext).expect("put");
        assert_eq!(h, ContentHash::blake3(plaintext));
        assert_eq!(store.get(&tenant, &h).expect("get"), plaintext);

        // Crypto-shred the subject DEK → the blob is unrecoverable (the GD-4 lever reaches blobs).
        assert!(kms.destroy_dek(&crate::kms::DekId::new(
            tenant.clone(),
            KeyClass::Subject("u-avatar".into())
        )));
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| store.get(&tenant, &h)));
        assert!(
            result.is_err(),
            "a crypto-shredded blob's unwrap must fail LOUDLY (unrecoverable), never silent serve"
        );
    }

    #[test]
    fn the_stored_blob_bytes_never_contain_the_plaintext() {
        let tenant = t("acme");
        let kms = arc_engine_for(&tenant);
        let wrap = DekContentWrap::new(kms.clone(), r(), ErasureMethod::PurgeReindex, None);

        let plaintext = b"super-secret-marker-string";
        let stored = wrap.wrap(&tenant, plaintext);
        // The framed envelope must not contain the plaintext byte-run anywhere (ciphertext-at-rest).
        assert!(
            !stored.windows(plaintext.len()).any(|w| w == plaintext),
            "stored blob bytes must be ciphertext, never contain the plaintext"
        );
        // ...and it round-trips.
        assert_eq!(wrap.unwrap(&tenant, &stored), plaintext);
    }

    #[test]
    fn contains_plaintext_detects_a_plaintext_run_and_ignores_absence() {
        // Kills the `contains_plaintext -> false` mutant: it must return TRUE when the byte-run is
        // present (a real leak detector), FALSE when absent / for an empty needle.
        let col = EncryptedColumn {
            key_ref: PiiKeyRef::new(t("acme"), 0, KeyClass::Tenant),
            nonce: [0u8; NONCE_LEN],
            ciphertext: b"--SECRET--padding".to_vec(),
        };
        assert!(
            col.contains_plaintext(b"SECRET"),
            "must detect a present plaintext run"
        );
        assert!(
            !col.contains_plaintext(b"ABSENT"),
            "must not false-positive on an absent run"
        );
        assert!(
            !col.contains_plaintext(b""),
            "an empty needle is never 'contained'"
        );
    }

    #[test]
    fn unframe_accepts_an_exactly_nonce_length_tail_empty_ciphertext() {
        // Kills the `< with <=` mutant in unframe's length guard: a frame whose binary tail is
        // EXACTLY a nonce (an empty-ciphertext column) is VALID and must parse — `<=` would wrongly
        // reject it. (An AEAD seal of empty input still produces a 16-byte tag, so this is a
        // hand-built edge frame proving the boundary.)
        let mut framed = b"kms://acme/0/tenant\n".to_vec();
        framed.extend_from_slice(&[0u8; NONCE_LEN]); // exactly a nonce, zero ciphertext bytes.
        let col =
            unframe(&framed).expect("an exactly-nonce-length tail is a valid (empty-ct) frame");
        assert!(col.ciphertext.is_empty());
        // One byte short of a full nonce is still rejected (the guard still rejects too-short).
        assert!(unframe(&framed[..framed.len() - 1]).is_none());
    }

    #[test]
    fn subject_id_as_str_returns_the_id() {
        // Kills the `as_str -> ""` / `"xyzzy"` mutants: the accessor must return the real id.
        assert_eq!(SubjectId::new("u-99").as_str(), "u-99");
    }

    #[test]
    fn frame_unframe_round_trips_and_rejects_corruption() {
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        let cryptor = ColumnCryptor::new(&kms, r());
        let col = cryptor
            .encrypt(&tenant, None, &ErasureMethod::PurgeReindex, b"x")
            .unwrap();
        let framed = frame(&col);
        assert_eq!(unframe(&framed).expect("round-trip"), col);
        // A frame with no newline / a too-short binary tail is rejected (loud, never wrong-key open).
        assert!(unframe(b"no-newline-here").is_none());
        assert!(
            unframe(b"kms://acme/0/tenant\n\x00").is_none(),
            "tail shorter than a nonce"
        );
    }

    #[test]
    fn key_choice_error_display_is_loud_and_specific() {
        // Kills the Display `fmt → Ok(())` mutants: each variant names its loud failure.
        let e = KeyChoiceError::SubjectClassMissingSubject("subject_dek".into());
        assert!(e.to_string().contains("subject_dek") && e.to_string().contains("GD-4"));
        let e = KeyChoiceError::UnknownKeyClass("zzz".into());
        assert!(e.to_string().contains("zzz") && e.to_string().contains("erasure-reach bug"));
        let e = KeyChoiceError::Kms(KmsError::KekUnavailable(KekId::new(t("acme"), r())));
        assert!(e.to_string().contains("classify→key-choice"));
    }

    #[test]
    fn audit_plaintext_is_the_defence_in_depth_counter() {
        // The GATE asserts 0 in the normal path; this proves the counter is real (a leak detector).
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        let cryptor = ColumnCryptor::new(&kms, r());
        assert_eq!(cryptor.plaintext_at_rest_count(), 0);
        cryptor.audit_plaintext();
        assert_eq!(
            cryptor.plaintext_at_rest_count(),
            1,
            "the leak detector counts a plaintext-at-rest"
        );
    }
}
