//! Contract 11.8 CDC pair — the T3 firehose-archive seam (the sealing + per-tenant-DEK segment
//! encryption half, P-ST-20 / global P-147). The `(job, step, byte-range)` index half is P-ST-26.
//!
//! The prompt requires "a provider+consumer pair for 11.8 (the firehose-archive seam — the non-CI
//! producer)". This is the consumer-driven contract test:
//!
//! - The **PROVIDER** is `myelin-storage`'s [`FirehoseArchiver`] (the sealing + per-tenant-DEK
//!   segment mechanism this prompt ships).
//! - The **CONSUMER** is a non-CI firehose producer/reader (modelled as a tiny `OpStreamArchiver`)
//!   that publishes a synthetic op-stream onto the Bus's 3.5 resume-cursor transport, seals a tail
//!   of it into a durable segment, carries ONLY the references-not-payloads pointer, and later
//!   resolves that pointer back to the exact frames (cold == live).
//!
//! The test pins the frozen 11.8 call shape every firehose-archiving subsystem (CI logs in M4, chat
//! message-log, collab op-stream archive) relies on — `seal` returns a content-addressed,
//! DEK-encrypted [`SealedSegment`]; `read_segment` resolves it back to the frames. If that shape
//! drifts, this stops compiling/passing.

use myelin_events::{Firehose, FirehoseScope, Frame, FrameDraft};
use myelin_storage::{ArchiveError, FirehoseArchiver, KekId, KmsEngine, SealedSegment};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

/// A consumer of 11.8: a NON-CI op-stream archiver. It rides the 3.5 firehose transport to capture
/// a high-volume op-stream, then asks the provider (`myelin-storage`) to seal a tail of it into a
/// durable, content-addressed, DEK-encrypted segment. It persists ONLY the [`SealedSegment`] pointer
/// (references-not-payloads) — exactly the durable-bus pointer-event shape (§3.3).
struct OpStreamArchiver {
    firehose: Firehose,
    archiver: FirehoseArchiver,
    stream: String,
    scope: FirehoseScope,
}

impl OpStreamArchiver {
    /// Boot the archiver over its per-tenant-DEK firehose archive (the provider) + a bounded-scope
    /// firehose subscription (the 3.5 transport).
    fn boot(
        tenant: TenantId,
        region: Region,
        engine: Arc<KmsEngine>,
        stream: &str,
        scope: &str,
    ) -> OpStreamArchiver {
        OpStreamArchiver {
            firehose: Firehose::new(),
            archiver: FirehoseArchiver::with_tenant_dek(tenant, region, engine),
            stream: stream.to_string(),
            scope: FirehoseScope::parse(scope).expect("a bounded scope"),
        }
    }

    /// Publish one op-stream frame onto the 3.5 transport (returns the assigned seq).
    fn publish(&mut self, payload: &str) -> u64 {
        self.firehose
            .publish(&self.stream, &self.scope, FrameDraft::new(payload))
            .seq
    }

    /// Seal a tail `[lo, hi]` of the live op-stream into a durable segment — the provider call.
    fn seal_tail(&self, lo: u64, hi: u64) -> SealedSegment {
        self.archiver
            .seal_from_firehose(&self.firehose, &self.stream, &self.scope, lo, hi)
            .expect("seal")
            .expect("frames were held")
    }

    /// Resolve a persisted pointer back to its exact frames (the durable tail; cold == live).
    fn resolve(&self, segment: &SealedSegment) -> Result<Vec<Frame>, ArchiveError> {
        self.archiver.read_segment(&segment.content_hash)
    }
}

/// THE CDC pair: a non-CI firehose producer publishes an op-stream, seals a tail through the trait,
/// carries only the pointer, and resolves the exact frames back — the provider (`myelin-storage`)
/// honours the frozen 11.8 sealing + per-tenant-DEK shape.
#[test]
fn cdc_11_8_non_ci_producer_seals_and_resolves_through_the_seam() {
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());
    let engine = Arc::new(KmsEngine::new());
    engine.ensure_kek(&KekId::new(tenant.clone(), region.clone()));

    let mut svc = OpStreamArchiver::boot(tenant, region, engine, "oplog", "board:42");

    // The producer emits a synthetic op-stream onto the 3.5 transport.
    for i in 1..=6u64 {
        let seq = svc.publish(&format!("op-{i}"));
        assert_eq!(
            seq, i,
            "the 3.5 transport assigns a monotonic per-(stream,scope) seq"
        );
    }

    // The provider seals a tail [2, 5] into a durable, content-addressed, DEK-encrypted segment.
    let segment = svc.seal_tail(2, 5);

    // The pointer is references-not-payloads: it names the content hash + range, never the bodies.
    assert_eq!(
        (segment.first_seq, segment.last_seq, segment.frame_count),
        (2, 5, 4)
    );
    let pointer = segment.pointer_payload();
    assert!(pointer.contains(&segment.content_hash.to_multihash_string()));
    assert!(
        !pointer.contains("op-2"),
        "11.8: the durable pointer must NOT inline the frame body"
    );

    // The consumer resolves the pointer back to the EXACT frames (cold == live) — proves the segment
    // is content-addressed AND decrypts under the per-tenant DEK.
    let resolved = svc.resolve(&segment).expect("resolve the segment");
    let live = svc.firehose.tail("oplog", &svc.scope, 2, 5);
    assert_eq!(
        resolved, live,
        "11.8: the archived segment matches the live firehose tail"
    );

    // The GATE telemetry the provider exposes: 0 unencrypted, content-addressed by construction.
    assert_eq!(svc.archiver.telemetry().unencrypted_segment_count(), 0);
    assert!(svc.archiver.telemetry().segment_content_addressed());
}
