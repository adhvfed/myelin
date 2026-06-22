//! Contract 11.4 CDC pair — the GD-4 classify-driven key-choice rule + the encrypted-column
//! read/write path (11.1/11.2 at-rest halves) — P-ST-08 / global P-095.
//!
//! Row 11.4 is the GD-4 granularity rule: the `erasure` tag (contract 10.2) drives the per-subject
//! vs per-tenant key choice. This CDC pair pins the seam between:
//!   - the **PROVIDER** = `myelin-storage` — [`key_class_for`] (the GD-4 rule) + [`ColumnCryptor`]
//!     (the encrypted-column read/write path) + [`DekContentWrap`] (the blob content-key wrap);
//!   - the **CONSUMER** = the DSR ORCHESTRATOR / a schema owner that classifies a field by its
//!     `#[personal_data(erasure = ...)]` tag and expects the harness to seal it under the right
//!     DEK granularity (so a later `erase(subject)` destroys exactly that subject's key — the GD-4
//!     individual-erasure lever; the destroy ALGORITHM is P-ST-09).
//!
//! If the classify→key-choice mapping or the encrypted-column shape drifts, this stops passing —
//! exactly the consumer-driven contract the DSR orchestrator (P-ST-09 / GDPR M1) depends on.

use myelin_gdpr::ErasureMethod;
use myelin_storage::{key_class_for, ColumnCryptor, DekContentWrap, KeyChoiceError, SubjectId};
use myelin_storage::{BlobStore, ContentHash, FsBlobStore, KekId, KeyClass, KmsEngine};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn region() -> Region {
    Region("eu-west".into())
}

fn engine(tenant: &TenantId) -> Arc<KmsEngine> {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    Arc::new(kms)
}

/// The CONSUMER's field classification: a schema owner declares each field's `erasure` tag; the
/// provider's [`key_class_for`] decides the key granularity. This is the exact decision the
/// `#[personal_data(erasure = ...)]` derive feeds the harness.
struct ClassifiedField {
    name: &'static str,
    erasure: ErasureMethod,
    subject: Option<SubjectId>,
}

#[test]
fn cdc_11_4_classify_drives_per_subject_vs_per_tenant_key_choice() {
    let fields = [
        // A free-text/profile PII column → per-subject DEK (erasure unit = the individual).
        ClassifiedField {
            name: "user_bio",
            erasure: ErasureMethod::CryptoShred("subject_dek".into()),
            subject: Some(SubjectId::new("u-7")),
        },
        // A bulk tenant-content column → per-tenant DEK (erasure = pseudonymise/tombstone).
        ClassifiedField {
            name: "pr_metadata",
            erasure: ErasureMethod::Pseudonymise,
            subject: None,
        },
        // The explicit tenant crypto-shred class → per-tenant DEK.
        ClassifiedField {
            name: "issue_field_value",
            erasure: ErasureMethod::CryptoShred("tenant_dek".into()),
            subject: None,
        },
    ];

    let expected = [
        KeyClass::Subject("u-7".into()),
        KeyClass::Tenant,
        KeyClass::Tenant,
    ];

    for (f, want) in fields.iter().zip(expected.iter()) {
        let got = key_class_for(&f.erasure, f.subject.as_ref())
            .unwrap_or_else(|e| panic!("classify {} failed: {e}", f.name));
        assert_eq!(
            &got, want,
            "11.4: field {} routes to the wrong key class",
            f.name
        );
    }
}

#[test]
fn cdc_11_4_subject_tag_without_subject_is_a_loud_error_never_a_tenant_downgrade() {
    // The GD-4 lever the DSR orchestrator depends on: a subject-class field with no subject is a
    // LOUD provider error, NEVER silently keyed under the tenant DEK (which would lose per-subject
    // erasure — a later erase(subject) would not crypto-shred that field).
    let err = key_class_for(&ErasureMethod::CryptoShred("subject_dek".into()), None)
        .expect_err("subject class with no subject must be loud");
    assert!(matches!(err, KeyChoiceError::SubjectClassMissingSubject(_)));
}

#[test]
fn cdc_11_1_encrypted_column_round_trips_and_is_ciphertext_at_rest() {
    // The PROVIDER's encrypted-column path (11.1 at-rest half): a tagged column written via the
    // ColumnCryptor is ciphertext-at-rest and decrypts back to the exact plaintext. The plaintext-
    // at-rest telemetry the GATE asserts is 0.
    let tenant = TenantId("acme".into());
    let kms = engine(&tenant);
    let cryptor = ColumnCryptor::new(&kms, region());

    let plaintext = b"alice@example.test";
    let col = cryptor
        .encrypt(
            &tenant,
            Some(&SubjectId::new("u-alice")),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            plaintext,
        )
        .expect("encrypt");

    // Sealed under the per-subject DEK; ciphertext-at-rest; decrypts back exactly.
    assert_eq!(col.key_ref.class, KeyClass::Subject("u-alice".into()));
    assert!(!col.contains_plaintext(plaintext), "ciphertext-at-rest");
    assert_eq!(
        cryptor.plaintext_at_rest_count(),
        0,
        "0 plaintext-at-rest for the tagged column"
    );
    assert_eq!(cryptor.decrypt(&col).expect("decrypt"), plaintext);
}

#[test]
fn cdc_11_2_blob_content_key_wrap_round_trips_and_is_ciphertext_at_rest() {
    // The PROVIDER's blob content-key wrap (11.2 content-key-wrap half): a blob stored through the
    // DekContentWrap is ciphertext-at-rest, yet its content address stays plaintext-derived (so the
    // swap from IdentityWrap moves nothing) and get() returns the exact plaintext.
    let tenant = TenantId("acme".into());
    let kms = engine(&tenant);
    let wrap = DekContentWrap::new(kms, region(), ErasureMethod::PurgeReindex, None);
    let store = FsBlobStore::with_wrap(Box::new(wrap));

    let plaintext = b"a repo object's bytes";
    let h = store.put(&tenant, plaintext).expect("put");
    // Content address is the PLAINTEXT hash — stable across the real wrap.
    assert_eq!(h, ContentHash::blake3(plaintext));
    // Ciphertext-at-rest: the stored envelope is larger than the plaintext.
    assert!(store.head(&tenant, &h).expect("head").stored_len > plaintext.len());
    // Round-trips back to the exact plaintext.
    assert_eq!(store.get(&tenant, &h).expect("get"), plaintext);
}
