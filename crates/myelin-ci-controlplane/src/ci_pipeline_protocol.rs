//! CT-007 slice 5b.3-6e.2: the production pipeline PROTOCOL DESCRIPTOR.
//!
//! The FOUR coupled production choices the activating slice binds — the operational reservation
//! WRITER version, the durable queue reservation MARKER a V2 dispatch persists, the per-job
//! CREDENTIAL writer version, and the ACCOUNTING writer version. Sol's 6e design §2: gathering them in
//! ONE named source keeps the coupled activation reviewable in a single place, and every production
//! composition root reads THESE constants so the four choices cannot silently diverge across the
//! starter, the runner, and the reporter/accounting roots.
//!
//! **Stage A (dormant): defined, selected by NOTHING.** No production root reads these yet, and this
//! file is NOT part of [`ci_manifest_pipeline_definition`](crate::ci_runtime_composition::ci_manifest_pipeline_definition)'s
//! hashed source set — adding it there (and pointing every production root at these constants) is the
//! ATOMIC Stage B activation, which also bumps `CI_MANIFEST_PIPELINE_VERSION` so the recorded manifest
//! digest binds the reservation/credential/accounting protocol without hashing all of `lib.rs`.
//!
//! **The queue marker is DERIVED, never hardcoded beside an unchecked handle** (Sol's 6e.2 ruling 2):
//! the enqueue path proves a reserve handle is genuinely V2-shaped and only THEN persists
//! [`PRODUCTION_RESERVATION_QUEUE_MARKER`]. This constant records the production CHOICE; it is not a
//! license to stamp `2` next to an unvalidated handle.

use crate::ci_credential_generation::CiJobCredentialWriteVersion;
use crate::ci_launch_authority::OperationalReservationWriteVersion;
use crate::job_accounting_store::CiJobAccountingWriteVersion;

/// The operational reservation WRITER version production selects — raw-dimension V2 amounts (Stage B
/// flips [`ci_run_starter_factory`](crate::ci_run_starter_factory) from `V1` to this).
pub const PRODUCTION_RESERVATION_WRITE_VERSION: OperationalReservationWriteVersion =
    OperationalReservationWriteVersion::V2;

/// The durable `job_queue.reservation_write_version` MARKER a validated V2 dispatch persists. The
/// enqueue path DERIVES this from a successfully-validated V2 reserve handle (never hardcodes it
/// beside an unchecked handle); a legacy/V1 dispatch leaves the column `NULL`. The `ci_0022*`
/// migrations' `CHECK (reservation_write_version = 2)` + the activation-readiness probe both assume
/// exactly this value.
pub const PRODUCTION_RESERVATION_QUEUE_MARKER: i16 = 2;

/// The per-job CREDENTIAL writer version production selects — the phase-bound V2 minter that issues a
/// distinct generation per checkout phase (Stage B: `V2CheckoutComposition` already pins this; the
/// descriptor records it so the runner root cannot drift onto `V1ClaimBound`).
pub const PRODUCTION_CREDENTIAL_WRITE_VERSION: CiJobCredentialWriteVersion =
    CiJobCredentialWriteVersion::V2PhaseBound;

/// The ACCOUNTING writer version production selects — the V4 receipt shape (Stage B converts all four
/// production accounting-store constructions from `with_pg` (V3) to explicit V4).
pub const PRODUCTION_ACCOUNTING_WRITE_VERSION: CiJobAccountingWriteVersion =
    CiJobAccountingWriteVersion::V4;
