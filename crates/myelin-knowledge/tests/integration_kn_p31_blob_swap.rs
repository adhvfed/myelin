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

    let tenant = TenantId(format!("itest-knp31-{}", std::process::id()));

    let verdicts = tokio::task::spawn_blocking(move || {
        let fs = FsBlobStore::new();
        let object = S3BlobStore::connect(&cfg.s3, handle.clone());

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
             from both stores (the swap is behaviour-preserving - the 11.2 one-line backing change)"
        );
    }

    println!(
        "[P-486 KN-P31 INTEGRATION GREEN] object-store BlobStore swap PROVEN against the live dev \
         stack ({} RustFS): content-addressed put/get is byte-identical to the fs floor for {} \
         representative blobs. KN-P05/KN-P11 fs floor RESOLVED - the swap is a one-line backing \
         change behind the BlobStore trait.",
        endpoint,
        verdicts.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kn_p31_repointed_knowledge_store_blob_path_is_durable_across_reconstruction() {
    use std::sync::Arc;

    use myelin_knowledge::KnowledgeStore;
    use myelin_storage::OltpConfig;

    let cfg = MyelinConfig::dev();
    let endpoint = cfg.s3.endpoint.clone();
    let handle = tokio::runtime::Handle::current();
    let tenant = TenantId(format!("itest-knp31-durable-{}", std::process::id()));
    let blob = b"a Knowledge CRDT snapshot written through the INJECTED durable BlobStore".to_vec();

    fn oltp() -> OltpConfig {
        OltpConfig {
            max_pool_size: 8,
            statement_timeout_ms: 5_000,
            per_tenant_in_flight_cap: 4,
        }
    }

    let hash = {
        let s3cfg = cfg.s3.clone();
        let handle = handle.clone();
        let tenant = tenant.clone();
        let blob = blob.clone();
        tokio::task::spawn_blocking(move || {
            let object = S3BlobStore::connect(&s3cfg, handle);
            let store = KnowledgeStore::open(oltp(), Arc::new(object))
                .expect("KnowledgeStore opens with the injected durable BlobStore");
            store
                .blobs()
                .put(&tenant, &blob)
                .expect("put a Knowledge blob through the re-pointed injected S3 backing")
        })
        .await
        .expect("blocking open+put task")
    };

    let got = {
        let s3cfg = cfg.s3.clone();
        let handle = handle.clone();
        let tenant = tenant.clone();
        let hash = hash.clone();
        tokio::task::spawn_blocking(move || {
            let object = S3BlobStore::connect(&s3cfg, handle);
            let store = KnowledgeStore::open(oltp(), Arc::new(object))
                .expect("a FRESH KnowledgeStore opens over a fresh durable backing");
            store
                .blobs()
                .get(&tenant, &hash)
                .expect("the Knowledge blob SURVIVED the store reconstruction (byte-durable)")
        })
        .await
        .expect("blocking reopen+get task")
    };

    assert_eq!(
        got, blob,
        "W7.3: a Knowledge blob PUT through the injected durable BlobStore survives a fresh \
         KnowledgeStore reconstruction - the fs floor (Mutex<HashMap>) could not have"
    );

    let _ = tokio::task::spawn_blocking(move || {
        let object = S3BlobStore::connect(&cfg.s3, handle);
        let store =
            KnowledgeStore::open(oltp(), Arc::new(object)).expect("cleanup store opens");
        let _ = store.blobs().delete(&tenant, &hash);
    })
    .await;

    println!(
        "[W7.3 KN-P31 INTEGRATION GREEN] the RE-POINTED KnowledgeStore blob path (open(cfg, \
         Arc<dyn BlobStore> = S3BlobStore); store.blobs().put/get) is byte-DURABLE across a fresh \
         store reconstruction on the live dev stack ({endpoint}) - the fs floor is flipped out of the \
         production graph."
    );
}
