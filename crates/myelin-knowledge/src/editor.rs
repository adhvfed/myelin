use crate::transport::{
    AllowAllAuthority, AuthAction, CollabTransport, Connected, DocOp, OpAuthority, OpId, OpKind,
    PersistedOp, SendOutcome, TransportError,
};
use myelin_content::editor::surgery::{insert_text, split_at};
use myelin_content::editor::{canonicalize, caret_count};
use myelin_content::inline::InlineNode;
use myelin_identity::Principal;
use myelin_tenancy::TenantId;

pub const BROWSER_DRIVE_EVIDENCE: &str =
    "partial - headless model gate green (CI); DOM-bridge round-trip exercised; \
     full Playwright drive against the design-system <BlockEditor> shell is the UI follow-on \
     (see crates/myelin-knowledge/editor-browser-drive.md, dated 2026-06-22)";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorBlock {
    pub md: String,
    pub nodes: Vec<InlineNode>,
}

impl EditorBlock {
    pub fn new(md: &str, nodes: &[InlineNode]) -> EditorBlock {
        let (md, nodes) = canonicalize(md, nodes);
        EditorBlock { md, nodes }
    }

    pub fn empty() -> EditorBlock {
        EditorBlock {
            md: String::new(),
            nodes: Vec::new(),
        }
    }

    pub fn caret_count(&self) -> usize {
        caret_count(&self.md)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditOp {
    InsertText {
        block: usize,
        offset: usize,
        text: String,
    },
    SplitBlock { block: usize, offset: usize },
    AppendBlock { md: String },
}

impl EditOp {
    pub fn kind(&self) -> OpKind {
        match self {
            EditOp::InsertText { .. } => OpKind::Insert,
            EditOp::SplitBlock { .. } => OpKind::BlockIns,
            EditOp::AppendBlock { .. } => OpKind::BlockIns,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        match self {
            EditOp::InsertText {
                block,
                offset,
                text,
            } => {
                format!("it\t{block}\t{offset}\t{text}").into_bytes()
            }
            EditOp::SplitBlock { block, offset } => format!("sb\t{block}\t{offset}").into_bytes(),
            EditOp::AppendBlock { md } => format!("ab\t{md}").into_bytes(),
        }
    }

    pub fn decode(bytes: &[u8]) -> Option<EditOp> {
        let s = core::str::from_utf8(bytes).ok()?;
        let (verb, rest) = s.split_once('\t').unwrap_or((s, ""));
        match verb {
            "it" => {
                let (block, rest) = rest.split_once('\t')?;
                let (offset, text) = rest.split_once('\t')?;
                Some(EditOp::InsertText {
                    block: block.parse().ok()?,
                    offset: offset.parse().ok()?,
                    text: text.to_string(),
                })
            }
            "sb" => {
                let (block, offset) = rest.split_once('\t')?;
                Some(EditOp::SplitBlock {
                    block: block.parse().ok()?,
                    offset: offset.parse().ok()?,
                })
            }
            "ab" => Some(EditOp::AppendBlock {
                md: rest.to_string(),
            }),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Document {
    pub blocks: Vec<EditorBlock>,
}

impl Document {
    pub fn new_page() -> Document {
        Document {
            blocks: vec![EditorBlock::empty()],
        }
    }

    pub fn blank() -> Document {
        Document { blocks: Vec::new() }
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub fn apply(&mut self, op: &EditOp) -> Option<usize> {
        match op {
            EditOp::InsertText {
                block,
                offset,
                text,
            } => {
                let b = self.blocks.get_mut(*block)?;
                let (md, nodes, caret) = insert_text(&b.md, &b.nodes, *offset, text);
                *b = EditorBlock { md, nodes };
                Some(caret)
            }
            EditOp::SplitBlock { block, offset } => {
                let b = self.blocks.get(*block)?;
                let split = split_at(&b.md, &b.nodes, *offset);
                self.blocks[*block] = EditorBlock {
                    md: split.left,
                    nodes: split.left_nodes,
                };
                self.blocks.insert(
                    *block + 1,
                    EditorBlock {
                        md: split.right,
                        nodes: split.right_nodes,
                    },
                );
                Some(split.caret)
            }
            EditOp::AppendBlock { md } => {
                self.blocks.push(EditorBlock::new(md, &[]));
                Some(0)
            }
        }
    }

    pub fn corpus_roundtrips(&self) -> bool {
        self.blocks.iter().all(|b| {
            let (re, _) = canonicalize(&b.md, &b.nodes);
            re == b.md
        })
    }

    pub fn to_markdown(&self) -> String {
        self.blocks
            .iter()
            .map(|b| b.md.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub struct Editor<A: OpAuthority = AllowAllAuthority> {
    doc: Document,
    transport: CollabTransport<A>,
    client_id: String,
    lamport: u64,
    actor: Principal,
}

impl Editor<AllowAllAuthority> {
    pub fn open_page(
        tenant: TenantId,
        page_id: &str,
        client_id: &str,
        actor: Principal,
    ) -> Result<Editor<AllowAllAuthority>, TransportError> {
        Editor::open_page_with_authority(tenant, page_id, client_id, actor, AllowAllAuthority)
    }
}

impl<A: OpAuthority> Editor<A> {
    pub fn open_page_with_authority(
        tenant: TenantId,
        page_id: &str,
        client_id: &str,
        actor: Principal,
        authority: A,
    ) -> Result<Editor<A>, TransportError> {
        let transport = CollabTransport::open_with_authority(tenant, page_id, authority)?;
        Ok(Editor {
            doc: Document::new_page(),
            transport,
            client_id: client_id.to_string(),
            lamport: 0,
            actor,
        })
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    pub fn page_id(&self) -> &str {
        self.transport.page_id()
    }

    pub fn head_seq(&self) -> u64 {
        self.transport.head_seq()
    }

    fn next_op_id(&mut self) -> OpId {
        self.lamport += 1;
        OpId::new(self.client_id.clone(), self.lamport)
    }

    pub fn apply_local(&mut self, op: EditOp) -> SendOutcome {
        self.doc.apply(&op);
        let op_id = self.next_op_id();
        let doc_op = DocOp::cas(
            op_id,
            self.actor.principal_id.0.clone(),
            op.kind(),
            op.encode(),
        );
        self.transport.send_op(doc_op)
    }

    pub fn type_text(&mut self, block: usize, offset: usize, text: &str) -> SendOutcome {
        self.apply_local(EditOp::InsertText {
            block,
            offset,
            text: text.to_string(),
        })
    }

    pub fn split_block(&mut self, block: usize, offset: usize) -> SendOutcome {
        self.apply_local(EditOp::SplitBlock { block, offset })
    }

    pub fn append_block(&mut self, md: &str) -> SendOutcome {
        self.apply_local(EditOp::AppendBlock { md: md.to_string() })
    }

    pub fn connect_viewer(
        &mut self,
        principal: &Principal,
        cursor: Option<u64>,
    ) -> Result<SecondViewer, TransportError> {
        let connected = self
            .transport
            .connect(principal, AuthAction::Edit, cursor)?;
        let backfill = match connected {
            Connected::Resumed { backfill } => backfill,
            Connected::ResyncFromSnapshot { tail, .. } => tail,
        };
        let mut viewer = SecondViewer::new();
        for persisted in &backfill {
            viewer.apply_persisted(persisted);
        }
        Ok(viewer)
    }

    pub fn subscribe(
        &mut self,
        cursor: Option<u64>,
    ) -> Result<myelin_events::FirehoseSubscription, myelin_events::FirehoseError> {
        self.transport.subscribe(cursor)
    }
}

#[derive(Debug)]
pub struct SecondViewer {
    doc: Document,
    seen: std::collections::HashSet<String>,
}

impl Default for SecondViewer {
    fn default() -> SecondViewer {
        SecondViewer::new()
    }
}

impl SecondViewer {
    pub fn new() -> SecondViewer {
        SecondViewer {
            doc: Document::new_page(),
            seen: std::collections::HashSet::new(),
        }
    }

    pub fn with_seed(seed: Document) -> SecondViewer {
        SecondViewer {
            doc: seed,
            seen: std::collections::HashSet::new(),
        }
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    pub fn apply_persisted(&mut self, persisted: &PersistedOp) -> bool {
        let key = persisted.op.op_id.wire();
        if !self.seen.insert(key) {
            return false;
        }
        match EditOp::decode(&persisted.op.payload) {
            Some(op) => {
                self.doc.apply(&op);
                true
            }
            None => false,
        }
    }

    pub fn observe(&mut self, persisted: &PersistedOp) -> bool {
        self.apply_persisted(persisted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::inline::OBJ;
    use myelin_events::ArtifactRef;
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn actor(name: &str) -> Principal {
        Principal::stub(PrincipalId(name.into()), PrincipalKind::Human, tenant())
    }

    fn editor(client: &str) -> Editor<AllowAllAuthority> {
        Editor::open_page(tenant(), "page-1", client, actor("alice")).expect("page opens")
    }

    #[test]
    fn new_page_is_one_empty_block() {
        let e = editor("c1");
        assert_eq!(e.document().block_count(), 1);
        assert_eq!(e.document().blocks[0], EditorBlock::empty());
        assert!(
            e.document().corpus_roundtrips(),
            "an empty doc is a KN-D2 fixed point"
        );
    }

    #[test]
    fn typing_updates_the_document_and_sends_an_op() {
        let mut e = editor("c1");
        let out = e.type_text(0, 0, "Severity high");
        assert!(
            out.applied(),
            "a fresh edit is applied (assigned an op_seq)"
        );
        assert_eq!(out.persisted().op_seq, 1);
        assert_eq!(e.document().blocks[0].md, "Severity high");
        assert!(e.document().corpus_roundtrips());
    }

    #[test]
    fn enter_splits_a_block_caret_at_start_of_new() {
        let mut e = editor("c1");
        e.type_text(0, 0, "hello world");
        e.split_block(0, 6);
        assert_eq!(e.document().block_count(), 2, "Enter added a block");
        assert_eq!(e.document().blocks[0].md, "hello ");
        assert_eq!(e.document().blocks[1].md, "world");
        assert!(e.document().corpus_roundtrips());
    }

    #[test]
    fn ime_commit_is_char_faithful_end_to_end() {
        let mut e = editor("c1");
        e.type_text(0, 0, "ab cd");
        let out = e.type_text(0, 3, "日本");
        assert!(out.applied());
        assert_eq!(e.document().blocks[0].md, "ab 日本cd");
        assert!(e.document().corpus_roundtrips());
    }

    #[test]
    fn typed_reserved_char_escapes_through_the_one_render_path() {
        let mut e = editor("c1");
        e.type_text(0, 0, "ax");
        e.type_text(0, 1, "*");
        assert_eq!(e.document().blocks[0].md, r"a\*x");
        assert!(
            e.document().corpus_roundtrips(),
            "the escaped form is canonical"
        );
    }

    #[test]
    fn kn_d2_integrated_path_roundtrips_100_percent() {
        let mut e = editor("c1");
        e.type_text(0, 0, "# Incident: API 5xx spike");
        e.append_block("Severity **high**. Owner @alice");
        e.append_block("- [ ] page the on-call");
        e.append_block(r"escaped \* and `code` and ~~strike~~");
        e.split_block(1, 9);
        for (i, b) in e.document().blocks.iter().enumerate() {
            let (re, _) = canonicalize(&b.md, &b.nodes);
            assert_eq!(re, b.md, "block {i} ({:?}) is NOT a fixed point", b.md);
        }
        assert!(
            e.document().corpus_roundtrips(),
            "the integrated-path corpus-pass-rate is 100%"
        );
    }

    #[test]
    fn kn_d2_corpus_loads_as_document_blocks_100_percent() {
        let mut doc = Document::blank();
        for f in myelin_content::corpus::CORPUS {
            let nodes = myelin_content::corpus::synthetic_nodes_for(f.md);
            doc.blocks.push(EditorBlock::new(f.md, &nodes));
        }
        assert!(
            doc.corpus_roundtrips(),
            "the whole frozen KN-D2 corpus is a fixed point loaded as integrated-editor blocks"
        );
        assert!(doc.block_count() >= 18, "the corpus must not be shrunk");
    }

    #[test]
    fn a_second_viewer_converges_on_the_editor_document() {
        let mut e = editor("c1");
        let stream: Vec<PersistedOp> = vec![
            e.type_text(0, 0, "Severity ").persisted().clone(),
            e.split_block(0, 9).persisted().clone(),
            e.type_text(1, 0, "high").persisted().clone(),
            e.append_block("Owner @alice").persisted().clone(),
        ];

        let mut viewer = SecondViewer::new();
        for p in &stream {
            assert!(viewer.observe(p), "each op applies freshly on the viewer");
        }
        assert_eq!(
            viewer.document().to_markdown(),
            e.document().to_markdown(),
            "the second viewer converged on the editor's document (live-second-viewer property)"
        );
        assert_eq!(viewer.document(), e.document(), "byte-identical documents");
    }

    #[test]
    fn a_redelivered_frame_is_an_idempotent_no_op_on_the_viewer() {
        let mut e = editor("c1");
        let p = e.type_text(0, 0, "x").persisted().clone();
        let mut viewer = SecondViewer::new();
        assert!(viewer.observe(&p), "first observe applies");
        let before = viewer.document().clone();
        assert!(
            !viewer.observe(&p),
            "a re-delivered frame is a no-op (the op_id dedup)"
        );
        assert_eq!(
            viewer.document(),
            &before,
            "the document did NOT double-apply"
        );
    }

    #[test]
    fn a_late_joiner_is_caught_up_by_the_backfill() {
        let mut e = editor("c1");
        e.type_text(0, 0, "before join");
        e.append_block("second block");
        let mut viewer = e
            .connect_viewer(&actor("bob"), None)
            .expect("the viewer connects + is backfilled");
        assert_eq!(
            viewer.document().to_markdown(),
            e.document().to_markdown(),
            "the late joiner caught up via the backfill"
        );
        let p = e.append_block("after join").persisted().clone();
        assert!(viewer.observe(&p));
        assert_eq!(viewer.document().to_markdown(), e.document().to_markdown());
    }

    #[test]
    fn a_live_subscription_receives_the_edit_frame() {
        let mut e = editor("c1");
        let sub = e.subscribe(None).expect("a live subscription opens");
        let out = e.type_text(0, 0, "live edit");
        let frames = sub.drain_ready();
        assert_eq!(
            frames.len(),
            1,
            "the live subscriber received the published frame"
        );
        assert_eq!(
            frames[0].seq,
            out.persisted().op_seq,
            "the live frame seq == the op_seq"
        );
    }

    #[test]
    fn structured_node_survives_the_integrated_editor() {
        let nodes = vec![InlineNode::ArtifactRefNode(ArtifactRef(
            "myelin://acme/k/1".into(),
        ))];
        let md = format!("see {OBJ} here");
        let mut doc = Document::blank();
        doc.blocks.push(EditorBlock::new(&md, &nodes));
        assert!(
            doc.corpus_roundtrips(),
            "the structured-node block is canonical"
        );
        let obj_pos = md.chars().position(|c| c == OBJ).unwrap();
        doc.apply(&EditOp::SplitBlock {
            block: 0,
            offset: obj_pos + 1,
        });
        assert_eq!(doc.block_count(), 2);
        assert_eq!(doc.blocks[0].nodes.len(), 1);
        assert!(
            doc.corpus_roundtrips(),
            "both halves are KN-D2 fixed points after the split"
        );
    }

    #[test]
    fn an_out_of_range_op_is_a_no_op() {
        let mut doc = Document::new_page();
        let before = doc.clone();
        assert_eq!(
            doc.apply(&EditOp::InsertText {
                block: 99,
                offset: 0,
                text: "x".into()
            }),
            None
        );
        assert_eq!(
            doc.apply(&EditOp::SplitBlock {
                block: 99,
                offset: 0
            }),
            None
        );
        assert_eq!(
            doc, before,
            "an out-of-range op did not mutate the document"
        );
    }

    #[test]
    fn edit_op_wire_form_roundtrips() {
        for op in [
            EditOp::InsertText {
                block: 2,
                offset: 5,
                text: "with\ttab and 日本".into(),
            },
            EditOp::SplitBlock {
                block: 0,
                offset: 7,
            },
            EditOp::AppendBlock {
                md: "a new line".into(),
            },
        ] {
            let bytes = op.encode();
            assert_eq!(
                EditOp::decode(&bytes),
                Some(op),
                "the wire form round-trips"
            );
        }
        assert_eq!(EditOp::decode(b"foreign-op-bytes"), None);
        assert_eq!(EditOp::decode(b"it\tnot-a-number\t0\tx"), None);
    }

    #[test]
    fn an_over_broad_page_scope_is_rejected_at_open() {
        let r = Editor::open_page(tenant(), "*", "c1", actor("alice"));
        assert!(matches!(r, Err(TransportError::OverBroadScope(_))));
    }

    #[test]
    fn browser_drive_evidence_is_recorded_and_honestly_marked() {
        assert!(
            BROWSER_DRIVE_EVIDENCE.contains("partial"),
            "the drive is honestly marked partial"
        );
        assert!(
            BROWSER_DRIVE_EVIDENCE.contains("editor-browser-drive.md"),
            "names the dated artifact"
        );
    }
}
