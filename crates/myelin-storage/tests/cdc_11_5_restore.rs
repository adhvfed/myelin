//! Contract 11.5 CDC pair — the `restore(to_offset T)` + cross-seam half (P-ST-12 / global P-060).
//!
//! The prompt requires "CDC: provider+consumer pair for 11.5 (the restore caller)". This is the
//! consumer-driven contract test:
//!
//! - the **PROVIDER** is `myelin-storage` — the [`restore_to_offset`] orchestration this prompt
//!   ships (PITR to the `seq ≤ T` cursor, referenced-hash-presence verification, reindex-from-source
//!   derived rebuild, KEK restore-except-crypto-shredded) + the [`RestoreReport`] / [`SourceLog`] /
//!   [`BlobPresence`] / [`WalRow`] types;
//! - the **CONSUMER** is the **restore caller** — the CI-wired restore-verify durability gate's
//!   restore job (P-ST-13 / global P-061, not yet built) modelled here as a tiny `RestoreVerifyJob`.
//!   It spins a clean target, restores OLTP/blob/KMS to the cross-seam point T, reindexes the derived
//!   stores from source to T, and asserts no-loss (0 dangling refs) + cross-seam consistency +
//!   crypto-shred-held. This is exactly the call shape the real restore-verify gate (P-061) relies
//!   on — if `restore_to_offset`'s signature / the report shape / the dangling-ref FAIL drift, this
//!   stops compiling/passing.
//!
//! It also pins the load-bearing contract properties the consumer depends on: a referenced-but-missing
//! `ContentHash` makes the restore FAIL (the §7.3 silent-corruption case, never a silent pass), and a
//! crypto-shredded KEK is NOT restored (it stays dead across a restore, §7.5).
//!
//! NOTE on row 11.5: the contract-index row 11.5 spans the BACKUP half (P-ST-11, `cdc_11_5_backup`),
//! this `restore(to_offset)` + cross-seam half (P-ST-12), and the CI-wired restore-verify GATE
//! (P-ST-13 / global P-061). This CDC pair covers the RESTORE caller; P-061 adds the CI-gate caller
//! to the same row.

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

/// A consumer of 11.5: the restore-verify durability gate's restore job (the P-061 caller). It drives
/// the provider exactly as the real CI restore-verify gate does: pick the cross-seam point T, restore
/// every tier to it, and read the report to decide pass/fail.
struct RestoreVerifyJob<'a> {
    archiver: ContinuousArchiver,
    kms: &'a KmsEngine,
}

impl<'a> RestoreVerifyJob<'a> {
    /// Boot against a KMS engine, with backups covering offsets `0..=tail` (a base at 0 + the WAL
    /// tail archived to `tail`).
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

    /// The restore-verify call the gate makes: restore every tier to the cross-seam point `t`.
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

/// **CDC happy path: the restore caller lands every tier at ONE consistent point T (0 dangling).**
/// The consumer drives the provider through the full restore and asserts the report shape it depends
/// on: restored to T, OLTP ≤ T, 0 dangling refs, derived reindexed-from-source resumed at T, the live
/// KEK restored.
#[test]
fn restore_caller_lands_a_consistent_point() {
    let kms = KmsEngine::new();
    let t = tenant("acme");
    kms.ensure_kek(&KekId::new(t.clone(), region()));
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
        }, // past T → dropped
    ];

    let report = job
        .restore(100, &rows, &blobs, &source)
        .expect("a consistent restore passes");

    // The report shape the consumer (P-061's gate) depends on:
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

/// **CDC failure-mode: the restore caller's no-loss assertion BITES.** A referenced-but-missing
/// `ContentHash` makes the restore FAIL (the §7.3 silent-corruption case) — the consumer's restore
/// returns `Err`, so the gate would FAIL CI (never a silent pass). The contract the gate relies on.
#[test]
fn restore_caller_fails_on_a_missing_referenced_blob() {
    let kms = KmsEngine::new();
    let job = RestoreVerifyJob::boot(&kms, 300);

    let mut blobs = BlobPresence::new();
    blobs.insert(h("present")); // "gone" is NOT restored — the injected silent-corruption case
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

/// **CDC: the restore caller's crypto-shred-held property.** A tenant whose KEK was destroyed since
/// the backup is NOT restored (it stays dead across the restore, §7.5) — the contract a restore-verify
/// gate's "erasure-held" assertion depends on.
#[test]
fn restore_caller_does_not_resurrect_a_shredded_tenant() {
    let kms = KmsEngine::new();
    let live = tenant("live");
    let shredded = tenant("shredded");
    let shredded_kek = KekId::new(shredded.clone(), region());
    kms.ensure_kek(&KekId::new(live.clone(), region()));
    kms.ensure_kek(&shredded_kek);
    kms.ensure_dek(&live, &region(), KeyClass::Tenant).unwrap();
    kms.ensure_dek(&shredded, &region(), KeyClass::Tenant)
        .unwrap();
    assert!(kms.destroy_kek(&shredded_kek), "crypto-shred the tenant");

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
