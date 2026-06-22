//! # KN-P09 integration — the integrated single-doc editor end to end (P-299, M3)
//!
//! The roadmap §4 **first-runnable**: a single editor + a live second viewer. This integration test
//! drives the WHOLE integrated editor path end to end (architecture
//! `02-internals-and-algorithms.md` §8 + `04-views-cli-and-api.md` §1.1): **create a page → type
//! blocks → a second connection observes the edits live over the transport**, with the KN-D2
//! round-trip re-asserted over the integrated path (every block a `serialize(parse(md))===md` fixed
//! point) and the offset-model DOM-bridge round-tripping on the live document (the caret coordinate
//! the browser caret binds to is the model coordinate, 0 off-by-one).
//!
//! This is the in-process editor↔transport integration (the firehose op stream is in-process — the
//! KN-P07 transport seam). The DURABLE `doc_op` co-commit proof against the live dev-stack Postgres is
//! KN-D7's (the `integration` cargo feature, `tests/integration_kn_d7_outbox.rs`, already green); the
//! editor convergence + the KN-D2 integrated-path gate proven here are engine-agnostic protocol
//! properties over the transport, independent of the apply engine (the named CAS/Yrs floor).

use myelin_content::editor::{dom_to_offset, offset_to_dom};
use myelin_knowledge::editor::{Document, EditorBlock, SecondViewer};
use myelin_knowledge::{Editor, PersistedOp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn principal(name: &str) -> Principal {
    Principal::stub(PrincipalId(name.into()), PrincipalKind::Human, tenant())
}

/// **The headline integration: create a page → type blocks → a second connection sees the edits LIVE
/// (the §1.1 / roadmap §4 first-runnable).** The editor types the S1 happy-path content; a second
/// viewer subscribes + observes each edit's op live; the two documents converge byte-for-byte; the
/// KN-D2 round-trip holds on the integrated path the whole way.
#[test]
fn create_page_type_blocks_second_connection_sees_edits_live() {
    // create a page (the S1 "create a page" entry — one empty block to type into).
    let mut editor = Editor::open_page(tenant(), "incident-api-5xx", "client-A", principal("alice"))
        .expect("the page opens");
    assert_eq!(editor.document().block_count(), 1, "a new page starts with one empty block");

    // a second connection opens a live subscription on this doc's (stream, scope) BEFORE the edits,
    // and seeds from the same fresh-page state — it will observe every op live.
    let sub = editor.subscribe(None).expect("the second connection's live subscription opens");
    let mut viewer = SecondViewer::new();

    // Type the S1 happy-path content, block by block, recording the op stream so the live frames can
    // be resolved to the op-log entry the viewer applies (the firehose frame is a
    // references-not-payloads pointer; the op BYTES live in the op-log). The `.persisted().clone()` of
    // each `send_op` outcome is the assigned `op_seq`-bearing op. Each line is a SEPARATE editor
    // intent with its own side effect (the optimistic local apply + the firehose publish), evaluated
    // left to right — the vec is the recorded transcript.
    let op_log: Vec<PersistedOp> = vec![
        // a heading + two blocks (the slash-menu "Text" + a to-do)
        editor.type_text(0, 0, "# Incident: API 5xx spike").persisted().clone(),
        editor.append_block("Severity **high**. Owner @alice").persisted().clone(),
        editor.append_block("- [ ] page the on-call").persisted().clone(),
        // press Enter mid-line to split a block (caret lands at the start of the new block)
        editor.split_block(1, 9).persisted().clone(),
        // an IME / CJK commit mid-line (the named top risk — char offsets, never byte)
        editor.type_text(0, 0, "緊急 ").persisted().clone(),
    ];

    // the live subscription received exactly one frame per fresh op (the live fan-out).
    let frames = sub.drain_ready();
    assert_eq!(frames.len(), op_log.len(), "the live subscriber saw a frame per edit");

    // the second connection applies each op live (resolving each frame → the op-log entry).
    for frame in &frames {
        let persisted = op_log
            .iter()
            .find(|p| p.op_seq == frame.seq)
            .expect("the live frame resolves to its op-log entry (op_seq is the cursor)");
        assert!(viewer.observe(persisted), "the viewer applies each live edit freshly");
    }

    // CONVERGENCE: the second connection's document is byte-identical to the editor's (the live
    // second-viewer property — a second connection sees the edits live and converges).
    assert_eq!(
        viewer.document().to_markdown(),
        editor.document().to_markdown(),
        "the second connection converged on the editor's document"
    );
    assert_eq!(viewer.document(), editor.document(), "byte-identical documents on both connections");

    // KN-D2 over the INTEGRATED path: every block on BOTH documents is a serialize(parse(md))===md
    // fixed point (100%, 0 regressions on the integrated path, not just the library).
    assert!(editor.document().corpus_roundtrips(), "the editor doc is a KN-D2 fixed point");
    assert!(viewer.document().corpus_roundtrips(), "the viewer doc is a KN-D2 fixed point");
}

/// **A re-delivered live frame is an idempotent no-op on the second connection (the `op_id` dedup —
/// KN-D1's 0-duplicate property carried into the editor).** An at-least-once redelivery of a frame
/// the viewer already applied does not double-apply.
#[test]
fn a_redelivered_live_frame_does_not_double_apply() {
    let mut editor = Editor::open_page(tenant(), "page-dup", "client-A", principal("alice"))
        .expect("the page opens");
    let p = editor.type_text(0, 0, "once").persisted().clone();

    let mut viewer = SecondViewer::new();
    assert!(viewer.observe(&p), "the first delivery applies");
    let converged = viewer.document().clone();
    // the SAME frame is re-delivered (an at-least-once retransmit) — a no-op.
    assert!(!viewer.observe(&p), "the re-delivered frame is an idempotent no-op");
    assert_eq!(viewer.document(), &converged, "the document did not double-apply");
}

/// **A late joiner is caught up by the CONNECT backfill, then sees live edits (the §1.1 resume
/// path).** A second connection joining AFTER edits gets the missed ops backfilled (replayed exactly
/// once) and converges; a subsequent live edit applies on top.
#[test]
fn a_late_joiner_is_caught_up_then_sees_live() {
    let mut editor = Editor::open_page(tenant(), "page-late", "client-A", principal("alice"))
        .expect("the page opens");
    editor.type_text(0, 0, "typed before the viewer joined");
    editor.append_block("a second pre-join block");

    // the viewer connects NOW (cursor None → the whole tail is backfilled, replayed exactly once).
    let mut viewer = editor
        .connect_viewer(&principal("bob"), None)
        .expect("the late joiner connects + is caught up by the backfill");
    assert_eq!(
        viewer.document().to_markdown(),
        editor.document().to_markdown(),
        "the late joiner caught up via the backfill"
    );

    // a subsequent live edit applies on top (the viewer observes the new frame's op → still converged).
    let p = editor.append_block("after the join").persisted().clone();
    assert!(viewer.observe(&p));
    assert_eq!(
        viewer.document().to_markdown(),
        editor.document().to_markdown(),
        "the late joiner stays converged on the subsequent live edit"
    );
}

/// **The offset-model DOM-bridge round-trips on the LIVE document (the jsdom-class bridge proof — the
/// caret coordinate the browser caret binds to IS the model coordinate, 0 off-by-one).** After a real
/// editing session, every caret position on every block round-trips offset ↔ DOM-position — the
/// offset primitive (KN-P08) composes under the integrated editor over a real document.
#[test]
fn the_dom_bridge_round_trips_on_the_live_document() {
    let mut editor = Editor::open_page(tenant(), "page-bridge", "client-A", principal("alice"))
        .expect("the page opens");
    editor.type_text(0, 0, "Severity **high**. café 日本");
    editor.append_block("- [ ] task with `code` and ~~strike~~");
    editor.split_block(0, 9);

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

/// **The whole frozen KN-D2 corpus round-trips loaded as integrated-editor document blocks (the
/// integrated path consumes the SAME corpus the `myelin-content` library gate does — 100%, 0
/// regressions).** This is the integrated-path corpus-pass-rate signal = 100%.
#[test]
fn the_kn_d2_corpus_round_trips_over_the_integrated_path() {
    let mut doc = Document::blank();
    for fixture in myelin_content::corpus::CORPUS {
        let nodes = myelin_content::corpus::synthetic_nodes_for(fixture.md);
        doc.blocks.push(EditorBlock::new(fixture.md, &nodes));
    }
    assert!(doc.block_count() >= 18, "the corpus must not be shrunk below its frozen size");
    assert!(
        doc.corpus_roundtrips(),
        "every frozen KN-D2 fixture is a fixed point loaded as an integrated-editor block (100%)"
    );
}
