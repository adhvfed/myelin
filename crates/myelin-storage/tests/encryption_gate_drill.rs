use myelin_gdpr::ErasureMethod;
use myelin_storage::{
    BlobStore, ColumnCryptor, DekContentWrap, FsBlobStore, KekId, KeyClass, KmsEngine, SubjectId,
};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn region() -> Region {
    Region("eu-west".into())
}

#[test]
fn classified_data_uses_the_right_keys_and_stores_only_ciphertext() {
    let tenant = TenantId("acme".into());
    let kms = Arc::new(KmsEngine::new());
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    let cryptor = ColumnCryptor::new(&kms, region());

    let subject_plain = b"alice.bio.free-text@example.test";
    let subject_col = cryptor
        .encrypt(
            &tenant,
            Some(&SubjectId::new("u-alice")),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            subject_plain,
        )
        .expect("subject column encrypt");
    assert_eq!(
        subject_col.key_ref.class,
        KeyClass::Subject("u-alice".into()),
        "GATE leg 2: erasure=subject MUST resolve to a per-subject DEK"
    );
    assert!(
        !subject_col.contains_plaintext(subject_plain),
        "GATE leg 2: a tagged subject column MUST be ciphertext-at-rest (0 plaintext-at-rest)"
    );

    let bulk_plain = b"PR-1234 bulk tenant metadata";
    let bulk_col = cryptor
        .encrypt(&tenant, None, &ErasureMethod::PurgeReindex, bulk_plain)
        .expect("bulk column encrypt");
    assert_eq!(
        bulk_col.key_ref.class,
        KeyClass::Tenant,
        "GATE leg 2: a bulk column MUST resolve to the tenant DEK"
    );
    assert!(
        !bulk_col.contains_plaintext(bulk_plain),
        "GATE leg 2: a bulk column MUST be ciphertext-at-rest"
    );

    let blob_wrap = DekContentWrap::new(
        kms.clone(),
        region(),
        ErasureMethod::CryptoShred("subject_dek".into()),
        Some(SubjectId::new("u-avatar")),
    );
    let blob_store = FsBlobStore::with_wrap(Box::new(blob_wrap));
    let blob_plain = b"avatar.png subject-scoped blob bytes";
    let h = blob_store.put(&tenant, blob_plain).expect("blob put");
    let stored_len = blob_store.head(&tenant, &h).expect("head").stored_len;
    assert!(
        stored_len > blob_plain.len(),
        "GATE leg 2: the stored blob is the ciphertext envelope, not plaintext"
    );
    assert_eq!(
        blob_store.get(&tenant, &h).expect("blob get"),
        blob_plain,
        "GATE leg 2: the blob round-trips back to the exact plaintext"
    );

    assert_eq!(
        cryptor.decrypt(&subject_col).expect("subject decrypt"),
        subject_plain
    );
    assert_eq!(
        cryptor.decrypt(&bulk_col).expect("bulk decrypt"),
        bulk_plain
    );

    println!(
        "[P-095 DRILL GREEN 2026-06-19] encryption gate: subject-column→{:?} tenant-column→{:?} \
         subject-blob→ciphertext({stored_len}B>{}B).",
        subject_col.key_ref.class,
        bulk_col.key_ref.class,
        blob_plain.len(),
    );
}
