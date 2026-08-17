use myelin_events::{Firehose, FirehoseScope, Frame, FrameDraft};
use myelin_storage::{ArchiveError, FirehoseArchiver, KekId, KmsEngine, SealedSegment};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

struct OpStreamArchiver {
    firehose: Firehose,
    archiver: FirehoseArchiver,
    stream: String,
    scope: FirehoseScope,
}

impl OpStreamArchiver {
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

    fn publish(&mut self, payload: &str) -> u64 {
        self.firehose
            .publish(&self.stream, &self.scope, FrameDraft::new(payload))
            .expect("the fixture publishes a valid frame")
            .seq
    }

    fn seal_tail(&self, lo: u64, hi: u64) -> SealedSegment {
        self.archiver
            .seal_from_firehose(&self.firehose, &self.stream, &self.scope, lo, hi)
            .expect("seal")
            .expect("frames were held")
    }

    fn resolve(&self, segment: &SealedSegment) -> Result<Vec<Frame>, ArchiveError> {
        self.archiver.read_segment(&segment.content_hash)
    }
}

#[test]
fn cdc_11_8_non_ci_producer_seals_and_resolves_through_the_seam() {
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());
    let engine = Arc::new(KmsEngine::new());
    engine
        .ensure_kek(&KekId::new(tenant.clone(), region.clone()))
        .expect("seed the in-memory KEK");

    let mut svc = OpStreamArchiver::boot(tenant, region, engine, "oplog", "board:42");

    for i in 1..=6u64 {
        let seq = svc.publish(&format!("op-{i}"));
        assert_eq!(
            seq, i,
            "the 3.5 transport assigns a monotonic per-(stream,scope) seq"
        );
    }

    let segment = svc.seal_tail(2, 5);

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

    let resolved = svc.resolve(&segment).expect("resolve the segment");
    let live = svc.firehose.tail("oplog", &svc.scope, 2, 5);
    assert_eq!(
        resolved, live,
        "11.8: the archived segment matches the live firehose tail"
    );

    assert_eq!(svc.archiver.telemetry().unencrypted_segment_count(), 0);
    assert!(svc.archiver.telemetry().segment_content_addressed());
}
