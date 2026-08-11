use myelin_gdpr::ErasureMethod;
use myelin_tenancy::{Region, TenantId};

use crate::blob::ContentWrap;
use crate::kms::{DekHandle, KeyClass, KmsEngine, KmsError, PiiKeyRef, NONCE_LEN};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubjectId(pub String);

impl SubjectId {
    pub fn new(id: impl Into<String>) -> SubjectId {
        SubjectId(id.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn key_class_for(
    erasure: &ErasureMethod,
    subject: Option<&SubjectId>,
) -> Result<KeyClass, KeyChoiceError> {
    match erasure {
        ErasureMethod::CryptoShred(class_ref) => {
            if names_subject_class(class_ref) {
                match subject {
                    Some(s) => Ok(KeyClass::Subject(s.0.clone())),
                    None => Err(KeyChoiceError::SubjectClassMissingSubject(
                        class_ref.clone(),
                    )),
                }
            } else if names_tenant_class(class_ref) {
                Ok(KeyClass::Tenant)
            } else {
                Err(KeyChoiceError::UnknownKeyClass(class_ref.clone()))
            }
        }
        ErasureMethod::Pseudonymise | ErasureMethod::PurgeReindex | ErasureMethod::CarveOut => {
            Ok(KeyClass::Tenant)
        }
    }
}

fn names_subject_class(class_ref: &str) -> bool {
    class_ref == "subject_dek" || class_ref == "subject" || class_ref.starts_with("subject:")
}

fn names_tenant_class(class_ref: &str) -> bool {
    class_ref == "tenant_dek" || class_ref == "tenant"
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyChoiceError {
    SubjectClassMissingSubject(String),
    UnknownKeyClass(String),
    Kms(KmsError),
}

impl core::fmt::Display for KeyChoiceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            KeyChoiceError::SubjectClassMissingSubject(c) => write!(
                f,
                "classify→key-choice: erasure tag names a per-subject key class ({c}) but no \
                 subject id was supplied - refused, NEVER downgraded to the tenant key (that \
                 would lose the GD-4 individual-erasure lever)"
            ),
            KeyChoiceError::UnknownKeyClass(c) => write!(
                f,
                "classify→key-choice: unrecognised CryptoShred key class ({c}) - refused, a \
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedColumn {
    pub key_ref: PiiKeyRef,
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

impl EncryptedColumn {
    pub fn contains_plaintext(&self, plaintext: &[u8]) -> bool {
        if plaintext.is_empty() {
            return false;
        }
        self.ciphertext
            .windows(plaintext.len())
            .any(|w| w == plaintext)
    }
}

pub struct ColumnCryptor<'a> {
    engine: &'a KmsEngine,
    region: Region,
    plaintext_at_rest: std::sync::atomic::AtomicU64,
}

impl<'a> ColumnCryptor<'a> {
    pub fn new(engine: &'a KmsEngine, region: Region) -> ColumnCryptor<'a> {
        ColumnCryptor {
            engine,
            region,
            plaintext_at_rest: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn encrypt(
        &self,
        tenant: &TenantId,
        subject: Option<&SubjectId>,
        erasure: &ErasureMethod,
        plaintext: &[u8],
    ) -> Result<EncryptedColumn, KeyChoiceError> {
        self.encrypt_with_aad(tenant, subject, erasure, plaintext, &[])
    }

    pub fn encrypt_with_aad(
        &self,
        tenant: &TenantId,
        subject: Option<&SubjectId>,
        erasure: &ErasureMethod,
        plaintext: &[u8],
        aad: &[u8],
    ) -> Result<EncryptedColumn, KeyChoiceError> {
        let class = key_class_for(erasure, subject)?;
        let key_ref = self
            .engine
            .ensure_dek(tenant, &self.region, class)
            .map_err(KeyChoiceError::Kms)?;
        let dek = self
            .engine
            .resolve_dek(&key_ref, &self.region)
            .map_err(KeyChoiceError::Kms)?;
        let (nonce, ciphertext) = dek.seal_with_aad(plaintext, aad);
        Ok(EncryptedColumn {
            key_ref,
            nonce,
            ciphertext,
        })
    }

    pub fn decrypt(&self, column: &EncryptedColumn) -> Result<Vec<u8>, KeyChoiceError> {
        self.decrypt_with_aad(column, &[])
    }

    pub fn decrypt_with_aad(
        &self,
        column: &EncryptedColumn,
        aad: &[u8],
    ) -> Result<Vec<u8>, KeyChoiceError> {
        let dek: DekHandle = self
            .engine
            .resolve_dek(&column.key_ref, &self.region)
            .map_err(KeyChoiceError::Kms)?;
        dek.open_with_aad(&column.nonce, &column.ciphertext, aad)
            .ok_or(KeyChoiceError::Kms(KmsError::UnwrapFailed(
                crate::kms::DekId::new(column.key_ref.tenant.clone(), column.key_ref.class.clone()),
            )))
    }

    pub fn audit_plaintext(&self) {
        self.plaintext_at_rest
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn plaintext_at_rest_count(&self) -> u64 {
        self.plaintext_at_rest
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

pub struct DekContentWrap {
    engine: std::sync::Arc<KmsEngine>,
    region: Region,
    erasure: ErasureMethod,
    subject: Option<SubjectId>,
}

impl DekContentWrap {
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

    fn seal(&self, tenant: &TenantId, plaintext: &[u8]) -> Result<Vec<u8>, KeyChoiceError> {
        let cryptor = ColumnCryptor::new(&self.engine, self.region.clone());
        let col = cryptor.encrypt(tenant, self.subject.as_ref(), &self.erasure, plaintext)?;
        Ok(frame(&col))
    }

    fn open(&self, stored: &[u8]) -> Result<Vec<u8>, KeyChoiceError> {
        let col = unframe(stored).ok_or(KeyChoiceError::UnknownKeyClass(
            "corrupt blob envelope frame".to_string(),
        ))?;
        let cryptor = ColumnCryptor::new(&self.engine, self.region.clone());
        cryptor.decrypt(&col)
    }
}

impl ContentWrap for DekContentWrap {
    fn wrap(&self, tenant: &TenantId, plaintext: &[u8]) -> Vec<u8> {
        self.seal(tenant, plaintext).unwrap_or_else(|e| {
            panic!(
                "blob content-key wrap FAILED ({e}) - refusing to store an un-encryptable \
                 personal-data blob as plaintext (fail-closed, NEVER fail-open / plaintext-at-rest)"
            )
        })
    }

    fn unwrap(&self, _tenant: &TenantId, stored: &[u8]) -> Vec<u8> {
        self.open(stored).unwrap_or_else(|e| {
            panic!(
                "blob content-key UNWRAP failed ({e}) - the blob is unrecoverable (crypto-shred) \
                 or the envelope is corrupt; refusing a silent wrong-bytes serve"
            )
        })
    }
}

fn frame(col: &EncryptedColumn) -> Vec<u8> {
    let mut out = col.key_ref.to_uri().into_bytes();
    out.push(b'\n');
    out.extend_from_slice(&col.nonce);
    out.extend_from_slice(&col.ciphertext);
    out
}

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
        kms.ensure_kek(&KekId::new(tenant.clone(), r()))
            .expect("seed the in-memory KEK");
        kms
    }
    fn arc_engine_for(tenant: &TenantId) -> std::sync::Arc<KmsEngine> {
        std::sync::Arc::new(engine_for(tenant))
    }

    #[test]
    fn classify_erasure_subject_routes_to_a_per_subject_dek() {
        let class = key_class_for(
            &ErasureMethod::CryptoShred("subject_dek".into()),
            Some(&SubjectId::new("u-42")),
        )
        .expect("subject class with a subject");
        assert_eq!(class, KeyClass::Subject("u-42".into()));
    }

    #[test]
    fn classify_erasure_subject_grammar_variants_route_to_subject() {
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
        assert_eq!(
            key_class_for(&ErasureMethod::CryptoShred("tenant_dek".into()), None).unwrap(),
            KeyClass::Tenant
        );
        for e in [
            ErasureMethod::Pseudonymise,
            ErasureMethod::PurgeReindex,
            ErasureMethod::CarveOut,
        ] {
            assert_eq!(key_class_for(&e, None).unwrap(), KeyClass::Tenant, "{e:?}");
            assert_eq!(
                key_class_for(&e, Some(&SubjectId::new("u-1"))).unwrap(),
                KeyClass::Tenant
            );
        }
    }

    #[test]
    fn classify_subject_tag_without_a_subject_is_a_loud_error_never_a_tenant_downgrade() {
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

    #[test]
    fn personal_data_column_is_ciphertext_at_rest_subject_class() {
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        let cryptor = ColumnCryptor::new(&kms, r());

        let plaintext = b"alice@example.test";
        let col = cryptor
            .encrypt(
                &tenant,
                Some(&SubjectId::new("u-alice")),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                plaintext,
            )
            .expect("encrypt under the per-subject DEK");

        assert_eq!(col.key_ref.class, KeyClass::Subject("u-alice".into()));
        assert_eq!(col.key_ref.tenant, tenant);
        assert!(
            !col.contains_plaintext(plaintext),
            "a tagged column must be ciphertext-at-rest (the plaintext-at-rest floor is closed)"
        );
        assert_eq!(cryptor.plaintext_at_rest_count(), 0);

        assert_eq!(cryptor.decrypt(&col).expect("decrypt"), plaintext);
    }

    #[test]
    fn bulk_column_is_ciphertext_under_the_tenant_dek() {
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        let cryptor = ColumnCryptor::new(&kms, r());

        let plaintext = b"PR-1234 metadata";
        let col = cryptor
            .encrypt(&tenant, None, &ErasureMethod::PurgeReindex, plaintext)
            .expect("encrypt under the tenant DEK");
        assert_eq!(col.key_ref.class, KeyClass::Tenant);
        assert!(!col.contains_plaintext(plaintext));
        assert_eq!(cryptor.decrypt(&col).expect("decrypt"), plaintext);
    }

    #[test]
    fn a_subject_column_does_not_open_under_a_different_subjects_dek() {
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

        assert!(kms.destroy_dek(&crate::kms::DekId::new(
            tenant.clone(),
            KeyClass::Subject("u-erase".into())
        )));

        assert!(matches!(cryptor.decrypt(&col), Err(KeyChoiceError::Kms(_))));
    }

    #[test]
    fn blob_content_key_wraps_under_the_tenant_dek_and_round_trips() {
        let tenant = t("acme");
        let kms = arc_engine_for(&tenant);
        let wrap = DekContentWrap::new(kms.clone(), r(), ErasureMethod::PurgeReindex, None);
        let store = FsBlobStore::with_wrap(Box::new(wrap));

        let plaintext = b"a repo object's bytes";
        let h = store
            .put(&tenant, plaintext)
            .expect("put through the DEK wrap");

        assert_eq!(h, ContentHash::blake3(plaintext));
        {
            let stored = store.head(&tenant, &h).expect("head").stored_len;
            assert!(
                stored > plaintext.len(),
                "stored is the ciphertext envelope, not plaintext"
            );
        }
        assert_eq!(store.get(&tenant, &h).expect("get round-trips"), plaintext);
    }

    #[test]
    fn blob_content_key_wraps_under_a_per_subject_dek() {
        let tenant = t("acme");
        let kms = arc_engine_for(&tenant);
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
        assert!(
            !stored.windows(plaintext.len()).any(|w| w == plaintext),
            "stored blob bytes must be ciphertext, never contain the plaintext"
        );
        assert_eq!(wrap.unwrap(&tenant, &stored), plaintext);
    }

    #[test]
    fn contains_plaintext_detects_a_plaintext_run_and_ignores_absence() {
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
        let mut framed = b"kms://acme/0/tenant\n".to_vec();
        framed.extend_from_slice(&[0u8; NONCE_LEN]);
        let col =
            unframe(&framed).expect("an exactly-nonce-length tail is a valid (empty-ct) frame");
        assert!(col.ciphertext.is_empty());
        assert!(unframe(&framed[..framed.len() - 1]).is_none());
    }

    #[test]
    fn subject_id_as_str_returns_the_id() {
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
        assert!(unframe(b"no-newline-here").is_none());
        assert!(
            unframe(b"kms://acme/0/tenant\n\x00").is_none(),
            "tail shorter than a nonce"
        );
    }

    #[test]
    fn key_choice_error_display_is_loud_and_specific() {
        let e = KeyChoiceError::SubjectClassMissingSubject("subject_dek".into());
        assert!(e.to_string().contains("subject_dek") && e.to_string().contains("GD-4"));
        let e = KeyChoiceError::UnknownKeyClass("zzz".into());
        assert!(e.to_string().contains("zzz") && e.to_string().contains("erasure-reach bug"));
        let e = KeyChoiceError::Kms(KmsError::KekUnavailable(KekId::new(t("acme"), r())));
        assert!(e.to_string().contains("classify→key-choice"));
    }

    #[test]
    fn audit_plaintext_is_the_defence_in_depth_counter() {
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
