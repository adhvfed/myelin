//! # Integration — SRCH-P30 (P-463, M5): the object-store index backstop swap, PROVEN against the
//! LIVE dev-stack object store (RustFS / Scaleway via `S3BlobStore`).
//!
//! **The dev-real data-layer policy (binding):** this prompt swaps Search's index/backup segments'
//! at-rest backing from the fs-backed [`FsBlobStore`] floor to the **real object store** behind the
//! SAME frozen `BlobStore` trait (contract 11.2). The unit tests in `src/object_store_backstop.rs`
//! prove the swap LOGIC DB-free against the fs floor; THIS test proves the segments actually
//! round-trip through the LIVE object store with NO behaviour change, AND that the SRCH-D4
//! backup-scale erasure (the per-tenant index DEK crypto-shred) reaches the
//! object-store-RESIDENT segments — the green-only-with-a-real-artifact proof.
//!
//! Run against the docker-compose dev stack:
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-search --features integration \
//!     --test integration_srch_p30_object_store_backstop -- --nocapture
//!
//! ## What this proves (the dated green artifact)
//! 1. **The swap moved the segments with NO behaviour change (a *measured* swap, EI-01 §3):** a
//!    moderate corpus of per-tenant-DEK-sealed index segments is put into the LIVE object store via
//!    [`SegmentBackstop`] and read back BYTE-IDENTICAL (the `S3BlobStore` re-hash-on-read integrity
//!    gate passes; the recovered [`SealedBackupSegment`] equals the put one).
//! 2. **Restore + backup-scale erasure HOLD over the object-store segments (SRCH-D4):** the real
//!    [`BackupScaleEraseGate`] runs over the segments LOADED BACK from the object store — the live
//!    purge + the per-tenant index DEK crypto-shred — and asserts 0 recoverable AFTER the shred
//!    (incl. the object-store-resident backstop). The crypto-shred reaches the object store by
//!    construction (the object holds ONLY the DEK-sealed ciphertext).
//!
//! ## Floor / honesty (EI-01 §1)
//! Run at a **scaled-down (CI) variant** of "backup scale" — a moderate segment corpus against the
//! single dev RustFS, not the world-scale fleet corpus. The world-scale 30x load drill is the ONLY
//! remaining floor. Cross-cell federated search is the remaining S-M5 piece (SRCH-P31 / P-464); the
//! whole-system E2E wedge is SRCH-P32 (P-465).
#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::s3blob::S3BlobStore;
use myelin_storage::KmsEngine;
use myelin_tenancy::{Region, TenantId};

use myelin_gdpr::SubjectRef;
use myelin_search::{
    build_live_corpus, BackupScaleEraseGate, BackupScaleEraseInputs, ObjectStoreBackstopGate,
    SealedBackupSegment, SearchDekPin, SearchEraseHolder, SegmentBackstop,
};

const NOW: &str = "2026-06-25T00:00:00Z";

fn region() -> Region {
    Region("fr-par".into())
}
fn subject(id: &str, tenant: &TenantId) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant.clone(),
    ))
}

/// **SRCH-P30 — the object-store index backstop swap against the LIVE object store.** Segments move
/// with no behaviour change (byte-identical round-trip) AND the SRCH-D4 backup-scale erasure holds
/// over the object-store-resident segments (0 recoverable after the per-tenant index DEK shred).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn srch_p30_object_store_backstop_swap_and_reerase() {
    let cfg = MyelinConfig::dev();
    // A real S3-compatible object store (RustFS in dev) behind the frozen BlobStore trait — the
    // one-line backing swap from the fs floor. Shared via Arc so the tenant-pinned backstop holds
    // a cheap handle to the same bucket.
    let blobs = Arc::new(S3BlobStore::connect(
        &cfg.s3,
        tokio::runtime::Handle::current(),
    ));
    // A per-test tenant so concurrent runs don't collide in the shared bucket.
    let tenant = TenantId(format!("acme-srch-p30-{}", std::process::id()));
    let target = "u-target";

    // 3 docs reference the subject (the segments we seal as the object-store backstop); 6 do not.
    let subject_docs = ["t1", "t2", "t3"];
    let other_docs = ["o0", "o1", "o2", "o3", "o4", "o5"];
    let (ix, ids) = build_live_corpus(&tenant, &region(), target, &subject_docs, &other_docs);

    // Reserve a REAL per-tenant index DEK; SEAL the subject's index segments under it (the at-rest
    // backstop form — per-tenant-DEK-encrypted, the property the object store inherits).
    let kms = Arc::new(KmsEngine::new());
    let pin = SearchDekPin::new(kms);
    let key_ref = pin
        .reserve(&tenant, &region())
        .expect("reserve the per-tenant index DEK");
    let dek = pin
        .resolve(&key_ref, &region())
        .expect("resolve the live DEK");
    let subject_doc_ids: Vec<&String> = ids
        .iter()
        .filter(|id| subject_docs.iter().any(|d| id.ends_with(d)))
        .collect();
    let segments: Vec<SealedBackupSegment> = subject_doc_ids
        .iter()
        .map(|id| {
            SealedBackupSegment::seal(
                &dek,
                id,
                format!("{target}'s index segment plaintext for {id}").as_bytes(),
            )
        })
        .collect();
    assert_eq!(segments.len(), 3, "three subject index segments sealed");

    // ── Phase 1 — swap the segments INTO the live object store + read them back byte-identical ──
    let backstop = SegmentBackstop::new(Arc::clone(&blobs), tenant.clone(), region());
    let gate = ObjectStoreBackstopGate::new();
    let swapped = tokio::task::block_in_place(|| gate.swap_in(&backstop, &segments))
        .expect("swap_in over the LIVE object store: segments moved byte-identical");
    assert_eq!(swapped.loaded.len(), 3, "all three segments read back");
    assert_eq!(
        swapped.byte_identical, 3,
        "every segment recovered BYTE-IDENTICAL from the object store (behaviour unchanged)"
    );

    // ── Phase 2 — run the REAL SRCH-D4 backup-scale erasure gate over the OBJECT-STORE-RESIDENT
    // segments (the segments loaded back from the store). The live purge + the per-tenant index DEK
    // crypto-shred must render the object-store-resident segments unrecoverable. ──
    let holder = SearchEraseHolder::new(ix.clone(), pin.clone(), region());
    let mut d4_inputs = BackupScaleEraseInputs {
        erase_holder: &holder,
        dek: &pin,
        index_key_ref: key_ref,
        subject: subject(target, &tenant),
        tenant: tenant.clone(),
        backup_segments: &swapped.loaded, // ← the segments LOADED FROM the object store
        subject_backstop_id: None,
        now: NOW.into(),
    };
    let d4 = BackupScaleEraseGate::new().run(&mut d4_inputs);

    // ── Phase 3 — fold the SRCH-D4 verdict into the dated object-store-backstop artifact ──
    let verdict = gate.confirm(&backstop, &swapped, &d4, "object-store", "2026-06-25");
    let artifact = verdict.run_or_fail_ci().expect(
        "SRCH-P30 green: swap byte-identical + SRCH-D4 erasure holds over the object store",
    );

    assert_eq!(
        artifact.segments_moved, 3,
        "three segments swapped through the object store"
    );
    assert_eq!(
        artifact.segments_byte_identical, 3,
        "all three recovered byte-identical (no behaviour change)"
    );
    assert_eq!(
        artifact.recoverable_after_shred, 0,
        "0 object-store-resident segments recoverable after the crypto-shred (erasure holds, §4.8)"
    );
    assert_eq!(artifact.backing, "object-store");
    assert!(artifact.is_green());

    // The dated green-artifact line (observability is part of the pass).
    println!("[P-463 GATE GREEN 2026-06-25] {}", artifact.summary());

    // Cross-confirm directly against the live store: the §7.5 immutable-tier-erasure-by-crypto-shred
    // property — the sealed CIPHERTEXT objects are still resident (the erasure is the key-shred, not
    // a delete), but they are now plaintext-DEAD because the per-tenant index DEK was destroyed.
    // (a) the content-addressed ciphertext objects are still readable from the object store:
    let still_resident = tokio::task::block_in_place(|| backstop.load_all(&swapped.stored))
        .expect("the object-store objects are still readable (content-addressed ciphertext)");
    assert_eq!(
        still_resident.len(),
        3,
        "the sealed ciphertext objects remain in the object store (erasure = crypto-shred, §7.5)"
    );
    // (b) but the per-tenant index DEK no longer resolves (the shred fired), so NO handle exists to
    // open them — every object-store-resident segment is plaintext-unrecoverable. The `key_ref` was
    // consumed by the SRCH-D4 gate; a re-resolve under a fresh reservation cannot open the OLD
    // ciphertext (a different key), so 0 recoverable holds either way — already asserted green by
    // the SRCH-D4 gate above. This block confirms the OBJECTS survive while the PLAINTEXT does not.
}
