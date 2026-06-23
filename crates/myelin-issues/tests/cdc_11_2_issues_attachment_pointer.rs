//! # The CDC pair for the Issues attachment BlobStore pointer (ISS-P19 / P-386): contract 11.2
//!
//! **Contract proven here:** 11.2 — `BlobStore{put, get, head, delete}`, content-addressed (BLAKE3,
//! per-tenant dedup), residency-pinned. Issues CONSUMES this row for attachments: the attachment bytes
//! live in the BlobStore; the OLTP `issue` row holds ONLY the content-addressed POINTER + per-subject-
//! DEK metadata (the `pii_key_ref` `kms://…` URN) + size + region — **0 bytes of the attachment in the
//! row** (arch §1.2: "the row holds the pointer + per-subject-DEK metadata, not the bytes").
//!
//! ## CDC pair markers (the contract-coverage gate)
//! - **PROVIDER side** — `myelin_storage::blob::FsBlobStore` (the REAL fs-backed BlobStore floor): the
//!   content-addressed `put`/`get` round-trip, the BLAKE3 address, the per-tenant dedup, the
//!   re-hash-on-read integrity refuse. Issues drives the real provider (not a mock — the fs floor IS
//!   the dev binding; dev<->prod is a backing swap to the object store, §3.5).
//! - **CONSUMER side** — `myelin_issues::time_axis::{attach, AttachmentPointer}`: Issues PUTs the bytes
//!   through the provider + builds the row pointer; the pointer holds the address + DEK ref, NEVER the
//!   bytes (`row_byte_count() == 0` — the 0-bytes-in-row GATE artifact). The bytes are resolvable from
//!   the blob tier on demand (re-hash-verified).
//!
//! **The two halves AGREE with no drift:** the address Issues stores on the pointer is BYTE-IDENTICAL
//! to the address the BlobStore returns from `put` and verifies on `get` (the content-address is the
//! shared contract). A pointer can never carry the payload (no `bytes` field — structural).
//!
//! Owning architecture: `04-subsystem-architectures/issue-tracker/architecture/01-tech-and-data-model.md`
//! §1.2 (attachments in BlobStore; the row holds the pointer). Storage: `storage.md` §3.2.

use myelin_issues::time_axis::{attach, subject_dek_ref, AttachmentPointer};
use myelin_storage::blob::{BlobStore, ContentHash, FsBlobStore};
use myelin_storage::encryption::SubjectId;
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

/// **PROVIDER ⇄ CONSUMER agree on the content address (no drift).** The address Issues stores on the
/// pointer equals the address the BlobStore `put` returned AND the address `get` re-hash-verifies.
#[test]
fn provider_and_consumer_agree_on_the_content_address() {
    let blob = FsBlobStore::new();
    let subject = SubjectId("u42".into());
    let bytes = b"an attachment uploaded to an issue";

    // CONSUMER: Issues attaches through the provider.
    let pointer: AttachmentPointer =
        attach(&blob, &tenant(), &subject, 3, "fr-par", "image/png", bytes).unwrap();

    // PROVIDER: the BlobStore put yields the SAME content address (the shared contract).
    let provider_addr = blob.put(&tenant(), bytes).unwrap();
    assert_eq!(
        pointer.blob_ref, provider_addr,
        "the pointer address == the provider's put address (no drift)"
    );
    // ... and it is the BLAKE3 address of the plaintext (content-addressed posture).
    assert_eq!(pointer.blob_ref, ContentHash::blake3(bytes));

    // PROVIDER re-hash-on-read returns the exact bytes (verified serve).
    let served = blob.get(&tenant(), &pointer.blob_ref).unwrap();
    assert_eq!(served, bytes);
}

/// **THE GATE — 0 bytes of the attachment in the OLTP row.** The consumer pointer holds the address +
/// metadata + the per-subject-DEK key ref; `row_byte_count()` is 0 by construction (no `bytes` field).
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

    // 0-bytes-in-row (the green artifact the GATE asserts).
    assert_eq!(pointer.row_byte_count(), 0);
    // the metadata the row DOES hold: address + size + region + the subject-DEK key ref.
    assert_eq!(pointer.size_bytes, bytes.len() as u64);
    assert_eq!(pointer.region, "fr-par");
    assert_eq!(pointer.content_type, "application/pdf");
    assert_eq!(
        pointer.pii_key_ref,
        subject_dek_ref("acme", 7, &subject),
        "the pointer carries the per-subject-DEK kms:// URN (crypto-shred reach)"
    );
    assert_eq!(pointer.pii_key_ref, "kms://acme/7/subject:u42");

    // the bytes are RESOLVABLE from the blob tier (never resident on the row).
    let fetched = pointer.fetch_bytes(&blob, &tenant()).unwrap();
    assert_eq!(fetched, bytes);
}

/// **Per-tenant content-addressed dedup (11.2 posture):** identical bytes → one address, stored once.
/// A DIFFERENT tenant putting the same bytes gets the same ADDRESS but a separate keyspace object
/// (cross-tenant dedup deliberately forgone — residency isolation).
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

    // a different tenant: same content ADDRESS, separate object (residency isolation).
    let other = TenantId("globex".into());
    let p3 = attach(&blob, &other, &subject, 1, "fr-par", "text/plain", bytes).unwrap();
    assert_eq!(
        p3.blob_ref, p1.blob_ref,
        "the content address is the same (content-addressed)"
    );
    // both keyspaces independently serve their object (cross-tenant dedup forgone).
    assert_eq!(blob.get(&tenant(), &p1.blob_ref).unwrap(), bytes);
    assert_eq!(blob.get(&other, &p3.blob_ref).unwrap(), bytes);
}
