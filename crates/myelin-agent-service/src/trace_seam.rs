use myelin_content::Block;
use myelin_refs::ArtifactRef;

pub const STATELESS_EXCEPT_TRACE_FLOOR: &str = "v1 agents are stateless across runs EXCEPT for the \
    content-addressed trace document (a Knowledge doc, erasable); long-term memory / RAG over prior \
    runs is a NAMED HOLDER SEAM, NOT BUILT (post-M5 AG-P25: embedding store via Search semantic 6.2, \
    ACL-filtered, purged on *.erased). Full DSR fan-out over the trace is AG-P23 (→ P-479).";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceDocument {
    pub run_id: u128,
    pub blocks: Vec<Block>,
}

impl TraceDocument {
    pub fn new(run_id: u128, blocks: Vec<Block>) -> TraceDocument {
        TraceDocument { run_id, blocks }
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let doc = serde_json::json!({ "run_id": self.run_id.to_string(), "blocks": self.blocks });
        serde_json::to_vec(&doc).expect("the frozen 13.1 Block taxonomy always serializes")
    }

    pub fn content_address(&self) -> ArtifactRef {
        let digest = blake3::hash(&self.canonical_bytes());
        ArtifactRef(format!("blake3:{}", hex::encode(digest.as_bytes())))
    }
}

pub fn trace_ref_of(doc: &TraceDocument) -> String {
    doc.content_address().0
}

pub fn is_content_addressed_kn_document(doc: &TraceDocument) -> bool {
    let addr = doc.content_address();
    addr.0.starts_with("blake3:") && !doc.canonical_bytes().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::{Inline, Span};

    fn text(s: &str) -> Inline {
        Inline {
            spans: vec![Span::Text {
                text: s.to_string(),
                marks: vec![],
                link: None,
            }],
            nodes: vec![],
        }
    }

    fn sample_trace(run_id: u128, reasoning: &str) -> TraceDocument {
        TraceDocument::new(
            run_id,
            vec![
                Block::Paragraph {
                    inline: text(reasoning),
                },
                Block::CodeBlock {
                    lang: Some("json".into()),
                    text: r#"{"tool":"draft","result":"ok"}"#.into(),
                },
            ],
        )
    }

    #[test]
    fn the_trace_is_a_13_1_block_document() {
        let doc = sample_trace(7, "the agent decided to draft a page");
        assert_eq!(
            doc.blocks.len(),
            2,
            "a paragraph + a code block - frozen 13.1 nodes"
        );
        assert!(
            matches!(doc.blocks[0], Block::Paragraph { .. }),
            "the reasoning is a 13.1 paragraph block"
        );
        assert!(
            matches!(doc.blocks[1], Block::CodeBlock { .. }),
            "the tool transcript is a 13.1 code_block"
        );
    }

    #[test]
    fn trace_ref_is_a_blake3_content_address() {
        let doc = sample_trace(7, "reasoning");
        let addr = doc.content_address();
        assert!(
            addr.0.starts_with("blake3:"),
            "the ONE platform multihash convention: {}",
            addr.0
        );
        assert_eq!(addr.0.len(), "blake3:".len() + 64, "blake3:<64-hex>");
        assert_eq!(
            trace_ref_of(&doc),
            addr.0,
            "trace_ref_of is the content address (the §4.5 seam)"
        );
    }

    #[test]
    fn content_address_is_deterministic_and_collision_sensitive() {
        let a1 = sample_trace(7, "reasoning A").content_address();
        let a2 = sample_trace(7, "reasoning A").content_address();
        assert_eq!(
            a1, a2,
            "identical trace bytes → identical content address (deterministic)"
        );

        let b = sample_trace(7, "reasoning B").content_address();
        assert_ne!(
            a1, b,
            "a different reasoning body → a different content address"
        );

        let c = sample_trace(8, "reasoning A").content_address();
        assert_ne!(
            a1, c,
            "the run_id is folded into the address - a per-run holder body"
        );
    }

    #[test]
    fn the_trace_is_a_content_addressed_kn_document() {
        assert!(is_content_addressed_kn_document(&sample_trace(7, "x")));
    }

    #[test]
    fn the_stateless_except_trace_floor_is_named() {
        assert!(STATELESS_EXCEPT_TRACE_FLOOR.contains("stateless across runs"));
        assert!(STATELESS_EXCEPT_TRACE_FLOOR.contains("NOT BUILT"));
        assert!(
            STATELESS_EXCEPT_TRACE_FLOOR.contains("AG-P25"),
            "names the long-term-memory follow-on"
        );
        assert!(
            STATELESS_EXCEPT_TRACE_FLOOR.contains("AG-P23"),
            "names the DSR fan-out follow-on"
        );
    }
}
