use myelin_agent_service as fabric;
use myelin_content::{parse_inline, Block};
use myelin_knowledge::agent::trace as kn;
use myelin_knowledge::agent::AgentTraceHolder;
use myelin_tenancy::TenantId;

fn trace_blocks() -> Vec<Block> {
    vec![Block::Paragraph {
        inline: parse_inline(
            "the agent read the page and drafted a summary for the support ticket",
            &[],
        ),
    }]
}

fn tenant() -> TenantId {
    TenantId("acme".into())
}

#[test]
fn cdc_8_8_kn_producer_trace_ref_is_content_addressed_like_the_fabric_seam() {
    let holder = AgentTraceHolder::new();
    let t = tenant();
    let producer_ref = kn::write_agent_trace(&holder, &t, "run-1", trace_blocks(), "p-agent");
    assert!(
        producer_ref.0.contains("/agent_trace/blake3:"),
        "the Knowledge producer's run.trace_ref is content-addressed (BLAKE3): {}",
        producer_ref.0
    );
    let (_, producer_hash) = producer_ref
        .0
        .rsplit_once("/agent_trace/")
        .expect("the ref carries the content hash");
    assert!(
        producer_hash.starts_with("blake3:"),
        "the producer hash is a BLAKE3 multihash: {producer_hash}"
    );

    let fabric_doc = fabric::TraceDocument::new(1, trace_blocks());
    let fabric_ref = fabric::trace_ref_of(&fabric_doc);
    assert!(
        fabric_ref.starts_with("blake3:"),
        "the Fabric consumer's run.trace_ref is a content address: {fabric_ref}"
    );
    assert!(
        fabric::is_content_addressed_kn_document(&fabric_doc),
        "the Fabric trace is the content-addressed KN document (8.8)"
    );

    let producer_digest = producer_hash
        .strip_prefix("blake3:")
        .expect("blake3 prefix");
    let fabric_digest = fabric_ref.strip_prefix("blake3:").expect("blake3 prefix");
    assert_eq!(
        producer_digest.len(),
        64,
        "BLAKE3 = 32 bytes = 64 hex chars (producer)"
    );
    assert_eq!(
        fabric_digest.len(),
        64,
        "BLAKE3 = 32 bytes = 64 hex chars (Fabric)"
    );
    assert!(producer_digest.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(fabric_digest.chars().all(|c| c.is_ascii_hexdigit()));

    let again = kn::write_agent_trace(&holder, &t, "run-1", trace_blocks(), "p-agent");
    assert_eq!(
        producer_ref, again,
        "the same content yields the same run.trace_ref"
    );
    assert_eq!(
        holder.len(),
        1,
        "the same trace content writes once (content-addressed)"
    );
}

#[test]
fn cdc_8_8_trace_holder_is_erasable_and_distinct_from_the_audit_log() {
    assert!(
        kn::trace_is_distinct_from_audit(),
        "the Knowledge trace holder is distinct from the audit log"
    );
    assert_ne!(
        kn::TRACE_ERASABLE,
        kn::AUDIT_LOG_ERASABLE,
        "the trace is erasable; the audit log is the retain carve-out (distinct mechanisms)"
    );

    assert_eq!(
        kn::TRACE_HOLDER_ID,
        myelin_gdpr_service::AGENT_TRACE_HOLDER_ID,
        "the Knowledge trace holder id IS the GDPR-service H17 `agent_fabric_trace` id (no parallel id)"
    );
    assert!(
        myelin_gdpr_service::trace_is_distinct_from_audit(),
        "the GDPR-service seam agrees the trace is distinct from the audit log"
    );
}
