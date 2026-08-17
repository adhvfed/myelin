use myelin_content::editor::{dom_to_offset, offset_to_dom};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::editor::{Document, EditorBlock, SecondViewer};
use myelin_knowledge::{AllowAllAuthority, Editor, EditorError, PersistedOp, SendOutcome};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn principal(name: &str) -> Principal {
    Principal::stub(PrincipalId(name.into()), PrincipalKind::Human, tenant())
}

fn editor(page_id: &str) -> Editor<AllowAllAuthority> {
    Editor::open_page(
        tenant(),
        page_id,
        "client-A",
        principal("alice"),
        AllowAllAuthority,
    )
    .expect("the page opens")
}

fn edit(result: Result<SendOutcome, EditorError>) -> SendOutcome {
    result.expect("the edit is valid and authorized")
}

#[test]
fn create_page_type_blocks_second_connection_sees_edits_live() {
    let mut editor = editor("incident-api-5xx");
    assert_eq!(
        editor.document().block_count(),
        1,
        "a new page starts with one empty block"
    );

    let sub = editor
        .subscribe(None)
        .expect("the second connection's live subscription opens");
    let mut viewer = SecondViewer::new();

    let op_log: Vec<PersistedOp> = vec![
        editor
            .type_text(0, 0, "# Incident: API 5xx spike")
            .expect("the edit is valid and authorized")
            .persisted()
            .clone(),
        editor
            .append_block("Severity **high**. Owner @alice")
            .expect("the edit is valid and authorized")
            .persisted()
            .clone(),
        editor
            .append_block("- [ ] page the on-call")
            .expect("the edit is valid and authorized")
            .persisted()
            .clone(),
        edit(editor.split_block(1, 9)).persisted().clone(),
        edit(editor.type_text(0, 0, "緊急 ")).persisted().clone(),
    ];

    let frames = sub.drain_ready();
    assert_eq!(
        frames.len(),
        op_log.len(),
        "the live subscriber saw a frame per edit"
    );

    for frame in &frames {
        let persisted = op_log
            .iter()
            .find(|p| p.op_seq == frame.seq)
            .expect("the live frame resolves to its op-log entry (op_seq is the cursor)");
        assert!(
            viewer.observe(persisted),
            "the viewer applies each live edit freshly"
        );
    }

    assert_eq!(
        viewer.document().to_markdown(),
        editor.document().to_markdown(),
        "the second connection converged on the editor's document"
    );
    assert_eq!(
        viewer.document(),
        editor.document(),
        "byte-identical documents on both connections"
    );

    assert!(
        editor.document().corpus_roundtrips(),
        "the editor doc is a KN-D2 fixed point"
    );
    assert!(
        viewer.document().corpus_roundtrips(),
        "the viewer doc is a KN-D2 fixed point"
    );
}

#[test]
fn a_redelivered_live_frame_does_not_double_apply() {
    let mut editor = editor("page-dup");
    let p = edit(editor.type_text(0, 0, "once")).persisted().clone();

    let mut viewer = SecondViewer::new();
    assert!(viewer.observe(&p), "the first delivery applies");
    let converged = viewer.document().clone();
    assert!(
        !viewer.observe(&p),
        "the re-delivered frame is an idempotent no-op"
    );
    assert_eq!(
        viewer.document(),
        &converged,
        "the document did not double-apply"
    );
}

#[test]
fn a_late_joiner_is_caught_up_then_sees_live() {
    let mut editor = editor("page-late");
    edit(editor.type_text(0, 0, "typed before the viewer joined"));
    edit(editor.append_block("a second pre-join block"));

    let mut viewer = editor
        .load_viewer(&principal("bob"), None)
        .expect("the late joiner is caught up by the backfill");
    assert_eq!(
        viewer.document().to_markdown(),
        editor.document().to_markdown(),
        "the late joiner caught up via the backfill"
    );

    let p = edit(editor.append_block("after the join"))
        .persisted()
        .clone();
    assert!(viewer.observe(&p));
    assert_eq!(
        viewer.document().to_markdown(),
        editor.document().to_markdown(),
        "the late joiner stays converged on the subsequent live edit"
    );
}

#[test]
fn the_dom_bridge_round_trips_on_the_live_document() {
    let mut editor = editor("page-bridge");
    edit(editor.type_text(0, 0, "Severity **high**. café 日本"));
    edit(editor.append_block("- [ ] task with `code` and ~~strike~~"));
    edit(editor.split_block(0, 9));

    for (i, block) in editor.document().blocks.iter().enumerate() {
        let len = block.md.chars().count();
        for off in 0..=len {
            let dom = offset_to_dom(&block.md, off);
            assert_eq!(
                dom_to_offset(&block.md, dom),
                off,
                "off-by-one on block {i} ({:?}) at offset {off}",
                block.md
            );
        }
    }
}

#[test]
fn the_kn_d2_corpus_round_trips_over_the_integrated_path() {
    let mut doc = Document::blank();
    for fixture in myelin_content::corpus::CORPUS {
        let nodes = myelin_content::corpus::synthetic_nodes_for(fixture.md);
        doc.blocks.push(EditorBlock::new(fixture.md, &nodes));
    }
    assert!(
        doc.block_count() >= 18,
        "the corpus must not be shrunk below its frozen size"
    );
    assert!(
        doc.corpus_roundtrips(),
        "every frozen KN-D2 fixture is a fixed point loaded as an integrated-editor block (100%)"
    );
}
