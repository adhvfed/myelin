//! Contract 11.8 CDC pair — the T3 CI log tier `(job, step, byte-range)` index (C2, P-ST-26 /
//! global P-328), with **CI as the consumer** (5.9 — the `CheckStatus.details_ref` `#step-<n>`).
//!
//! The prompt requires "a provider+consumer pair for 11.8 (CI as the consumer)". This is the
//! consumer-driven contract test:
//!
//! - The **PROVIDER** is `myelin-storage`'s [`CiLogTier`] (the C2 index this prompt ships — the
//!   CI-keyed instance of the P-ST-20 [`FirehoseArchiver`] + the `(job, step, byte-range)` index).
//! - The **CONSUMER** is CI: it streams a run's redacted step logs as CI log frames, seals them into
//!   content-addressed T2 segments, mints a `CheckStatus` whose `details_ref` carries the X-1
//!   `myelin://<tenant>/ci/run/<id>#step-<n>` jump-to-failure sub-anchor, and later RESOLVES that
//!   sub-anchor back to the exact failing step's bytes — the GIT-D10 / CI-D8 jump-to-failure path.
//!
//! The test pins the frozen 11.8 C2 call shape CI relies on: `seal_ci_batch` returns a
//! content-addressed [`SealedSegment`] AND builds the `(job, step, byte-range)` index;
//! `resolve_step_anchor` resolves the `details_ref` `#step-<n>` to the EXACT step bytes. If that shape
//! drifts, this stops compiling/passing.

use myelin_storage::{CiLogError, CiLogFrame, CiLogTier, KekId, KmsEngine};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

/// The CI-owned `CheckStatus` (the consumer's view) — references-not-payloads: it carries the
/// `details_ref` jump-to-failure sub-anchor, NEVER the inline log bytes. This is the 5.9 shape CI
/// stamps onto a `ci.check.updated` fact; here the test models only the `details_ref` field the C2
/// index resolves (the Bus carries the whole `CheckStatus` opaquely — proven in
/// `myelin-events/tests/cdc_5_9_check_seam_carriage.rs`).
struct CheckStatus {
    /// `failure` for a failed check; the `details_ref` jumps to the failing step.
    conclusion: &'static str,
    /// The X-1 `myelin://<tenant>/ci/run/<id>#step-<n>` sub-anchor — references-not-payloads.
    details_ref: String,
}

/// The CONSUMER: CI streaming a run's step logs through the C2 provider, then resolving the
/// `details_ref` `#step-<n>` jump-to-failure.
struct CiRunLogs {
    tier: CiLogTier,
    run_id: String,
    next_seq: u64,
}

impl CiRunLogs {
    fn boot(tenant: TenantId, region: Region, engine: Arc<KmsEngine>, run_id: &str) -> CiRunLogs {
        CiRunLogs {
            tier: CiLogTier::with_tenant_dek(run_id, tenant, region, engine),
            run_id: run_id.to_string(),
            next_seq: 0,
        }
    }

    /// Stream one step's log chunk as a sealed CI log batch (one segment). Returns nothing — the
    /// runner only carries the references-not-payloads pointer.
    fn stream_step(&mut self, step_no: u32, log: &str) {
        self.next_seq += 1;
        let frame = CiLogFrame::new(&self.run_id, step_no, log.as_bytes().to_vec());
        self.tier
            .seal_ci_batch(&[(self.next_seq, frame)])
            .expect("CI seals its step log into a content-addressed T2 segment");
    }

    /// Mint the `CheckStatus` for a failed run — `details_ref` is the `#step-<n>` jump-to-failure.
    fn mint_check_status(&self, failing_step: u32) -> CheckStatus {
        CheckStatus {
            conclusion: "failure",
            details_ref: format!("myelin://acme/ci/run/{}#step-{failing_step}", self.run_id),
        }
    }

    /// Resolve the `CheckStatus.details_ref` `#step-<n>` back to the EXACT failing step's bytes (the
    /// jump-to-failure) — the provider call CI depends on.
    fn jump_to_failure(&self, check: &CheckStatus) -> Result<Vec<u8>, CiLogError> {
        self.tier.resolve_step_anchor(&check.details_ref)
    }
}

/// THE CDC pair: CI streams a run's step logs through the C2 provider, mints a `CheckStatus` with a
/// `#step-<n>` `details_ref`, and resolves it back to the exact failing step's bytes — the provider
/// (`myelin-storage`'s `CiLogTier`) honours the frozen 11.8 C2 `(job, step, byte-range)` shape.
#[test]
fn cdc_11_8_ci_resolves_details_ref_step_anchor_to_exact_failing_bytes() {
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());
    let engine = Arc::new(KmsEngine::new());
    engine.ensure_kek(&KekId::new(tenant.clone(), region.clone()));

    let mut ci = CiRunLogs::boot(tenant, region, engine, "run-42");

    // CI streams a run's step logs (each its own sealed, content-addressed, DEK-encrypted segment).
    ci.stream_step(1, "==> checkout\nFetching repo... ok\n");
    ci.stream_step(2, "==> build\ncargo build... ok\n");
    ci.stream_step(
        3,
        "==> test\ntest auth::login ... FAILED\nassertion failed at line 88\n",
    );

    // The check failed at step 3 — CI mints a CheckStatus whose details_ref jumps to it.
    let check = ci.mint_check_status(3);
    assert_eq!(check.conclusion, "failure");
    // references-not-payloads: the details_ref is a sub-anchor, NOT the log bytes.
    assert!(check.details_ref.ends_with("#step-3"));
    assert!(!check.details_ref.contains("FAILED"));

    // The consumer (CI / a viewer / an agent) follows the details_ref to the EXACT failing bytes.
    let bytes = ci
        .jump_to_failure(&check)
        .expect("resolve the #step-3 jump-to-failure");
    assert_eq!(
        bytes, b"==> test\ntest auth::login ... FAILED\nassertion failed at line 88\n",
        "11.8/C2: the details_ref #step-<n> resolves to step 3's EXACT bytes, not a neighbour's"
    );

    // A non-failing step still resolves to ITS exact bytes (the index is per-step, byte-exact).
    let step1 = ci
        .tier
        .resolve_step_anchor("myelin://acme/ci/run/run-42#step-1")
        .expect("resolve step 1");
    assert_eq!(step1, b"==> checkout\nFetching repo... ok\n");

    // The C2 index telemetry rides the P-ST-20 archiver: 0 unencrypted, content-addressed.
    assert_eq!(
        ci.tier.archiver().telemetry().unencrypted_segment_count(),
        0
    );
    assert!(ci.tier.archiver().telemetry().segment_content_addressed());
    assert_eq!(ci.tier.indexed_step_count(), 3);
}
