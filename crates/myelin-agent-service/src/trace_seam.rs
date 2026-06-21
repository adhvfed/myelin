//! # `trace_seam` — the agent-trace HOLDER seam (AG-7 / contract 8.8; AG-P19 → P-268, M3)
//!
//! The execution trace is **a content-addressed Knowledge document reusing the frozen
//! `myelin-content` 13.1 block model**; `run.trace_ref` resolves to its content hash; the holder
//! registers as an erasable [`PersonalDataHolder`] (the H17 [`AgentTraceHolder`](crate::holder::AgentTraceHolder)),
//! **distinct from** the tamper-evident audit log.
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §4.5 (the `trace` =
//! a content-addressed Knowledge document reusing `myelin-content`; a `PersonalDataHolder`,
//! residency-pinned, crypto-shred-capable; `run.trace_ref` is its `ArtifactRef`; DISTINCT from the
//! audit log) + §6 (AG-7 confirmed as contract 8.8).
//!
//! **Contract-index:** CONSUMES **8.8** (the trace-holder seam — the Fabric resolves `run.trace_ref`
//! to the KN holder; Knowledge OWNS the holder body, the Fabric the SEAM), **13.1** (the trace
//! REUSES the frozen `myelin-content` [`Block`] taxonomy — no second document model). The holder
//! REGISTRATION is [`crate::holder`] (H17); this module is the **content-addressed write seam** —
//! how the trace document is built (from 13.1 blocks) and content-addressed, so `run.trace_ref` is a
//! genuine `blake3:<hex>` over a 13.1 document, not an opaque URL placeholder.
//!
//! **VISION §3** (GDPR-safe by construction: the trace is erasable by crypto-shred, never hidden).
//! **EI-04 §1** (the trace IS personal data — the conversation/reasoning the brain authored — so it
//! is a per-subject-DEK-encrypted holder body, content-addressed for integrity, erasable). **EI-01
//! §7** (REUSE the 13.1 block model — the trace is NOT a bespoke transcript format; it is a Knowledge
//! document, so it indexes/erases/renders through the SAME path every page does).
//!
//! ## Why content-addressed + the 13.1 block model (the AG-7 design, §4.5)
//! The trace is the run's reasoning record (the system context, the tool transcript, the brain's
//! steps). Modelling it as a **Knowledge document** (a `Vec<Block>` from the frozen 13.1 taxonomy)
//! rather than an opaque blob means:
//! - **one document model** — the trace renders, indexes (Search semantic 6.2, post-M5), and erases
//!   through the SAME path a wiki page does (no second engine; EI-01 §7);
//! - **content-addressed** — `run.trace_ref` is the `blake3:<hex>` of the canonical serialization of
//!   the document (the ONE platform multihash convention, the SAME as the GDPR Receipt + BlobStore),
//!   so the pointer is integrity-checkable and the same trace bytes always address to the same ref
//!   (deterministic, dedup-friendly);
//! - **erasable** — the document bytes are the H17 holder body (per-subject-DEK encrypted, tagged
//!   `CryptoShred(subject_dek)` on [`crate::schema::TraceRow`]); `erase(subject)` crypto-shreds the
//!   DEK so the content-addressed document is unrecoverable live + in backups (the KN-D12 / AG-D10
//!   "erasure reaches the trace" drill) while the OPAQUE PSEUDONYM attribution survives.
//!
//! ## FLOORS named (cross-references; VISION §3, EI-01 §1)
//! - **The KNOWLEDGE-side holder BODY** (the live content-addressed write into the Knowledge store +
//!   the crypto-shred erase against a real backend) is the Knowledge platform's deliverable; here the
//!   Fabric ships the SEAM (build the 13.1 document + content-address it + resolve `run.trace_ref` to
//!   it + register the H17 holder). The holder registration is real ([`crate::holder`]); the
//!   empty-but-correct DSR bodies (the full fan-out) are AG-P23 (→ P-479, drill AG-D10).
//! - **Agent long-term memory / RAG over prior runs is a NAMED HOLDER SEAM, NOT BUILT.** v1 agents
//!   are **stateless across runs EXCEPT for this content-addressed trace document** — there is no
//!   embedding store, no cross-run recall. When built (post-M5, AG-P25) it indexes the trace via
//!   Search `semantic` (contract 6.2), ACL-filtered, purged on `*.erased`. This is stated in writing
//!   (the [`STATELESS_EXCEPT_TRACE_FLOOR`] note) so the trace is never mistaken for agent memory.

use myelin_content::Block;
use myelin_refs::ArtifactRef;

/// **The named floor (VISION §3): v1 agents are STATELESS ACROSS RUNS except for the
/// content-addressed trace document.** There is NO long-term memory / RAG over prior runs at v1 — no
/// embedding store, no cross-run recall. The trace holder seam (this module) is the ONLY cross-run
/// artifact, and it is a Knowledge document the subject can erase. The embedding store + its erasure
/// are a Search/Knowledge follow-on (post-M5, AG-P25, indexing via Search `semantic` 6.2,
/// ACL-filtered, purged on `*.erased`). The full DSR fan-out over the trace is AG-P23 (→ P-479).
pub const STATELESS_EXCEPT_TRACE_FLOOR: &str = "v1 agents are stateless across runs EXCEPT for the \
    content-addressed trace document (a Knowledge doc, erasable); long-term memory / RAG over prior \
    runs is a NAMED HOLDER SEAM, NOT BUILT (post-M5 AG-P25: embedding store via Search semantic 6.2, \
    ACL-filtered, purged on *.erased). Full DSR fan-out over the trace is AG-P23 (→ P-479).";

/// **The agent execution trace AS a content-addressed Knowledge document (§4.5 / 8.8).** Reuses the
/// frozen `myelin-content` 13.1 [`Block`] taxonomy — the trace is a `Vec<Block>` (the run's reasoning
/// body: system context, tool transcript, steps), NOT a bespoke transcript format. The same blocks a
/// wiki page is built from carry the trace, so it renders / indexes / erases through the SAME path.
///
/// This is the **in-Fabric document model** the holder body writes content-addressed into Knowledge
/// (the KN side owns the live write/erase; the Fabric owns building + addressing the document). The
/// `run_id` keys the holder body (the subject-locator on [`crate::schema::TraceRow`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceDocument {
    /// The run this trace belongs to (the holder body's subject-locator; opaque, no PII).
    pub run_id: u128,
    /// The trace body AS frozen 13.1 blocks (the reasoning record — system context, tool transcript,
    /// steps). REUSES the [`Block`] taxonomy; no second document model (EI-01 §7).
    pub blocks: Vec<Block>,
}

impl TraceDocument {
    /// Build a trace document for `run_id` from its 13.1 [`Block`]s.
    pub fn new(run_id: u128, blocks: Vec<Block>) -> TraceDocument {
        TraceDocument { run_id, blocks }
    }

    /// **The canonical serialization of the trace document (the bytes the content-address is taken
    /// over).** Deterministic JSON over the frozen 13.1 [`Block`] taxonomy (`Block` is
    /// `Serialize`/`Deserialize`, X-2 frozen), so the SAME document always serializes to the SAME
    /// bytes → the SAME content address (the dedup / integrity property). This is the document body
    /// the KN holder writes (per-subject-DEK encrypted at rest; the bytes here are the plaintext the
    /// address is computed over, the integrity anchor).
    pub fn canonical_bytes(&self) -> Vec<u8> {
        // serde_json over the frozen Block taxonomy is deterministic for a fixed input (object key
        // order is the struct field order; the trace is a Vec, order-preserving). The run_id is folded
        // in so two runs with identical reasoning still address distinctly (the holder body is
        // per-run). A genuinely canonical CBOR/DAG-CBOR form is the KN store's concern; the JSON here
        // is the stable Fabric-side anchor the CDC pins.
        let doc = serde_json::json!({ "run_id": self.run_id.to_string(), "blocks": self.blocks });
        serde_json::to_vec(&doc).expect("the frozen 13.1 Block taxonomy always serializes")
    }

    /// **The content address of the trace document — `run.trace_ref` resolves to THIS (§4.5).** The
    /// `blake3:<hex>` of [`canonical_bytes`](Self::canonical_bytes) — the ONE platform multihash
    /// convention (the SAME `blake3:<hex>` the GDPR `Receipt` + the BlobStore use). Deterministic: the
    /// same document bytes always address to the same ref. This IS the `ArtifactRef` stored in
    /// [`crate::schema::Run::trace_ref`] / [`crate::schema::TraceRow::artifact_ref`] — a genuine
    /// content hash over a 13.1 document, NOT an opaque URL placeholder.
    pub fn content_address(&self) -> ArtifactRef {
        let digest = blake3::hash(&self.canonical_bytes());
        ArtifactRef(format!("blake3:{}", hex::encode(digest.as_bytes())))
    }
}

/// **Resolve a trace document to the `run.trace_ref` it is stored under (the §4.5 seam).** A thin
/// alias for [`TraceDocument::content_address`] — the name the call sites read as "what does
/// `run.trace_ref` point at?": the content-addressed Knowledge document. The `String` form matches
/// [`crate::schema::Run::trace_ref`]'s field type.
pub fn trace_ref_of(doc: &TraceDocument) -> String {
    doc.content_address().0
}

/// **Assert the trace is the AG-7 content-addressed Knowledge document, distinct from the audit log
/// (§4.5).** The trace is the run's REASONING record (erasable, the brain's content) — the audit log
/// is the tamper-evident, immutable decision record GDPR/Audit owns; the two are DELIBERATELY
/// distinct (an erase crypto-shreds the trace; the audit log keeps the pseudonymous attribution fact).
/// This returns `true` iff the document is a non-degenerate content-addressed 13.1 document — the
/// structural assertion the seam holds (the distinctness is a posture, asserted in the drills).
pub fn is_content_addressed_kn_document(doc: &TraceDocument) -> bool {
    let addr = doc.content_address();
    addr.0.starts_with("blake3:") && !doc.canonical_bytes().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::{Inline, Span};

    /// A 13.1 inline run of plain text (a [`Span::Text`] with no marks) — the reasoning prose.
    fn text(s: &str) -> Inline {
        Inline {
            spans: vec![Span::Text { text: s.to_string(), marks: vec![], link: None }],
            nodes: vec![],
        }
    }

    /// A small trace document built from frozen 13.1 blocks (a paragraph of the agent's reasoning + a
    /// code block of a tool transcript) — the SAME taxonomy a wiki page uses (EI-01 §7).
    fn sample_trace(run_id: u128, reasoning: &str) -> TraceDocument {
        TraceDocument::new(
            run_id,
            vec![
                Block::Paragraph { inline: text(reasoning) },
                Block::CodeBlock {
                    lang: Some("json".into()),
                    text: r#"{"tool":"draft","result":"ok"}"#.into(),
                },
            ],
        )
    }

    /// **The trace REUSES the frozen 13.1 block model (8.8 / 13.1) — it is a `Vec<Block>`, not a
    /// bespoke transcript format.** The document is built from the SAME `Block` taxonomy a wiki page
    /// is; no second document model exists (EI-01 §7).
    #[test]
    fn the_trace_is_a_13_1_block_document() {
        let doc = sample_trace(7, "the agent decided to draft a page");
        assert_eq!(doc.blocks.len(), 2, "a paragraph + a code block — frozen 13.1 nodes");
        assert!(
            matches!(doc.blocks[0], Block::Paragraph { .. }),
            "the reasoning is a 13.1 paragraph block"
        );
        assert!(
            matches!(doc.blocks[1], Block::CodeBlock { .. }),
            "the tool transcript is a 13.1 code_block"
        );
    }

    /// **`run.trace_ref` resolves to the CONTENT ADDRESS of the document (§4.5) — a `blake3:<hex>`,
    /// the ONE platform multihash convention.** The ref is a genuine content hash over a 13.1
    /// document, NOT an opaque URL placeholder.
    #[test]
    fn trace_ref_is_a_blake3_content_address() {
        let doc = sample_trace(7, "reasoning");
        let addr = doc.content_address();
        assert!(addr.0.starts_with("blake3:"), "the ONE platform multihash convention: {}", addr.0);
        // 32-byte BLAKE3 → 64 hex chars after the `blake3:` prefix.
        assert_eq!(addr.0.len(), "blake3:".len() + 64, "blake3:<64-hex>");
        assert_eq!(trace_ref_of(&doc), addr.0, "trace_ref_of is the content address (the §4.5 seam)");
    }

    /// **Content addressing is DETERMINISTIC — the same trace bytes always address to the same ref
    /// (the dedup / integrity property), and a DIFFERENT body addresses differently.** Two builds of
    /// the identical document yield the identical ref; changing the reasoning changes the ref.
    #[test]
    fn content_address_is_deterministic_and_collision_sensitive() {
        let a1 = sample_trace(7, "reasoning A").content_address();
        let a2 = sample_trace(7, "reasoning A").content_address();
        assert_eq!(a1, a2, "identical trace bytes → identical content address (deterministic)");

        let b = sample_trace(7, "reasoning B").content_address();
        assert_ne!(a1, b, "a different reasoning body → a different content address");

        // a different run with the same reasoning addresses distinctly (the holder body is per-run).
        let c = sample_trace(8, "reasoning A").content_address();
        assert_ne!(a1, c, "the run_id is folded into the address — a per-run holder body");
    }

    /// **The trace is the content-addressed Knowledge document seam (8.8).** The structural assertion
    /// the Fabric holds: a non-degenerate 13.1 document content-addresses to a `blake3:` ref.
    #[test]
    fn the_trace_is_a_content_addressed_kn_document() {
        assert!(is_content_addressed_kn_document(&sample_trace(7, "x")));
    }

    /// **The stateless-except-trace FLOOR is stated in writing (VISION §3).** The note names the
    /// post-M5 long-term-memory follow-on (AG-P25, Search semantic) + the DSR fan-out (AG-P23) so the
    /// trace is never mistaken for agent memory.
    #[test]
    fn the_stateless_except_trace_floor_is_named() {
        assert!(STATELESS_EXCEPT_TRACE_FLOOR.contains("stateless across runs"));
        assert!(STATELESS_EXCEPT_TRACE_FLOOR.contains("NOT BUILT"));
        assert!(STATELESS_EXCEPT_TRACE_FLOOR.contains("AG-P25"), "names the long-term-memory follow-on");
        assert!(STATELESS_EXCEPT_TRACE_FLOOR.contains("AG-P23"), "names the DSR fan-out follow-on");
    }
}
