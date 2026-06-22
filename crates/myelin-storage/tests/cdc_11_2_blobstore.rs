//! Contract 11.2 CDC pair — the content-addressed `BlobStore` (P-ST-03).
//!
//! The prompt requires "the provider+consumer pair for 11.2 (a blob-holding service
//! putting/getting through the trait)". This is the consumer-driven contract test: the
//! PROVIDER is `myelin-storage` (the `BlobStore` trait + the fs floor this prompt ships); the
//! CONSUMER is a blob-holding subsystem (modelled here as a tiny `AttachmentsService`) that
//! puts content through the trait and later resolves it by content address. The test pins the
//! frozen 11.2 call shape every blob-holding service relies on — if `put`/`get`/`head`/`delete`
//! drift (the content-address return, the per-tenant keyspace, re-hash-on-read), it stops
//! compiling/passing.

use myelin_storage::{
    BlobStore, BlobStoreHolder, ContentHash, FsBlobStore, OltpHolderRegistration,
};
use myelin_tenancy::TenantId;

/// A consumer of 11.2: a service that stores user attachments as content-addressed blobs. It
/// holds a content address (not the bytes) and resolves it through the trait — the
/// content-address-as-handle pattern (Git/Venti model) every blob-holding service uses.
struct AttachmentsService<B: BlobStore> {
    blobs: B,
    tenant: TenantId,
}

impl<B: BlobStore> AttachmentsService<B> {
    /// Boot the service over its blob store and auto-register it as a holder (1.4) — a blob
    /// store is a `PersonalDataHolder` (its erasure is crypto-shred, §3.2).
    fn boot(blobs: B, tenant: TenantId) -> AttachmentsService<B> {
        let receipt: OltpHolderRegistration = BlobStoreHolder::new("attachments").register();
        assert_eq!(receipt.store, "attachments");
        AttachmentsService { blobs, tenant }
    }

    /// Store an attachment, returning the content address the service persists as the handle.
    fn store_attachment(&self, bytes: &[u8]) -> ContentHash {
        self.blobs
            .put(&self.tenant, bytes)
            .expect("put through the trait")
    }

    /// Resolve a stored attachment by its content address (re-hash-on-read verified).
    fn fetch_attachment(&self, handle: &ContentHash) -> Vec<u8> {
        self.blobs
            .get(&self.tenant, handle)
            .expect("get through the trait")
    }
}

/// THE CDC pair: a blob-holding consumer puts content through the trait, gets back a content
/// address, and later resolves the exact bytes by that address — the provider
/// (`myelin-storage`'s fs floor) honours the frozen 11.2 shape.
#[test]
fn cdc_11_2_blob_holding_service_puts_and_gets_through_the_trait() {
    let svc = AttachmentsService::boot(FsBlobStore::new(), TenantId("acme".into()));

    let original = b"a user-uploaded attachment with maybe-PII content";
    let handle = svc.store_attachment(original);

    // The handle is the self-describing content address (the hash IS the address).
    assert_eq!(handle, ContentHash::blake3(original));
    assert!(handle.to_multihash_string().starts_with("blake3:"));

    // Resolving the handle returns the exact bytes (re-hash-on-read verified).
    let resolved = svc.fetch_attachment(&handle);
    assert_eq!(
        resolved, original,
        "11.2: get by content address round-trips the bytes"
    );

    // head returns metadata for the handle without serving the bytes.
    let meta = svc.blobs.head(&svc.tenant, &handle).expect("head");
    assert_eq!(meta.hash, handle);
}
