use myelin_issues::time_axis::{attach, subject_dek_ref, AttachmentPointer};
use myelin_storage::blob::{BlobStore, ContentHash, FsBlobStore};
use myelin_storage::encryption::SubjectId;
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

#[test]
fn provider_and_consumer_agree_on_the_content_address() {
    let blob = FsBlobStore::new();
    let subject = SubjectId("u42".into());
    let bytes = b"an attachment uploaded to an issue";

    let pointer: AttachmentPointer =
        attach(&blob, &tenant(), &subject, 3, "fr-par", "image/png", bytes).unwrap();

    let provider_addr = blob.put(&tenant(), bytes).unwrap();
    assert_eq!(
        pointer.blob_ref, provider_addr,
        "the pointer address == the provider's put address (no drift)"
    );
    assert_eq!(pointer.blob_ref, ContentHash::blake3(bytes));

    let served = blob.get(&tenant(), &pointer.blob_ref).unwrap();
    assert_eq!(served, bytes);
}

#[test]
fn the_row_holds_zero_attachment_bytes_and_a_dek_pointer() {
    let blob = FsBlobStore::new();
    let subject = SubjectId("u42".into());
    let bytes = b"bytes that MUST NOT touch the OLTP row";

    let pointer = attach(
        &blob,
        &tenant(),
        &subject,
        7,
        "fr-par",
        "application/pdf",
        bytes,
    )
    .unwrap();

    assert_eq!(pointer.row_byte_count(), 0);
    assert_eq!(pointer.size_bytes, bytes.len() as u64);
    assert_eq!(pointer.region, "fr-par");
    assert_eq!(pointer.content_type, "application/pdf");
    assert_eq!(
        pointer.pii_key_ref,
        subject_dek_ref("acme", 7, &subject),
        "the pointer carries the per-subject-DEK kms:// URN (crypto-shred reach)"
    );
    assert_eq!(pointer.pii_key_ref, "kms://acme/7/subject:u42");

    let fetched = pointer.fetch_bytes(&blob, &tenant()).unwrap();
    assert_eq!(fetched, bytes);
}

#[test]
fn content_addressed_dedup_per_tenant() {
    let blob = FsBlobStore::new();
    let subject = SubjectId("u42".into());
    let bytes = b"identical bytes";

    let p1 = attach(&blob, &tenant(), &subject, 1, "fr-par", "text/plain", bytes).unwrap();
    let p2 = attach(&blob, &tenant(), &subject, 1, "fr-par", "text/plain", bytes).unwrap();
    assert_eq!(
        p1.blob_ref, p2.blob_ref,
        "same bytes → same address (dedup)"
    );

    let other = TenantId("globex".into());
    let p3 = attach(&blob, &other, &subject, 1, "fr-par", "text/plain", bytes).unwrap();
    assert_eq!(
        p3.blob_ref, p1.blob_ref,
        "the content address is the same (content-addressed)"
    );
    assert_eq!(blob.get(&tenant(), &p1.blob_ref).unwrap(), bytes);
    assert_eq!(blob.get(&other, &p3.blob_ref).unwrap(), bytes);
}
