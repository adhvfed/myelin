//! # KN-P31 — the object-store BlobStore swap parity, PROVEN against the LIVE object store
//! (P-486, M5 — the integration drill, registered red-until-proven)
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. This runs ONLY against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-knowledge --features integration --test integration_kn_p31_blob_swap -- --nocapture
//!
//! The endpoint comes from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam), so the
//! same test runs against Scaleway Object Storage by exporting the prod env vars.
//!
//! ## What this proves (the 11.2 swap is behaviour-preserving)
//! The fs floor (`FsBlobStore`, KN-P05/KN-P11) and the live S3-compatible object store
//! (`S3BlobStore`, RustFS in dev) assign the SAME content address (BLAKE3-of-plaintext is
//! backing-independent) and round-trip the SAME bytes for a representative CRDT-snapshot / media
//! blob. [`materialise_blob_store_parity`] is the SAME parity oracle the CI drill runs fs↔fs; here
//! one side is the REAL object store — the real artifact that flips this drill green. The compactor
//! ([`myelin_knowledge::compaction::SnapshotCompactor`]) is already generic over `B: BlobStore`, so
//! the swap is a construction-time backing change, NOT a code change.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_knowledge::materialise_blob_store_parity;
use myelin_storage::blob::FsBlobStore;
use myelin_storage::s3blob::S3BlobStore;
use myelin_tenancy::TenantId;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kn_p31_object_store_swap_is_byte_identical_to_the_fs_floor() {
    let cfg = MyelinConfig::dev();
    let endpoint = cfg.s3.endpoint.clone();
    let handle = tokio::runtime::Handle::current();

    // A unique tenant per run so concurrent runs don't collide in the real bucket.
    let tenant = TenantId(format!("itest-knp31-{}", std::process::id()));

    // The whole parity check runs on ONE blocking thread (the sync BlobStore trait drives the async
    // SDK via block_in_place; both stores live for the put → get round-trip).
    let verdicts = tokio::task::spawn_blocking(move || {
        let fs = FsBlobStore::new();
        let object = S3BlobStore::connect(&cfg.s3, handle.clone());

        // Representative Knowledge blobs: a compacted Yrs CRDT snapshot + a media payload.
        let payloads: Vec<Vec<u8>> = vec![
            b"compacted Yrs CRDT snapshot bytes (content-addressed, BLAKE3)".to_vec(),
            vec![0x5Au8; 64 * 1024],
        ];
        payloads
            .into_iter()
            .map(|bytes| {
                materialise_blob_store_parity(&fs, &object, &tenant, &bytes)
                    .expect("the parity check runs against the live object store")
            })
            .collect::<Vec<_>>()
    })
    .await
    .expect("the blocking parity task completes");

    for verdict in &verdicts {
        assert_eq!(
            verdict.fs_address, verdict.object_address,
            "the content address is IDENTICAL across the fs floor and the live object store \
             (BLAKE3-of-plaintext is backing-independent)"
        );
        assert!(
            verdict.byte_identical,
            "the object-store swap is BYTE-IDENTICAL to the fs floor: same address, same bytes back \
             from both stores (the swap is behaviour-preserving — the 11.2 one-line backing change)"
        );
    }

    println!(
        "[P-486 KN-P31 INTEGRATION GREEN] object-store BlobStore swap PROVEN against the live dev \
         stack ({} RustFS): content-addressed put/get is byte-identical to the fs floor for {} \
         representative blobs. KN-P05/KN-P11 fs floor RESOLVED — the swap is a one-line backing \
         change behind the BlobStore trait.",
        endpoint,
        verdicts.len()
    );
}
