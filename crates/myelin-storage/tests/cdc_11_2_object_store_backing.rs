use myelin_storage::{
    BlobStore, BlobStoreHolder, ContentHash, FsBlobStore, OltpHolderRegistration,
    ReplicatedBlobStore,
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
fn cdc_11_2_object_store_backing_honours_the_frozen_blobstore_shape() {
    let backing = ReplicatedBlobStore::new(
        FsBlobStore::new(),
        vec![FsBlobStore::new(), FsBlobStore::new()],
    );
    let svc = AttachmentsService::boot(backing, TenantId("acme".into()));

    let original = b"a user-uploaded attachment, now on the object store";
    let handle = svc.store_attachment(original);

    assert_eq!(handle, ContentHash::blake3(original));
    assert!(handle.to_multihash_string().starts_with("blake3:"));

    let resolved = svc.fetch_attachment(&handle);
    assert_eq!(
        resolved, original,
        "11.2 object-store backing: get by content address round-trips the bytes"
    );

    let meta = svc.blobs.head(&svc.tenant, &handle).expect("head");
    assert_eq!(meta.hash, handle);

    svc.blobs.delete(&svc.tenant, &handle).expect("delete");
}

#[test]
fn fs_to_object_swap_is_a_backing_change_only_consumer_is_identical() {
    fn exercise<B: BlobStore>(store: &B, tenant: &TenantId) -> (ContentHash, Vec<u8>, usize) {
        let bytes = b"backing-swap-invariant payload";
        let h = store.put(tenant, bytes).expect("put");
        let got = store.get(tenant, &h).expect("get");
        let len = store.head(tenant, &h).expect("head").stored_len;
        (h, got, len)
    }

    let tenant = TenantId("acme".into());
    let fs_result = exercise(&FsBlobStore::new(), &tenant);
    let object_result = exercise(
        &ReplicatedBlobStore::new(FsBlobStore::new(), vec![FsBlobStore::new()]),
        &tenant,
    );

    assert_eq!(
        fs_result, object_result,
        "the same 11.2 consumer yields identical results on the fs floor and the object backing \
         - the swap is a backing change, not a contract change"
    );
}
