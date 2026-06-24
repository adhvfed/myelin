//! # `content` — the message body over the FROZEN `myelin-content` Chat SUBSET +
//! the inline-node → `refs.edge.created` producer (CHAT-P11 / P-405, M4-C3)
//!
//! This is M4-C3's **content-core slice**: it makes a chat message's **body** a real
//! [`myelin_content`] document — the FROZEN markdown-subset string + the three positional
//! structured inline nodes ([`InlineNode::Mention`] / [`InlineNode::ArtifactRefNode`] /
//! [`InlineNode::Embed`], contract 13.1, X-2/OQ-B) — and emits the reference edges those
//! structured nodes produce **uniformly via the outbox** (`refs.edge.created`, contract 5.4).
//! Every inline run round-trips `render(parse(md)) === md` through the **ONE WASM render path**
//! (the SAME [`myelin_content::parse_inline`] / [`myelin_content::serialize_inline`] compiled
//! native on the server and to `wasm32-unknown-unknown` for the editor — there is no second
//! renderer, so the two-divergent-renderers trap is eliminated structurally, EI-01 §7).
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `../../VISION.md` §3 (design-before-implementation; content round-trips through the one editor
//!   render path) + `external-insights/01-process-and-quality-doctrine.md` §3 (the
//!   `render(parse(md)) === md` round-trip is a quantified gate).
//! - `04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md` §1.4 (the message body
//!   = a markdown-subset string `body_inline` + the positional `body_nodes` array + the three
//!   structured nodes) + `04-views-cli-and-api.md` §1 (the ONE editor render path) +
//!   `03-events-contracts-and-glue.md` §1.1 (`chat.message.edited` carries the new `edited_seq` — the
//!   per-message CAS lands in CHAT-P12, NOT here).
//! - `00-reconciliation-decisions.md` §X-2 (the frozen `myelin-content` taxonomy + the WASM compile
//!   target; Chat consumes a strict SUBSET; neither Chat nor Issues adds a node type).
//! - `contract-index.md` rows **13.1** (the Chat subset + the WASM render core; `render(parse(md))
//!   === md`) + **5.4** (`refs.edge.created` — the three inline ref nodes are the producers; emitted
//!   via the outbox, no standalone edge-write API).
//!
//! ## What this prompt (CHAT-P11 / P-405) ships
//! 1. [`CHAT_EXCLUDED_BLOCKS`] / [`is_chat_block`] / [`validate_subtree`] — the **consumed Chat
//!    SUBSET** of the frozen contract-13.1 [`Block`] taxonomy. Chat consumes a STRICT subset (X-2): it
//!    never adds a node type, and it never AUTHORS the three Knowledge-only blocks
//!    (`db_view`/`sync_block`/`toggle`). The validator walks the subtree (recursing into the container
//!    blocks) and REJECTS any excluded node LOUDLY — a [`SubsetError`], never a silent drop (EI-01 §2).
//! 2. [`MessageBody`] — a chat message body as a `myelin-content` block subtree (a `Vec<Block>` from
//!    the Chat subset) whose every inline run round-trips `render(parse(md)) === md` through the ONE
//!    [`myelin_content::wasm`] render path ([`MessageBody::round_trips`]). NO CAS here — chat is
//!    single-author and the per-message version (`edited_seq`) is CHAT-P12's (a named floor below).
//! 3. [`extract_body_edges`] / [`emit_body_edges`] — the **Chat-owned producer half** of contract 5.4:
//!    each structured ref node maps to EXACTLY ONE `refs.edge.created` by **matching the structured
//!    enum variant**, never a regex over prose (the reliability guarantee, EI-04 §2.4: a `@` in a code
//!    span is not an edge; a structured `mention` node is). The frozen X-2 uniform mapping
//!    (`mention → mentions`, `artifact_ref → links`, `embed → embeds`) emits the byte-identical
//!    `refs.edge.created` wire shape the Refs edge-builder ingests, **in the SAME outbox transaction**
//!    as the body's `chat.message.*` content event (emit-iff-committed — no edge without its message,
//!    no message without its edge).
//!
//! ## Why a thin Chat consumer over the frozen shared crate (EI-01 §7 — reuse, never duplicate)
//! The block/inline AST, the markdown-subset grammar, the ONE WASM render path, the round-trip
//! invariant, AND the content-node → edge extraction are ALREADY frozen in [`myelin_content`]
//! (Knowledge LEADS + freezes the taxonomy, KN-P01) and proven by sibling producers
//! ([`myelin_git::body`], `myelin_issues::content`). This module does NOT re-define a single node type
//! and does NOT author a second renderer or a second edge vocabulary — it LINKS the frozen
//! [`Block`]/[`Inline`]/[`InlineNode`] + calls the ONE [`myelin_content::wasm`] render entry points
//! (the EXACT seam the editor's WASM glue calls) + emits the byte-identical `refs.edge.created` wire
//! shape (`source`/`target`/`rel`/`rel_class` + the shared `edge:<source>-><target>` aggregate). The
//! encoding equivalence with the Refs seam is PINNED by the CDC
//! (`tests/cdc_5_4_13_1_chat_content_edges.rs`). The same posture as [`myelin_git::body`] — a producer
//! LEAF that owns the producer half because it cannot depend on the Refs SERVICE crate (the §2.9
//! acyclic DAG); extend/reconcile in place, never a parallel second implementation.
//!
//! ## Named floors (VISION §3 / EI-01 §1)
//! - **The per-message CAS (`edited_seq`) is CHAT-P12 / P-406, NOT here.** Chat is single-author per
//!   message (NOT a CRDT — `03-events-contracts-and-glue.md` §1.1 / OQ "chat is single-author; n/a
//!   follow-on"); an edit re-stamps the body and the `chat.message.edited` event carries the new
//!   `edited_seq`. This module is the **document + the round-trip + the edge-producer logic**; the
//!   optimistic-concurrency CAS over the body is the composer slice's (CHAT-P12). Named so the body
//!   document is not mistaken for the concurrency-controlled stored row.
//! - **The at-rest per-subject-DEK body ciphertext** (`body_inline` / `body_nodes` sealed under the
//!   author's per-subject DEK, contract 11.4 — `erasure = CryptoShred`) is the storage layer's
//!   ([`crate::dek`], CHAT-P6). [`MessageBody`] is the **cleartext in-memory document** the round-trip,
//!   the subset validation, and the edge extraction run over; the live OLTP body columns + the DEK
//!   seal/unseal ride [`crate::store`]. Named so the cleartext document is not mistaken for the at-rest
//!   form.
//! - **The strict-subset FLOOR (X-2).** Chat consumes a strict SUBSET; chat must NOT add a node
//!   outside the frozen subset. A needed change is a whole-workspace contract PR, escalated — NEVER a
//!   local extension. Stated so no agent extends the subset locally (the [`validate_subtree`] reject
//!   is the structural guard).

use myelin_content::{
    parse_inline, serialize_inline, wasm, Block, Cell, Column, Inline, InlineNode, ListItem,
};
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType, OutboxTx,
    Result as BusResult, Visibility,
};
use myelin_identity::Principal;

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE CONSUMED CHAT SUBSET of the frozen contract-13.1 Block taxonomy (X-2)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The three Knowledge-only block variants Chat does NOT author (X-2).** Chat consumes a STRICT
/// subset of the frozen taxonomy: paragraph/heading(1..3)/bullet_list/ordered_list/task_list/
/// blockquote/code_block/callout/table/divider/image + the three inline ref nodes — the full block
/// set MINUS `db_view`/`sync_block`/`toggle` (arch §1.4; recon §X-2 — "Chat … excludes `db_view,
/// sync_block, toggle`"). The frozen [`Block`] enum is owned by Knowledge and is NOT redefined here
/// (EI-01 §7); this is the Chat-side admission policy over it. A `&'static str` name per excluded
/// variant so a [`SubsetError`] names the offender, never a literal.
pub const CHAT_EXCLUDED_BLOCKS: [&str; 3] = ["db_view", "sync_block", "toggle"];

/// Why a block subtree is NOT a valid Chat message body — it carries a Knowledge-only node Chat's
/// subset excludes (X-2). LOUD + typed: an excluded node is REJECTED, never silently dropped (EI-01
/// §2 — silent data loss outranks every feature). Carries the excluded variant's frozen name (for the
/// audit / the composer's error surface).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubsetError {
    /// The excluded variant's frozen name (one of [`CHAT_EXCLUDED_BLOCKS`]).
    pub excluded: &'static str,
}

impl std::fmt::Display for SubsetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "block `{}` is Knowledge-only and not in the Chat content subset (X-2) — rejected, not dropped",
            self.excluded
        )
    }
}

impl std::error::Error for SubsetError {}

/// Whether a single block is in the **consumed Chat subset** (X-2). `true` for every variant EXCEPT
/// the three Knowledge-only ones ([`CHAT_EXCLUDED_BLOCKS`]). This is a shallow check on the block's
/// own variant; container recursion is [`validate_subtree`]'s job (a `db_view` nested inside an
/// admitted `blockquote` is still rejected by the recursive walk).
pub fn is_chat_block(block: &Block) -> bool {
    !matches!(
        block,
        Block::DbView { .. } | Block::SyncBlock { .. } | Block::Toggle { .. }
    )
}

/// The excluded-variant name of a block, if it is one of the three Knowledge-only nodes (else
/// `None`). Used to build the [`SubsetError`] that names the offender.
fn excluded_name(block: &Block) -> Option<&'static str> {
    match block {
        Block::DbView { .. } => Some("db_view"),
        Block::SyncBlock { .. } => Some("sync_block"),
        Block::Toggle { .. } => Some("toggle"),
        _ => None,
    }
}

/// **Validate a block subtree is entirely within the consumed Chat subset (X-2) — recursively.**
///
/// Walks the WHOLE tree (recursing into the container blocks — `blockquote`/`callout` blocks, list
/// items, table cells), rejecting the FIRST Knowledge-only node it finds with a LOUD [`SubsetError`]
/// (never a silent drop — EI-01 §2). A subtree of only admitted variants returns `Ok(())`. This is the
/// Chat-side admission policy over the frozen [`Block`] taxonomy; it never mutates the tree and never
/// re-defines a node (X-2 — Chat consumes a strict subset, it does not author a new one). The
/// recursion mirrors `myelin_issues::content::validate_subtree` (Issues consumes the IDENTICAL block
/// subset, CR-9) — one admission policy, no drift.
pub fn validate_subtree(blocks: &[Block]) -> Result<(), SubsetError> {
    for block in blocks {
        if let Some(excluded) = excluded_name(block) {
            return Err(SubsetError { excluded });
        }
        // Recurse into the container blocks — a Knowledge-only node nested inside an admitted
        // container is still out of subset (the check is over the WHOLE subtree, not just the roots).
        match block {
            Block::Blockquote { blocks } | Block::Callout { blocks, .. } => {
                validate_subtree(blocks)?;
            }
            Block::BulletList { items } | Block::OrderedList { items, .. } => {
                for ListItem { blocks } in items {
                    validate_subtree(blocks)?;
                }
            }
            Block::Table { rows, columns } => {
                // columns carry inline headers (no nested blocks); cells carry block subtrees.
                let _ = columns; // headers are inline-only — no recursion needed.
                for row in rows {
                    for Cell { blocks } in row {
                        validate_subtree(blocks)?;
                    }
                }
            }
            // The remaining admitted variants carry only inline / leaf content (no nested Block
            // children to recurse into): paragraph/heading/task_list (inline), code_block/divider/
            // image/embed (leaf). They cannot smuggle an excluded node.
            _ => {}
        }
    }
    Ok(())
}

/// Collect every inline run in a block subtree (in document order) — the [`Inline`] values the ONE
/// WASM render path round-trips. Walks the container blocks recursively, gathering: paragraph/heading
/// inlines, task-item inlines, table column headers + cell subtrees, image captions, blockquote/
/// callout subtrees. `code_block.text` is RAW (not markdown-parsed, §2.1) so it is NOT an [`Inline`]
/// run — it round-trips as verbatim bytes, not through the inline grammar. The collected inlines are
/// what [`MessageBody::round_trips`] proves `render(parse(md)) === md` over.
fn inline_runs(blocks: &[Block]) -> Vec<&Inline> {
    let mut out = Vec::new();
    collect_inlines(blocks, &mut out);
    out
}

fn collect_inlines<'a>(blocks: &'a [Block], out: &mut Vec<&'a Inline>) {
    for block in blocks {
        match block {
            Block::Paragraph { inline } | Block::Heading { inline, .. } => out.push(inline),
            Block::TaskList { items } => {
                for item in items {
                    out.push(&item.inline);
                }
            }
            Block::Blockquote { blocks } | Block::Callout { blocks, .. } => {
                collect_inlines(blocks, out)
            }
            Block::BulletList { items } | Block::OrderedList { items, .. } => {
                for ListItem { blocks } in items {
                    collect_inlines(blocks, out);
                }
            }
            Block::Table { columns, rows } => {
                for Column { header } in columns {
                    out.push(header);
                }
                for row in rows {
                    for Cell { blocks } in row {
                        collect_inlines(blocks, out);
                    }
                }
            }
            Block::Image {
                caption: Some(caption),
                ..
            } => out.push(caption),
            // code_block.text is RAW verbatim (NOT an inline run); divider/image-without-caption/
            // embed carry no inline grammar; db_view/sync_block/toggle are out of subset (rejected
            // by validate_subtree before this is ever reached).
            _ => {}
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE MESSAGE BODY — a chat message body as a block subtree over the frozen Chat subset
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A chat **message body** as a frozen [`myelin_content`] **block subtree** (the consumed Chat
/// SUBSET, X-2). This is the cleartext in-memory document the round-trip + subset validation + edge
/// extraction run over (the at-rest per-subject-DEK `body_inline`/`body_nodes` ciphertext is the
/// storage layer's — a named floor).
///
/// **No CAS here (chat is single-author).** The per-message version (`edited_seq`) + the
/// `chat.message.edited` re-stamp is CHAT-P12's (a named floor); this document is the round-trip + the
/// edge-producer logic, NOT the concurrency-controlled stored row. Chat is single-author per message
/// (NOT the Knowledge CRDT, NOT a multi-author merge — arch §1.1 / OQ).
///
/// The `blocks` field is the block subtree (the consumed Chat subset — paragraph/heading/lists/
/// task_list/blockquote/code_block/callout/table/divider/image + the three inline ref nodes); it is
/// validated against the subset by [`MessageBody::new`]. Every inline run round-trips `render(parse(md))
/// === md` through the ONE WASM render path ([`MessageBody::round_trips`]).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MessageBody {
    /// The block subtree (the consumed Chat subset, X-2). Validated on construction.
    pub blocks: Vec<Block>,
}

impl MessageBody {
    /// Build a new message body from a block subtree. The subtree is VALIDATED against the consumed
    /// Chat subset (X-2) — a Knowledge-only node is rejected LOUDLY ([`SubsetError`]), never admitted.
    /// The round-trip invariant ([`MessageBody::round_trips`]) holds iff every inline run's `md` is
    /// canonical.
    pub fn new(blocks: Vec<Block>) -> Result<MessageBody, SubsetError> {
        validate_subtree(&blocks)?;
        Ok(MessageBody { blocks })
    }

    /// An empty body (no blocks) — a message with no rendered content yet. Round-trips trivially (0
    /// inline runs ⇒ 0 round-trip work) and is always in-subset.
    pub fn empty() -> MessageBody {
        MessageBody::default()
    }

    /// **Parse → serialize EVERY inline run through the ONE WASM render path, asserting
    /// `render(parse(md)) === md` (contract 13.1; the chat instance of KN-D2 — the content-core
    /// round-trip gate).** `true` iff re-serialising every inline run reproduces its canonical `md`
    /// byte-identically. Read + send use the IDENTICAL parser (the [`myelin_content::wasm`] entry
    /// points the composer's WASM glue calls — there is no Chat-local renderer, EI-01 §7). A corpus of
    /// message bodies round-tripping at 100% is the CI gate ([`crate::content::tests`] +
    /// `tests/roundtrip_chat_bodies.rs`).
    pub fn round_trips(&self) -> bool {
        inline_runs(&self.blocks)
            .iter()
            .copied()
            .all(roundtrips_inline)
    }

    /// The structured ref nodes across the WHOLE subtree (the `mention`/`artifact_ref`/`embed`
    /// producers of `refs.edge.created`, contract 5.4) — a node-array walk over every inline run,
    /// NEVER a regex over the prose (the reliability guarantee, EI-04 §2.4). This is the seam
    /// [`extract_body_edges`] reads.
    pub fn structured_nodes(&self) -> Vec<&InlineNode> {
        inline_runs(&self.blocks)
            .into_iter()
            .flat_map(|inline| inline.structured_nodes().iter())
            .collect()
    }
}

/// Round-trip ONE inline run through the ONE WASM render path: re-derive the canonical `md` from the
/// stored [`Inline`] via [`serialize_inline`], re-parse it via [`myelin_content::wasm::render_parse`]
/// (the SAME entry the composer's WASM glue calls) using the run's own node array, then re-serialise
/// via [`myelin_content::wasm::render_serialize`] — and assert the result equals the canonical `md`.
/// `render(parse(md)) === md` (contract 13.1). The node array is positional (the i-th `OBJ` ↔
/// `nodes[i]`).
fn roundtrips_inline(inline: &Inline) -> bool {
    let md = serialize_inline(inline);
    let reparsed = wasm::render_parse(&md, &inline.nodes);
    wasm::render_serialize(&reparsed) == md
}

/// Round-trip a RAW markdown-subset string (+ its positional node array) through the ONE WASM render
/// path: `render_serialize(render_parse(md, nodes)) == md`. This is the composer's exact entry — the
/// corpus gate (`tests/roundtrip_chat_bodies.rs`) feeds hand-authored chat-message markdown through
/// THIS function, so the proof is over the identical code path the client composer compiles to
/// `wasm32-unknown-unknown` (EI-01 §7 — one renderer). Returns `true` iff the raw `md` is a byte-exact
/// fixed point (i.e. `md` is canonical).
pub fn roundtrips_md(md: &str, nodes: &[InlineNode]) -> bool {
    let parsed = wasm::render_parse(md, nodes);
    wasm::render_serialize(&parsed) == md
}

/// Parse a markdown-subset body string into a single-paragraph message body (the composer's simplest
/// shape): one [`Block::Paragraph`] wrapping the parsed [`Inline`]. A plain message body is
/// `[paragraph{inline}]`; richer bodies (headings/lists/tables) are built by the composer from the
/// same subset. Uses the ONE [`parse_inline`] (no Chat-local parser). Always in-subset (a paragraph is
/// admitted).
pub fn paragraph_body(md: &str, nodes: Vec<InlineNode>) -> MessageBody {
    let inline = parse_inline(md, &nodes);
    MessageBody {
        blocks: vec![Block::Paragraph { inline }],
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. THE CONTENT-NODE → refs.edge.created PRODUCER (contract 5.4 — Chat-owned producer half)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The frozen `rel` column token a structured content node produces (contract 5.4 / Refs §4.1 — the
/// `{mentions, links, embeds}` vocabulary). PII-free token. Each of the three structured inline nodes
/// maps to **exactly one** rel — the uniform X-2 producer (byte-identical across Chat/Issues/Knowledge/
/// Git). Mirrors `myelin_refs_service::emit::EdgeRel` + `myelin_git::body::EdgeRel` byte-for-byte (the
/// encoding equivalence is pinned by the CDC) — Chat produces the SAME wire tokens the Refs consumer
/// ingests, it does not author a second vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeRel {
    /// `mention(Principal)` → `mentions` (the @-mention reference edge).
    Mentions,
    /// `artifact_ref(ArtifactRef)` → `links` (the inline reference edge).
    Links,
    /// `embed(ArtifactRef)` → `embeds` (the inline embed/unfurl reference edge).
    Embeds,
}

impl EdgeRel {
    /// The frozen `rel` column token (`'mentions' | 'links' | 'embeds'`, contract 5.4).
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeRel::Mentions => "mentions",
            EdgeRel::Links => "links",
            EdgeRel::Embeds => "embeds",
        }
    }
}

/// The frozen `rel_class` token a CONTENT-NODE edge carries (contract 5.4 / Refs §3.2). A content-node
/// reference edge is ALWAYS `reference` (Refs-authoritative); the `lifecycle` class is the TE-7
/// typed-edge mirror's (a DISTINCT producer). A `&'static str` constant so the drills assert against
/// the token, never a literal — byte-identical to `myelin_git::body::REL_CLASS_REFERENCE` /
/// `RelClass::Reference.as_str()`.
pub const REL_CLASS_REFERENCE: &str = "reference";

/// The frozen `refs.edge.created` event type (contract 5.4 — the emit-side token). The ONLY
/// edge-creation event a content-node producer emits. A named constant so drills assert against the
/// NAME, never a literal (EI-01 §3). Byte-identical to `myelin_git::body::REFS_EDGE_CREATED` /
/// `myelin_refs_service::emit::REFS_EDGE_CREATED`.
pub const REFS_EDGE_CREATED: &str = "refs.edge.created";

/// **One extracted reference edge** from a message body's structured node — the `(source, target,
/// rel)` triple (`rel_class = reference`, always). The deterministic `edge_id = hash(tenant, source,
/// target, rel)` is the CONSUMER's (the Refs edge-builder derives it from the payload triple); here
/// the producer ships the triple. PII-free: `source`/`target` are opaque `ArtifactRef` URNs (a
/// mention's target is the PSEUDONYMOUS `member` URN, never a name — erasure-safe). Mirrors
/// `myelin_git::body::BodyEdge` / `myelin_refs_service::emit::EdgeDraft` (the encoding equivalence is
/// CDC-pinned).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyEdge {
    /// The referencing side — the chat message body URN this node lives in (the same for every node
    /// in one body). The full `#sub`-precise URN (the `#message-<id>` sub-URN, [`crate::subs`]).
    pub source: ArtifactRef,
    /// The referenced side — the artifact the structured node points at (the mention's pseudonymous
    /// `member` URN, or the `artifact_ref`/`embed` target URN).
    pub target: ArtifactRef,
    /// The relation token this node kind produces (`mentions`/`links`/`embeds`).
    pub rel: EdgeRel,
}

/// **The shared edge-aggregate-key convention `edge:<source>-><target>` (EB-03 ordering anchor).**
/// Every `refs.edge.*` event for ONE logical edge shares this aggregate, so an edge's create → remove
/// → create sequence is per-aggregate ordered (gap-free, in commit order). Byte-identical to
/// `myelin_git::body::edge_aggregate_key` / `myelin_refs_service::emit::edge_aggregate_key` — Chat's
/// content-node edges share the SAME ordering aggregate the Refs consumer + the other producers use
/// (one ordering key across producers). PII-free.
pub fn edge_aggregate_key(source: &ArtifactRef, target: &ArtifactRef) -> AggregateKey {
    AggregateKey(format!("edge:{}->{}", source.0, target.0))
}

/// The canonical `member` URN for a mentioned principal (`myelin://<tenant>/identity/member/<id>` —
/// the §6.2 `identity`/`member` token pair). The mention target is the principal's PSEUDONYMOUS opaque
/// `principal_id` as an `ArtifactRef`, NEVER the name — so a mention edge is erasure-safe (the name
/// lives behind Identity's pseudonym map; the mention-shred to `[erased user]` is CHAT-P23 / P-417).
/// Byte-identical to `myelin_git::body::principal_member_ref` / the Refs seam's `principal_member_ref`.
fn principal_member_ref(p: &Principal) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/identity/member/{}",
        p.tenant.0, p.principal_id.0
    ))
}

/// **Extract one reference edge per structured ref node (the contract-5.4 producer; structured, NOT
/// regex).**
///
/// Given the body's own URN (`source` — the `#message-<id>` sub-precise root, [`crate::subs`]) and its
/// structured inline nodes (`nodes`), produce exactly one [`BodyEdge`] per node by **matching the enum
/// variant** — the reliability guarantee (EI-04 §2.4): extraction reads structured nodes, never scans
/// prose, so a literal `@alice` inside a code span or a `myelin://…` URL written as prose is NOT an
/// edge (only a structured `mention`/`artifact_ref`/`embed` node is). The frozen X-2 uniform mapping:
///
/// - [`InlineNode::Mention`]`(principal)` → `(source, member-urn(principal), mentions)`;
/// - [`InlineNode::ArtifactRefNode`]`(target)` → `(source, target, links)`;
/// - [`InlineNode::Embed`]`(target)` → `(source, target, embeds)`.
///
/// A body with **no** structured ref nodes yields **zero** edges (a plain-prose message — the no-op
/// case). N structured nodes → N edges, in body order. This is the SAME mapping
/// `myelin_git::body::extract_body_edges` / `myelin_refs_service::emit::extract_edges` run (CDC-pinned);
/// Chat owns this half because it cannot depend on the Refs SERVICE crate (the §2.9 acyclic DAG).
pub fn extract_body_edges(source: &ArtifactRef, nodes: &[InlineNode]) -> Vec<BodyEdge> {
    nodes
        .iter()
        .map(|node| {
            let (target, rel) = match node {
                InlineNode::Mention(principal) => {
                    (principal_member_ref(principal), EdgeRel::Mentions)
                }
                InlineNode::ArtifactRefNode(target) => (target.clone(), EdgeRel::Links),
                InlineNode::Embed(target) => (target.clone(), EdgeRel::Embeds),
            };
            BodyEdge {
                source: source.clone(),
                target,
                rel,
            }
        })
        .collect()
}

/// Extract every reference edge across a WHOLE message body subtree (the node-array walk over every
/// inline run, then the per-node mapping). Convenience over
/// [`extract_body_edges`]`(source, &body.structured_nodes().cloned())` — it preserves document order
/// (the order [`MessageBody::structured_nodes`] yields) and is the seam [`emit_body_edges`] reads when
/// the caller holds a [`MessageBody`] rather than a bare node array.
pub fn extract_message_edges(source: &ArtifactRef, body: &MessageBody) -> Vec<BodyEdge> {
    let nodes: Vec<InlineNode> = body.structured_nodes().into_iter().cloned().collect();
    extract_body_edges(source, &nodes)
}

/// Build the canonical `refs.edge.created` [`EventDraft`] for one extracted [`BodyEdge`].
///
/// The references-not-payloads payload carries `source`/`target`/`rel`/`rel_class` (the Refs
/// edge-builder reads exactly these; the deterministic `edge_id` is derived from `tenant + source +
/// target + rel`, so the producer ships the triple, not the id). The aggregate is the
/// `edge:<source>-><target>` identity — the SAME convention the Refs consumer + the other producers
/// use — so per-aggregate ordering (EB-03) holds for an edge's create/remove sequence.
/// `contains_personal_data = false`: every field is an opaque ref/token (the mention target is the
/// PSEUDONYMOUS member URN, not a name), so no inline-PII envelope key is needed (references-not-
/// payloads, contract 2.7).
fn edge_event_draft(edge: &BodyEdge) -> EventDraft {
    EventDraft {
        type_: EventType(REFS_EDGE_CREATED.into()),
        // The referencing side is the event subject (the chat message body that authored the edge).
        subject: edge.source.clone(),
        aggregate: edge_aggregate_key(&edge.source, &edge.target),
        payload: serde_json::json!({
            "source": edge.source.0,
            "target": edge.target.0,
            "rel": edge.rel.as_str(),
            "rel_class": REL_CLASS_REFERENCE,
        }),
        // Refs is the CONTROLLER of the edge fact it authors (the reference graph is Refs-owned) — the
        // SAME role the Refs/Knowledge/Git producer stamps (the edge fact is Refs', not chat's).
        data_role: DataRole::Controller,
        // An edge inherits the referencing content's internal visibility (a routing hint, never an
        // authz decision — Identity decides at resolve-time). The default for derived index events is
        // Internal.
        visibility: Visibility::Internal,
        // References-not-payloads: opaque refs only, no inline PII, so no envelope key.
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// **Emit one `refs.edge.created` per structured ref node, IN THE SAME TRANSACTION as the message
/// body's content write (the contract-5.4 producer seam — Chat-owned half).**
///
/// `tx` is the OPEN outbox transaction the caller is writing the message's `chat.message.created` /
/// `chat.message.edited` content event into (the message row + the content event are staged in `tx`);
/// `content_event` is that content event (the CAUSE). For each extracted [`BodyEdge`], this calls
/// [`OutboxTx::emit`]`(draft, cause = Some(content_event))` — the ONE sanctioned emit verb (contract
/// 2.2; the `no-raw-publish` lint). There is **NO standalone edge-write API** — the edges are emitted
/// from the content nodes only. Returns the minted [`EventId`]s in body order.
///
/// **Causality correct-by-construction (P-S06):** because `cause = Some(content_event)`, the envelope
/// derivation sets `correlation_id = content_event.correlation_id` (the root carries), `causation_id =
/// content_event.event_id`, and `depth = content_event.depth + 1` (the loop-guard stamp). The caller
/// CANNOT typo a wrong parent: the causal triple is not on [`EventDraft`].
///
/// **Emit-iff-committed (the silent-data-loss floor, EI-01 §2):** `emit` BUFFERS the row into `tx`; it
/// becomes durable iff the caller commits `tx`. An aborted message write drops the buffered edge rows
/// with it — **no edge without its message, no message without its edge** (the message content + the
/// edge events co-commit). This function performs NO commit (the caller owns the transaction lifecycle
/// — the SAME discipline [`myelin_git::body::emit_body_edges`] / the chat store's outbox co-commit use).
pub fn emit_body_edges(
    tx: &mut dyn OutboxTx,
    source: &ArtifactRef,
    nodes: &[InlineNode],
    content_event: &EventEnvelope,
) -> BusResult<Vec<EventId>> {
    let edges = extract_body_edges(source, nodes);
    let mut ids = Vec::with_capacity(edges.len());
    for edge in &edges {
        // The ONE sanctioned emit path (contract 2.2; no-raw-publish). `cause = Some(content_event)` →
        // the correlation root carries + causation = the content event + depth+1. The row is BUFFERED
        // into `tx` — durable iff the caller commits (the message write + these edges co-commit).
        let id = tx.emit(edge_event_draft(edge), Some(content_event))?;
        ids.push(id);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::{CalloutTone, HeadingLevel, TaskItem, OBJ};
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, EmitContextBase, EventEnvelope,
        MonotonicMinter, OutboxStore, Region, TenantId, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::sync::Arc;

    fn para(md: &str, nodes: Vec<InlineNode>) -> Block {
        Block::Paragraph {
            inline: parse_inline(md, &nodes),
        }
    }

    fn alice() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    /// A chat message body source URN (a `#message-<id>` sub-URN, the referencing side).
    fn message_source() -> ArtifactRef {
        crate::subs::mint_message("acme", "01J0MSGULID").unwrap()
    }

    // ── 1. the consumed Chat SUBSET (X-2) ──────────────────────────────────────────────────────────

    /// **The admitted Chat block subset round-trips + validates** — paragraph/heading/lists/task_list/
    /// blockquote/code_block/callout/table/divider/image are all in-subset.
    #[test]
    fn admitted_chat_subset_is_valid_and_round_trips() {
        let blocks = vec![
            para("a **rich** message", vec![]),
            Block::Heading {
                level: HeadingLevel::new(3).unwrap(),
                inline: parse_inline("**Heading**", &[]),
            },
            Block::BulletList {
                items: vec![ListItem {
                    blocks: vec![para("item", vec![])],
                }],
            },
            Block::TaskList {
                items: vec![TaskItem {
                    checked: true,
                    inline: parse_inline("done", &[]),
                }],
            },
            Block::Blockquote {
                blocks: vec![para("quoted", vec![])],
            },
            Block::CodeBlock {
                lang: Some("rust".into()),
                text: "let x = **not bold**;".into(),
            },
            Block::Callout {
                tone: CalloutTone::Warn,
                blocks: vec![para("note", vec![])],
            },
            Block::Table {
                columns: vec![Column {
                    header: parse_inline("col", &[]),
                }],
                rows: vec![vec![Cell {
                    blocks: vec![para("cell", vec![])],
                }]],
            },
            Block::Divider,
            Block::Image {
                blob: ArtifactRef("myelin://acme/blob/1".into()),
                alt: "a".into(),
                caption: Some(parse_inline("*cap*", &[])),
            },
        ];
        assert!(validate_subtree(&blocks).is_ok(), "the subset is admitted");
        let body = MessageBody::new(blocks).expect("in-subset");
        assert!(
            body.round_trips(),
            "render(parse(md)) === md over the subtree"
        );
    }

    /// **Each of the three Knowledge-only blocks is REJECTED (X-2) — never silently dropped.** A
    /// `db_view`/`sync_block`/`toggle` at the top level is a loud [`SubsetError`].
    #[test]
    fn knowledge_only_blocks_are_rejected() {
        use myelin_query::{FieldId, ViewSpec};
        let excluded = [
            Block::DbView {
                db: ArtifactRef("myelin://acme/db/1".into()),
                view: ViewSpec::table(FieldId::new("order_key")),
            },
            Block::SyncBlock {
                source: ArtifactRef("myelin://acme/block/9".into()),
            },
            Block::Toggle {
                summary: parse_inline("more", &[]),
                blocks: vec![],
            },
        ];
        for (block, name) in excluded.into_iter().zip(CHAT_EXCLUDED_BLOCKS) {
            assert!(!is_chat_block(&block), "{name} is out of subset");
            let err = validate_subtree(std::slice::from_ref(&block)).unwrap_err();
            assert_eq!(err.excluded, name, "the error names the excluded variant");
            // MessageBody::new also rejects it loudly (never admitted).
            assert!(MessageBody::new(vec![block]).is_err());
        }
    }

    /// **A Knowledge-only block NESTED inside an admitted container is STILL rejected** — the subset
    /// check walks the whole subtree, not just the top level (a `toggle` smuggled inside a blockquote
    /// is caught; a `db_view` inside a list item / table cell is caught).
    #[test]
    fn nested_knowledge_only_block_is_rejected() {
        let smuggled = Block::Blockquote {
            blocks: vec![
                para("ok", vec![]),
                Block::SyncBlock {
                    source: ArtifactRef("myelin://acme/block/9".into()),
                },
            ],
        };
        assert_eq!(
            validate_subtree(&[smuggled]).unwrap_err().excluded,
            "sync_block"
        );

        let in_list = Block::BulletList {
            items: vec![ListItem {
                blocks: vec![Block::Toggle {
                    summary: parse_inline("x", &[]),
                    blocks: vec![],
                }],
            }],
        };
        assert_eq!(validate_subtree(&[in_list]).unwrap_err().excluded, "toggle");

        let in_table = Block::Table {
            columns: vec![Column {
                header: parse_inline("c", &[]),
            }],
            rows: vec![vec![Cell {
                blocks: vec![Block::DbView {
                    db: ArtifactRef("myelin://acme/db/1".into()),
                    view: myelin_query::ViewSpec::table(myelin_query::FieldId::new("k")),
                }],
            }]],
        };
        assert_eq!(
            validate_subtree(&[in_table]).unwrap_err().excluded,
            "db_view"
        );
    }

    /// **The `SubsetError` Display names the offender + is loud about not dropping** (the auditable
    /// reject surface — EI-01 §2, silent data loss outranks every feature).
    #[test]
    fn subset_error_display_names_the_offender() {
        let msg = SubsetError {
            excluded: "sync_block",
        }
        .to_string();
        assert!(msg.contains("sync_block"), "names the offender: {msg}");
        assert!(
            msg.to_lowercase().contains("rejected") && !msg.to_lowercase().contains("dropped, ok"),
            "loud about not dropping: {msg}"
        );
    }

    // ── 2. round-trip render(parse(md)) === md over the ONE WASM render path (KN-D2 / chat) ─────────

    /// **`render(parse(md)) === md` byte-identical over a body with every mark + a structured node**
    /// (the chat instance of KN-D2) — through the ONE WASM render path.
    #[test]
    fn body_round_trips_byte_identical_via_wasm_path() {
        let md = format!("**bold** and *italic* with `code` and a {OBJ} mention");
        let nodes = vec![InlineNode::Mention(alice())];
        assert!(
            roundtrips_md(&md, &nodes),
            "render(parse(md)) === md via the WASM path"
        );
        let body = paragraph_body(&md, nodes);
        assert!(body.round_trips());
    }

    /// **An empty body round-trips trivially and is in-subset + yields zero edges.**
    #[test]
    fn empty_body_round_trips_and_has_no_edges() {
        let body = MessageBody::empty();
        assert!(body.round_trips());
        assert!(extract_message_edges(&message_source(), &body).is_empty());
    }

    /// **`roundtrips_md` (the composer-entry raw-string path) is FALSE on a NON-canonical body** (a
    /// literal `*` that opens no mark is re-emitted escaped) — the round-trip invariant is a real
    /// check, not a constant. The canonical escaped form IS a byte-exact fixed point.
    #[test]
    fn round_trips_md_is_false_on_non_canonical_body() {
        assert!(
            !roundtrips_md("a*b", &[]),
            "a non-canonical source body must NOT round-trip byte-exact"
        );
        assert!(
            roundtrips_md(r"a\*b", &[]),
            "the canonical form IS a fixed point"
        );
    }

    /// **`MessageBody::round_trips` proves AST idempotency** — a stored block subtree (the parsed AST)
    /// serialises to a string that re-parses to the SAME string. A non-canonical SOURCE is normalised
    /// by `parse_inline` at construction, so the STORED AST is always canonical (its serialization IS a
    /// fixed point).
    #[test]
    fn message_body_round_trips_is_ast_idempotent() {
        let from_non_canonical = paragraph_body("a*b", vec![]);
        assert!(
            from_non_canonical.round_trips(),
            "the stored (canonical) AST is always a fixed point"
        );
        if let Block::Paragraph { inline } = &from_non_canonical.blocks[0] {
            assert_eq!(serialize_inline(inline), r"a\*b");
        } else {
            panic!("expected a paragraph body");
        }
    }

    /// **The WASM render path is the IDENTICAL parser native + wasm32** — `roundtrips_md` (the
    /// composer's exact entry) and the native `serialize_inline(parse_inline(..))` agree byte-for-byte
    /// (there is no second renderer).
    #[test]
    fn wasm_path_is_identical_to_native_parse() {
        let md = "**a** `b` ~~c~~ [t](u)";
        let native = serialize_inline(&parse_inline(md, &[]));
        let via_wasm = wasm::render_serialize(&wasm::render_parse(md, &[]));
        assert_eq!(native, via_wasm, "one renderer, native === wasm path");
        assert_eq!(native, md, "and it round-trips");
    }

    // ── 3. the three inline nodes → refs.edge.created uniformly (contract 5.4) ──────────────────────

    /// **Each of the three structured nodes yields exactly one edge with the correct `rel` and
    /// target** (the X-2 uniform producer mapping, structured-node-driven NOT regex). The mention's
    /// target is the PSEUDONYMOUS `member` URN (erasure-safe), never the name.
    #[test]
    fn each_node_kind_yields_one_edge_with_correct_rel_and_target() {
        let src = message_source();
        let page = ArtifactRef("myelin://acme/knowledge/page/7c2".into());
        let issue = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        let nodes = vec![
            InlineNode::Mention(alice()),
            InlineNode::ArtifactRefNode(issue.clone()),
            InlineNode::Embed(page.clone()),
        ];
        let edges = extract_body_edges(&src, &nodes);
        assert_eq!(edges.len(), 3, "N structured nodes → N edges (1 per node)");

        assert_eq!(edges[0].rel, EdgeRel::Mentions);
        assert_eq!(edges[0].rel.as_str(), "mentions");
        assert_eq!(edges[0].source, src);
        assert_eq!(
            edges[0].target.0, "myelin://acme/identity/member/p-opaque-alice",
            "mention target is the pseudonymous member URN, never the name"
        );

        assert_eq!(edges[1].rel, EdgeRel::Links);
        assert_eq!(edges[1].rel.as_str(), "links");
        assert_eq!(edges[1].target, issue);

        assert_eq!(edges[2].rel, EdgeRel::Embeds);
        assert_eq!(edges[2].rel.as_str(), "embeds");
        assert_eq!(edges[2].target, page);
    }

    /// **A prose `myelin://…` / `@alice` written as text produces ZERO edges** — extraction is
    /// structured, never a regex over prose (the reliability guarantee). Only a structured node is an
    /// edge.
    #[test]
    fn prose_reference_is_not_a_content_edge() {
        let body = paragraph_body("see myelin://acme/issue/ENG-1 and ping @alice", vec![]);
        assert!(body.round_trips());
        let edges = extract_message_edges(&message_source(), &body);
        assert!(edges.is_empty(), "a prose reference is NOT a content edge");
    }

    /// **The structured-node walk reaches every inline run across the subtree** (a node-array walk,
    /// never a regex). A body with a mention in a paragraph + an embed in a list item yields both, in
    /// document order.
    #[test]
    fn structured_nodes_walks_the_whole_subtree() {
        let page = ArtifactRef("myelin://acme/knowledge/page/7".into());
        let body = MessageBody::new(vec![
            para(&format!("hi {OBJ}"), vec![InlineNode::Mention(alice())]),
            Block::BulletList {
                items: vec![ListItem {
                    blocks: vec![para(
                        &format!("see {OBJ}"),
                        vec![InlineNode::Embed(page.clone())],
                    )],
                }],
            },
        ])
        .unwrap();
        let nodes = body.structured_nodes();
        assert_eq!(nodes.len(), 2, "both structured nodes are reached");
        assert!(matches!(nodes[0], InlineNode::Mention(_)));
        assert!(matches!(nodes[1], InlineNode::Embed(_)));

        let edges = extract_message_edges(&message_source(), &body);
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].rel, EdgeRel::Mentions);
        assert_eq!(edges[1].rel, EdgeRel::Embeds);
    }

    /// **The edge event draft is `refs.edge.created` with the references-not-payloads triple + the
    /// shared `edge:<source>-><target>` aggregate + `rel_class = reference`.** This is the
    /// byte-identical shape the Refs edge-builder ingests (CDC-pinned). `contains_personal_data =
    /// false` (opaque refs only).
    #[test]
    fn edge_event_draft_is_refs_edge_created_with_the_triple() {
        let src = message_source();
        let target = ArtifactRef("myelin://acme/knowledge/page/7c2#block-3".into());
        let edge = BodyEdge {
            source: src.clone(),
            target: target.clone(),
            rel: EdgeRel::Embeds,
        };
        let draft = edge_event_draft(&edge);
        assert_eq!(draft.type_.0, "refs.edge.created");
        assert_eq!(draft.subject, src, "the subject is the referencing body");
        assert_eq!(draft.payload["source"], src.0);
        assert_eq!(draft.payload["target"], target.0);
        assert_eq!(draft.payload["rel"], "embeds");
        assert_eq!(draft.payload["rel_class"], "reference");
        assert_eq!(draft.aggregate.0, format!("edge:{}->{}", src.0, target.0));
        assert!(
            !draft.contains_personal_data,
            "references-not-payloads: no inline PII"
        );
        assert!(draft.pii_key_ref.is_none());
        assert_eq!(draft.data_role, DataRole::Controller);
    }

    /// The frozen tokens are exactly the Refs wire tokens (the names anchor X-5; no second vocabulary).
    #[test]
    fn frozen_tokens_match_the_refs_wire_shape() {
        assert_eq!(REFS_EDGE_CREATED, "refs.edge.created");
        assert_eq!(REL_CLASS_REFERENCE, "reference");
        assert_eq!(EdgeRel::Mentions.as_str(), "mentions");
        assert_eq!(EdgeRel::Links.as_str(), "links");
        assert_eq!(EdgeRel::Embeds.as_str(), "embeds");
    }

    // ── 4. the body's edges co-commit IN THE SAME TX as the message content event (contract 5.4/2.2) ─

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(alice()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T10:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T10:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    /// The message's `chat.message.created` content event (the CAUSE) — built directly (the message
    /// write holds it in hand; the message row + this event co-commit in the chat store's outbox). The
    /// body edges hang off it in the SAME transaction.
    fn content_event(source: &ArtifactRef) -> EventEnvelope {
        EventEnvelope {
            event_id: myelin_events::EventId("01J-msg".into()),
            type_: EventType(crate::events::CHAT_MESSAGE_CREATED.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(alice()),
            subject: source.clone(),
            aggregate: AggregateKey("chat:conv:01J0CONV".into()),
            causation_id: None,
            correlation_id: CorrelationId("01J-msg-corr".into()),
            caused_by: Some(CausedBy("session:abc".into())),
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-21T10:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T10:00:01Z".into()),
            payload: serde_json::json!({ "message": source.0 }),
        }
    }

    /// **A message body's structured nodes co-commit `refs.edge.created` through the ONE outbox
    /// emit, caused by the message content event.** After the write, one edge event is committed per
    /// structured node, inheriting the message's correlation root + causation, with the
    /// references-not-payloads triple + NO inline body on the wire.
    #[test]
    fn message_edges_co_commit_with_the_content_event() {
        let store = OutboxStore::new();
        let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(MonotonicMinter::new());
        let src = message_source();

        let body = MessageBody::new(vec![para(
            &format!("hi {OBJ} see {OBJ}"),
            vec![
                InlineNode::Mention(alice()),
                InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())),
            ],
        )])
        .unwrap();

        let cause = content_event(&src);
        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("chat message 01J0MSGULID body written");
        let nodes: Vec<InlineNode> = body.structured_nodes().into_iter().cloned().collect();
        let ids = emit_body_edges(&mut tx, &src, &nodes, &cause).unwrap();
        tx.commit().unwrap();

        assert_eq!(ids.len(), 2, "one edge event per structured node");
        for id in &ids {
            let row = store.row(id).expect("the committed edge row is present");
            assert_eq!(row.envelope.type_.0, "refs.edge.created");
            // causality correct-by-construction: caused by the message content event.
            assert_eq!(
                row.envelope.correlation_id, cause.correlation_id,
                "the edge inherits the message's correlation root"
            );
            assert_eq!(
                row.envelope.causation_id.as_ref().unwrap().0,
                cause.event_id.0
            );
            assert_eq!(
                row.envelope.depth,
                cause.depth + 1,
                "depth+1 loop-guard stamp"
            );
            // references-not-payloads: no inline body on the wire.
            let payload = serde_json::to_string(&row.envelope.payload).unwrap();
            assert!(!payload.contains("hi "), "no inline body on the wire");
        }
    }
}
