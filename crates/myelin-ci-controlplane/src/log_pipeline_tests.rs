use super::*;
use myelin_storage::FsBlobStore;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}

fn region() -> Region {
    Region("fr-par".into())
}

fn pipeline(coalesce_bytes: u64, seal_bytes: u64) -> LogPipeline<FsBlobStore> {
    LogPipeline::new(
        tenant(),
        region(),
        FsBlobStore::new(),
        SecretRedactor::default(),
    )
    .with_thresholds(
        CoalesceBudget {
            bytes_per_pointer: coalesce_bytes,
        },
        SealThreshold {
            seal_at_bytes: seal_bytes,
        },
    )
}

fn coord() -> LogCoord {
    LogCoord::new("run-1", "job-1", 1)
}

#[test]
fn ci_log_available_is_coalesced_never_per_line() {
    let mut p = pipeline(1024, 1 << 20);
    let c = coord();
    for i in 0..1000 {
        p.ship_line(&c, &format!("log line number {i}"))
            .expect("in-region ship");
    }
    let lines = p.lines_shipped();
    let pointers = p.durable_pointer_count();
    assert_eq!(
        lines, 1000,
        "every line was shipped to the firehose (live tail)"
    );
    assert!(
        pointers > 0,
        "at least one coalesced pointer was emitted (the bytes are durably available)"
    );
    assert!(
        pointers * 10 < lines,
        "ci.log.available is COALESCED: {pointers} durable pointers ≪ {lines} lines (NOT per-line)"
    );
}

#[test]
fn a_single_line_emits_zero_durable_pointers() {
    let mut p = pipeline(1024, 1 << 20);
    let c = coord();
    let seq = p.ship_line(&c, "one short line").expect("ship");
    assert_eq!(
        seq, 1,
        "the firehose seq advanced (the live tail shipped it)"
    );
    assert_eq!(
        p.durable_pointer_count(),
        0,
        "a single short line is below the coalesce budget - 0 durable pointers (NOT per-line)"
    );
    assert_eq!(
        p.lines_shipped(),
        1,
        "but the line WAS shipped to the firehose"
    );
}

#[test]
fn crossing_the_coalesce_budget_emits_one_pointer_per_window() {
    let mut p = pipeline(20, 1 << 20);
    let c = coord();
    for _ in 0..10 {
        p.ship_line(&c, "0123456789AB").expect("ship");
    }
    assert_eq!(
        p.durable_pointer_count(),
        5,
        "one pointer per coalesce window (120 bytes / 20-byte budget), NOT per line (10)"
    );
    let pointers = p.drain_pointers();
    let mut prev_end = 0i64;
    for ptr in &pointers {
        assert_eq!(ptr.byte_start, prev_end, "pointers cover contiguous ranges");
        assert!(
            ptr.byte_end > ptr.byte_start,
            "each pointer covers new bytes"
        );
        prev_end = ptr.byte_end;
    }
}

#[test]
fn sealed_segments_index_correctly_with_zero_dangling_anchors() {
    let mut p = pipeline(1 << 20, 50);
    let c = coord();
    for _ in 0..10 {
        p.ship_line(&c, "0123456789").expect("ship");
    }
    assert!(
        !p.segment_rows().is_empty(),
        "a segment sealed (the bytes crossed the seal threshold)"
    );
    for seg in p.segment_rows() {
        assert!(
            seg.blob_ref.is_some(),
            "a sealed segment has a content-addressed blob_ref (the T2 flush)"
        );
        assert!(
            seg.byte_end > seg.byte_start,
            "the sealed segment covers a non-empty byte range"
        );
        assert!(
            seg.pii_key_ref.starts_with("kms://"),
            "the segment names a DEK ref (per-tenant; per-subject is CI-P22)"
        );
    }
    assert_eq!(
        p.dangling_anchor_count(),
        0,
        "0 dangling anchors at seal time - every (job, step, byte-range) anchor addresses real bytes"
    );
    assert_eq!(p.anchor_rows().len(), 1, "the step has a log_anchor");
}

#[test]
fn the_sealed_segment_blob_round_trips_the_exact_bytes() {
    let blobs = FsBlobStore::new();
    let mut p = LogPipeline::new(tenant(), region(), blobs, SecretRedactor::default())
        .with_thresholds(
            CoalesceBudget::default(),
            SealThreshold { seal_at_bytes: 1 },
        );
    let c = coord();
    p.ship_line(&c, "the only line").expect("ship");
    assert_eq!(p.segment_rows().len(), 1, "one sealed segment");
    let seg = &p.segment_rows()[0];
    let addr = seg
        .blob_ref
        .as_ref()
        .expect("sealed segment has a blob_ref");
    assert!(
        addr.starts_with("blake3:"),
        "the blob_ref is a BLAKE3 multihash: {addr}"
    );
    let expected = ContentHash::blake3(b"the only line").to_multihash_string();
    assert_eq!(
        addr, &expected,
        "the blob_ref is the content address of the sealed bytes"
    );
}

#[test]
fn close_step_writes_the_terminal_anchor_with_zero_dangling() {
    let mut p = pipeline(1 << 20, 1 << 20);
    let c = coord();
    p.ship_line(&c, "step output line").expect("ship");
    p.close_step(&c, AnchorStatus::Failed)
        .expect("close in-region");
    let anchors = p.anchor_rows();
    assert_eq!(anchors.len(), 1);
    let a = anchors[0];
    assert_eq!(
        a.status,
        AnchorStatus::Failed,
        "the terminal status is recorded"
    );
    assert!(
        a.byte_end.is_some(),
        "a terminal anchor closes its byte_end"
    );
    assert!(a.status.is_terminal(), "failed is a terminal status");
    assert_eq!(
        a.status.token(),
        "failed",
        "the canonical CHECK-constraint token"
    );
    assert_eq!(
        p.dangling_anchor_count(),
        0,
        "the closed anchor is within the produced bytes"
    );
}

#[test]
fn flush_job_seals_the_trailing_partial_segment() {
    let mut p = pipeline(1 << 20, 1 << 20);
    let c = coord();
    p.ship_line(&c, "trailing partial").expect("ship");
    assert!(
        p.segment_rows().is_empty(),
        "nothing sealed yet (below the threshold)"
    );
    p.flush_job("run-1", "job-1", 1).expect("flush in-region");
    assert_eq!(
        p.segment_rows().len(),
        1,
        "flush_job sealed the trailing partial segment"
    );
    assert!(
        p.segment_rows()[0].blob_ref.is_some(),
        "the flushed segment is in T2"
    );
    assert_eq!(
        p.dangling_anchor_count(),
        0,
        "0 dangling anchors after the flush"
    );
}

#[test]
fn the_residency_pin_is_green_on_every_log_write() {
    let mut p = pipeline(1 << 20, 30);
    let c = coord();
    for _ in 0..10 {
        p.ship_line(&c, "0123456789")
            .expect("in-region ship is admitted");
    }
    p.close_step(&c, AnchorStatus::Passed)
        .expect("in-region close is admitted");
    assert!(
        p.admitted_log_writes() > 0,
        "in-region log writes are admitted (logs near the runner region)"
    );
    assert_eq!(
        p.dangling_anchor_count(),
        0,
        "the in-region pipeline is consistent (0 dangling anchors)"
    );
}

#[test]
fn the_residency_pin_rejects_a_cross_region_write_loudly() {
    let mut pin = LogWritePin::for_cell("01J0ACME", Region("fr-par".into()));
    assert!(pin.admit_log_write(&Region("fr-par".into())).is_ok());
    let err = pin
        .admit_log_write(&Region("de-fra".into()))
        .expect_err("a cross-region log write is refused");
    assert_eq!(err.cell_region.as_str(), "fr-par");
    assert_eq!(err.row_region.as_str(), "de-fra");
    let msg = err.to_string();
    assert!(msg.contains("REFUSED"), "the refusal is LOUD: {msg}");
    assert!(
        msg.contains("fr-par") && msg.contains("de-fra"),
        "names both regions"
    );
    assert_eq!(
        pin.cross_region_log_writes_admitted(),
        1,
        "only the in-region write was admitted (the cross-region one was refused before the count)"
    );
}

#[test]
fn ship_line_is_pinned_to_the_cells_region() {
    let p = pipeline(1 << 20, 1 << 20);
    assert_eq!(
        p.admitted_log_writes(),
        0,
        "no writes yet - the residency counter starts at 0"
    );
}

#[test]
fn secret_redact_masks_a_known_secret_value_in_flight() {
    let redactor = SecretRedactor::for_job(["s3cr3t-token-value".to_string()]);
    let mut p = LogPipeline::new(tenant(), region(), FsBlobStore::new(), redactor).with_thresholds(
        CoalesceBudget::default(),
        SealThreshold { seal_at_bytes: 1 },
    );
    let c = coord();
    p.ship_line(&c, "deploying with key=s3cr3t-token-value now")
        .expect("ship");
    let seg = &p.segment_rows()[0];
    let expected =
        ContentHash::blake3(b"deploying with key=***REDACTED*** now").to_multihash_string();
    assert_eq!(
        seg.blob_ref.as_ref().unwrap(),
        &expected,
        "the sealed bytes carry the redaction marker, never the secret value"
    );
}

#[test]
fn an_empty_redactor_is_the_identity() {
    let r = SecretRedactor::default();
    assert!(r.is_empty(), "a default redactor has no needles");
    assert_eq!(
        r.redact("no secrets here"),
        "no secrets here",
        "identity redaction"
    );
    let r2 = SecretRedactor::for_job(["".to_string()]);
    assert!(r2.is_empty(), "an empty needle is dropped");
}

#[test]
fn the_log_available_pointer_is_references_not_payloads() {
    let ptr = LogAvailablePointer {
        coord: coord(),
        byte_start: 0,
        byte_end: 4096,
        segment_ref: Some("blake3:abc".into()),
    };
    let draft = ptr.to_draft(&tenant()).expect("canonical log pointer");
    assert_eq!(
        draft.type_.0, CI_LOG_AVAILABLE,
        "the type is the durable log token"
    );
    assert!(
        !draft.contains_personal_data,
        "references-not-payloads - no inline PII in the pointer"
    );
    assert!(
        draft.pii_key_ref.is_none(),
        "no inline-PII key (the bytes are behind the ref)"
    );
    assert_eq!(
        draft.aggregate.0, "ci/run/run-1/job/job-1",
        "per-(run, job)-aggregate ordering"
    );
    assert_eq!(
        draft.subject.0, "myelin://01J0ACME/ci/log/run-1:job-1:1",
        "the searchable document identity is canonical and tenant-bound"
    );
    let payload = draft.payload.as_object().expect("object payload");
    assert_eq!(payload["byte_start"], 0);
    assert_eq!(payload["byte_end"], 4096);
    assert_eq!(payload["segment_ref"], "blake3:abc");
    assert_eq!(
        payload["details_ref"], "myelin://01J0ACME/ci/run/run-1#step-1",
        "the separate human deep link is canonical and tenant-bound"
    );
}

#[test]
fn the_log_available_token_is_durable_the_appended_token_is_firehose() {
    use crate::events::is_durable;
    assert!(
        is_durable(CI_LOG_AVAILABLE),
        "ci.log.available is the durable pointer"
    );
    assert!(
        !is_durable("ci.log.appended"),
        "ci.log.appended is firehose-only (never the durable bus)"
    );
}

#[test]
fn every_shipped_line_is_a_firehose_frame_on_a_bounded_scope() {
    let mut p = pipeline(1 << 20, 1 << 20);
    let c = coord();
    for i in 1..=5 {
        let seq = p.ship_line(&c, "frame").expect("ship");
        assert_eq!(
            seq, i,
            "the firehose seq advances per line (the live tail / resume cursor)"
        );
    }
    assert_eq!(
        p.firehose_window_len(&c),
        5,
        "the live tail holds the 5 frames"
    );
    let scope = c.firehose_scope().expect("a bounded run scope");
    assert_eq!(
        scope.selector(),
        "run:run-1",
        "the live tail scope is bounded run:<id>"
    );
}

#[test]
fn the_firehose_stream_is_the_fixed_ci_log_stream() {
    assert_eq!(CI_LOG_STREAM, "ci-log");
    let c = LogCoord::new("run-x", "job-y", 3);
    assert_eq!(
        c.details_ref(&tenant()).expect("canonical details ref").0,
        "myelin://01J0ACME/ci/run/run-x#step-3",
        "the details_ref is the #step-<n> jump-to-failure ref (CI-P21 resolves it)"
    );
}

#[test]
fn the_index_write_sql_is_idempotent_bind_param() {
    assert!(INSERT_LOG_SEGMENT_QUERY.contains("INSERT INTO log_segment"));
    assert!(
        INSERT_LOG_SEGMENT_QUERY.contains("ON CONFLICT"),
        "the segment write is idempotent on (tenant, run, job, seq)"
    );
    assert!(UPSERT_LOG_ANCHOR_QUERY.contains("INSERT INTO log_anchor"));
    assert!(
        UPSERT_LOG_ANCHOR_QUERY.contains("ON CONFLICT"),
        "the anchor write is idempotent on (tenant, run, job, step)"
    );
    assert!(
        INSERT_LOG_SEGMENT_QUERY.contains("$2"),
        "region is a bind param"
    );
}

#[test]
fn the_buffered_index_rows_carry_the_cell_region_and_no_bytes() {
    let mut p = pipeline(1 << 20, 1);
    let c = coord();
    p.ship_line(&c, "x").expect("ship");
    let seg = &p.segment_rows()[0];
    assert_eq!(
        seg.region, "fr-par",
        "the segment row carries the cell's region"
    );
    assert_eq!(seg.tenant_id, "01J0ACME", "the opaque tenant token");
    assert_eq!(seg.run_id, "run-1");
    assert!(
        seg.blob_ref.is_some(),
        "the row points at the blob, never the bytes"
    );
}

#[test]
fn malformed_run_coordinates_are_refused_without_mutating_log_state() {
    let mut p = pipeline(1 << 20, 1);
    let invalid = LogCoord::new("", "job-1", 1);

    assert!(matches!(
        p.ship_line(&invalid, "untrusted output"),
        Err(LogPipelineError::InvalidScope(_))
    ));
    assert!(p.segment_rows().is_empty());
    assert!(p.anchor_rows().is_empty());
    assert!(p.drain_pointers().is_empty());
}

#[test]
fn ambiguous_log_coordinates_never_become_durable_subjects() {
    for coord in [
        LogCoord::new("", "job-1", 1),
        LogCoord::new("run-1", "", 1),
        LogCoord::new("run:1", "job-1", 1),
        LogCoord::new("run-1", "job/1", 1),
    ] {
        let pointer = LogAvailablePointer {
            coord,
            byte_start: 0,
            byte_end: 1,
            segment_ref: None,
        };
        assert!(matches!(
            pointer.to_draft(&tenant()),
            Err(LogReferenceError::InvalidCoordinate { .. })
        ));
    }
}
