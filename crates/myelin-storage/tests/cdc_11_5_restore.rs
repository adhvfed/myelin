use myelin_storage::{
    restore_to_offset, BlobPresence, ContentHash, ContinuousArchiver, KekId, KeyClass, KmsEngine,
    RestoreError, RestoreReport, SourceLog, WalRow, WalSegment,
};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("eu-west".into())
}
fn tenant(s: &str) -> TenantId {
    TenantId(s.into())
}
fn h(s: &str) -> ContentHash {
    ContentHash::blake3(s.as_bytes())
}

struct RestoreVerifyJob<'a> {
    archiver: ContinuousArchiver,
    kms: &'a KmsEngine,
}

impl<'a> RestoreVerifyJob<'a> {
    fn boot(kms: &'a KmsEngine, tail: u64) -> Self {
        let mut archiver = ContinuousArchiver::new();
        archiver
            .archive_segment(WalSegment {
                end_offset: 0,
                committed_at: 0,
            })
            .unwrap();
        archiver.take_base_backup(1);
        archiver
            .archive_segment(WalSegment {
                end_offset: tail,
                committed_at: 10,
            })
            .unwrap();
        RestoreVerifyJob { archiver, kms }
    }

    fn restore(
        &self,
        t: u64,
        rows: &[WalRow],
        blobs: &BlobPresence,
        source: &SourceLog,
    ) -> Result<RestoreReport, RestoreError> {
        restore_to_offset(&self.archiver, t, rows, blobs, source, self.kms)
    }
}

#[test]
fn restore_caller_lands_a_consistent_point() {
    let kms = KmsEngine::new();
    let t = tenant("acme");
    kms.ensure_kek(&KekId::new(t.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(&t, &region(), KeyClass::Tenant).unwrap();

    let job = RestoreVerifyJob::boot(&kms, 300);

    let mut blobs = BlobPresence::new();
    blobs.insert(h("a")).insert(h("b"));
    let mut source = SourceLog::new();
    source.append(90, "r1").append(100, "r2");
    let rows = vec![
        WalRow {
            id: "r1".into(),
            written_at: 90,
            blob_ref: Some(h("a")),
        },
        WalRow {
            id: "r2".into(),
            written_at: 100,
            blob_ref: Some(h("b")),
        },
        WalRow {
            id: "r3".into(),
            written_at: 250,
            blob_ref: None,
        },
    ];

    let report = job
        .restore(100, &rows, &blobs, &source)
        .expect("a consistent restore passes");

    assert_eq!(
        report.restored_to_offset, 100,
        "restored to the cross-seam point T"
    );
    assert_eq!(
        report.oltp_rows.len(),
        2,
        "OLTP rows ≤ T (the row past T was dropped)"
    );
    assert_eq!(
        report.dangling_ref_count, 0,
        "no-loss: 0 dangling blob refs"
    );
    assert!(
        report.derived.has_doc("r1") && report.derived.has_doc("r2"),
        "derived == source-replay"
    );
    assert_eq!(report.derived.resumed_at(), 100, "consumers resume at T");
    assert!(
        report.restored_key_for_tenant(&t),
        "the live tenant's KEK is restored"
    );
}

#[test]
fn restore_caller_fails_on_a_missing_referenced_blob() {
    let kms = KmsEngine::new();
    let job = RestoreVerifyJob::boot(&kms, 300);

    let mut blobs = BlobPresence::new();
    blobs.insert(h("present"));
    let source = SourceLog::new();
    let rows = vec![
        WalRow {
            id: "ok".into(),
            written_at: 50,
            blob_ref: Some(h("present")),
        },
        WalRow {
            id: "bad".into(),
            written_at: 90,
            blob_ref: Some(h("gone")),
        },
    ];

    let err = job
        .restore(100, &rows, &blobs, &source)
        .expect_err("a row → missing blob MUST fail the restore (the gate fails CI)");
    assert!(
        matches!(err, RestoreError::DanglingBlobRef { ref row_id, .. } if row_id == "bad"),
        "the restore caller's no-loss assertion must name the offending row: {err}"
    );
}

#[test]
fn restore_caller_does_not_resurrect_a_shredded_tenant() {
    let kms = KmsEngine::new();
    let live = tenant("live");
    let shredded = tenant("shredded");
    let shredded_kek = KekId::new(shredded.clone(), region());
    kms.ensure_kek(&KekId::new(live.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_kek(&shredded_kek)
        .expect("seed the in-memory KEK");
    kms.ensure_dek(&live, &region(), KeyClass::Tenant).unwrap();
    kms.ensure_dek(&shredded, &region(), KeyClass::Tenant)
        .unwrap();
    assert!(
        kms.destroy_kek(&shredded_kek).unwrap(),
        "crypto-shred the tenant"
    );

    let job = RestoreVerifyJob::boot(&kms, 300);
    let report = job
        .restore(100, &[], &BlobPresence::new(), &SourceLog::new())
        .expect("the restore itself passes");

    assert!(
        report.restored_key_for_tenant(&live),
        "the live tenant's KEK is restored"
    );
    assert!(
        !report.restored_key_for_tenant(&shredded),
        "a CRYPTO-SHREDDED tenant must NOT be resurrected by a restore (§7.5)"
    );
}
