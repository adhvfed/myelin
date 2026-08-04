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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn srch_p30_object_store_backstop_swap_and_reerase() {
    let cfg = MyelinConfig::dev();
    let blobs = Arc::new(S3BlobStore::connect(
        &cfg.s3,
        tokio::runtime::Handle::current(),
    ));
    let tenant = TenantId(format!("acme-srch-p30-{}", std::process::id()));
    let target = "u-target";

    let subject_docs = ["t1", "t2", "t3"];
    let other_docs = ["o0", "o1", "o2", "o3", "o4", "o5"];
    let (ix, ids) = build_live_corpus(&tenant, &region(), target, &subject_docs, &other_docs);

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

    let backstop = SegmentBackstop::new(Arc::clone(&blobs), tenant.clone(), region());
    let gate = ObjectStoreBackstopGate::new();
    let swapped = tokio::task::block_in_place(|| gate.swap_in(&backstop, &segments))
        .expect("swap_in over the LIVE object store: segments moved byte-identical");
    assert_eq!(swapped.loaded.len(), 3, "all three segments read back");
    assert_eq!(
        swapped.byte_identical, 3,
        "every segment recovered BYTE-IDENTICAL from the object store (behaviour unchanged)"
    );

    let holder = SearchEraseHolder::new(ix.clone(), pin.clone(), region());
    let mut d4_inputs = BackupScaleEraseInputs {
        erase_holder: &holder,
        dek: &pin,
        index_key_ref: key_ref,
        subject: subject(target, &tenant),
        tenant: tenant.clone(),
        backup_segments: &swapped.loaded,
        subject_backstop_id: None,
        now: NOW.into(),
    };
    let d4 = BackupScaleEraseGate::new().run(&mut d4_inputs);

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

    println!("[P-463 GATE GREEN 2026-06-25] {}", artifact.summary());

    let still_resident = tokio::task::block_in_place(|| backstop.load_all(&swapped.stored))
        .expect("the object-store objects are still readable (content-addressed ciphertext)");
    assert_eq!(
        still_resident.len(),
        3,
        "the sealed ciphertext objects remain in the object store (erasure = crypto-shred, §7.5)"
    );
}
