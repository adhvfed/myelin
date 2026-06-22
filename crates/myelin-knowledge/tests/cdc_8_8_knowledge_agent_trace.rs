//! # CDC 8.8 (Knowledge slice) — the AG-7 content-addressed agent-trace holder PRODUCER agrees with
//! the Fabric CONSUMER seam (KN-P28 → P-318, M3 — drill KN-D12)
//!
//! **Contract:** index row 8.8 — *AG-7 agent trace: Knowledge accepts a content-addressed agent-trace
//! write reusing the block model, returns `run.trace_ref`, and registers it as an **erasable holder**;
//! **distinct from the audit log*** (architecture §5.2 / gdpr §3.2 H17).
//!
//! KN-P28 is the live KNOWLEDGE-side PRODUCER (the architecture places the AG-7 holder in
//! `myelin-knowledge`'s agent module): [`myelin_knowledge::agent::trace::write_agent_trace`]
//! content-addresses a `Vec<Block>` trace to a `blake3:<hex>` `run.trace_ref` and registers it in the
//! erasable [`AgentTraceHolder`](myelin_knowledge::agent::AgentTraceHolder). The CONSUMER is the Agent
//! Fabric's trace seam (`myelin_agent_service::knowledge_tools`, AG-P19 → P-268): the run records
//! `run.trace_ref` (a `blake3:<hex>` over the frozen 13.1 block model) and treats the trace as the H17
//! erasable holder distinct from the audit log.
//!
//! **CDC pair (8.8, Knowledge slice).** This file pins that the two sides AGREE by construction:
//! - **PROVIDER** = `myelin_knowledge::agent::trace` — the content-addressed write (the ref IS a
//!   BLAKE3 over the block-model trace; idempotent-by-content) + the erasable holder distinct from the
//!   audit log + the KN-D12 erase (crypto-shred, attribution falls back to the pseudonym).
//! - **CONSUMER** = `myelin_agent_service::knowledge_tools` — the Fabric seam that records
//!   `run.trace_ref` as a `blake3:<hex>` over a 13.1 `Vec<Block>` and reads the trace as the H17
//!   holder distinct from the audit log.
//!
//! The test asserts: (1) the Knowledge producer's `run.trace_ref` is content-addressed (BLAKE3 over the
//! block model) exactly as the Fabric consumer's `trace_ref_of` is (the SAME `blake3:` multihash
//! convention over the SAME 13.1 `Vec<Block>` taxonomy — one document model, no second taxonomy); (2)
//! the producer is idempotent-by-content (the same trace writes once; distinct content → distinct refs);
//! (3) the trace holder is distinct from the audit log on both sides (the §6.5 boundary).
//!
//! NOTE on the byte-exact digest: the Fabric anchor's canonical JSON (`{run_id, blocks}`) and the
//! Knowledge store's canonical JSON (`{schema, run_id, actor, blocks}`) deliberately differ — the
//! Fabric doc'd its JSON as the "Fabric-side anchor" while "a genuinely canonical form is the KN
//! store's concern" (trace_seam.rs §canonical_bytes). So the CDC pins the SHARED convention (a BLAKE3
//! `blake3:<hex>` over the frozen `Vec<Block>`), not byte-identical digests — the contract is "content
//! -addressed reusing the block model", which both honour.

use myelin_agent_service as fabric;
use myelin_content::{parse_inline, Block};
use myelin_knowledge::agent::trace as kn;
use myelin_knowledge::agent::AgentTraceHolder;
use myelin_tenancy::TenantId;

/// The SHARED trace narrative (the conversation rendered to the frozen 13.1 block model). Both sides
/// content-address THIS — one document model, no second taxonomy.
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

/// **8.8: the Knowledge PRODUCER's `run.trace_ref` is content-addressed (BLAKE3 over the block model)
/// — the SAME `blake3:<hex>` convention over the SAME 13.1 `Vec<Block>` taxonomy the Fabric CONSUMER
/// uses.** Both sides honour "content-addressed reusing the block model" (8.8): the producer's ref
/// carries a `blake3:<hex>` over the block document; the Fabric's `trace_ref_of` over the SAME blocks
/// is also a `blake3:<hex>` over the SAME block taxonomy. The producer is idempotent-by-content.
#[test]
fn cdc_8_8_kn_producer_trace_ref_is_content_addressed_like_the_fabric_seam() {
    // PROVIDER (Knowledge): write_agent_trace content-addresses the block-model trace → run.trace_ref.
    let holder = AgentTraceHolder::new();
    let t = tenant();
    let producer_ref = kn::write_agent_trace(&holder, &t, "run-1", trace_blocks(), "p-agent");
    assert!(
        producer_ref.0.contains("/agent_trace/blake3:"),
        "the Knowledge producer's run.trace_ref is content-addressed (BLAKE3): {}",
        producer_ref.0
    );
    // the bare content hash the producer minted (the trailing `blake3:<hex>` of the ref).
    let (_, producer_hash) = producer_ref
        .0
        .rsplit_once("/agent_trace/")
        .expect("the ref carries the content hash");
    assert!(
        producer_hash.starts_with("blake3:"),
        "the producer hash is a BLAKE3 multihash: {producer_hash}"
    );

    // CONSUMER (Fabric): trace_ref_of over the SAME 13.1 block document is also a blake3:<hex>.
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

    // Both `blake3:<hex>` digests are well-formed 32-byte (64 hex) BLAKE3 hashes — the SHARED multihash
    // convention over the SAME block taxonomy (one document model). The exact bytes differ by design
    // (the Fabric anchor folds run_id only; the KN store folds schema+run_id+actor — see the module
    // header), so we pin the convention, not byte-equality.
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

    // The producer is idempotent-by-content: the SAME trace writes ONCE (one stored copy).
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

/// **8.8 / §6.5: the trace holder is an ERASABLE holder DISTINCT from the tamper-evident audit log —
/// on the Knowledge producer side AND agreeing with the GDPR-service H17 seam.** The Knowledge holder
/// id IS the H17 `agent_fabric_trace` id the GDPR-service seam registers (ONE name across the seam),
/// and the trace is erasable while the audit log is the retain carve-out (distinct mechanisms).
#[test]
fn cdc_8_8_trace_holder_is_erasable_and_distinct_from_the_audit_log() {
    // PROVIDER (Knowledge): the trace holder is distinct from the audit log (the §6.5 boundary).
    assert!(
        kn::trace_is_distinct_from_audit(),
        "the Knowledge trace holder is distinct from the audit log"
    );
    // the trace IS erasable (crypto-shred); the audit log is the retain carve-out — distinct.
    assert_ne!(
        kn::TRACE_ERASABLE,
        kn::AUDIT_LOG_ERASABLE,
        "the trace is erasable; the audit log is the retain carve-out (distinct mechanisms)"
    );

    // CONSUMER (the GDPR-service H17 seam): the Knowledge holder id IS the seam's H17 id (one name).
    assert_eq!(
        kn::TRACE_HOLDER_ID,
        myelin_gdpr_service::AGENT_TRACE_HOLDER_ID,
        "the Knowledge trace holder id IS the GDPR-service H17 `agent_fabric_trace` id (no parallel id)"
    );
    // the seam agrees the trace is distinct from the audit log.
    assert!(
        myelin_gdpr_service::trace_is_distinct_from_audit(),
        "the GDPR-service seam agrees the trace is distinct from the audit log"
    );
}
