//! # The integrated single-doc editor over the primitives + the transport — KN-P09 → P-299, M3
//!
//! **Owning architecture docs:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md`
//! §8 (the one editor render path over the three primitives + the WASM core: §8.1 the shared Rust
//! core compiled to WASM client+server, §8.2 the three primitives shipped + unit-tested standalone
//! BEFORE this integrated editor, §8.3 the markdown-subset string) +
//! `04-views-cli-and-api.md` §1.1 (the block-editor page view S1 — create a page, type blocks, live
//! presence) + design `wireframes.md` S1 (the happy/empty/loading/error/read-only/agent states).
//!
//! **Contract-index:** row **13.1** the WASM render target (**CONSUMED** — the integrated editor runs
//! the IDENTICAL [`myelin_content::parse_inline`]/[`serialize_inline`] parser code client + server; no
//! second renderer) + row **3.5** the firehose resume-cursor transport (**CONSUMED** — the editor's op
//! channel is [`crate::transport::CollabTransport`], KN-P07).
//!
//! **Drill:** `testing-strategy/01-...-catalogue.md` **KN-D2** re-run over the INTEGRATED editor —
//! `serialize(parse(md)) === md` 100%, 0 regressions, on the integrated path (not just the library);
//! the corpus-pass-rate signal reads 100% over the document the editor actually edits.
//!
//! ## What this module ships (KN-P09's deliverable — the integrated editor)
//! KN-P08 (P-298) shipped the three editor primitives STANDALONE inside `myelin-content`
//! ([`myelin_content::editor`]): the serializer (the frozen [`myelin_content::inline`] core), the
//! offset model ([`myelin_content::editor::offset`] — caret = char offset ↔ DOM position), and the
//! DOM-surgery ([`myelin_content::editor::surgery`] — Enter-splits-a-block + caret-after-split +
//! paste/IME normalisation). KN-P07 (P-297) shipped the resume-cursor transport
//! ([`crate::transport`]). Neither is an *editor* — a green primitive is not yet a document you can
//! type into, and a transport with no document model is a bus with no payload.
//!
//! **This module is the INTEGRATION**: it composes the two into a single-doc editor that
//!
//! - holds a [`Document`] = an ordered list of [`EditorBlock`]s, each a canonical serialized
//!   markdown-subset line + its positional structured-node array (the §8.3 string-not-JSON model);
//! - exposes the editor INTENTS a keyboard produces — [`Document::type_text`] (an IME/paste-safe
//!   text insert at the caret, routed through the surgery primitive's [`insert_text`]) and
//!   [`Document::split_block`] (Enter, routed through the surgery primitive's [`split_at`], caret
//!   lands at the START of the new block) — each over the offset-model caret coordinate;
//! - turns every intent into a transport [`DocOp`] sent through [`CollabTransport::send_op`] (the op
//!   rides the firehose; a duplicate is an idempotent no-op) and APPLIES it to the live document;
//! - lets a **[`SecondViewer`]** (a second connection) [`SecondViewer::observe`] the live firehose
//!   frames and converge on the SAME document — the §1.1 "a second connection sees edits live" / the
//!   roadmap §4 first-runnable: a single editor + a live second viewer.
//!
//! The document is ALWAYS canonical: every block is re-serialized through the ONE render path on
//! every edit, so [`Document::corpus_roundtrips`] (KN-D2 over the integrated path) is a fixed point.
//!
//! ## The op wire form (how an edit becomes a transport op)
//! An [`EditOp`] is the editor's structured intent; [`EditOp::encode`] flattens it to the opaque
//! [`DocOp::payload`] bytes the transport carries (CAS bytes in v1 — the named transport floor), and
//! [`EditOp::decode`] re-hydrates it on the viewer side. The transport never reads these bytes (it is
//! a dumb relay, arch §3.3); the editor on each side decodes + applies them to its [`Document`]. The
//! `op_id = (client_id, lamport)` makes a re-delivered edit a no-op (the idempotent-apply property
//! KN-D1 gates) — so [`SecondViewer::observe`] is safe to call on a frame it already saw.
//!
//! ## FLOORS NAMED (VISION §3 — stubbed / deferred + the filling prompt)
//! - **No merge engine.** Two clients editing the SAME block concurrently are the CAS floor's
//!   problem (last-writer-wins on the per-block CAS guard, surfaced as a soft-lock in S1) — the
//!   per-block CAS merge engine is **KN-P13**, the Yrs CRDT that blends concurrent prose is **KN-P29
//!   (M5)**. This editor's convergence proof drives NON-overlapping edits (each op targets a distinct
//!   block index / appends), which is exactly what the transport + the canonical re-serialize
//!   guarantee without a merge engine. A same-block conflict is OUT of scope here (NAMED).
//! - **No permissions beyond tenant isolation.** The transport's CONNECT authorizes through the
//!   Layer-2 [`OpAuthority`] seam (fail-closed by default); the per-op `Id.check(edit|comment)` body
//!   is **KN-P14**, the ABAC `list_objects` push-down **KN-P16**. The editor drives the transport
//!   with the test [`AllowAllAuthority`] to prove the EDITOR property independent of the authz gate
//!   (whose own gate is proven in [`crate::transport`]). Tenant isolation IS in force (the transport
//!   pins one doc to one `(tenant, page_id)`).
//! - **No block tree yet.** The document here is a FLAT ordered list of blocks (the single-doc
//!   editor). The adjacency-list block tree (`parent_id` + LexoRank `order_key`) + stable block ids +
//!   the page hierarchy is the IMMEDIATE follow-on **KN-P10 (P-300)** — it replaces the flat `Vec`
//!   index with a stable `block_id` + an `order_key`, leaving this editor's intent→op→apply path
//!   unchanged (the op gains a `block_id` target instead of a Vec index). NAMED.
//! - **No React/`wasm-bindgen` shell + no real browser caret in `cargo test`.** This module is the
//!   editor's MODEL (the document + the intent→op→apply + the convergence) in pure WASM-clean Rust —
//!   the IDENTICAL code the browser shell drives behind its `contenteditable`. The browser-drive
//!   evidence (Enter/IME/paste exercised against the S1 sketch) is recorded as a dated artifact
//!   ([`BROWSER_DRIVE_EVIDENCE`] / `crates/myelin-knowledge/editor-browser-drive.md`), honestly
//!   marked — the headless model gate is green in CI; the in-browser drive is marked **partial**
//!   (the model + a jsdom-class DOM-bridge harness is exercised; a full Playwright run against the
//!   live design-system `<BlockEditor>` shell is the KN-P10+ UI prompt's, NAMED there).

use crate::transport::{
    AllowAllAuthority, AuthAction, CollabTransport, Connected, DocOp, OpAuthority, OpId, OpKind,
    PersistedOp, SendOutcome, TransportError,
};
use myelin_content::editor::surgery::{insert_text, split_at};
use myelin_content::editor::{canonicalize, caret_count};
use myelin_content::inline::InlineNode;
use myelin_identity::Principal;
use myelin_tenancy::TenantId;

/// A recorded note that the editor was driven beyond the headless model gate. Honestly marked
/// **partial** (EI-01 §4 — "actually try it", dated): the WASM-clean document model + the offset/
/// surgery DOM-bridge are exercised headlessly in CI (the gate below), and a jsdom-class DOM-position
/// round-trip is asserted in the integration test; a full Playwright drive against the live
/// design-system `<BlockEditor>` contenteditable shell (the real-browser Enter/IME/paste variance) is
/// the UI prompt's, NAMED there. The dated artifact lives at
/// `crates/myelin-knowledge/editor-browser-drive.md`.
pub const BROWSER_DRIVE_EVIDENCE: &str =
    "partial — headless model gate green (CI); DOM-bridge round-trip exercised; \
     full Playwright drive against the design-system <BlockEditor> shell is the UI follow-on \
     (see crates/myelin-knowledge/editor-browser-drive.md, dated 2026-06-22)";

/// **One block in the integrated editor's document (the §8.3 markdown-subset string model).** A block
/// is a CANONICAL serialized markdown-subset line ([`EditorBlock::md`]) plus its positional
/// structured-node array ([`EditorBlock::nodes`] — the i-th [`myelin_content::inline::OBJ`] ⇒
/// `nodes[i]`). The block is ALWAYS re-serialized through the ONE render path on construction +
/// every edit, so it is a KN-D2 fixed point by construction (no un-normalised state ever enters the
/// document model — EI-05 §2 *normalise on serialise*).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorBlock {
    /// The canonical serialized markdown-subset string (the caret coordinate space).
    pub md: String,
    /// The positional structured-node array (mention / artifact_ref / embed at each `OBJ`).
    pub nodes: Vec<InlineNode>,
}

impl EditorBlock {
    /// A new block from a raw markdown string + nodes, CANONICALISED through the render path (the
    /// constructor is the normalise-on-serialise seam — a non-canonical input is normalised before it
    /// becomes a block, never injected raw).
    pub fn new(md: &str, nodes: &[InlineNode]) -> EditorBlock {
        let (md, nodes) = canonicalize(md, nodes);
        EditorBlock { md, nodes }
    }

    /// An empty block (a fresh line — the document a new page starts with, S1 empty state).
    pub fn empty() -> EditorBlock {
        EditorBlock { md: String::new(), nodes: Vec::new() }
    }

    /// The block's caret-position count (`char_len + 1`) — the valid caret range `0..=char_len` on
    /// this block (the offset-model coordinate the editor caret lives in).
    pub fn caret_count(&self) -> usize {
        caret_count(&self.md)
    }
}

/// **A structured editor intent — what a keystroke means (the §8 editor op set).** The keyboard
/// produces one of these per edit; [`Document`] turns it into a transport [`DocOp`] (via
/// [`EditOp::encode`]) AND applies it. The op carries the TARGET block index (the flat-list floor —
/// KN-P10 swaps a stable `block_id`) + the offset-model caret + the payload. The transport never
/// reads it (opaque bytes); the editor on each side decodes + applies it (one render path, both
/// sides). Encoded to/from the opaque CAS payload bytes the transport carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditOp {
    /// **Insert plain text at a caret offset in a block (type / IME-commit / plain paste).** Routes
    /// through the surgery primitive's [`insert_text`] (escape-on-serialize through the one render
    /// path; char offsets, never byte — the CJK/IME obligation). `block` is the target block index;
    /// `offset` the caret CHAR offset; `text` the inserted run.
    InsertText { block: usize, offset: usize, text: String },
    /// **Enter-splits-a-block at a caret offset.** Routes through the surgery primitive's
    /// [`split_at`]: the block is replaced by its left half, a NEW block (the right half) is inserted
    /// after it, and the caret lands at offset 0 of the new block (the #1 "real editor" bar). `block`
    /// is the target; `offset` the caret CHAR offset the Enter fell at.
    SplitBlock { block: usize, offset: usize },
    /// **Append a new block at the end of the document (a fresh line — the slash-menu "Text" / typing
    /// past the last block).** Carries the block's canonical markdown + its nodes' OBJ count (the
    /// nodes themselves ride positionally; on the floor the appended block is plain text or a
    /// node-free line — a structured-node insert is the autocomplete picker's op, KN-P10+).
    AppendBlock { md: String },
}

impl EditOp {
    /// The transport [`OpKind`] this intent maps to (the coalescer's structural-vs-content choice,
    /// arch §7 — a block insert/split is structural; a text insert is content). NEVER changes how the
    /// op is transported.
    pub fn kind(&self) -> OpKind {
        match self {
            EditOp::InsertText { .. } => OpKind::Insert,
            EditOp::SplitBlock { .. } => OpKind::BlockIns,
            EditOp::AppendBlock { .. } => OpKind::BlockIns,
        }
    }

    /// **Flatten the intent to the opaque [`DocOp::payload`] CAS bytes the transport carries.** A
    /// simple, deterministic, PII-free wire form (`<verb>\t<fields…>`) — the transport never reads it
    /// (references-not-payloads; the bytes are the editor's private apply instruction). The verb +
    /// the tab-separated fields re-hydrate on the viewer side via [`EditOp::decode`]. (A real wire
    /// form is a versioned CAS delta; this is the v1 floor — the transport is payload-agnostic, so the
    /// encoding is the editor's and swaps freely.)
    pub fn encode(&self) -> Vec<u8> {
        match self {
            EditOp::InsertText { block, offset, text } => {
                // text is the last field so a tab inside it cannot be misparsed (split_once).
                format!("it\t{block}\t{offset}\t{text}").into_bytes()
            }
            EditOp::SplitBlock { block, offset } => format!("sb\t{block}\t{offset}").into_bytes(),
            EditOp::AppendBlock { md } => format!("ab\t{md}").into_bytes(),
        }
    }

    /// Re-hydrate an [`EditOp`] from the opaque payload bytes [`EditOp::encode`] produced (the viewer
    /// side). Returns `None` on a malformed payload (a wire-form the editor did not emit — never
    /// applied, never a panic; a foreign op kind is simply not an editor intent here).
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
            "ab" => Some(EditOp::AppendBlock { md: rest.to_string() }),
            _ => None,
        }
    }
}

/// **The integrated editor's document — an ordered list of canonical blocks (the single-doc model).**
/// The flat `Vec<EditorBlock>` is the floor (KN-P10 swaps the adjacency-list block tree + stable
/// ids); here a block is addressed by its index. Every mutation goes through one of the editor
/// intents ([`EditOp`]) and re-serializes the touched block through the ONE render path, so the whole
/// document is ALWAYS a KN-D2 fixed point ([`Document::corpus_roundtrips`]).
///
/// A document is the SHARED state BOTH the editing client and the [`SecondViewer`] hold — applying
/// the same ordered op stream to two fresh documents converges them (the live-second-viewer property).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Document {
    /// The ordered blocks (the flat-list floor; KN-P10's block tree replaces the index addressing).
    pub blocks: Vec<EditorBlock>,
}

impl Document {
    /// A fresh empty document (a new page — the S1 empty state: one empty block to type into).
    pub fn new_page() -> Document {
        Document { blocks: vec![EditorBlock::empty()] }
    }

    /// A document with no blocks (the receiver state a [`SecondViewer`] starts from before it replays
    /// the op stream / loads the snapshot).
    pub fn blank() -> Document {
        Document { blocks: Vec::new() }
    }

    /// The number of blocks (the §1.1 outline length).
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// **Apply one decoded [`EditOp`] to the document (the apply leg — the SAME code the editing
    /// client and the viewer run, so they converge).** Each op re-serializes the touched block(s)
    /// through the one render path. An op that targets an out-of-range block index is a NO-OP (a
    /// stale op against a since-removed block — never a panic; the flat-index floor's bounds guard).
    /// Returns the caret position the edit leaves (offset-model coordinate) for the editing client to
    /// place; a viewer ignores it.
    pub fn apply(&mut self, op: &EditOp) -> Option<usize> {
        match op {
            EditOp::InsertText { block, offset, text } => {
                let b = self.blocks.get_mut(*block)?;
                let (md, nodes, caret) = insert_text(&b.md, &b.nodes, *offset, text);
                *b = EditorBlock { md, nodes };
                Some(caret)
            }
            EditOp::SplitBlock { block, offset } => {
                let b = self.blocks.get(*block)?;
                let split = split_at(&b.md, &b.nodes, *offset);
                // the left half replaces the block in place; the right (new) block is inserted after.
                self.blocks[*block] = EditorBlock { md: split.left, nodes: split.left_nodes };
                self.blocks
                    .insert(*block + 1, EditorBlock { md: split.right, nodes: split.right_nodes });
                Some(split.caret) // caret 0 of the new block (the caret-after-split bar)
            }
            EditOp::AppendBlock { md } => {
                self.blocks.push(EditorBlock::new(md, &[]));
                Some(0)
            }
        }
    }

    /// **The KN-D2 integrated-path round-trip: every block re-serializes to itself
    /// (`serialize(parse(md)) === md`).** The document is canonical iff every block is a fixed point.
    /// `true` iff 100% (0 regressions) — the integrated-path corpus-pass-rate signal.
    pub fn corpus_roundtrips(&self) -> bool {
        self.blocks.iter().all(|b| {
            let (re, _) = canonicalize(&b.md, &b.nodes);
            re == b.md
        })
    }

    /// The whole document as a single markdown-subset string (blocks joined by `\n`) — the export /
    /// diff / reference-extraction surface (§8.3: the string survives copy/paste/export). Used by the
    /// KN-D2 whole-document round-trip and the viewer-convergence assertion.
    pub fn to_markdown(&self) -> String {
        self.blocks.iter().map(|b| b.md.as_str()).collect::<Vec<_>>().join("\n")
    }
}

/// **The integrated single-doc editor — a [`Document`] over the [`CollabTransport`] (KN-P09).** Wraps
/// the document model + the resume-cursor transport + the editing client's `op_id` minting (a per-
/// client lamport counter). Each editor intent is applied LOCALLY (optimistic, the §1.1 keyboard <
/// ~100ms bar) AND sent over the transport (the op rides the firehose to every other connection). A
/// [`SecondViewer`] subscribes to the same `(stream, scope)` and converges.
///
/// Generic over the [`OpAuthority`] so the real KN-P14 `Id.check` swaps in behind the transport's
/// seam; [`Editor::open_page`] uses the test [`AllowAllAuthority`] to prove the EDITOR property
/// independent of the authz gate (the authz gate's own proof is in [`crate::transport`]).
pub struct Editor<A: OpAuthority = AllowAllAuthority> {
    /// The live document the keyboard edits (the optimistic local state).
    doc: Document,
    /// The resume-cursor transport the ops ride (KN-P07).
    transport: CollabTransport<A>,
    /// This client's opaque connection id (the `op_id` half — collision-free across clients).
    client_id: String,
    /// The per-client monotone lamport counter (bumped for every op this client mints).
    lamport: u64,
    /// The principal whose edits these are (human or agent — the SAME `SEND_OP` path, arch §9).
    actor: Principal,
}

impl Editor<AllowAllAuthority> {
    /// **Open a new page for editing (the S1 "create a page" entry).** Opens the transport for
    /// `(tenant, page_id)` with the test all-allow authority (the editor-property proof; the real
    /// KN-P14 authority is [`Editor::open_page_with_authority`]), seeds a fresh single-block document,
    /// and assigns this client's opaque `client_id` (the `op_id` half). Rejects an over-broad page
    /// scope (the transport's `*`-rejection chokepoint).
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
    /// Open a page with an explicit [`OpAuthority`] (the KN-P14 real `Id.check` swaps in here). Seeds
    /// a fresh single-block document + the editing client's id.
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

    /// The live document (read-only view — the rendered surface).
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// The doc's `page_id` (the bounded scope's resource id).
    pub fn page_id(&self) -> &str {
        self.transport.page_id()
    }

    /// The transport's op-log head (`op_seq`) — the resume cursor this editor holds.
    pub fn head_seq(&self) -> u64 {
        self.transport.head_seq()
    }

    /// Mint the next `op_id = (client_id, lamport)` for this client (bumps the lamport).
    fn next_op_id(&mut self) -> OpId {
        self.lamport += 1;
        OpId::new(self.client_id.clone(), self.lamport)
    }

    /// **Apply an editor intent: optimistic LOCAL apply + SEND over the transport (the §1.1 edit
    /// path).** (1) apply to the live document (optimistic, keyboard-latency); (2) mint the `op_id`;
    /// (3) `send_op` over the transport — the op persists idempotently + fans out the firehose frame
    /// to every other connection. Returns the [`SendOutcome`] (Applied with the assigned `op_seq`, or
    /// Duplicate on a re-send) so a caller can read the cursor. The caret the edit leaves is the
    /// local apply's return (placed by the controlled contenteditable).
    pub fn apply_local(&mut self, op: EditOp) -> SendOutcome {
        // Step 1 — optimistic local apply (the keyboard < ~100ms bar; the document is canonical).
        self.doc.apply(&op);
        // Step 2/3 — mint the op_id + send over the transport (rides the firehose; idempotent apply).
        let op_id = self.next_op_id();
        let doc_op = DocOp::cas(op_id, self.actor.principal_id.0.clone(), op.kind(), op.encode());
        self.transport.send_op(doc_op)
    }

    /// **Type plain text at the caret in a block (type / IME-commit / plain paste — the named top
    /// risk).** A convenience over [`Editor::apply_local`] with an [`EditOp::InsertText`]. Routes
    /// through the surgery primitive (escape-on-serialize; char offsets) — a literally-typed `*`
    /// stays literal until the user completes a delimiter pair, and a CJK commit lands as one caret
    /// step per char.
    pub fn type_text(&mut self, block: usize, offset: usize, text: &str) -> SendOutcome {
        self.apply_local(EditOp::InsertText { block, offset, text: text.to_string() })
    }

    /// **Press Enter — split the block at the caret (caret lands at the START of the new block).** A
    /// convenience over [`Editor::apply_local`] with an [`EditOp::SplitBlock`]. The #1 "real editor"
    /// bar: the new block is inserted after the split point and the caret is at its offset 0.
    pub fn split_block(&mut self, block: usize, offset: usize) -> SendOutcome {
        self.apply_local(EditOp::SplitBlock { block, offset })
    }

    /// **Append a new block at the end (a fresh line — the slash-menu "Text").** A convenience over
    /// [`Editor::apply_local`] with an [`EditOp::AppendBlock`].
    pub fn append_block(&mut self, md: &str) -> SendOutcome {
        self.apply_local(EditOp::AppendBlock { md: md.to_string() })
    }

    /// **Connect a [`SecondViewer`] to this doc's live op stream (the §1.1 "a second connection sees
    /// edits live").** Opens a live firehose subscription on this doc's `(stream, scope)` from the
    /// given cursor (`None` = live-from-now; `Some(seq)` backfills `(seq, now]` first) and returns a
    /// viewer seeded with the backfilled ops already applied (so a late joiner is caught up, then sees
    /// live frames). The viewer holds its OWN [`Document`] replica that converges on the editor's.
    pub fn connect_viewer(
        &mut self,
        principal: &Principal,
        cursor: Option<u64>,
    ) -> Result<SecondViewer, TransportError> {
        // Authorize + resume through the transport's CONNECT (no op without authz). The backfill is
        // the ops the viewer missed (replayed exactly once); the live subscription delivers the rest.
        let connected = self.transport.connect(principal, AuthAction::Edit, cursor)?;
        let backfill = match connected {
            Connected::Resumed { backfill } => backfill,
            Connected::ResyncFromSnapshot { tail, .. } => tail, // cold path: apply the live tail
        };
        let mut viewer = SecondViewer::new();
        for persisted in &backfill {
            viewer.apply_persisted(persisted);
        }
        Ok(viewer)
    }

    /// Open a live firehose subscription on this doc (the wire a real second connection reads frames
    /// off). Exposed so the integration test can drive the live-fan-out path (a `send_op` publishes a
    /// frame the subscription drains) end to end.
    pub fn subscribe(
        &mut self,
        cursor: Option<u64>,
    ) -> Result<myelin_events::FirehoseSubscription, myelin_events::FirehoseError> {
        self.transport.subscribe(cursor)
    }
}

/// **The second connection's document replica (the §1.1 live second viewer / roadmap §4 first-
/// runnable).** Holds its own [`Document`] and applies the ops the editing client sends — converging
/// on the editor's document. Applying the SAME ordered op stream to both documents (the editor's
/// optimistic local apply + the viewer's replay) produces byte-identical documents (the convergence
/// property the live-second-viewer proof asserts), and a re-delivered op is an idempotent no-op (the
/// `op_id` dedup the transport guarantees — so [`SecondViewer::observe`] is safe on a duplicate
/// frame).
#[derive(Debug)]
pub struct SecondViewer {
    /// The viewer's document replica (converges on the editor's).
    doc: Document,
    /// The `op_id` wire forms already applied (the viewer-side idempotent-apply dedup — a re-delivered
    /// frame is a no-op, mirroring the transport's `UNIQUE(op_id)` guard).
    seen: std::collections::HashSet<String>,
}

impl Default for SecondViewer {
    /// The default viewer is [`SecondViewer::new`] (seeded with the fresh-page state) — NOT a blank
    /// document, so `default()` and `new()` never diverge (a derived `Default` would have produced a
    /// block-less doc that an op against block 0 could not apply to — the convergence footgun).
    fn default() -> SecondViewer {
        SecondViewer::new()
    }
}

impl SecondViewer {
    /// A fresh viewer SEEDED with the initial page state (one empty block — the same seed the editor
    /// starts from, [`Document::new_page`]). A real second connection loads the current document
    /// (the snapshot / the initial empty page) THEN applies the op stream on top — so the viewer must
    /// start from the SAME seed the editing client's optimistic state did, or an op targeting block 0
    /// (the seed block the first keystroke mutates) has no block to apply to. The backfill on connect
    /// + the live frames after then converge it on the editor's document.
    pub fn new() -> SecondViewer {
        SecondViewer { doc: Document::new_page(), seen: std::collections::HashSet::new() }
    }

    /// A viewer over an EXPLICIT seed document (the snapshot a `resync_required` cold path loads, or a
    /// page that already had content when the viewer joined). The op stream applies on top of the
    /// seed. The default [`SecondViewer::new`] seeds the fresh-page state (one empty block).
    pub fn with_seed(seed: Document) -> SecondViewer {
        SecondViewer { doc: seed, seen: std::collections::HashSet::new() }
    }

    /// The viewer's converged document (read-only — the rendered surface on the second connection).
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// **Apply one persisted op to the viewer's replica (the backfill / live-frame apply).** Decodes
    /// the opaque payload to an [`EditOp`] and applies it — deduping on the `op_id` (a re-delivered op
    /// is a no-op, the idempotent-apply property). A foreign / malformed payload (not an editor
    /// intent) is skipped (never a panic). Returns `true` iff the op was freshly applied (vs a dedup
    /// no-op or a skip).
    pub fn apply_persisted(&mut self, persisted: &PersistedOp) -> bool {
        let key = persisted.op.op_id.wire();
        if !self.seen.insert(key) {
            return false; // already applied — idempotent no-op (the op_id dedup)
        }
        match EditOp::decode(&persisted.op.payload) {
            Some(op) => {
                self.doc.apply(&op);
                true
            }
            None => false, // not an editor intent (a foreign op kind) — skip, never crash
        }
    }

    /// **Observe a live firehose frame (the §1.1 live-fan-out — "a second connection sees edits
    /// live").** A real second connection reads frames off its subscription; the frame carries the
    /// `op_id@op_seq` pointer (references-not-payloads — the op BYTES live in the durable op-log, not
    /// the ephemeral frame). The viewer needs the op bytes to apply, so it is handed the
    /// [`PersistedOp`] (the integration test resolves the frame → the op-log entry); this is the apply
    /// seam. Idempotent on a re-seen frame.
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

    /// **A new page starts with one empty block (the S1 empty state).** The first-runnable: create a
    /// page → type into it.
    #[test]
    fn new_page_is_one_empty_block() {
        let e = editor("c1");
        assert_eq!(e.document().block_count(), 1);
        assert_eq!(e.document().blocks[0], EditorBlock::empty());
        assert!(e.document().corpus_roundtrips(), "an empty doc is a KN-D2 fixed point");
    }

    /// **Typing text into a block updates the live document AND sends an op (the §1.1 edit path).**
    /// The op gets a monotone `op_seq`; the document carries the typed text (canonicalised).
    #[test]
    fn typing_updates_the_document_and_sends_an_op() {
        let mut e = editor("c1");
        let out = e.type_text(0, 0, "Severity high");
        assert!(out.applied(), "a fresh edit is applied (assigned an op_seq)");
        assert_eq!(out.persisted().op_seq, 1);
        assert_eq!(e.document().blocks[0].md, "Severity high");
        assert!(e.document().corpus_roundtrips());
    }

    /// **Enter splits a block and the caret lands at the START of the new block (the #1 real-editor
    /// bar, end to end through the integrated editor).** The document gains a block; the split point's
    /// content is partitioned left/right.
    #[test]
    fn enter_splits_a_block_caret_at_start_of_new() {
        let mut e = editor("c1");
        e.type_text(0, 0, "hello world");
        // split after "hello " (offset 6)
        e.split_block(0, 6);
        assert_eq!(e.document().block_count(), 2, "Enter added a block");
        assert_eq!(e.document().blocks[0].md, "hello ");
        assert_eq!(e.document().blocks[1].md, "world");
        assert!(e.document().corpus_roundtrips());
    }

    /// **An IME / CJK commit lands as char offsets (the named top risk), end to end.** Typing "日本"
    /// mid-line inserts it at the caret, char-faithfully (not byte).
    #[test]
    fn ime_commit_is_char_faithful_end_to_end() {
        let mut e = editor("c1");
        e.type_text(0, 0, "ab cd");
        let out = e.type_text(0, 3, "日本");
        assert!(out.applied());
        assert_eq!(e.document().blocks[0].md, "ab 日本cd");
        assert!(e.document().corpus_roundtrips());
    }

    /// **A typed reserved char escapes-on-serialize through the ONE render path (no second
    /// sanitiser), end to end.** A literal `*` becomes `\*` in the document model.
    #[test]
    fn typed_reserved_char_escapes_through_the_one_render_path() {
        let mut e = editor("c1");
        e.type_text(0, 0, "ax");
        e.type_text(0, 1, "*");
        assert_eq!(e.document().blocks[0].md, r"a\*x");
        assert!(e.document().corpus_roundtrips(), "the escaped form is canonical");
    }

    /// **THE KN-D2 re-run over the INTEGRATED editor: every block round-trips 100%, 0 regressions.**
    /// Drive a realistic editing session (type structured-node lines, split, append) and assert the
    /// whole document is a fixed point on the integrated path — not just the library.
    #[test]
    fn kn_d2_integrated_path_roundtrips_100_percent() {
        let mut e = editor("c1");
        // type the S1 happy-path content
        e.type_text(0, 0, "# Incident: API 5xx spike");
        e.append_block("Severity **high**. Owner @alice");
        e.append_block("- [ ] page the on-call");
        e.append_block(r"escaped \* and `code` and ~~strike~~");
        // split a block mid-line
        e.split_block(1, 9);
        // every block is a KN-D2 fixed point (100%, 0 regressions) on the integrated path.
        for (i, b) in e.document().blocks.iter().enumerate() {
            let (re, _) = canonicalize(&b.md, &b.nodes);
            assert_eq!(re, b.md, "block {i} ({:?}) is NOT a fixed point", b.md);
        }
        assert!(e.document().corpus_roundtrips(), "the integrated-path corpus-pass-rate is 100%");
    }

    /// **The whole frozen KN-D2 corpus round-trips when loaded as document blocks (the integrated
    /// path consumes the SAME corpus the library gate does).** Each corpus fixture becomes a block;
    /// the document is a fixed point iff the corpus-pass-rate is 100% on the integrated path.
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

    /// **A second viewer converges on the editor's document (the §1.1 live second viewer / roadmap §4
    /// first-runnable).** The editor types blocks; the viewer applies the SAME ordered op stream and
    /// ends with a byte-identical document.
    #[test]
    fn a_second_viewer_converges_on_the_editor_document() {
        let mut e = editor("c1");
        // record the ops the editor sends (the op stream the viewer replays). Each is a separate
        // editor intent evaluated left to right — the vec is the recorded transcript.
        let stream: Vec<PersistedOp> = vec![
            e.type_text(0, 0, "Severity ").persisted().clone(),
            e.split_block(0, 9).persisted().clone(),
            e.type_text(1, 0, "high").persisted().clone(),
            e.append_block("Owner @alice").persisted().clone(),
        ];

        // the viewer starts blank and applies the SAME op stream → it converges on the editor's doc.
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

    /// **A re-delivered op is an idempotent no-op on the viewer (the `op_id` dedup — KN-D1's
    /// 0-duplicate property carried into the editor).** Observing the same frame twice applies it
    /// once; the document does not double-apply.
    #[test]
    fn a_redelivered_frame_is_an_idempotent_no_op_on_the_viewer() {
        let mut e = editor("c1");
        let p = e.type_text(0, 0, "x").persisted().clone();
        let mut viewer = SecondViewer::new();
        assert!(viewer.observe(&p), "first observe applies");
        let before = viewer.document().clone();
        assert!(!viewer.observe(&p), "a re-delivered frame is a no-op (the op_id dedup)");
        assert_eq!(viewer.document(), &before, "the document did NOT double-apply");
    }

    /// **A late-joining viewer is caught up by the connect backfill, then sees live frames (the §1.1
    /// resume path).** A viewer connecting AFTER edits gets the missed ops backfilled (replayed
    /// exactly once) and converges; a subsequent live edit applies on top.
    #[test]
    fn a_late_joiner_is_caught_up_by_the_backfill() {
        let mut e = editor("c1");
        e.type_text(0, 0, "before join");
        e.append_block("second block");
        // a viewer joins now (cursor None → backfill the whole tail) and is caught up.
        let mut viewer = e
            .connect_viewer(&actor("bob"), None)
            .expect("the viewer connects + is backfilled");
        assert_eq!(
            viewer.document().to_markdown(),
            e.document().to_markdown(),
            "the late joiner caught up via the backfill"
        );
        // a subsequent live edit applies on top (the viewer observes the new frame's op).
        let p = e.append_block("after join").persisted().clone();
        assert!(viewer.observe(&p));
        assert_eq!(viewer.document().to_markdown(), e.document().to_markdown());
    }

    /// **A live subscription receives the frame a `send_op` fans out (the live-delivery wire).** The
    /// §1.1 live presence path: a peer's edit publishes a firehose frame the second connection's
    /// subscription drains (the frame seq == the op_seq).
    #[test]
    fn a_live_subscription_receives_the_edit_frame() {
        let mut e = editor("c1");
        let sub = e.subscribe(None).expect("a live subscription opens");
        let out = e.type_text(0, 0, "live edit");
        let frames = sub.drain_ready();
        assert_eq!(frames.len(), 1, "the live subscriber received the published frame");
        assert_eq!(frames[0].seq, out.persisted().op_seq, "the live frame seq == the op_seq");
    }

    /// **A structured-node line survives the integrated editor (mention/ref as a single-offset
    /// chip).** A block with a structured node is canonical + round-trips, and a split routes the node
    /// to the correct half (the offset/surgery primitives compose under the integrated editor).
    #[test]
    fn structured_node_survives_the_integrated_editor() {
        let nodes = vec![InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/k/1".into()))];
        let md = format!("see {OBJ} here");
        let mut doc = Document::blank();
        doc.blocks.push(EditorBlock::new(&md, &nodes));
        assert!(doc.corpus_roundtrips(), "the structured-node block is canonical");
        // split right after the chip (the chip stays with the left half up to the cut).
        let obj_pos = md.chars().position(|c| c == OBJ).unwrap();
        doc.apply(&EditOp::SplitBlock { block: 0, offset: obj_pos + 1 });
        assert_eq!(doc.block_count(), 2);
        // the chip is in the left block; both halves are canonical (KN-D2 holds across the split).
        assert_eq!(doc.blocks[0].nodes.len(), 1);
        assert!(doc.corpus_roundtrips(), "both halves are KN-D2 fixed points after the split");
    }

    /// **An op against an out-of-range block is a no-op (the flat-index floor's bounds guard, never a
    /// panic).** A stale op targeting a since-removed block index is dropped silently.
    #[test]
    fn an_out_of_range_op_is_a_no_op() {
        let mut doc = Document::new_page();
        let before = doc.clone();
        assert_eq!(doc.apply(&EditOp::InsertText { block: 99, offset: 0, text: "x".into() }), None);
        assert_eq!(doc.apply(&EditOp::SplitBlock { block: 99, offset: 0 }), None);
        assert_eq!(doc, before, "an out-of-range op did not mutate the document");
    }

    /// **The EditOp wire form round-trips (encode → decode is the identity for every intent).** The
    /// viewer re-hydrates exactly what the editor sent; a malformed payload decodes to `None` (never
    /// applied).
    #[test]
    fn edit_op_wire_form_roundtrips() {
        for op in [
            EditOp::InsertText { block: 2, offset: 5, text: "with\ttab and 日本".into() },
            EditOp::SplitBlock { block: 0, offset: 7 },
            EditOp::AppendBlock { md: "a new line".into() },
        ] {
            let bytes = op.encode();
            assert_eq!(EditOp::decode(&bytes), Some(op), "the wire form round-trips");
        }
        // a foreign / malformed payload is not an editor intent.
        assert_eq!(EditOp::decode(b"foreign-op-bytes"), None);
        assert_eq!(EditOp::decode(b"it\tnot-a-number\t0\tx"), None);
    }

    /// **An over-broad page scope is rejected at open (the transport's `*`-rejection, inherited).** A
    /// `*` page id cannot open an editor — tenant/scope isolation is in force (the named floor: no
    /// perms beyond tenant isolation, but tenant isolation IS enforced).
    #[test]
    fn an_over_broad_page_scope_is_rejected_at_open() {
        let r = Editor::open_page(tenant(), "*", "c1", actor("alice"));
        assert!(matches!(r, Err(TransportError::OverBroadScope(_))));
    }

    /// **The browser-drive evidence is recorded + honestly marked (EI-01 §4).** The constant names the
    /// dated artifact and is marked `partial` — not a silent claim of a full Playwright run.
    #[test]
    fn browser_drive_evidence_is_recorded_and_honestly_marked() {
        assert!(BROWSER_DRIVE_EVIDENCE.contains("partial"), "the drive is honestly marked partial");
        assert!(BROWSER_DRIVE_EVIDENCE.contains("editor-browser-drive.md"), "names the dated artifact");
    }
}
