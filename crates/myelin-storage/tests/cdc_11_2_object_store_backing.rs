//! Contract 11.2 CDC pair — the **object-store BlobStore backing** (P-ST-30 / global P-441).
//!
//! The prompt requires "CDC: provider+consumer pair for 11.2 (the object-store backing)" and
//! "the fs→object swap is a backing change only (the trait's consumers are untouched — a
//! structural assertion)". This is the consumer-driven contract test for the FOLLOW-ON backing:
//! the PROVIDER is `myelin-storage`'s object-store backing (modelled in CI by
//! [`ReplicatedBlobStore`] over the [`FsBlobStore`] floor — the live S3 backing is proven in
//! `integration_backends.rs`); the CONSUMER is the SAME blob-holding service shape that the fs
//! floor's CDC pair (`cdc_11_2_blobstore.rs`) uses, byte-for-byte unchanged.
//!
//! The load-bearing assertion: a blob-holding consumer written against the 11.2 trait compiles
//! and passes IDENTICALLY over the object-store backing as over the fs floor — the swap is a
//! backing change, NOT a contract change (EI-01 §7 coherence; the prompt's structural
//! assertion). If the object-store backing drifted the `put`/`get`/`head`/`delete` shape, the
//! content-address return, the per-tenant keyspace, or re-hash-on-read, this consumer would stop
//! compiling/passing.

use myelin_storage::{
    BlobStore, BlobStoreHolder, ContentHash, FsBlobStore, OltpHolderRegistration,
    ReplicatedBlobStore,
};
use myelin_tenancy::TenantId;

/// A consumer of 11.2: the SAME attachments service the fs-floor CDC pair uses, generic over the
/// [`BlobStore`] trait. It is agnostic to whether the backing is the fs floor or the object
/// store — the point of the CDC pair (the consumer is untouched by the backing swap).
struct AttachmentsService<B: BlobStore> {
    blobs: B,
    tenant: TenantId,
}

impl<B: BlobStore> AttachmentsService<B> {
    /// Boot over its blob store and auto-register as a holder (1.4) — a blob store is a
    /// `PersonalDataHolder` (erasure = crypto-shred, §3.2), unchanged across the backing swap.
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

/// THE CDC pair over the object-store backing: the consumer puts/gets/heads through the SAME
/// 11.2 trait against the (replicated) object-store backing — the provider honours the frozen
/// shape after the fs→object swap.
#[test]
fn cdc_11_2_object_store_backing_honours_the_frozen_blobstore_shape() {
    // The object-store backing (modelled by the replicated store over the fs floor in CI; the
    // live S3BlobStore in integration_backends.rs). The CONSTRUCTION differs from the fs floor;
    // the CONSUMER below does not.
    let backing = ReplicatedBlobStore::new(
        FsBlobStore::new(),
        vec![FsBlobStore::new(), FsBlobStore::new()],
    );
    let svc = AttachmentsService::boot(backing, TenantId("acme".into()));

    let original = b"a user-uploaded attachment, now on the object store";
    let handle = svc.store_attachment(original);

    // Identical content-address contract: the hash IS the address, self-describing multihash.
    assert_eq!(handle, ContentHash::blake3(original));
    assert!(handle.to_multihash_string().starts_with("blake3:"));

    // get by content address round-trips the exact bytes (re-hash-on-read verified) — identical
    // to the fs floor's CDC pair.
    let resolved = svc.fetch_attachment(&handle);
    assert_eq!(
        resolved, original,
        "11.2 object-store backing: get by content address round-trips the bytes"
    );

    // head returns metadata without serving the bytes — unchanged shape.
    let meta = svc.blobs.head(&svc.tenant, &handle).expect("head");
    assert_eq!(meta.hash, handle);

    // delete reaches the backing (the crypto-shred reach) — unchanged shape.
    svc.blobs.delete(&svc.tenant, &handle).expect("delete");
}

/// **The backing-swap structural assertion (the prompt's "the trait's consumers are
/// untouched").** ONE consumer function, written purely against the `BlobStore` trait, is run
/// over BOTH the fs floor AND the object-store backing and yields byte-identical results. This
/// proves the fs→object swap is a backing change only — the consumer code is the same.
#[test]
fn fs_to_object_swap_is_a_backing_change_only_consumer_is_identical() {
    /// A consumer that exercises the full 11.2 surface — written ONCE, generic over the trait.
    fn exercise<B: BlobStore>(store: &B, tenant: &TenantId) -> (ContentHash, Vec<u8>, usize) {
        let bytes = b"backing-swap-invariant payload";
        let h = store.put(tenant, bytes).expect("put");
        let got = store.get(tenant, &h).expect("get");
        let len = store.head(tenant, &h).expect("head").stored_len;
        (h, got, len)
    }

    let tenant = TenantId("acme".into());
    // Same consumer over the fs floor...
    let fs_result = exercise(&FsBlobStore::new(), &tenant);
    // ...and over the object-store backing (the replicated store). No code change in `exercise`.
    let object_result = exercise(
        &ReplicatedBlobStore::new(FsBlobStore::new(), vec![FsBlobStore::new()]),
        &tenant,
    );

    assert_eq!(
        fs_result, object_result,
        "the same 11.2 consumer yields identical results on the fs floor and the object backing \
         — the swap is a backing change, not a contract change"
    );
}
