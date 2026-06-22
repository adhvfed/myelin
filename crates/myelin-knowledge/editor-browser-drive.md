# Integrated editor — browser-drive evidence (KN-P09 / P-299, KN-D2 re-run + the switch-test drive)

**Dated:** 2026-06-22 · **Marked:** **PARTIAL** (honest, EI-01 §4 — "actually try it").

This is the recorded driving evidence for the integrated single-doc editor (KN-P09): the
`render(parse(md)) === md` round-trip re-run over the **integrated** path (not just the
`myelin-content` library) and the Enter / IME / paste "this isn't a real editor" tells exercised
against the S1 block-editor design sketch
(`planning/04-subsystem-architectures/knowledge-platform/design/wireframes.md` S1).

## What was driven (green, in CI)

The integrated editor's **model** — the `myelin_knowledge::editor::Document` over the KN-P08
primitives (`myelin_content::editor` offset model + DOM-surgery) + the KN-P07 transport
(`myelin_knowledge::transport::CollabTransport`) — is the IDENTICAL WASM-clean Rust the browser shell
drives behind its controlled `contenteditable`. It is exercised headlessly, every run:

- **Create a page → type blocks** (`Editor::open_page` → `type_text` / `append_block`): the S1
  happy-path content ("# Incident: API 5xx spike", "Severity **high**. Owner @alice", a to-do, a
  code/strike line) is typed and the document is canonical.
- **KN-D2 re-run over the integrated path** (`Document::corpus_roundtrips`,
  `kn_d2_integrated_path_roundtrips_100_percent`, `kn_d2_corpus_loads_as_document_blocks_100_percent`):
  every block is a `serialize(parse(md)) === md` fixed point — **100%, 0 regressions** over the frozen
  corpus loaded as integrated-editor blocks (≥ 18 fixtures).
- **Enter splits a block, caret at the START of the new block** (`enter_splits_a_block_caret_at_start_of_new`):
  the #1 real-editor bar, end to end through the integrated editor.
- **IME / CJK commit is char-faithful** (`ime_commit_is_char_faithful_end_to_end`): typing "日本"
  mid-line lands as char offsets, never byte offsets.
- **Paste / typed reserved char normalises on serialize** (`typed_reserved_char_escapes_through_the_one_render_path`):
  a literal `*` becomes `\*` through the ONE render path (no second sanitiser).
- **A second connection sees edits live** (`a_second_viewer_converges_on_the_editor_document`,
  `a_live_subscription_receives_the_edit_frame`, `a_late_joiner_is_caught_up_by_the_backfill`): the
  `SecondViewer` converges on the editor's document over the transport's firehose op stream; a
  re-delivered frame is an idempotent no-op (the `op_id` dedup); a late joiner is caught up by the
  CONNECT backfill. This is the roadmap §4 first-runnable: a single editor + a live second viewer.
- **A jsdom-class DOM-position round-trip** (the integration test, `tests/integration_kn_p09_integrated_editor.rs`):
  the offset-model `offset_to_dom` ↔ `dom_to_offset` bridge round-trips on the live document's blocks
  (0 off-by-one), proving the caret coordinate the browser caret binds to is the model coordinate.

## What is NOT yet driven (the NAMED partial)

A full **Playwright** drive against the live design-system `<BlockEditor>` `contenteditable` shell —
real Chromium/Firefox caret variance, a real IME composition event, a real paste-from-Word — is the
**UI follow-on prompt's** (the React/`wasm-bindgen` shell wraps these SAME offset/surgery free
functions; the shell + its Playwright drive is named in KN-P10+ / the design-system editor surface
prompt). This module ships the editor MODEL + the headless gate; the in-browser caret is therefore
honestly marked **partial** — the model is proven, the live-browser drive is the UI prompt's.

The mutation floor + the KN-D2 corpus gate carry the correctness bar regardless of the rendering
shell (KN-4 — the correctness-bar-regardless-of-engine thesis): a shell swap cannot regress the
round-trip or the convergence, because both run on this identical compiled model.
