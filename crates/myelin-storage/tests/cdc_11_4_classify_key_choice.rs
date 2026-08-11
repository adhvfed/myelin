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
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    Arc::new(kms)
}

struct ClassifiedField {
    name: &'static str,
    erasure: ErasureMethod,
    subject: Option<SubjectId>,
}

#[test]
fn cdc_11_4_classify_drives_per_subject_vs_per_tenant_key_choice() {
    let fields = [
        ClassifiedField {
            name: "user_bio",
            erasure: ErasureMethod::CryptoShred("subject_dek".into()),
            subject: Some(SubjectId::new("u-7")),
        },
        ClassifiedField {
            name: "pr_metadata",
            erasure: ErasureMethod::Pseudonymise,
            subject: None,
        },
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
    let err = key_class_for(&ErasureMethod::CryptoShred("subject_dek".into()), None)
        .expect_err("subject class with no subject must be loud");
    assert!(matches!(err, KeyChoiceError::SubjectClassMissingSubject(_)));
}

#[test]
fn cdc_11_1_encrypted_column_round_trips_and_is_ciphertext_at_rest() {
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
    let tenant = TenantId("acme".into());
    let kms = engine(&tenant);
    let wrap = DekContentWrap::new(kms, region(), ErasureMethod::PurgeReindex, None);
    let store = FsBlobStore::with_wrap(Box::new(wrap));

    let plaintext = b"a repo object's bytes";
    let h = store.put(&tenant, plaintext).expect("put");
    assert_eq!(h, ContentHash::blake3(plaintext));
    assert!(store.head(&tenant, &h).expect("head").stored_len > plaintext.len());
    assert_eq!(store.get(&tenant, &h).expect("get"), plaintext);
}
