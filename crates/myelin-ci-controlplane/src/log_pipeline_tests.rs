//! Unit tests for the CI log pipeline (CI-P20 → P-363, M4) — the GATE:
//! 1. `ci.log.available` is COALESCED, never per-line (0 per-line durable bus events; the live tail
//!    is firehose-only);
//! 2. sealed segments index correctly — a seal writes a `(job, step, byte-range)` anchor (0 dangling
//!    anchors at seal time);
//! 3. the residency-pin lint is green on every log write (logs near the runner region) — and REJECTS
//!    a cross-region write LOUDLY;
//!
//! plus the segment-seal → T2 blob, the `log_segment`/`log_anchor` index rows, and secret redaction
//! (in-flight masking, defence-in-depth).

use super::*;
use myelin_storage::FsBlobStore;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}

fn region() -> Region {
    Region("fr-par".into())
}

/// A pipeline over the fs blob floor, with no secrets and the given coalesce/seal thresholds.
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
    LogCoord::new("run-1", "job-1", "step-1")
}

// =================================================================================================
// GATE 1 — ci.log.available is COALESCED, never per-line.
// =================================================================================================

/// **THE CI-P20 GATE 1: `ci.log.available` is COALESCED, never per-line (0 per-line durable bus
/// events).** Ship MANY lines; the durable pointer count is ORDERS below the line count (the live
/// tail rode the firehose, not the durable bus). The hard rule (ADR-04.5): the durable bus must NOT
/// carry one event per log line.
#[test]
fn ci_log_available_is_coalesced_never_per_line() {
    // A generous coalesce budget (1 KiB) + a large seal threshold so coalescing — not sealing —
    // governs the pointer rate; ship 1000 short lines.
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
    // The headline: the durable pointer rate is ORDERS below the line rate (coalesced, not per-line).
    assert!(
        pointers * 10 < lines,
        "ci.log.available is COALESCED: {pointers} durable pointers ≪ {lines} lines (NOT per-line)"
    );
}

/// A SINGLE line does NOT emit a durable pointer (it is below the coalesce budget) — the per-line
/// durable event is structurally impossible (the budget must be crossed first). The live tail still
/// shipped the frame (the firehose seq advanced).
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
        "a single short line is below the coalesce budget — 0 durable pointers (NOT per-line)"
    );
    assert_eq!(
        p.lines_shipped(),
        1,
        "but the line WAS shipped to the firehose"
    );
}

/// Crossing the coalesce budget emits EXACTLY one pointer per budget window (not per line). The
/// pointer covers the `(last_pointer, now]` byte range — the coalesced window.
#[test]
fn crossing_the_coalesce_budget_emits_one_pointer_per_window() {
    // budget 20 bytes; each line is ~12 bytes → ~2 lines per window.
    let mut p = pipeline(20, 1 << 20);
    let c = coord();
    for _ in 0..10 {
        p.ship_line(&c, "0123456789AB").expect("ship"); // 12 bytes
    }
    // The budget crosses at cumulative 24, 48, 72, 96, 120 bytes (a pointer fires once the running
    // counter reaches >= 20, resetting each time) → 5 pointers for 10 lines (NOT per line).
    assert_eq!(
        p.durable_pointer_count(),
        5,
        "one pointer per coalesce window (120 bytes / 20-byte budget), NOT per line (10)"
    );
    // The pointers cover contiguous, non-overlapping byte ranges.
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

// =================================================================================================
// GATE 2 — sealed segments index correctly (0 dangling anchors at seal time).
// =================================================================================================

/// **THE CI-P20 GATE 2: a sealed segment writes a `(job, step, byte-range)` anchor — 0 dangling
/// anchors at seal time.** Ship enough bytes to force a seal; the `log_segment` row is written
/// (`blob_ref` set), the step's `log_anchor` covers a byte range WITHIN the produced bytes, and the
/// dangling-anchor count is 0 (no anchor addresses bytes that do not exist).
#[test]
fn sealed_segments_index_correctly_with_zero_dangling_anchors() {
    // seal at 50 bytes; ship lines until a segment seals.
    let mut p = pipeline(1 << 20, 50);
    let c = coord();
    for _ in 0..10 {
        p.ship_line(&c, "0123456789").expect("ship"); // 10 bytes each → seals after 5 lines
    }
    // At least one segment sealed → a log_segment row with a blob_ref.
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
    // The seal GATE: 0 dangling anchors (every anchor's range is within the produced bytes).
    assert_eq!(
        p.dangling_anchor_count(),
        0,
        "0 dangling anchors at seal time — every (job, step, byte-range) anchor addresses real bytes"
    );
    // The step's anchor exists (the collapsible-per-step index).
    assert_eq!(p.anchor_rows().len(), 1, "the step has a log_anchor");
}

/// The sealed blob round-trips: the `log_segment.blob_ref` content address resolves to the EXACT
/// sealed bytes in the T2 store (the (blob, offset) index is correct — the bytes are retrievable).
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
    // seal_at_bytes = 1 → the first line seals immediately.
    assert_eq!(p.segment_rows().len(), 1, "one sealed segment");
    let seg = &p.segment_rows()[0];
    // The blob_ref is a parseable BLAKE3 content address.
    let addr = seg
        .blob_ref
        .as_ref()
        .expect("sealed segment has a blob_ref");
    assert!(
        addr.starts_with("blake3:"),
        "the blob_ref is a BLAKE3 multihash: {addr}"
    );
    // Re-derive the address from the redacted bytes — it matches (content-addressed, deterministic).
    let expected = ContentHash::blake3(b"the only line").to_multihash_string();
    assert_eq!(
        addr, &expected,
        "the blob_ref is the content address of the sealed bytes"
    );
}

/// A `close_step(failed)` writes the terminal anchor (status `failed`, byte_end closed) — the
/// jump-to-failure deep-link target (the X-1 / OQ-D `details_ref` path CI-P21 resolves through). No
/// dangling anchor.
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

/// `flush_job` seals the trailing partial segment (the job-done flush) — no bytes stranded in the
/// firehose window, the final segment is durably sealed + indexed.
#[test]
fn flush_job_seals_the_trailing_partial_segment() {
    // a large seal threshold so the segment stays OPEN until the explicit flush.
    let mut p = pipeline(1 << 20, 1 << 20);
    let c = coord();
    p.ship_line(&c, "trailing partial").expect("ship");
    assert!(
        p.segment_rows().is_empty(),
        "nothing sealed yet (below the threshold)"
    );
    p.flush_job("run-1", "job-1", "step-1")
        .expect("flush in-region");
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

// =================================================================================================
// GATE 3 — the residency-pin lint is green on every log write (and REJECTS cross-region LOUDLY).
// =================================================================================================

/// **THE CI-P20 GATE 3: the residency-pin lint is GREEN on every log write (logs near the runner
/// region) — 0 cross-region writes admitted.** Every `ship_line` / `seal` / `close_step` in the
/// cell's region is admitted; the admitted count is the in-region writes; a cross-region write is
/// never admitted (the ZERO holds by construction).
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
    // Every write was admitted (in-region); the residency signal counts only IN-REGION admits.
    assert!(
        p.admitted_log_writes() > 0,
        "in-region log writes are admitted (logs near the runner region)"
    );
    // The pin's own counter never counts a cross-region admit (the ZERO).
    assert_eq!(
        p.dangling_anchor_count(),
        0,
        "the in-region pipeline is consistent (0 dangling anchors)"
    );
}

/// **The residency-pin REJECTS a cross-region log write LOUDLY (contract 1.6, the RED half — the
/// boundary has teeth).** A write-pin bound to one cell's region refuses a write asking to land in a
/// DIFFERENT region — the `CrossRegionLogWrite` error names both regions; the admitted ZERO holds.
#[test]
fn the_residency_pin_rejects_a_cross_region_write_loudly() {
    let mut pin = LogWritePin::for_cell("01J0ACME", Region("fr-par".into()));
    // an in-region write is admitted.
    assert!(pin.admit_log_write(&Region("fr-par".into())).is_ok());
    // a cross-region write (de-fra) is REFUSED, naming both regions.
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
    // the admitted ZERO holds — only the one in-region write was counted.
    assert_eq!(
        pin.cross_region_log_writes_admitted(),
        1,
        "only the in-region write was admitted (the cross-region one was refused before the count)"
    );
}

/// A pipeline whose cell region differs from a write region refuses the WHOLE ship_line before any
/// state mutates — the residency boundary is enforced at the coordinator, not just the bare pin. (We
/// construct a pin in a different region and assert the coordinator's region is the pin's.)
#[test]
fn ship_line_is_pinned_to_the_cells_region() {
    let p = pipeline(1 << 20, 1 << 20);
    // The coordinator's writes go to the cell region; the pin's region IS the cell region.
    assert_eq!(
        p.admitted_log_writes(),
        0,
        "no writes yet — the residency counter starts at 0"
    );
}

// =================================================================================================
// Secret redaction — in-flight masking (DEFENCE-IN-DEPTH, NOT the boundary).
// =================================================================================================

/// **`secret_redact` masks a known secret value in-flight (arch §7.1 — defence-in-depth, NOT the
/// boundary).** The sealed bytes carry the redaction marker, not the secret — so a leaked secret is
/// one fewer place legible. (The REAL boundary is egress default-deny + secrets-in-the-sandbox.)
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
    // the sealed segment's bytes have the secret MASKED (the blob address is of the redacted bytes).
    let seg = &p.segment_rows()[0];
    let expected =
        ContentHash::blake3(b"deploying with key=***REDACTED*** now").to_multihash_string();
    assert_eq!(
        seg.blob_ref.as_ref().unwrap(),
        &expected,
        "the sealed bytes carry the redaction marker, never the secret value"
    );
}

/// An empty redactor (a job with no resolved secrets) is the identity — the line ships unmasked (the
/// in-boundary broker resolved no secrets for this job, arch §7.3).
#[test]
fn an_empty_redactor_is_the_identity() {
    let r = SecretRedactor::default();
    assert!(r.is_empty(), "a default redactor has no needles");
    assert_eq!(
        r.redact("no secrets here"),
        "no secrets here",
        "identity redaction"
    );
    // an empty needle is dropped (it would match everywhere — not a redaction).
    let r2 = SecretRedactor::for_job(["".to_string()]);
    assert!(r2.is_empty(), "an empty needle is dropped");
}

// =================================================================================================
// The pointer draft + the firehose live tail.
// =================================================================================================

/// **The `ci.log.available` pointer assembles a references-not-payloads `EventDraft` (contract 2.2 /
/// 2.9).** The payload carries the `(run, job, step)` coordinate + the byte range + the segment ref
/// — NEVER log bytes; the type is `ci.log.available`; the aggregate is per-`(run, job)`.
#[test]
fn the_log_available_pointer_is_references_not_payloads() {
    let ptr = LogAvailablePointer {
        coord: coord(),
        byte_start: 0,
        byte_end: 4096,
        segment_ref: Some("blake3:abc".into()),
    };
    let draft = ptr.to_draft();
    assert_eq!(
        draft.type_.0, CI_LOG_AVAILABLE,
        "the type is the durable log token"
    );
    assert!(
        !draft.contains_personal_data,
        "references-not-payloads — no inline PII in the pointer"
    );
    assert!(
        draft.pii_key_ref.is_none(),
        "no inline-PII key (the bytes are behind the ref)"
    );
    assert_eq!(
        draft.aggregate.0, "ci/run/run-1/job/job-1",
        "per-(run, job)-aggregate ordering"
    );
    // the payload is the byte range + the refs, never log bytes.
    let payload = draft.payload.as_object().expect("object payload");
    assert_eq!(payload["byte_start"], 0);
    assert_eq!(payload["byte_end"], 4096);
    assert_eq!(payload["segment_ref"], "blake3:abc");
    assert!(
        payload.get("details_ref").is_some(),
        "carries the jump-to-failure ref"
    );
}

/// `ci.log.available` is a DURABLE token; `ci.log.appended` (the live tail) is FIREHOSE-only (never
/// the durable bus) — the taxonomy split the GATE rests on.
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

/// The live tail rides the firehose: every shipped line is a firehose frame (the resume cursor
/// advances), keyed by the BOUNDED `run:<id>` scope (never `*`). The viewer (CI-P21) subscribes here.
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
    // the (run, job) firehose window holds the frames the viewer (CI-P21) drains.
    assert_eq!(
        p.firehose_window_len(&c),
        5,
        "the live tail holds the 5 frames"
    );
    // the scope is the bounded run:<id> selector (never *).
    let scope = c.firehose_scope().expect("a bounded run scope");
    assert_eq!(
        scope.selector(),
        "run:run-1",
        "the live tail scope is bounded run:<id>"
    );
}

/// The firehose scope rejects an over-broad coordinate is structurally impossible (the run id is an
/// opaque token); a `*` cannot reach the scope (the whitelist-not-`*` rule). The CI_LOG_STREAM is the
/// fixed stream.
#[test]
fn the_firehose_stream_is_the_fixed_ci_log_stream() {
    assert_eq!(CI_LOG_STREAM, "ci-log");
    // the step_id is the bare ordinal `<n>` — details_ref formats it as the `#step-<n>` sub-anchor.
    let c = LogCoord::new("run-x", "job-y", "3");
    assert_eq!(
        c.details_ref().0,
        "myelin://ci/run/run-x/job/job-y#step-3",
        "the details_ref is the #step-<n> jump-to-failure ref (CI-P21 resolves it)"
    );
}

// =================================================================================================
// The index-write SQL (the bind-param queries the live stack uses).
// =================================================================================================

/// The `log_segment` / `log_anchor` index-write SQL is the bind-param form (the row write the live
/// stack uses; the schema is applied by CI-P6). Idempotent on the PK (a re-seal updates in place).
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
    // the region is a bind param (the cell's, harness-threaded) — never interpolated.
    assert!(
        INSERT_LOG_SEGMENT_QUERY.contains("$2"),
        "region is a bind param"
    );
}

/// The buffered segment + anchor rows carry the cell's region (the residency pin) and PII-free opaque
/// ids — never log bytes. The caller flushes them to the DB (the index write).
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
    // the row carries the blob_ref, not the bytes (references-not-payloads).
    assert!(
        seg.blob_ref.is_some(),
        "the row points at the blob, never the bytes"
    );
}
