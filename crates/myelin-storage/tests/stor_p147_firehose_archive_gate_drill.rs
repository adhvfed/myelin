use myelin_events::{Firehose, FirehoseScope, FrameDraft};
use myelin_storage::{
    ContentHash, DekId, FirehoseArchiver, KekId, KeyClass, KmsEngine, SegmentBytes,
};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn engine() -> Arc<KmsEngine> {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant(), region()));
    Arc::new(kms)
}

#[test]
fn p147_gate_firehose_segment_seals_content_addressed_under_tenant_dek() {
    let mut firehose = Firehose::new();
    let scope = FirehoseScope::parse("board:logs").expect("a bounded 3.5 scope");
    for i in 1..=8u64 {
        firehose.publish("oplog", &scope, FrameDraft::new(format!("op-{i}")));
    }

    let eng = engine();
    let arch = FirehoseArchiver::with_tenant_dek(tenant(), region(), eng.clone());

    let segment = arch
        .seal_from_firehose(&firehose, "oplog", &scope, 3, 6)
        .expect("seal from the 3.5 transport")
        .expect("frames were held in the window");

    let live_frames = firehose.tail("oplog", &scope, 3, 6);
    let expected_address = ContentHash::blake3(&SegmentBytes::encode(&live_frames).0);
    assert_eq!(
        segment.content_hash, expected_address,
        "GATE leg 1: the segment is content-addressed (its hash IS the BLAKE3 of its bytes)"
    );
    assert!(segment
        .content_hash
        .to_multihash_string()
        .starts_with("blake3:"));
    assert!(
        arch.telemetry().segment_content_addressed(),
        "GATE leg 1: segment_content_addressed == true"
    );
    assert_eq!(
        (segment.first_seq, segment.last_seq),
        (3, 6),
        "the segment range aligns with the cursor"
    );

    let stored_len = arch
        .read_segment(&segment.content_hash)
        .map(|f| f.len())
        .expect("the segment decrypts back to its frames");
    assert_eq!(
        stored_len, 4,
        "the sealed segment round-trips back to its 4 frames (cold == live)"
    );
    assert_eq!(
        arch.read_segment(&segment.content_hash).expect("read"),
        live_frames,
        "GATE leg 2: the DEK-sealed segment decrypts to the EXACT live frames"
    );

    let unencrypted = arch.telemetry().unencrypted_segment_count();
    assert_eq!(
        unencrypted, 0,
        "GATE leg 3: unencrypted_segment_count == 0 (no plaintext-write path)"
    );
    assert_eq!(
        arch.telemetry().sealed_segment_count(),
        1,
        "the archive sealed exactly one segment"
    );

    println!(
        "[P-147 DRILL GREEN 2026-06-20] T3 firehose-archive seal: rode the 3.5 transport \
         (tail[3,6] of `oplog`/board:logs) → content-addressed segment {} under the per-tenant DEK; \
         unencrypted_segment_count={unencrypted}, segment_content_addressed={}; 4 frames round-trip \
         (cold == live).",
        segment.content_hash.to_multihash_string(),
        arch.telemetry().segment_content_addressed(),
    );
}

#[test]
fn p147_gate_destroyed_tenant_dek_crypto_shreds_the_segment_live_and_in_backups() {
    let eng = engine();
    let arch = FirehoseArchiver::with_tenant_dek(tenant(), region(), eng.clone());

    let mut firehose = Firehose::new();
    let scope = FirehoseScope::parse("channel:eng").expect("scope");
    firehose.publish("oplog", &scope, FrameDraft::new("inline-PII-op"));
    let segment = arch
        .seal_from_firehose(&firehose, "oplog", &scope, 1, 1)
        .expect("seal")
        .expect("held");
    assert!(
        arch.read_segment(&segment.content_hash).is_ok(),
        "the segment reads before the shred"
    );

    let destroyed = eng.destroy_dek(&DekId::new(tenant(), KeyClass::Tenant));
    assert!(
        destroyed,
        "the tenant DEK was destroyed (the crypto-shred lever)"
    );

    let live = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        arch.read_segment(&segment.content_hash)
    }));
    assert!(
        live.is_err(),
        "live: a crypto-shredded segment is unrecoverable (LOUD), never served"
    );

    let snapshot = eng.backup_snapshot();
    let tenant_dek_in_backup = snapshot
        .iter()
        .any(|(id, _)| *id == DekId::new(tenant(), KeyClass::Tenant));
    assert!(
        !tenant_dek_in_backup,
        "in backups: the shredded tenant DEK is excluded from the backup snapshot \
         (restore resurrects nothing - the segment stays dead across a restore, §7.5)"
    );

    println!(
        "[P-147 DRILL GREEN 2026-06-20] crypto-shred inheritance: destroying the per-tenant DEK \
         renders the sealed segment unrecoverable LIVE (loud refusal) and IN BACKUPS (the DEK is \
         excluded from backup_snapshot) - STOR-D1 erasure-held inherited by the T3 archive."
    );
}
