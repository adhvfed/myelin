use myelin_storage::{
    BlobStore, BlobStoreHolder, ContentHash, FsBlobStore, OltpHolderRegistration,
};
use myelin_tenancy::TenantId;

struct AttachmentsService<B: BlobStore> {
    blobs: B,
    tenant: TenantId,
}

impl<B: BlobStore> AttachmentsService<B> {
    fn boot(blobs: B, tenant: TenantId) -> AttachmentsService<B> {
        let receipt: OltpHolderRegistration = BlobStoreHolder::new("attachments").register();
        assert_eq!(receipt.store, "attachments");
        AttachmentsService { blobs, tenant }
    }

    fn store_attachment(&self, bytes: &[u8]) -> ContentHash {
        self.blobs
            .put(&self.tenant, bytes)
            .expect("put through the trait")
    }

    fn fetch_attachment(&self, handle: &ContentHash) -> Vec<u8> {
        self.blobs
            .get(&self.tenant, handle)
            .expect("get through the trait")
    }
}

#[test]
fn cdc_11_2_blob_holding_service_puts_and_gets_through_the_trait() {
    let svc = AttachmentsService::boot(FsBlobStore::new(), TenantId("acme".into()));

    let original = b"a user-uploaded attachment with maybe-PII content";
    let handle = svc.store_attachment(original);

    assert_eq!(handle, ContentHash::blake3(original));
    assert!(handle.to_multihash_string().starts_with("blake3:"));

    let resolved = svc.fetch_attachment(&handle);
    assert_eq!(
        resolved, original,
        "11.2: get by content address round-trips the bytes"
    );

    let meta = svc.blobs.head(&svc.tenant, &handle).expect("head");
    assert_eq!(meta.hash, handle);
}
