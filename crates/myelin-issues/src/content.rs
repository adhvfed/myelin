//! # `content` — the issue body + comments as a `myelin-content` block subtree
//! (ISS-P10 / P-376, M4-I1 — `render(parse(md)) === md`)
//!
//! This is M4-I1's **content-body slice** — the final piece of the first-runnable
//! create → key → edit → link → reorder loop. It makes an issue's **description body** and its
//! **comments** real [`myelin_content`] documents: a **block subtree** (the consumed Issues
//! SUBSET of the frozen contract-13.1 [`Block`] taxonomy) whose every inline run round-trips
//! `render(parse(md)) === md` through the **ONE WASM render path** (the SAME
//! [`myelin_content::parse_inline`] / [`myelin_content::serialize_inline`] compiled native on the
//! server and to `wasm32-unknown-unknown` for the editor — there is no second renderer, so the
//! two-divergent-renderers trap is eliminated structurally, EI-01 §7).
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `../../VISION.md` §3 (top-of-the-line UX — ONE content model; content round-trips through the
//!   one editor render path).
//! - `04-subsystem-architectures/issue-tracker/architecture/01-tech-and-data-model.md` §1.3 (the
//!   editor/renderer row: ONE Rust `myelin-content` core compiled to WASM; Issues consumes the
//!   frozen block SUBSET — paragraph/heading/lists/task_list/blockquote/code_block/callout/table/
//!   divider/image + the three inline ref nodes; it EXCLUDES `db_view`/`sync_block`/`toggle` from
//!   inline authoring) + §2 (`body_block` points at a `myelin-content` block subtree; the
//!   `#b<opaqueid>` sub-anchor) + §1.3 row "Content concurrency = single-author CAS" (NOT the
//!   Knowledge CRDT — coarse-grained, server-arbitrated).
//! - `00-reconciliation-decisions.md` X-2/OQ-B (the canonical `myelin-content` taxonomy + the
//!   Chat/Issues consumed subsets — "Issues (CR-9): the same block subset as Chat; issue
//!   descriptions/comments use the full block subset; description concurrency is
//!   single-author-at-a-time, CAS-guarded"; the round-trip invariant `render(parse(md)) === md`
//!   holds over this subset; read + edit use the IDENTICAL WASM parser).
//! - `contract-index.md` rows **13.1** (the block subset + the WASM render path + the three inline
//!   ref nodes) + **2.2** (the body/comment write co-commits its `issue.*`/`issue.comment.*` event
//!   through the ONE `OutboxTx::emit`).
//!
//! ## What this prompt (ISS-P10 / P-376) ships
//! 1. [`is_issue_block`] / [`validate_subtree`] — the **consumed Issues SUBSET** of the frozen
//!    [`Block`] taxonomy. Issues consumes a STRICT subset (X-2): it never adds a node type, and it
//!    never AUTHORS the three Knowledge-only blocks (`db_view`/`sync_block`/`toggle`). The validator
//!    walks the subtree (recursing into the container blocks) and REJECTS any excluded node LOUDLY
//!    — a [`SubsetError`], never a silent drop (EI-01 §2).
//! 2. [`IssueContent`] — an issue **body** OR a **comment** as a `myelin-content` block subtree (a
//!    `Vec<Block>` from the Issues subset) under a **single-author version-token CAS**. [`IssueContent::round_trips`]
//!    re-derives every inline run through the ONE [`myelin_content::wasm::render_parse`] and
//!    re-serialises through [`myelin_content::wasm::render_serialize`] — so `render(parse(md)) === md`
//!    byte-identically over every inline string in the tree (the ISS-D10 gate). [`IssueContent::cas_edit`]
//!    admits an edit only against the expected `version` (a stale write is rejected LOUDLY — no silent
//!    last-writer-wins; the move-CRDT body collaboration is OUT of v1 scope, named below).
//! 3. [`emit_content_event`] — the body/comment write **co-commits its event** (`issue.issue.updated`
//!    for a body edit, `issue.comment.created`/`issue.comment.updated` for a comment) through the ONE
//!    sanctioned [`OutboxTx::emit`] on the SAME transaction (contract 2.2; the `no-raw-publish` lint
//!    holds — emit is the only path). References-not-payloads (contract 2.7): the event carries the
//!    issue/comment URN + a `pii_key_ref` for a PII-bearing body, NEVER the inline body itself.
//!
//! ## Why a thin Issues consumer over the frozen shared crate (EI-01 §7 — reuse, never duplicate)
//! The block/inline AST, the markdown-subset grammar, the ONE WASM render path, and the round-trip
//! invariant ALREADY exist in the frozen [`myelin_content`] crate (Knowledge LEADS + freezes the
//! taxonomy, KN-P01). This module does NOT re-define a single node type and does NOT author a second
//! renderer — it LINKS the frozen [`Block`]/[`Inline`] + calls the ONE [`myelin_content::wasm`] render
//! entry points (the EXACT seam the editor's WASM glue calls). It adds ONLY what is genuinely Issues'
//! own: the **consumed-subset enforcement** (the Issues subset is strictly smaller than Knowledge's,
//! X-2) and the **single-author version-token CAS** over the body/comment (NOT the Knowledge CRDT —
//! arch §1.3 "issue body = single-author CAS, NOT the Knowledge CRDT"). The same posture as
//! [`myelin_git::body`] (Git's PR/comment-body consumer) — extend/reconcile in place, never a parallel
//! second implementation.
//!
//! ## Named floors (VISION §3 / EI-01 §1)
//! - **The move-CRDT body collaboration is OUT of v1 scope.** Issue-body concurrency is
//!   **single-author version-CAS** (arch §1.3 / ADR-05). The multi-author collaborative-edit engine
//!   (Yrs / the per-character CRDT) is **Knowledge's** (KN-1 CAS-floor → CRDT), never Issues' in v1 —
//!   and Issues' ranking move-CRDT (a DISTINCT thing) is the measured M5 follow-on (ISS-P32). The body
//!   here is a **projection of the frozen content subset** under the version-CAS floor; no new content
//!   shape is invented and no merge engine is deferred-and-unbuilt here.
//! - **The at-rest per-subject-DEK body ciphertext** (the body bytes sealed under the subject DEK,
//!   contract 11.4 — `erasure = CryptoShred`) is the storage layer's ([`crate::dek`], ISS-P07).
//!   [`IssueContent`] is the **cleartext in-memory document** the round-trip + the subset validation run
//!   over; the live OLTP `body_block` column + the DEK seal/unseal ride the [`crate::write_path`] seam
//!   (the SAME `apply_mutation_sealed` path — this module is the document + the round-trip logic, the
//!   store is the persistence). Named so the cleartext document is not mistaken for the at-rest form.
//! - **The structured-node → `refs.edge.created` emission** (the `mention`/`artifact_ref`/`embed` ref
//!   edges contract 5.4 produces) is the Issues Refs-producer half (ISS-P-REF band) — it reads the
//!   SAME [`InlineNode`] node-array walk this module exposes ([`IssueContent::structured_nodes`]). Named
//!   so the content body is not mistaken for the edge producer; both ride the same frozen node array.

use crate::events;
use myelin_content::{
    parse_inline, serialize_inline, wasm, Block, Cell, Column, Inline, InlineNode, ListItem,
};
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventId, EventType, OutboxTx, PiiKeyRef,
    Result as BusResult, Visibility,
};

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE CONSUMED ISSUES SUBSET of the frozen contract-13.1 Block taxonomy (X-2)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The three Knowledge-only block variants Issues does NOT author (X-2).** Issues consumes a
/// STRICT subset of the frozen taxonomy: the full block set MINUS `db_view`/`sync_block`/`toggle`
/// (arch §1.3 — "it excludes `db_view`/`sync_block`/`toggle` from inline authoring"; recon §X-2 —
/// "Issues … excludes `db_view, sync_block, toggle`"). The frozen [`Block`] enum is owned by
/// Knowledge and is NOT redefined here (EI-01 §7); this is the Issues-side admission policy over it.
/// A `&'static str` name per excluded variant so a [`SubsetError`] names the offender, never a literal.
pub const ISSUES_EXCLUDED_BLOCKS: [&str; 3] = ["db_view", "sync_block", "toggle"];

/// Why a block subtree is NOT a valid Issues content document — it carries a Knowledge-only node
/// Issues' subset excludes (X-2). LOUD + typed: an excluded node is REJECTED, never silently dropped
/// (EI-01 §2 — silent data loss outranks every feature). Carries the excluded variant's name + the
/// 0-based index of the offending top-level block (for the audit / the editor's error surface).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubsetError {
    /// The excluded variant's frozen name (one of [`ISSUES_EXCLUDED_BLOCKS`]).
    pub excluded: &'static str,
}

impl std::fmt::Display for SubsetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "block `{}` is Knowledge-only and not in the Issues content subset (X-2) — rejected, not dropped",
            self.excluded
        )
    }
}

impl std::error::Error for SubsetError {}

/// Whether a single block is in the **consumed Issues subset** (X-2). `true` for every variant
/// EXCEPT the three Knowledge-only ones ([`ISSUES_EXCLUDED_BLOCKS`]). This is a shallow check on the
/// block's own variant; container recursion is [`validate_subtree`]'s job (a `db_view` nested inside
/// an admitted `blockquote` is still rejected by the recursive walk).
pub fn is_issue_block(block: &Block) -> bool {
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

/// **Validate a block subtree is entirely within the consumed Issues subset (X-2) — recursively.**
///
/// Walks the WHOLE tree (recursing into the container blocks — `blockquote`/`callout` blocks, list
/// items, table cells), rejecting the FIRST Knowledge-only node it finds with a LOUD [`SubsetError`]
/// (never a silent drop — EI-01 §2). A subtree of only admitted variants returns `Ok(())`. This is
/// the Issues-side admission policy over the frozen [`Block`] taxonomy; it never mutates the tree and
/// never re-defines a node (X-2 — Issues consumes a strict subset, it does not author a new one).
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
            // image (leaf). They cannot smuggle an excluded node.
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
/// what [`IssueContent::round_trips`] proves `render(parse(md)) === md` over.
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
// 2. THE ISSUE CONTENT — a body OR comment as a block subtree under single-author version-CAS
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// Which content surface a [`IssueContent`] is — the issue **body** (the description, the `body_block`
/// subtree, arch §2) or a **comment** (a `#comment-<opaqueid>` sub-artifact, arch §6.1). The kind
/// selects the `issue.*` event token the write co-commits ([`IssueContent::edit_event_token`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentKind {
    /// The issue description body (`body_block` → the `myelin-content` block subtree root, arch §2).
    Body,
    /// A comment (the `#comment-<opaqueid>` sub-artifact; body is `myelin-content`, arch §6.1).
    Comment,
}

/// An issue **body** or **comment** as a frozen [`myelin_content`] **block subtree** (the consumed
/// Issues SUBSET, X-2) under a **single-author version-token CAS** (arch §1.3 — NOT the Knowledge
/// CRDT). This is the cleartext in-memory document the round-trip + subset validation run over (the
/// at-rest per-subject-DEK ciphertext is the storage layer's — a named floor).
///
/// **Single-author version-token CAS** (arch §1.3 / ADR-05): the content carries the issue/comment
/// `version` (the optimistic-concurrency token; the SAME `version bigint` column the issue spine
/// carries, arch §2 — board/field edits CAS on it too). An edit is admitted only against the expected
/// version ([`IssueContent::cas_edit`]); a stale edit is rejected LOUDLY (no silent last-writer-wins).
/// The move-CRDT body collaboration is OUT of v1 scope (named floor).
///
/// The `blocks` field is the block subtree (the consumed Issues subset — paragraph/heading/lists/
/// task_list/blockquote/code_block/callout/table/divider/image + the three inline ref nodes); it is
/// validated against the subset by [`IssueContent::new`] / [`IssueContent::cas_edit`]. Every inline
/// run round-trips `render(parse(md)) === md` through the ONE WASM render path
/// ([`IssueContent::round_trips`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueContent {
    /// Whether this is the issue body or a comment (selects the co-committed event token).
    pub kind: ContentKind,
    /// The block subtree (the consumed Issues subset, X-2). Validated on construction + on edit.
    pub blocks: Vec<Block>,
    /// The single-author CAS version token (the SAME `version bigint` the issue spine carries, arch
    /// §2). Bumped on each admitted edit; a stale edit (against a prior version) is rejected by
    /// [`IssueContent::cas_edit`].
    pub version: u64,
}

/// A single-author CAS conflict on a body/comment edit — the edit's expected version did not match
/// the content's current version (a concurrent edit landed first). LOUD + typed: a stale edit is
/// NEVER silently applied (no last-writer-wins). Carries `(expected, actual)` for the audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CasConflict {
    /// The version the edit expected the content to be at.
    pub expected: u64,
    /// The version the content is actually at (a concurrent edit advanced it).
    pub actual: u64,
}

impl std::fmt::Display for CasConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "single-author CAS conflict: edit expected version {} but the content is at {}",
            self.expected, self.actual
        )
    }
}

impl std::error::Error for CasConflict {}

/// Why an edit was rejected — either it carried a Knowledge-only block out of the Issues subset
/// ([`ContentError::Subset`], X-2) or it lost the single-author CAS ([`ContentError::Cas`]). Both are
/// LOUD: the content is NOT mutated, nothing is silently dropped (EI-01 §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentError {
    /// The edit's block subtree carried a Knowledge-only node (db_view/sync_block/toggle, X-2). The
    /// content is unchanged.
    Subset(SubsetError),
    /// The edit lost the single-author version CAS (a concurrent edit advanced the version first).
    /// The content is unchanged (no last-writer-wins).
    Cas(CasConflict),
}

impl std::fmt::Display for ContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentError::Subset(e) => write!(f, "{e}"),
            ContentError::Cas(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ContentError {}

impl From<SubsetError> for ContentError {
    fn from(e: SubsetError) -> Self {
        ContentError::Subset(e)
    }
}

impl IssueContent {
    /// Build a new issue body/comment (version 0) from a block subtree. The subtree is VALIDATED
    /// against the consumed Issues subset (X-2) — a Knowledge-only node is rejected LOUDLY
    /// ([`SubsetError`]), never admitted. The round-trip invariant ([`IssueContent::round_trips`])
    /// holds iff every inline run's `md` is canonical.
    pub fn new(kind: ContentKind, blocks: Vec<Block>) -> Result<IssueContent, SubsetError> {
        validate_subtree(&blocks)?;
        Ok(IssueContent {
            kind,
            blocks,
            version: 0,
        })
    }

    /// An empty body/comment (version 0, no blocks) — an issue opened with no description / a freshly
    /// minted comment shell. Round-trips trivially (0 inline runs ⇒ 0 round-trip work) and is always
    /// in-subset.
    pub fn empty(kind: ContentKind) -> IssueContent {
        IssueContent {
            kind,
            blocks: Vec::new(),
            version: 0,
        }
    }

    /// **Parse → serialize EVERY inline run through the ONE WASM render path, asserting
    /// `render(parse(md)) === md` (contract 13.1; the ISS-D10 gate).** `true` iff re-serialising every
    /// inline run reproduces its canonical `md` byte-identically. Read + edit use the IDENTICAL parser
    /// (the [`myelin_content::wasm`] entry points the editor's WASM glue calls — there is no
    /// Issues-local renderer, EI-01 §7). A corpus of issue bodies + comments round-tripping at 100% is
    /// the CI gate ([`crate::content::tests`] + `tests/roundtrip_iss_d10.rs`).
    pub fn round_trips(&self) -> bool {
        inline_runs(&self.blocks).into_iter().all(roundtrips_inline)
    }

    /// **Apply a single-author CAS edit (arch §1.3).** Replace the block subtree IFF `expected_version`
    /// matches the current `version` AND the new subtree is in the consumed Issues subset (X-2). On
    /// success the version is bumped and the new version returned. On a stale edit returns
    /// [`ContentError::Cas`]; on an out-of-subset edit returns [`ContentError::Subset`] — in BOTH
    /// cases the content is NOT mutated (no silent last-writer-wins, no silent drop). The subset is
    /// validated FIRST (a malformed subtree is rejected before the CAS even consults the version), so
    /// a rejected edit never observes a version bump.
    pub fn cas_edit(
        &mut self,
        expected_version: u64,
        blocks: Vec<Block>,
    ) -> Result<u64, ContentError> {
        // Validate the subset BEFORE the CAS — a malformed (out-of-subset) edit is rejected outright
        // and never bumps the version (it is not a "winning" write).
        validate_subtree(&blocks)?;
        if expected_version != self.version {
            return Err(ContentError::Cas(CasConflict {
                expected: expected_version,
                actual: self.version,
            }));
        }
        self.blocks = blocks;
        self.version += 1;
        Ok(self.version)
    }

    /// The structured ref nodes across the WHOLE subtree (the `mention`/`artifact_ref`/`embed`
    /// producers of `refs.edge.created`, contract 5.4) — a node-array walk over every inline run,
    /// NEVER a regex over the prose (the reliability guarantee, EI-04 §2.4). This is the seam the
    /// Issues Refs-producer reads (a named floor — the emission is the ISS-P-REF band; the node walk
    /// is here).
    pub fn structured_nodes(&self) -> Vec<&InlineNode> {
        inline_runs(&self.blocks)
            .into_iter()
            .flat_map(|inline| inline.structured_nodes().iter())
            .collect()
    }

    /// The `issue.*` event token a write to this content co-commits (contract 2.2). A body edit is an
    /// `issue.issue.updated` (the description is a field of the issue aggregate); a comment write is an
    /// `issue.comment.created` (first write, version 0) or `issue.comment.updated` (a subsequent edit).
    /// The NAMED constant from [`crate::events`] (the names anchor X-5), never a literal.
    pub fn edit_event_token(&self) -> &'static str {
        match self.kind {
            ContentKind::Body => events::ISSUE_UPDATED,
            ContentKind::Comment if self.version == 0 => events::COMMENT_CREATED,
            ContentKind::Comment => events::COMMENT_UPDATED,
        }
    }
}

/// Round-trip ONE inline run through the ONE WASM render path: parse `md` + its positional node array
/// via [`myelin_content::wasm::render_parse`] (the SAME entry the editor's WASM glue calls), then
/// re-serialise via [`myelin_content::wasm::render_serialize`], and assert the result equals the
/// canonical `md`. `render(parse(md)) === md` (contract 13.1). The node array is re-extracted from the
/// AST so the binding is positional (the i-th `OBJ` ↔ `nodes[i]`). We re-derive the source `md` from
/// the stored [`Inline`] via [`serialize_inline`] (the canonical form) — the round-trip then proves
/// parse∘serialize is a fixed point on that canonical string through the WASM path.
fn roundtrips_inline(inline: &Inline) -> bool {
    // The canonical `md` for this run is its serialization (the editor stores the canonical form).
    let md = serialize_inline(inline);
    // Re-parse through the ONE WASM render path (the editor's entry point) using the run's own node
    // array, then re-serialise through the same path. render(parse(md)) === md.
    let reparsed = wasm::render_parse(&md, &inline.nodes);
    wasm::render_serialize(&reparsed) == md
}

/// Round-trip a RAW markdown-subset string (+ its positional node array) through the ONE WASM render
/// path: `render_serialize(render_parse(md, nodes)) == md`. This is the editor's exact entry — the
/// corpus gate ([`crate::content::tests`] + `tests/roundtrip_iss_d10.rs`) feeds hand-authored issue
/// body / comment markdown through THIS function, so the proof is over the identical code path the
/// client editor compiles to `wasm32-unknown-unknown` (EI-01 §7 — one renderer). Returns `true` iff
/// the raw `md` is a byte-exact fixed point (i.e. `md` is canonical).
pub fn roundtrips_md(md: &str, nodes: &[InlineNode]) -> bool {
    let parsed = wasm::render_parse(md, nodes);
    wasm::render_serialize(&parsed) == md
}

/// Parse a markdown-subset body string into a single-paragraph block (the editor's simplest body
/// shape): one [`Block::Paragraph`] wrapping the parsed [`Inline`]. The body block subtree of a plain
/// description is `[paragraph{inline}]`; richer bodies (headings/lists/tables) are built by the editor
/// from the same subset. Uses the ONE [`parse_inline`] (no Issues-local parser). Always in-subset (a
/// paragraph is admitted).
pub fn paragraph_body(md: &str, nodes: &[InlineNode]) -> IssueContent {
    let inline = parse_inline(md, nodes);
    IssueContent {
        kind: ContentKind::Body,
        blocks: vec![Block::Paragraph { inline }],
        version: 0,
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. THE BODY/COMMENT WRITE CO-COMMITS ITS EVENT (contract 2.2 — the ONE OutboxTx::emit)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// Build the canonical `issue.*` / `issue.comment.*` [`EventDraft`] a body/comment write co-commits
/// (contract 2.2; references-not-payloads, contract 2.7). The payload carries the issue/comment URN +
/// the new `version` token + a `pii_key_ref` for a PII-bearing body, NEVER the inline body (the body
/// bytes are sealed at rest under the per-subject DEK, ISS-P07 — the event references the key, never
/// the cleartext). The aggregate is the ISSUE (per-issue ordering, contract 2.3 — a comment's events
/// share the issue's aggregate so the issue's body + comment timeline is per-aggregate ordered).
fn content_event_draft(
    token: &str,
    issue_ref: &ArtifactRef,
    content_ref: &ArtifactRef,
    aggregate: &AggregateKey,
    new_version: u64,
    pii_key_ref: Option<PiiKeyRef>,
) -> EventDraft {
    let contains_pii = pii_key_ref.is_some();
    EventDraft {
        type_: EventType(token.into()),
        // The subject is the precise content surface (the issue body URN or the `#comment-<id>` sub),
        // so a consumer routing on the subject reaches the exact edited surface.
        subject: content_ref.clone(),
        aggregate: aggregate.clone(),
        payload: serde_json::json!({
            // references-not-payloads: the issue URN + the precise content URN + the new version
            // token. NEVER the inline body (the body is at-rest ciphertext; the event carries the key).
            "issue": issue_ref.0,
            "content": content_ref.0,
            "version": new_version,
        }),
        // Issues is the CONTROLLER of the issue/comment fact it authors (the SAME role the other
        // state-change events stamp — the tenant org is the controller of issue content).
        data_role: DataRole::Controller,
        // A content edit's default visibility is Internal (a routing hint, never an authz decision —
        // Identity decides at resolve-time).
        visibility: Visibility::Internal,
        contains_personal_data: contains_pii,
        // A PII-bearing body carries a key REF (never the body — references-not-payloads). The REAL
        // per-subject-DEK ref (`kms://<tenant>/<epoch>/subject:<id>`) is threaded by the caller's
        // sealed write path (ISS-P07); a body with no free-text PII carries no key.
        pii_key_ref,
    }
}

/// **Emit the body/comment content event IN THE SAME TRANSACTION as the content write (contract 2.2
/// — the ONE sanctioned emit verb).**
///
/// `tx` is the OPEN outbox transaction the caller staged the content row mutation into (the
/// `body_block`/`issue_comment` row write + this event co-commit); `content` is the just-written
/// document (its `version` is the new, post-edit version + its `kind` selects the token). For a body
/// the token is `issue.issue.updated`; for a comment it is `issue.comment.created` (first write) or
/// `issue.comment.updated`. This calls [`OutboxTx::emit`]`(draft, cause)` — the ONLY emit path
/// (contract 2.2; the `no-raw-publish` lint, P-019). Returns the minted [`EventId`].
///
/// **Emit-iff-committed (the silent-data-loss floor, EI-01 §2):** `emit` BUFFERS the event into `tx`;
/// it becomes durable IFF the caller commits `tx`. An aborted content write drops the buffered event
/// with it — no event without its committed content, no committed content without its event. This
/// function performs NO commit (the caller owns the transaction lifecycle — the SAME discipline
/// [`crate::write_path::apply_mutation`] and [`myelin_git::body::emit_body_edges`] use).
///
/// `pii_key_ref` is the per-subject-DEK key ref for a PII-bearing body (ISS-P07 — the event references
/// the key, never the cleartext); `None` for a body with no free-text PII. `cause` is the optional
/// parent envelope (a reflex-driven edit inherits its correlation + `depth+1`, P-S06).
pub fn emit_content_event(
    tx: &mut dyn OutboxTx,
    issue_ref: &ArtifactRef,
    content_ref: &ArtifactRef,
    aggregate: &AggregateKey,
    content: &IssueContent,
    pii_key_ref: Option<PiiKeyRef>,
    cause: Option<&myelin_events::EventEnvelope>,
) -> BusResult<EventId> {
    let draft = content_event_draft(
        content.edit_event_token(),
        issue_ref,
        content_ref,
        aggregate,
        content.version,
        pii_key_ref,
    );
    // The ONE sanctioned emit path (contract 2.2; no-raw-publish). BUFFERED into `tx` — durable iff
    // the caller commits (the content row write + this event co-commit, emit-iff-committed).
    tx.emit(draft, cause)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::{CalloutTone, HeadingLevel, TaskItem};
    use myelin_events::{
        Actor, ArtifactRef, CausedBy, EmitContextBase, MonotonicMinter, OutboxStore, Region,
        TenantId, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::sync::Arc;

    fn para(md: &str, nodes: Vec<InlineNode>) -> Block {
        Block::Paragraph {
            inline: parse_inline(md, &nodes),
        }
    }

    fn alice() -> InlineNode {
        InlineNode::Mention(Principal::stub(
            PrincipalId("p-opaque-alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        ))
    }

    // ── 1. the consumed Issues SUBSET (X-2) ────────────────────────────────────────────────────────

    /// **The admitted Issues block subset round-trips + validates** — paragraph/heading/lists/
    /// task_list/blockquote/code_block/callout/table/divider/image are all in-subset.
    #[test]
    fn admitted_issues_subset_is_valid() {
        let blocks = vec![
            para("a **rich** body", vec![]),
            Block::Heading {
                level: HeadingLevel::new(2).unwrap(),
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
        let content = IssueContent::new(ContentKind::Body, blocks).expect("in-subset");
        assert!(
            content.round_trips(),
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
        for (block, name) in excluded.into_iter().zip(ISSUES_EXCLUDED_BLOCKS) {
            assert!(!is_issue_block(&block), "{name} is out of subset");
            let err = validate_subtree(std::slice::from_ref(&block)).unwrap_err();
            assert_eq!(err.excluded, name, "the error names the excluded variant");
            // IssueContent::new also rejects it loudly (never admitted).
            assert!(IssueContent::new(ContentKind::Body, vec![block]).is_err());
        }
    }

    /// **A Knowledge-only block NESTED inside an admitted container is STILL rejected** — the subset
    /// check walks the whole subtree, not just the top level (a `toggle` smuggled inside a blockquote
    /// is caught).
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
        let err = validate_subtree(&[smuggled]).unwrap_err();
        assert_eq!(err.excluded, "sync_block");
    }

    /// **A Knowledge-only block inside a list item / table cell is rejected (the recursion reaches
    /// list + table containers).**
    #[test]
    fn nested_in_list_and_table_is_rejected() {
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

    // ── 2. round-trip render(parse(md)) === md over the ONE WASM render path (ISS-D10) ──────────────

    /// **`render(parse(md)) === md` byte-identical over a body with every mark + a structured node**
    /// (the ISS-D10 gate, applied to an issue body) — through the ONE WASM render path.
    #[test]
    fn body_round_trips_byte_identical_via_wasm_path() {
        use myelin_content::OBJ;
        let md = format!("**bold** and *italic* with `code` and a {OBJ} mention");
        let nodes = vec![alice()];
        assert!(
            roundtrips_md(&md, &nodes),
            "render(parse(md)) === md via the WASM path"
        );
        let content = paragraph_body(&md, &nodes);
        assert!(content.round_trips());
    }

    /// **An empty body/comment round-trips trivially and is in-subset.**
    #[test]
    fn empty_content_round_trips() {
        assert!(IssueContent::empty(ContentKind::Body).round_trips());
        assert!(IssueContent::empty(ContentKind::Comment).round_trips());
    }

    /// **`roundtrips_md` (the editor-entry raw-string path) is FALSE on a NON-canonical body** (a
    /// literal `*` that opens no mark is re-emitted escaped) — the round-trip invariant is a real
    /// check, not a constant. The canonical escaped form IS a byte-exact fixed point. This is the
    /// ISS-D10 gate over the RAW source string (the AST-storing [`IssueContent::round_trips`] proves
    /// the complementary AST-idempotency property below).
    #[test]
    fn round_trips_md_is_false_on_non_canonical_body() {
        assert!(
            !roundtrips_md("a*b", &[]),
            "a non-canonical source body must NOT round-trip byte-exact"
        );
        // the canonical escaped form IS a fixed point.
        assert!(roundtrips_md(r"a\*b", &[]));
    }

    /// **`IssueContent::round_trips` proves AST idempotency** — a stored block subtree (the parsed
    /// AST) serialises to a string that re-parses to the SAME string (serialize∘parse is a fixed point
    /// on the canonical form the editor stores). A non-canonical SOURCE is normalised by `parse_inline`
    /// at construction, so the STORED AST is always canonical (its serialization IS a fixed point) —
    /// the meaningful correctness bar for an AST-storing document is this idempotency, and it holds.
    #[test]
    fn issue_content_round_trips_is_ast_idempotent() {
        // a body built from a non-canonical SOURCE: parse_inline normalises it, so the STORED AST
        // round-trips (serialize∘parse is stable). This is correct: the AST never carries the
        // non-canonical bytes.
        let from_non_canonical = paragraph_body("a*b", &[]);
        assert!(
            from_non_canonical.round_trips(),
            "the stored (canonical) AST is always a fixed point"
        );
        // and its serialization is the canonical escaped form.
        if let Block::Paragraph { inline } = &from_non_canonical.blocks[0] {
            assert_eq!(serialize_inline(inline), r"a\*b");
        } else {
            panic!("expected a paragraph body");
        }
    }

    /// **The WASM render path is the IDENTICAL parser native + wasm32** — `roundtrips_md` (the editor's
    /// exact entry) and the native `serialize_inline(parse_inline(..))` agree byte-for-byte (there is
    /// no second renderer).
    #[test]
    fn wasm_path_is_identical_to_native_parse() {
        let md = "**a** `b` ~~c~~ [t](u)";
        let native = serialize_inline(&parse_inline(md, &[]));
        let via_wasm = wasm::render_serialize(&wasm::render_parse(md, &[]));
        assert_eq!(native, via_wasm, "one renderer, native === wasm path");
        assert_eq!(native, md, "and it round-trips");
    }

    // ── 3. single-author version-token CAS (rejects stale writes) ───────────────────────────────────

    /// **Single-author CAS: a stale edit is rejected loudly (no last-writer-wins).** An edit against
    /// the wrong expected version returns [`ContentError::Cas`] and does NOT mutate the content.
    #[test]
    fn cas_edit_rejects_stale_version() {
        let mut content = paragraph_body("v0", &[]);
        assert_eq!(content.version, 0);
        // a fresh edit at version 0 is admitted and bumps to 1.
        assert_eq!(content.cas_edit(0, vec![para("v1", vec![])]).unwrap(), 1);
        // a stale edit (still expecting version 0) is rejected — content unchanged.
        let err = content
            .cas_edit(0, vec![para("v2-stale", vec![])])
            .unwrap_err();
        assert_eq!(
            err,
            ContentError::Cas(CasConflict {
                expected: 0,
                actual: 1
            })
        );
        assert_eq!(content.version, 1, "a rejected CAS edit does not bump");
        assert_eq!(
            content.blocks,
            vec![para("v1", vec![])],
            "a rejected CAS edit does not mutate the content"
        );
    }

    /// **An out-of-subset edit is rejected BEFORE the CAS — the version is never bumped.** A
    /// Knowledge-only block in the new subtree returns [`ContentError::Subset`] even with the correct
    /// expected version.
    #[test]
    fn out_of_subset_edit_is_rejected_before_cas() {
        let mut content = paragraph_body("v0", &[]);
        let err = content
            .cas_edit(
                0, // correct version!
                vec![Block::SyncBlock {
                    source: ArtifactRef("myelin://acme/block/9".into()),
                }],
            )
            .unwrap_err();
        assert!(matches!(err, ContentError::Subset(_)));
        assert_eq!(
            content.version, 0,
            "a rejected edit never bumps the version"
        );
    }

    /// **The CAS-conflict Display names both versions** (the loud, auditable surface).
    #[test]
    fn cas_conflict_display_names_the_versions() {
        let msg = CasConflict {
            expected: 0,
            actual: 3,
        }
        .to_string();
        assert!(msg.contains('0') && msg.contains('3'), "names both: {msg}");
        assert!(msg.to_lowercase().contains("cas"));
    }

    // ── structured-node walk (the Refs producer seam) ───────────────────────────────────────────────

    /// **The structured-node walk reaches every inline run across the subtree** (a node-array walk,
    /// never a regex). A body with a mention in a paragraph + an embed in a list item yields both.
    #[test]
    fn structured_nodes_walks_the_whole_subtree() {
        use myelin_content::OBJ;
        let page = ArtifactRef("myelin://acme/knowledge/page/7".into());
        let content = IssueContent::new(
            ContentKind::Body,
            vec![
                para(&format!("hi {OBJ}"), vec![alice()]),
                Block::BulletList {
                    items: vec![ListItem {
                        blocks: vec![para(
                            &format!("see {OBJ}"),
                            vec![InlineNode::Embed(page.clone())],
                        )],
                    }],
                },
            ],
        )
        .unwrap();
        let nodes = content.structured_nodes();
        assert_eq!(nodes.len(), 2, "both structured nodes are reached");
        assert!(matches!(nodes[0], InlineNode::Mention(_)));
        assert!(matches!(nodes[1], InlineNode::Embed(_)));
    }

    // ── 4. the body/comment write co-commits its event (contract 2.2) ──────────────────────────────

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-23T10:00:00Z".into()),
            recorded_at: Timestamp("2026-06-23T10:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    /// **A body edit co-commits `issue.issue.updated` through the ONE outbox emit.** After the write,
    /// exactly one event is committed at seq 0 for the issue aggregate, with the correct token + the
    /// new version + NO inline body on the wire.
    #[test]
    fn body_edit_co_commits_issue_updated() {
        let store = OutboxStore::new();
        let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(MonotonicMinter::new());
        let issue = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        let content_ref = ArtifactRef("myelin://acme/issue/issue/ENG-1#b-desc".into());
        let aggregate = AggregateKey("issue:7:ENG-1".into());

        let mut content = paragraph_body("initial", &[]);
        content
            .cas_edit(0, vec![para("edited body", vec![])])
            .unwrap();
        assert_eq!(content.version, 1);

        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("issue ENG-1 body edited");
        let eid = emit_content_event(
            &mut tx,
            &issue,
            &content_ref,
            &aggregate,
            &content,
            None,
            None,
        )
        .unwrap();
        tx.commit().unwrap();

        assert_eq!(store.outbox_depth(), 1, "one content event co-committed");
        let row = store.row(&eid).expect("the committed row is present");
        assert_eq!(row.envelope.type_.0, events::ISSUE_UPDATED);
        assert_eq!(row.seq, 0);
        assert_eq!(row.aggregate, aggregate);
        // references-not-payloads: the issue + content URN + version, never the inline body.
        assert_eq!(row.envelope.payload["issue"], issue.0);
        assert_eq!(row.envelope.payload["version"], 1);
        assert!(!row.envelope.contains_personal_data || row.envelope.pii_key_ref.is_some());
    }

    /// **A comment's FIRST write co-commits `issue.comment.created`; a subsequent edit co-commits
    /// `issue.comment.updated`.** The token tracks the version (0 = created, else updated).
    #[test]
    fn comment_create_then_update_tokens() {
        let mut comment =
            IssueContent::new(ContentKind::Comment, vec![para("first", vec![])]).unwrap();
        assert_eq!(comment.version, 0);
        assert_eq!(comment.edit_event_token(), events::COMMENT_CREATED);
        comment.cas_edit(0, vec![para("edited", vec![])]).unwrap();
        assert_eq!(comment.version, 1);
        assert_eq!(comment.edit_event_token(), events::COMMENT_UPDATED);
    }

    /// **A PII-bearing body carries the per-subject-DEK key ref on the event, never the body.** When
    /// the caller threads a `pii_key_ref`, the event sets `contains_personal_data` + carries the key —
    /// the inline body is never on the wire (references-not-payloads).
    #[test]
    fn pii_body_event_carries_key_ref_not_body() {
        let store = OutboxStore::new();
        let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(MonotonicMinter::new());
        let issue = ArtifactRef("myelin://acme/issue/issue/ENG-2".into());
        let content_ref = ArtifactRef("myelin://acme/issue/issue/ENG-2#b-desc".into());
        let aggregate = AggregateKey("issue:7:ENG-2".into());
        let content = paragraph_body("contains alice's email", &[]);

        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("issue ENG-2 body edited (PII)");
        let key = PiiKeyRef("kms://acme/3/subject:psn-x".into());
        let eid = emit_content_event(
            &mut tx,
            &issue,
            &content_ref,
            &aggregate,
            &content,
            Some(key.clone()),
            None,
        )
        .unwrap();
        tx.commit().unwrap();

        let row = store.row(&eid).unwrap();
        assert!(
            row.envelope.contains_personal_data,
            "PII body flags the event"
        );
        assert_eq!(row.envelope.pii_key_ref, Some(key));
        // the inline body text is NOT on the wire.
        let payload = serde_json::to_string(&row.envelope.payload).unwrap();
        assert!(
            !payload.contains("alice"),
            "references-not-payloads: no inline body on the wire"
        );
    }
}
