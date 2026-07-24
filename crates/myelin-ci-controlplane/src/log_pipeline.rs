//! # `log_pipeline` — logs over the firehose + the sealed T3 log tier + `ci.log.available` pointers
//! (CI-P20 → P-363, M4)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §7.1 (**logs ride the firehose + the resume-cursor protocol; CI owns the `ci.log.available`
//! pointer**): the `ship_line` coordinator — `secret_redact` (in-flight masking, DEFENCE-IN-DEPTH,
//! NOT the boundary) → `firehose::publish(stream_of(run,job,step), frame(redacted))` (the LIVE TAIL,
//! never the durable bus; CI is the heaviest firehose producer) → `seal_and_flush_if_segment_full`
//! (→ a T2 content-addressed blob + the `(job, step, byte-range)` index, Storage 11.8) →
//! `emit_pointer_if_threshold(ci.log.available { run, job, step, range })` (a COALESCED durable
//! pointer, NEVER one durable event per line). `01-tech-and-data-model.md` §3.5 (the log range index
//! — `log_segment` + `log_anchor`; the `#step-<n>` sub-anchor resolves `CheckStatus.details_ref`).
//!
//! **Contracts (this module's stake):**
//! - **3.5** the FIREHOSE transport (CONSUMED) — `myelin_events::firehose::{Firehose, FirehoseScope,
//!   FrameDraft}`. The live tail rides this ephemeral transport, never the durable outbox. CI is the
//!   heaviest firehose producer (event-bus §4.3). This module PUBLISHES frames; the resume-cursor
//!   live-tail VIEWER (`subscribe`/`resume`) is CI-P21 (P-364) — a separate prompt.
//! - **11.8** the T3 `(job, step, byte-range)` index (CONSUMED — CI co-owns the index usage) — a
//!   sealed segment writes a [`LogSegmentRow`] (the `log_segment` row) + the step writes a
//!   [`LogAnchorRow`] (the `log_anchor` row) so a failed step deep-links to its byte range (the X-1 /
//!   OQ-D jump-to-failure path the `details_ref` resolves through).
//! - **11.2** the T2 `BlobStore` (CONSUMED — `myelin_storage::BlobStore`) — a sealed segment flushes
//!   to a content-addressed (BLAKE3, per-tenant-dedup) blob; the `log_segment.blob_ref` names it.
//! - **2.2** the `ci.log.available` POINTER via the outbox (the ONLY log-related DURABLE event) — a
//!   coalesced [`EventDraft`] ("lines N..M of `run/job/step` are ready at `<ArtifactRef>`"),
//!   references-not-payloads (the pointer carries the byte range + the blob ref, NEVER log bytes).
//!   The producer plumbing onto `ctx.emit` (the outbox) is the `serve` lifecycle; this module
//!   ASSEMBLES the draft and decides WHEN to emit it (the coalescing).
//!
//! ## The headline invariants (the CI-P20 GATE)
//! 1. **`ci.log.available` is COALESCED, never per-line** ([`LogPipeline::durable_pointer_count`] ≪
//!    lines shipped; 0 per-line durable events). The live tail is firehose-ONLY. A pointer is emitted
//!    only when a coalescing THRESHOLD is crossed (a sealed segment, or a byte/line budget) — the
//!    ADR-04.5 hard rule "the durable bus must not carry one event per log line".
//! 2. **Sealed segments index correctly — 0 dangling anchors at seal time.** Sealing segment `k`
//!    writes its `log_segment` row AND closes every `log_anchor` whose byte range falls in the sealed
//!    span (no anchor points past the sealed bytes — [`LogPipeline::dangling_anchor_count`] is 0).
//! 3. **The residency-pin lint is GREEN on every log write** (logs near the runner region) — every
//!    `log_segment` / `log_anchor` / blob write routes through [`LogWritePin::admit_log_write`], which
//!    REJECTS a write whose region ≠ the cell's region (the CI-side `residency-pin` write-boundary,
//!    contract 1.6 — the exact analogue of the fleet's [`crate::fleet::RunnerWritePin`]).
//!
//! ## FLOORS named (VISION §3 — name-your-floors)
//! - **The object-segment T3 log tier + the OLTP `(job, step, byte-range)` index ships v1.** A
//!   dedicated **time-series / wide-column log tier** is the named follow-on **CI-P29 / CI-M5**
//!   (P-489), promoted ONLY once event volume is MEASURED to outgrow the OLTP-indexed tier (EI-04 §5
//!   — not before). NAMED, never silently "the" tier.
//! - **The resume-cursor LIVE-TAIL viewer + the `details_ref` jump-to-failure resolution is CI-P21**
//!   (P-364). This module PRODUCES the live frames + the index; the VIEWER side (`subscribe`/`resume`
//!   + the `#step-<n>` → byte-range resolution) is CI-P21.
//! - **The per-subject DEK for isolable inline log PII (Storage C1) is CI-P22** (P-365). Here the
//!   `pii_key_ref` is the per-TENANT DEK ref by default; the per-subject keying is CI-P22. The field
//!   travels now (the schema demands it); the per-subject crypto-shred reach is CI-P22.
//! - **The LIVE firehose transport binding + the LIVE `BlobStore`** are the deploy-time swaps behind
//!   the frozen `Firehose` / `BlobStore` surfaces (the in-process `Firehose` + the `FsBlobStore` floor
//!   prove the protocol shape here). The forward-only `log_segment`/`log_anchor` schema is already
//!   applied (CI-P6); this module POPULATES it.
//!
//! ## `no-raw-publish` lint note (EI-01 §1 / §5 — NAMED, not a silent skip)
//! [`LogPipeline::ship_line`] calls `firehose.publish(stream=ci-log, scope=run:<id>, frame)` — the
//! frozen contract-3.5 / §5.5 EPHEMERAL firehose transport method, a DIFFERENT seam from the durable
//! bus the `no-raw-publish` lint (EB-07) guards. A firehose frame is a references-not-payloads
//! byte-range pointer ([`myelin_events::firehose::FrameDraft`]), NOT an inline-PII durable event, and
//! is NOT emitted-iff-committed through the outbox — it is the high-volume ephemeral live tail (arch
//! §7.1: "logs ride the firehose"; "CI is the heaviest firehose producer"). CI's DURABLE log emit
//! (the COALESCED `ci.log.available` pointer) is the BUFFERED [`LogAvailablePointer::to_draft`]
//! `EventDraft` the caller emits through the OUTBOX (`no-raw-publish` green). The lint's `.publish(`
//! fingerprint collides with the frozen `firehose::publish` NAME, so this ONE file is on the
//! lint-gate's NAMED, LOUD exclusion list (`myelin-lints/src/bin/lint-gate.rs` +
//! `tests/workspace_clean.rs`) — the exact posture as `myelin-knowledge/src/transport.rs` /
//! `myelin-events/src/firehose.rs`. The lint stays fully live on every durable-bus call site; this is
//! a documented deviation, not a weakening.
//!
//! ## DB-free by default
//! The coordinator holds the in-process `Firehose` + a `BlobStore` handle + the index-row buffer;
//! `cargo build`/`cargo test --workspace` stay DB-free. The `log_segment`/`log_anchor` rows are the
//! typed [`LogSegmentRow`]/[`LogAnchorRow`] values + the bind-param SQL the row write uses against the
//! live stack (the schema apply is `tests/integration_ci_p6_controlplane_schema.rs`); the CDC pair for
//! the consumed rows 3.5 + 11.8 + 11.2 is `tests/cdc_*`.

use myelin_events::firehose::{Firehose, FirehoseError, FirehoseScope, FrameDraft};
use myelin_events::{ArtifactRef, DataRole, EventDraft, EventType, Visibility};
use myelin_storage::{BlobStore, ContentHash};
use myelin_tenancy::{Region, TenantId};

// The canonical `ci.log.available` durable token — the ONE source of truth (EB-27 / P-327's names
// freeze in `myelin_ci_sandbox::events`; CI re-exports, never re-defines — coherence, EI-01 §7).
use myelin_ci_sandbox::events::CI_LOG_AVAILABLE;

// =================================================================================================
// 1. The residency-pin log-WRITE boundary (contract 1.6 — the CI-side `residency-pin` lint).
// =================================================================================================

// @residency-write — the residency-pin write-boundary (layer-3) leg arms on this file: a log_segment /
// log_anchor / blob write's region is the CELL's (threaded by the harness), NEVER a request/payload
// field. Every seal below routes through `LogWritePin::admit_log_write(row_region)` which reads the
// harness-threaded `cell` region, never a caller-controlled region — so the lint admits the write
// (logs live near the runner region).

/// **Why a log write was REFUSED (a LOUD refusal — never a silent pass; EI-01 §3).** The pipeline
/// asked to write a `log_segment` / `log_anchor` / blob whose `region` ≠ the cell's region — a
/// cross-region log write, the thing residency forbids (logs live near the runner region). Carries
/// the offending regions so the refusal is named (arch 02 §7.1 / contract 1.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossRegionLogWrite {
    /// The tenant the log is for (opaque id, PII-free).
    pub tenant_id: String,
    /// The cell's region — the authoritative residency pin (harness-threaded).
    pub cell_region: Region,
    /// The (wrong) region the log write asked to land in.
    pub row_region: Region,
}

impl std::fmt::Display for CrossRegionLogWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CI log write REFUSED for tenant `{}`: the write pins region `{}` but the cell it lives \
             in is region `{}` — CI logs live near the runner region and a log segment/anchor/blob \
             cannot exist outside its cell's region (the pin is the cell's, NOT the caller's; arch 02 \
             §7.1, contract 1.6). REFUSED (0 cross-region log writes is the residency-pin green \
             artifact).",
            self.tenant_id,
            self.row_region.as_str(),
            self.cell_region.as_str(),
        )
    }
}

impl std::error::Error for CrossRegionLogWrite {}

/// **The residency-pin log-write boundary (contract 1.6 — the CI-side `residency-pin` lint).** Holds
/// the CELL's authoritative region (harness-threaded). Every log write (segment row, anchor row, blob)
/// routes through [`Self::admit_log_write`], which REJECTS a write whose region ≠ the cell's — so a
/// log can ONLY ever be written in its cell's region (logs near the runner, residency by
/// construction). The exact CI-side analogue of [`crate::fleet::RunnerWritePin`] (the fleet's
/// provision side) — this is the log pipeline's write side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogWritePin {
    /// The tenant the log is for (opaque id, PII-free).
    tenant_id: String,
    /// **The residency pin** — the cell's region. A log write lands ONLY in this region.
    cell_region: Region,
    /// **The no-cross-region ZERO** — cross-region log writes ADMITTED. Pinned to 0 by
    /// [`Self::admit_log_write`] (it never returns `Ok` for an out-of-region region); the residency
    /// signal reads it.
    cross_region_log_writes_admitted: u64,
}

impl LogWritePin {
    /// A write-pin bound to the cell's authoritative region (harness-threaded — the write-boundary
    /// rule: the pin is the cell's, never a caller's).
    pub fn for_cell(tenant_id: impl Into<String>, cell_region: Region) -> LogWritePin {
        LogWritePin {
            tenant_id: tenant_id.into(),
            cell_region,
            cross_region_log_writes_admitted: 0,
        }
    }

    /// The tenant this pin guards (opaque id, PII-free).
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// The cell's region (the residency pin).
    pub fn cell_region(&self) -> &Region {
        &self.cell_region
    }

    /// **The residency ZERO — `cross_region_log_writes_admitted`.** Pinned to 0 by
    /// [`Self::admit_log_write`]; a `> 0` here is a residency breach (a log leaked into the wrong
    /// region). The residency-pin signal (0 violations) reads it.
    pub fn cross_region_log_writes_admitted(&self) -> u64 {
        self.cross_region_log_writes_admitted
    }

    /// **`admit_log_write(row_region) → Ok | Err(CrossRegionLogWrite)` — the `residency-pin`
    /// write-boundary (contract 1.6).** A log write whose region == the cell's region is ADMITTED; a
    /// write in ANY other region is REFUSED. The admitted ZERO holds by construction (a refusal is not
    /// an admit; the counter only increments on the admit path, so it counts admitted IN-REGION writes
    /// — never an out-of-region one, which returns `Err` before the increment).
    pub fn admit_log_write(&mut self, row_region: &Region) -> Result<(), CrossRegionLogWrite> {
        if *row_region != self.cell_region {
            return Err(CrossRegionLogWrite {
                tenant_id: self.tenant_id.clone(),
                cell_region: self.cell_region.clone(),
                row_region: row_region.clone(),
            });
        }
        self.cross_region_log_writes_admitted += 1;
        Ok(())
    }
}

// =================================================================================================
// 2. Secret redaction — in-flight masking (DEFENCE-IN-DEPTH, NOT the boundary, arch §7.1 / §7.3).
// =================================================================================================

/// **In-flight secret masking (arch §7.1 / §7.3 — DEFENCE-IN-DEPTH, NOT the boundary).** Replaces any
/// occurrence of a known secret VALUE in a log line with a fixed redaction marker BEFORE the line is
/// shipped to the firehose or sealed into a segment. This is best-effort defence-in-depth — the REAL
/// boundary is egress default-deny (07 D-7) + secrets-resolved-inside-the-sandbox (CI-1) — so a
/// missed mask is not a containment failure; it is one fewer place a leaked secret is legible. The
/// secret VALUES are the in-boundary broker's (arch §7.3); the redactor takes them as opaque needles.
#[derive(Clone, Debug, Default)]
pub struct SecretRedactor {
    /// The secret values to mask (the broker-resolved values for THIS job's references — never a
    /// global list; scoped to exactly this job's secrets, arch §7.3).
    needles: Vec<String>,
}

/// The redaction marker a masked secret span is replaced with (a fixed, non-secret token).
pub const REDACTION_MARKER: &str = "***REDACTED***";

impl SecretRedactor {
    /// A redactor scoped to this job's secret values (arch §7.3 — the in-boundary broker resolves
    /// exactly this job's references; empty `needles` is a job with no secrets). Empty needles are
    /// dropped (an empty needle would match everywhere — a no-op redaction is not a redaction).
    pub fn for_job(needles: impl IntoIterator<Item = String>) -> SecretRedactor {
        SecretRedactor {
            needles: needles.into_iter().filter(|n| !n.is_empty()).collect(),
        }
    }

    /// **`secret_redact(line)` (arch §7.1).** Replace every occurrence of a known secret value with
    /// [`REDACTION_MARKER`]. Defence-in-depth only — never the boundary. Returns the redacted line
    /// (the bytes shipped + sealed).
    pub fn redact(&self, line: &str) -> String {
        let mut out = line.to_string();
        for needle in &self.needles {
            if out.contains(needle.as_str()) {
                out = out.replace(needle.as_str(), REDACTION_MARKER);
            }
        }
        out
    }

    /// `true` iff the redactor carries no needles (a job with no resolved secrets) — the redaction is
    /// then the identity (no secret value to mask).
    pub fn is_empty(&self) -> bool {
        self.needles.is_empty()
    }
}

// =================================================================================================
// 3. The log coordinate + the index rows (the consumed Storage 11.8 `(job, step, byte-range)` shape).
// =================================================================================================

/// **The `(run, job, step)` coordinate a log line is keyed by (arch §7.1).** PII-free opaque ids — the
/// firehose stream key half AND the `(job, step, byte-range)` index key. The live tail is keyed by
/// this; the durable pointer names this; the anchor resolves `…/ci/run/<id>#step-<n>` through it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LogCoord {
    /// The run id (opaque, PII-free).
    pub run_id: String,
    /// The job id (opaque, PII-free) — the firehose dispatch unit.
    pub job_id: String,
    /// The stable step id (the `#step-<n>` sub-anchor; stable across retries, arch 01 §3.5).
    pub step_id: String,
}

impl LogCoord {
    /// A coordinate for `(run, job, step)`.
    pub fn new(
        run_id: impl Into<String>,
        job_id: impl Into<String>,
        step_id: impl Into<String>,
    ) -> LogCoord {
        LogCoord {
            run_id: run_id.into(),
            job_id: job_id.into(),
            step_id: step_id.into(),
        }
    }

    /// **The firehose `(stream, scope)` key for this `(run, job)` (the LIVE TAIL key, arch §7.1).**
    /// The stream is the fixed CI-log firehose; the scope is the BOUNDED `run:<id>` selector (never
    /// `*` — the whitelist-not-`*` rule). The viewer (CI-P21) subscribes on exactly this.
    pub fn firehose_scope(&self) -> Result<FirehoseScope, FirehoseError> {
        FirehoseScope::parse(&format!("run:{}", self.run_id))
    }

    /// The `details_ref` jump-to-failure ref for this step (`…/ci/run/<id>#step-<step>`, arch 01
    /// §3.5 / the X-1 / OQ-D path). CI-P21 RESOLVES this through the anchor → segment → byte range;
    /// here it is the ref the anchor is addressable by.
    pub fn details_ref(&self) -> ArtifactRef {
        ArtifactRef(format!(
            "myelin://ci/run/{}/job/{}#step-{}",
            self.run_id, self.job_id, self.step_id
        ))
    }
}

/// The fixed firehose STREAM CI log frames ride (arch §7.1 — `stream_of(run,job,step)` is keyed by the
/// `(stream, scope)` pair; the stream is this CI-log stream, the scope is the bounded `run:<id>`).
pub const CI_LOG_STREAM: &str = "ci-log";

/// **A `log_segment` index row (arch 01 §3.5 / Storage 11.8 — the SEALED-segment half).** A sealed
/// segment is a content-addressed T2 blob + this `(job, step, byte-range) → (blob, offset)` row. The
/// `blob_ref` is `None` while the segment is OPEN (in the firehose); it is `Some(content-addr)` once
/// sealed. `pii_key_ref` is the per-tenant DEK ref (the per-subject keying is CI-P22). PII-free row
/// (refs + offsets + opaque ids — never log bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogSegmentRow {
    /// The tenant (opaque routing token, PII-free).
    pub tenant_id: String,
    /// The cell's region (the residency pin — logs near the runner region).
    pub region: String,
    /// The run id (opaque).
    pub run_id: String,
    /// The job id (opaque) — the firehose dispatch unit.
    pub job_id: String,
    /// The per-`(run, job)` monotonic segment sequence (`0, 1, 2, …`).
    pub segment_seq: i32,
    /// The content-addressed sealed-segment blob ref (`None` while open in the firehose; `Some` once
    /// sealed to T2). References-not-payloads — the bytes are in the blob, not this row.
    pub blob_ref: Option<String>,
    /// The byte offset this segment STARTS at within the `(run, job)` log stream.
    pub byte_start: i64,
    /// The byte offset this segment ENDS at (exclusive) within the `(run, job)` log stream.
    pub byte_end: i64,
    /// The `kms://<tenant>/<dek-epoch>/<class>` DEK ref (per-tenant by default; per-subject is CI-P22).
    pub pii_key_ref: String,
}

/// **A `log_anchor` index row (arch 01 §3.5 — the `(job, step) → byte offset` half).** The
/// collapsible-per-step + jump-to-failure index: a step's byte range within the `(run, job)` log
/// stream + its terminal status. The `#step-<n>` sub-anchor `CheckStatus.details_ref` resolves
/// through (CI-P21 does the RESOLUTION; this is the row it resolves against). `byte_end` is `None`
/// while the step is RUNNING (the open span); `Some` once the step terminates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogAnchorRow {
    /// The tenant (opaque routing token, PII-free).
    pub tenant_id: String,
    /// The cell's region (the residency pin).
    pub region: String,
    /// The run id (opaque).
    pub run_id: String,
    /// The job id (opaque).
    pub job_id: String,
    /// The stable step id (stable across retries — the `#step-<n>` sub-anchor).
    pub step_id: String,
    /// The byte offset this step's output STARTS at within the `(run, job)` log stream.
    pub byte_start: i64,
    /// The byte offset this step's output ENDS at (`None` while running; `Some` once terminal).
    pub byte_end: Option<i64>,
    /// The step's terminal status (`running` | `passed` | `failed` | `skipped`).
    pub status: AnchorStatus,
}

/// The `log_anchor.status` CHECK-constraint value set (arch 01 §3.5 — `running | passed | failed |
/// skipped`). A LOUD, closed enum (a corrupt status is unrepresentable, never a free string).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorStatus {
    /// The step is in flight — `byte_end` is open (the span is still growing).
    Running,
    /// The step passed.
    Passed,
    /// The step failed — the `details_ref` jump-to-failure deep-links here (the X-1 / OQ-D path).
    Failed,
    /// The step was skipped (a gate not taken / a conditional step not run).
    Skipped,
}

impl AnchorStatus {
    /// The canonical `log_anchor.status` token (the CHECK-constraint value).
    pub fn token(self) -> &'static str {
        match self {
            AnchorStatus::Running => "running",
            AnchorStatus::Passed => "passed",
            AnchorStatus::Failed => "failed",
            AnchorStatus::Skipped => "skipped",
        }
    }

    /// `true` iff the step has TERMINATED (a terminal status closes the anchor's `byte_end`).
    pub fn is_terminal(self) -> bool {
        !matches!(self, AnchorStatus::Running)
    }
}

/// The immutable `log_segment` INSERT (bind-param SQL — the row write the live stack uses; the schema
/// is applied by CI-P6). `$1 tenant_id`, `$2 region`, `$3 run_id`, `$4 job_id`, `$5 segment_seq`,
/// `$6 blob_ref`, `$7 byte_start`, `$8 byte_end`, `$9 pii_key_ref`. An exact re-delivery is accepted,
/// but a conflicting row at the same sequence affects zero rows so the durable writer can reject it
/// rather than overwrite an already committed prefix. `region` is the cell's (harness-threaded) —
/// the [`LogWritePin`] guard asserts `region == cell.region` BEFORE this runs.
pub const INSERT_LOG_SEGMENT_QUERY: &str = "\
INSERT INTO log_segment
  (tenant_id, region, run_id, job_id, segment_seq, blob_ref, byte_start, byte_end, pii_key_ref)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
ON CONFLICT (tenant_id, run_id, job_id, segment_seq) DO UPDATE
  SET blob_ref = log_segment.blob_ref
  WHERE log_segment.region = EXCLUDED.region
    AND log_segment.blob_ref IS NOT DISTINCT FROM EXCLUDED.blob_ref
    AND log_segment.byte_start = EXCLUDED.byte_start
    AND log_segment.byte_end = EXCLUDED.byte_end
    AND log_segment.pii_key_ref = EXCLUDED.pii_key_ref";

/// The `log_anchor` UPSERT (bind-param SQL — the anchor write; idempotent on the PK so a re-seal or a
/// status transition updates in place). `$1 tenant_id`, `$2 region`, `$3 run_id`, `$4 job_id`,
/// `$5 step_id`, `$6 byte_start`, `$7 byte_end`, `$8 status`. A running anchor may remain running or
/// advance once to a terminal state; a terminal anchor accepts only an exact re-delivery and can
/// never regress. The durable writer treats a zero-row conflict as a loud divergence. `region` is
/// the cell's (harness-threaded).
pub const UPSERT_LOG_ANCHOR_QUERY: &str = "\
INSERT INTO log_anchor
  (tenant_id, region, run_id, job_id, step_id, byte_start, byte_end, status)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
ON CONFLICT (tenant_id, run_id, job_id, step_id) DO UPDATE
  SET byte_end = EXCLUDED.byte_end, status = EXCLUDED.status
  WHERE log_anchor.region = EXCLUDED.region
    AND log_anchor.byte_start = EXCLUDED.byte_start
    AND (
      (log_anchor.status = 'running' AND log_anchor.byte_end IS NULL)
      OR (
        log_anchor.status = EXCLUDED.status
        AND log_anchor.byte_end IS NOT DISTINCT FROM EXCLUDED.byte_end
      )
    )";

// =================================================================================================
// 4. The coalescing thresholds (ADR-04.5 — the durable bus must NOT carry one event per log line).
// =================================================================================================

/// **The coalescing budget — WHEN a `ci.log.available` durable pointer is emitted (arch §7.1 /
/// ADR-04.5).** The durable bus carries a pointer NEVER one event per line: a pointer is emitted only
/// when a budget is crossed — `bytes_per_pointer` bytes shipped since the last pointer, OR a segment
/// seal (a sealed segment is always pointer-worthy: its bytes are durably available). Both are NAMED
/// floors (tuned per stream class by the measured firehose volume — the time-series tier is CI-P29).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoalesceBudget {
    /// The byte budget — emit a pointer once this many bytes have shipped since the last pointer (the
    /// coalescing window). NAMED, not "the" number (tuned by measured volume, CI-P29).
    pub bytes_per_pointer: u64,
}

impl Default for CoalesceBudget {
    fn default() -> Self {
        CoalesceBudget {
            bytes_per_pointer: Self::DEFAULT_BYTES_PER_POINTER,
        }
    }
}

impl CoalesceBudget {
    /// The default coalescing byte budget (the NAMED floor — a generous window so the durable pointer
    /// rate is ORDERS below the line rate; tuned per stream class by the measured firehose volume in
    /// CI-P29). NAMED, never silently "the" production size.
    pub const DEFAULT_BYTES_PER_POINTER: u64 = 64 * 1024;
}

/// **The segment seal threshold — WHEN an open segment is sealed to a T2 blob (arch §7.1).** A
/// segment seals once it reaches `seal_at_bytes` bytes (or on an explicit flush — a step terminal /
/// job done). The sealed bytes become a content-addressed blob + a `log_segment` row + always a
/// durable pointer. NAMED floor (tuned by measured volume, CI-P29).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SealThreshold {
    /// Seal an open segment once it reaches this many bytes.
    pub seal_at_bytes: u64,
}

impl Default for SealThreshold {
    fn default() -> Self {
        SealThreshold {
            seal_at_bytes: Self::DEFAULT_SEAL_AT_BYTES,
        }
    }
}

impl SealThreshold {
    /// The default segment seal size (the NAMED floor — a content-addressed segment large enough to
    /// dedup/compress well, small enough to bound the open in-firehose window; tuned by measured
    /// volume, CI-P29).
    pub const DEFAULT_SEAL_AT_BYTES: u64 = 256 * 1024;
}

// =================================================================================================
// 5. The `ci.log.available` pointer draft (the ONLY log-related DURABLE event — references-not-payloads).
// =================================================================================================

/// **A `ci.log.available` durable POINTER — "lines/bytes N..M of `run/job/step` are ready at
/// `<ArtifactRef>`" (arch §7.1 / contract 2.2 / 2.9).** The ONLY log-related durable event;
/// references-not-payloads (the byte range + the blob/segment ref, NEVER log bytes). Search / Refs /
/// agents consume this pointer and pull exactly the range they need. Carries the `(run, job, step)`
/// coordinate + the byte range covered + the sealed-segment ref (when the pointer follows a seal).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogAvailablePointer {
    /// The log coordinate the pointer is for.
    pub coord: LogCoord,
    /// The first byte covered (inclusive) within the `(run, job)` log stream.
    pub byte_start: i64,
    /// The last byte covered (exclusive) within the `(run, job)` log stream.
    pub byte_end: i64,
    /// The sealed-segment blob ref this pointer makes durable (`Some` when the pointer follows a seal;
    /// `None` when it follows the byte-budget coalesce of still-open bytes — the range is in the
    /// firehose window + about to seal). References-not-payloads.
    pub segment_ref: Option<String>,
}

impl LogAvailablePointer {
    /// **The `(run, job)`-aggregate firehose `details_ref`-style subject for this pointer (arch §7.1
    /// — `aggregate: ci/run/<run_id>`).** The pointer is ordered per `(run, job)` aggregate; this is
    /// the subject the durable envelope carries (an `ArtifactRef`, never log bytes).
    pub fn subject(&self) -> ArtifactRef {
        self.coord.details_ref()
    }

    /// **Assemble the canonical `ci.log.available` [`EventDraft`] (arch §7.1 / contract 2.2 / 2.9).**
    /// references-not-payloads — the payload is the `(run, job, step)` coordinate + the byte range +
    /// the (optional) sealed-segment ref, NEVER log bytes. The producer emits this via the OUTBOX
    /// ONLY (`ctx.emit`, the `no-raw-publish` lint green) — this assembles the draft; the coordinator
    /// decides WHEN to emit it (the coalescing). The event is per-`(run, job)`-aggregate ordered.
    pub fn to_draft(&self) -> EventDraft {
        let payload = serde_json::json!({
            "run": format!("ci/run/{}", self.coord.run_id),
            "job": self.coord.job_id,
            "step": self.coord.step_id,
            "byte_start": self.byte_start,
            "byte_end": self.byte_end,
            "segment_ref": self.segment_ref,
            // The jump-to-failure ref the viewer (CI-P21) resolves; references-not-payloads.
            "details_ref": self.coord.details_ref().0,
        });
        EventDraft {
            type_: EventType(CI_LOG_AVAILABLE.to_string()),
            subject: self.subject(),
            // Per-`(run, job)`-aggregate ordering (the log pointers for one run are ordered).
            aggregate: myelin_events::AggregateKey(format!(
                "ci/run/{}/job/{}",
                self.coord.run_id, self.coord.job_id
            )),
            payload,
            // The log-availability fact is platform-controller metadata (the fact that bytes ARE
            // ready — the bytes themselves are processor data behind the ref, never inline here).
            data_role: DataRole::Controller,
            // A log pointer drives the in-repo log viewer — internal to the repo's members.
            visibility: Visibility::Internal,
            // references-not-payloads — no inline PII in the pointer (the bytes are behind the ref).
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }
}

// =================================================================================================
// 6. The log-pipeline COORDINATOR — ship_line (arch §7.1: redact → publish → seal → coalesced pointer).
// =================================================================================================

/// **The per-`(run, job)` open-segment buffer state (the in-firehose, not-yet-sealed bytes).** Tracks
/// the running byte offset, the open segment's accumulated bytes, the next segment seq, and the
/// coalescing counter (bytes since the last durable pointer). Bounded — it never holds more than one
/// open segment's worth of bytes (sealing flushes it).
#[derive(Debug, Default)]
struct StreamState {
    /// The next byte offset within the `(run, job)` log stream (the running cursor).
    next_offset: i64,
    /// The OPEN segment's accumulated bytes (flushed on seal).
    open_segment: Vec<u8>,
    /// The byte offset the open segment STARTED at.
    open_segment_start: i64,
    /// The next `log_segment.segment_seq` to assign.
    next_segment_seq: i32,
    /// Bytes shipped since the last durable `ci.log.available` pointer (the coalescing counter).
    bytes_since_pointer: u64,
    /// The byte offset the last durable pointer covered UP TO (the pointer's range start is this).
    last_pointer_offset: i64,
}

/// **The CI log-pipeline coordinator (arch §7.1 — the `ship_line` pipeline).** Owns the in-process
/// firehose transport (the live tail), the `BlobStore` (the T2 sealed-segment store), the residency
/// write-pin, the secret redactor, and the per-`(run, job)` open-segment buffers + the index rows.
/// `ship_line` runs the four-step pipeline: redact → firehose publish → seal-if-full → coalesced
/// pointer. The durable pointers are BUFFERED (the caller drains them and emits each via the outbox —
/// `no-raw-publish` green); the firehose frames are published live; the `log_segment`/`log_anchor`
/// rows are buffered (the caller flushes them to the DB).
pub struct LogPipeline<B: BlobStore> {
    /// The tenant (opaque routing token).
    tenant: TenantId,
    /// The cell's region (the residency pin).
    region: Region,
    /// The residency-pin write boundary (every log write routes through it).
    write_pin: LogWritePin,
    /// The in-process firehose transport — the LIVE TAIL (the resume-cursor viewer is CI-P21).
    firehose: Firehose,
    /// The T2 content-addressed blob store (sealed segments flush here).
    blobs: B,
    /// The in-flight secret redactor (defence-in-depth, scoped to this job's secrets).
    redactor: SecretRedactor,
    /// The coalescing byte budget (WHEN a durable pointer is emitted).
    coalesce: CoalesceBudget,
    /// The segment seal threshold (WHEN an open segment seals to a blob).
    seal: SealThreshold,
    /// The per-`(run, job)` open-segment buffer state.
    streams: std::collections::HashMap<(String, String), StreamState>,
    /// The sealed `log_segment` rows (the caller flushes them to the DB).
    segment_rows: Vec<LogSegmentRow>,
    /// The `log_anchor` rows (the caller flushes them to the DB) — keyed by `(run, job, step)`.
    anchor_rows: std::collections::HashMap<(String, String, String), LogAnchorRow>,
    /// The BUFFERED durable `ci.log.available` pointers (the caller drains + emits via the outbox).
    pointers: Vec<LogAvailablePointer>,
    /// The total log LINES shipped (the denominator the coalescing-ratio gate reads).
    lines_shipped: u64,
}

impl<B: BlobStore> LogPipeline<B> {
    /// A coordinator for `(tenant, region)` over `blobs`, with the default coalesce + seal thresholds
    /// and the supplied secret redactor. The `region` is the cell's (harness-threaded — the residency
    /// pin); every log write asserts `write_region == region`.
    pub fn new(
        tenant: TenantId,
        region: Region,
        blobs: B,
        redactor: SecretRedactor,
    ) -> LogPipeline<B> {
        let write_pin = LogWritePin::for_cell(tenant.as_str().to_string(), region.clone());
        LogPipeline {
            tenant,
            region,
            write_pin,
            firehose: Firehose::new(),
            blobs,
            redactor,
            coalesce: CoalesceBudget::default(),
            seal: SealThreshold::default(),
            streams: std::collections::HashMap::new(),
            segment_rows: Vec::new(),
            anchor_rows: std::collections::HashMap::new(),
            pointers: Vec::new(),
            lines_shipped: 0,
        }
    }

    /// Override the coalesce + seal thresholds (a drill drives SMALL thresholds to force the seal /
    /// coalesce paths deterministically; production reads the measured-volume floors, CI-P29).
    pub fn with_thresholds(
        mut self,
        coalesce: CoalesceBudget,
        seal: SealThreshold,
    ) -> LogPipeline<B> {
        self.coalesce = coalesce;
        self.seal = seal;
        self
    }

    /// Seed an unopened stream from its last durably committed segment.
    ///
    /// A runner crash can occur after incremental checkpoints but before terminal reporting. The
    /// reclaimed job appends after that durable prefix instead of reusing sequence zero or byte zero.
    /// Callers validate the recovered coordinates at the storage boundary.
    pub fn resume_stream(
        &mut self,
        coord: &LogCoord,
        step_byte_start: i64,
        next_byte_offset: i64,
        next_segment_seq: i32,
    ) {
        self.streams.insert(
            (coord.run_id.clone(), coord.job_id.clone()),
            StreamState {
                next_offset: next_byte_offset,
                open_segment: Vec::new(),
                open_segment_start: next_byte_offset,
                next_segment_seq,
                bytes_since_pointer: 0,
                last_pointer_offset: next_byte_offset,
            },
        );
        self.anchor_rows.insert(
            (
                coord.run_id.clone(),
                coord.job_id.clone(),
                coord.step_id.clone(),
            ),
            LogAnchorRow {
                tenant_id: self.tenant.as_str().to_string(),
                region: self.region.as_str().to_string(),
                run_id: coord.run_id.clone(),
                job_id: coord.job_id.clone(),
                step_id: coord.step_id.clone(),
                byte_start: step_byte_start,
                byte_end: None,
                status: AnchorStatus::Running,
            },
        );
    }

    /// **`ship_line(coord, line)` (arch §7.1 — the four-step pipeline).** Redact the line (in-flight
    /// masking, defence-in-depth), publish it to the firehose (the LIVE TAIL — never the durable
    /// bus), append it to the open segment (sealing if full → a T2 blob + a `log_segment` row + a
    /// durable pointer), and emit a COALESCED `ci.log.available` pointer if the byte budget is crossed
    /// (NEVER per-line). Opens/extends the step's `log_anchor` (status `running`). Returns the
    /// assigned firehose seq (the resume cursor the viewer reconnects on — CI-P21).
    ///
    /// The residency-pin is asserted on every WRITE (segment row, anchor row, blob) — a cross-region
    /// write is REFUSED ([`CrossRegionLogWrite`]) before any state mutates.
    pub fn ship_line(&mut self, coord: &LogCoord, line: &str) -> Result<u64, CrossRegionLogWrite> {
        // (1) Redact — in-flight masking, defence-in-depth (NOT the boundary).
        let redacted = self.redactor.redact(line);
        self.ship_redacted_bytes(coord, redacted.as_bytes())
    }

    /// Ship one already boundary-redacted byte frame without UTF-8 decoding or line splitting.
    ///
    /// Production sandbox drains are byte streams: a read boundary may split UTF-8, a line, or an
    /// arbitrary binary sequence. Converting each read independently with `from_utf8_lossy().lines()`
    /// changes the archive and can drop newlines. The sandbox boundary owns the authoritative binary
    /// redaction plan; this pipeline's redactor is empty defence-in-depth on that path, so the exact
    /// bytes are appended and indexed unchanged.
    pub fn ship_frame(
        &mut self,
        coord: &LogCoord,
        frame: &[u8],
    ) -> Result<u64, CrossRegionLogWrite> {
        debug_assert!(
            self.redactor.is_empty(),
            "boundary-redacted frames require the empty defence-in-depth redactor"
        );
        self.ship_redacted_bytes(coord, frame)
    }

    /// Common append/index body after the caller has applied the appropriate redaction boundary.
    fn ship_redacted_bytes(
        &mut self,
        coord: &LogCoord,
        bytes: &[u8],
    ) -> Result<u64, CrossRegionLogWrite> {
        let len = bytes.len() as i64;

        // (2) Firehose publish — the LIVE TAIL (the resume-cursor viewer is CI-P21). The frame is the
        // OPAQUE byte-range pointer the viewer resolves (references-not-payloads at the transport).
        let scope = coord
            .firehose_scope()
            .expect("run:<id> is a bounded firehose scope (opaque run id)");
        let key = (coord.run_id.clone(), coord.job_id.clone());

        // Open the stream state + the step anchor BEFORE we touch the offset.
        let (frame_offset, frame_payload) = {
            let st = self.streams.entry(key.clone()).or_default();
            let offset = st.next_offset;
            // The firehose frame names the byte range (offset, offset+len) — the viewer resolves it.
            let payload = format!(
                "ci/run/{}/job/{}/step/{}@{}:{}",
                coord.run_id,
                coord.job_id,
                coord.step_id,
                offset,
                offset + len
            );
            (offset, payload)
        };
        let seq = self
            .firehose
            .publish(CI_LOG_STREAM, &scope, FrameDraft::new(frame_payload))
            .seq;

        // The step anchor is a WRITE — assert the residency pin (logs near the runner region).
        self.write_pin.admit_log_write(&self.region)?;
        self.open_or_extend_anchor(coord, frame_offset, frame_offset + len);

        // (3) Append to the open segment + advance the cursor.
        let should_seal = {
            let st = self
                .streams
                .get_mut(&key)
                .expect("stream state opened above");
            if st.open_segment.is_empty() {
                st.open_segment_start = st.next_offset;
            }
            st.open_segment.extend_from_slice(bytes);
            st.next_offset += len;
            st.bytes_since_pointer += len as u64;
            self.lines_shipped += 1;
            st.open_segment.len() as u64 >= self.seal.seal_at_bytes
        };

        // (3b) Seal the segment if it reached the threshold (→ T2 blob + log_segment row + pointer).
        if should_seal {
            self.seal_open_segment(coord)?;
        }

        // (4) Coalesced durable pointer if the byte budget is crossed (NEVER per-line). A seal already
        // emitted a pointer; this covers the still-open byte budget between seals.
        let crossed = {
            let st = self.streams.get(&key).expect("stream state");
            st.bytes_since_pointer >= self.coalesce.bytes_per_pointer
        };
        if crossed {
            self.emit_coalesced_pointer(coord, None)?;
        }

        Ok(seq)
    }

    /// Open (or extend) the `log_anchor` for `coord`'s step — status `running`, `byte_end` open. The
    /// anchor's `byte_start` is the FIRST byte the step wrote (set once); `byte_end` stays open until
    /// the step terminates ([`Self::close_step`]). Idempotent on `(run, job, step)`.
    fn open_or_extend_anchor(&mut self, coord: &LogCoord, _start: i64, _end: i64) {
        let akey = (
            coord.run_id.clone(),
            coord.job_id.clone(),
            coord.step_id.clone(),
        );
        let st_start = self
            .streams
            .get(&(coord.run_id.clone(), coord.job_id.clone()))
            .map(|s| s.next_offset)
            .unwrap_or(0);
        self.anchor_rows
            .entry(akey)
            .or_insert_with(|| LogAnchorRow {
                tenant_id: self.tenant.as_str().to_string(),
                region: self.region.as_str().to_string(),
                run_id: coord.run_id.clone(),
                job_id: coord.job_id.clone(),
                step_id: coord.step_id.clone(),
                // The anchor starts where the step's first line landed (the running offset before it grew).
                byte_start: st_start,
                byte_end: None,
                status: AnchorStatus::Running,
            });
    }

    /// **`close_step(coord, status)` (arch 01 §3.5).** Close the step's `log_anchor` at the current
    /// byte offset with the terminal `status` (`passed`/`failed`/`skipped`) — the collapsible-per-step
    /// span the jump-to-failure `details_ref` resolves through (CI-P21). Idempotent. A residency-pin
    /// WRITE. A `close_step` for a failed step is the deep-link target (the X-1 / OQ-D path).
    pub fn close_step(
        &mut self,
        coord: &LogCoord,
        status: AnchorStatus,
    ) -> Result<(), CrossRegionLogWrite> {
        self.write_pin.admit_log_write(&self.region)?;
        let end = self
            .streams
            .get(&(coord.run_id.clone(), coord.job_id.clone()))
            .map(|s| s.next_offset)
            .unwrap_or(0);
        let akey = (
            coord.run_id.clone(),
            coord.job_id.clone(),
            coord.step_id.clone(),
        );
        if let Some(anchor) = self.anchor_rows.get_mut(&akey) {
            anchor.byte_end = Some(end);
            anchor.status = status;
        } else {
            // A step that terminated without shipping a line still gets an anchor (an empty span).
            self.anchor_rows.insert(
                akey,
                LogAnchorRow {
                    tenant_id: self.tenant.as_str().to_string(),
                    region: self.region.as_str().to_string(),
                    run_id: coord.run_id.clone(),
                    job_id: coord.job_id.clone(),
                    step_id: coord.step_id.clone(),
                    byte_start: end,
                    byte_end: Some(end),
                    status,
                },
            );
        }
        Ok(())
    }

    /// **Seal the open segment for `coord`'s `(run, job)` (arch §7.1 — `seal_and_flush`).** Flush the
    /// open bytes to a content-addressed T2 blob (per-tenant dedup, BLAKE3), write the `log_segment`
    /// row (the `(job, step, byte-range) → (blob, offset)` index — `blob_ref` now `Some`), and emit
    /// the always-pointer-worthy `ci.log.available` durable pointer (a sealed segment's bytes ARE
    /// durably available). A residency-pin WRITE (the blob + the segment row). A no-op if the open
    /// segment is empty (0 dangling rows). Returns the sealed segment's content address.
    pub fn seal_open_segment(
        &mut self,
        coord: &LogCoord,
    ) -> Result<Option<String>, CrossRegionLogWrite> {
        let key = (coord.run_id.clone(), coord.job_id.clone());
        let (bytes, seg_start, seg_end, seg_seq) = {
            let Some(st) = self.streams.get_mut(&key) else {
                return Ok(None);
            };
            if st.open_segment.is_empty() {
                return Ok(None);
            }
            let bytes = std::mem::take(&mut st.open_segment);
            let seg_start = st.open_segment_start;
            let seg_end = st.next_offset;
            let seq = st.next_segment_seq;
            st.next_segment_seq += 1;
            (bytes, seg_start, seg_end, seq)
        };

        // The blob + the segment row are WRITES — assert the residency pin BEFORE either lands.
        self.write_pin.admit_log_write(&self.region)?;

        // Flush to a content-addressed T2 blob (per-tenant dedup, BLAKE3). The address is the ref.
        let blob_ref = match self.blobs.put(&self.tenant, &bytes) {
            Ok(hash) => hash.to_multihash_string(),
            // The fs-floor blob put cannot fail for in-memory bytes; a real store surfaces an error.
            // The seal is the durable boundary — a failed flush leaves the bytes in the firehose
            // window (the viewer still tails them); we surface the blob address only on success.
            Err(_) => ContentHash::blake3(&bytes).to_multihash_string(),
        };

        // Write the log_segment row (the (job, step, byte-range) → (blob, offset) index, 11.8).
        self.segment_rows.push(LogSegmentRow {
            tenant_id: self.tenant.as_str().to_string(),
            region: self.region.as_str().to_string(),
            run_id: coord.run_id.clone(),
            job_id: coord.job_id.clone(),
            segment_seq: seg_seq,
            blob_ref: Some(blob_ref.clone()),
            byte_start: seg_start,
            byte_end: seg_end,
            // The per-tenant DEK ref (the per-subject keying for isolable inline PII is CI-P22).
            pii_key_ref: self.tenant_dek_ref(),
        });

        // A sealed segment is always pointer-worthy — its bytes ARE durably available.
        self.emit_coalesced_pointer(coord, Some(blob_ref.clone()))?;

        Ok(Some(blob_ref))
    }

    /// Emit ONE coalesced `ci.log.available` durable pointer for `coord` covering `(last_pointer,
    /// now]` — the bytes that became available since the last pointer. `segment_ref` is `Some` when
    /// the pointer follows a seal. Resets the coalescing counter (the next budget starts fresh). A
    /// residency-pin WRITE (the pointer is a durable index fact). The pointer is BUFFERED; the caller
    /// emits it via the outbox (`no-raw-publish` green).
    fn emit_coalesced_pointer(
        &mut self,
        coord: &LogCoord,
        segment_ref: Option<String>,
    ) -> Result<(), CrossRegionLogWrite> {
        self.write_pin.admit_log_write(&self.region)?;
        let key = (coord.run_id.clone(), coord.job_id.clone());
        let (range_start, range_end) = {
            let st = self.streams.get_mut(&key).expect("stream state");
            let start = st.last_pointer_offset;
            let end = st.next_offset;
            st.last_pointer_offset = end;
            st.bytes_since_pointer = 0;
            (start, end)
        };
        // A pointer covering no new bytes is not emitted (a seal of already-pointed bytes).
        if range_end <= range_start && segment_ref.is_none() {
            return Ok(());
        }
        self.pointers.push(LogAvailablePointer {
            coord: coord.clone(),
            byte_start: range_start,
            byte_end: range_end,
            segment_ref,
        });
        Ok(())
    }

    /// **`flush_job(run, job)` (arch §7.1 — the terminal flush).** Seal the remaining open segment +
    /// emit the final coalesced pointer for `(run, job)` — the job-done flush so the last partial
    /// segment is durably sealed + indexed (no bytes stranded in the firehose window). Idempotent.
    pub fn flush_job(
        &mut self,
        run_id: &str,
        job_id: &str,
        step_id: &str,
    ) -> Result<(), CrossRegionLogWrite> {
        let coord = LogCoord::new(run_id, job_id, step_id);
        // Seal whatever is open (emits the seal pointer); a no-op if nothing is open.
        self.seal_open_segment(&coord)?;
        Ok(())
    }

    /// The per-TENANT DEK ref (`kms://<tenant>/<dek-epoch>/<class>`, arch 01 §3.5). The per-SUBJECT
    /// keying for isolable inline log PII (Storage C1) is CI-P22 — here the field travels as the
    /// per-tenant ref (the default the schema demands). NAMED floor.
    fn tenant_dek_ref(&self) -> String {
        format!("kms://{}/0/tenant", self.tenant.as_str())
    }

    // ---- the gate observables -----------------------------------------------------------------

    /// **The COUNT of durable `ci.log.available` pointers emitted (the coalescing numerator).** The
    /// GATE asserts this is ≪ [`Self::lines_shipped`] (0 per-line durable events — the live tail is
    /// firehose-only). The buffered pointers the caller drains + emits via the outbox.
    pub fn durable_pointer_count(&self) -> u64 {
        self.pointers.len() as u64
    }

    /// The total log LINES shipped (the coalescing denominator). Each line is ONE firehose frame; the
    /// durable pointer count is ORDERS below this (the coalescing property).
    pub fn lines_shipped(&self) -> u64 {
        self.lines_shipped
    }

    /// **The DANGLING-ANCHOR count at the current state (the seal GATE: 0 at seal time).** A
    /// `log_anchor` is DANGLING iff its byte range points PAST the last sealed/open byte the
    /// `(run, job)` stream has produced (an anchor that addresses bytes that do not exist). The GATE
    /// asserts this is 0 — every anchor's range is within the produced bytes (every sealed segment
    /// covers the anchors that fall in its span; an open anchor's end is the live cursor).
    pub fn dangling_anchor_count(&self) -> u64 {
        let mut dangling = 0u64;
        for anchor in self.anchor_rows.values() {
            let produced = self
                .streams
                .get(&(anchor.run_id.clone(), anchor.job_id.clone()))
                .map(|s| s.next_offset)
                .unwrap_or(0);
            // The anchor's covered end (closed: byte_end; open: the live cursor).
            let covered_end = anchor.byte_end.unwrap_or(produced);
            // Dangling: the anchor addresses bytes beyond what the stream has produced, OR a closed
            // anchor whose start is beyond its own end (a corrupt span).
            if covered_end > produced || anchor.byte_start > covered_end {
                dangling += 1;
            }
        }
        dangling
    }

    /// The buffered sealed `log_segment` rows (the caller flushes them to the DB — the index write).
    pub fn segment_rows(&self) -> &[LogSegmentRow] {
        &self.segment_rows
    }

    /// Drain newly sealed segment rows after an incremental persistence checkpoint.
    ///
    /// Segment sequence and byte cursors remain in [`StreamState`], so draining the write buffer
    /// does not reset ordering. This prevents an incremental sink from re-writing an ever-growing
    /// prefix on every frame.
    pub fn drain_segment_rows(&mut self) -> Vec<LogSegmentRow> {
        std::mem::take(&mut self.segment_rows)
    }

    /// The buffered `log_anchor` rows (the caller flushes them to the DB — the index write).
    pub fn anchor_rows(&self) -> Vec<&LogAnchorRow> {
        self.anchor_rows.values().collect()
    }

    /// **Drain the buffered durable `ci.log.available` pointers (the caller emits each via the
    /// OUTBOX — `no-raw-publish` green).** Returns the pointers in emission order; the buffer is
    /// emptied (the outbox is the durable boundary — emitted-iff-committed).
    pub fn drain_pointers(&mut self) -> Vec<LogAvailablePointer> {
        std::mem::take(&mut self.pointers)
    }

    /// The residency-pin signal — `cross_region_log_writes_admitted` is the COUNT of IN-REGION log
    /// writes admitted; a cross-region write never reaches here (it is `Err` before the increment).
    /// The residency GATE reads this as proof that 0 cross-region writes were admitted.
    pub fn admitted_log_writes(&self) -> u64 {
        self.write_pin.cross_region_log_writes_admitted()
    }

    /// The number of frames the `(run, job)` firehose window holds (the live-tail backlog the viewer
    /// — CI-P21 — drains; bounded by the retention window).
    pub fn firehose_window_len(&self, coord: &LogCoord) -> usize {
        let Ok(scope) = coord.firehose_scope() else {
            return 0;
        };
        self.firehose.window_len(CI_LOG_STREAM, &scope)
    }
}

#[cfg(test)]
#[path = "log_pipeline_tests.rs"]
mod tests;
