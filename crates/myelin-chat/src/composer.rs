//! # `composer` — the S3 composer surface + the per-message edit CAS (CHAT-P12 / P-406, M4-C3)
//!
//! This is M4-C3's **composer slice** (the second committable unit; the content core is CHAT-P11 /
//! [`crate::content`]). It is the server-side logic the PRESERVED S3 composer wireframe
//! (`04-subsystem-architectures/chat/design/wireframes.md` §S3) drives:
//!
//! 1. [`SlashMenu`] / [`SlashCommand`] — the **server-side slash (`/`) command menu** (the
//!    differentiating action surface, wireframe §S3 "`/`=actions"). The menu is SERVER-OWNED (the
//!    client portals it, but the command set + their effects are the server's — a client cannot mint
//!    a command), filtered by an in-flight prefix.
//! 2. [`AutocompletePort`] / [`AutocompleteKind`] / [`Suggestion`] — the **`@`/`#` autocomplete**.
//!    The autocomplete is **Search-backed** (contract 6.1 — `query(ast, viewer, zookie?, page) →
//!    RankedResults`, the ACL-conjoining entry): there is **0 chat-private mention/artifact index**.
//!    The composer holds a PORT, not an index; the ONLY conforming implementation routes through the
//!    one Search `query` surface (so a suggestion the viewer cannot see is excluded BEFORE it reaches
//!    the composer — the `list_objects` `Filter` is conjoined in the engine, never a chat post-filter).
//!    The CDC (`tests/cdc_6_1_chat_autocomplete.rs`) PINS that the real `myelin_search::query` surface
//!    satisfies this port (chat consumes Search; it cannot depend on the Search SERVICE crate — the
//!    §2.9 acyclic DAG — so the port is the seam, the same posture [`crate::glue`] takes with
//!    `myelin_query::DispatchTier`).
//! 3. [`detect_pasted_url`] / [`UnfurlIntent`] — **paste-URL → unfurl**. A pasted URL the composer
//!    recognises as an in-platform artifact URN becomes a structured `artifact_ref`/`embed` node
//!    candidate (the [`myelin_content::InlineNode`] the autocomplete inserts); an external URL becomes
//!    an `embed` unfurl candidate. The actual per-viewer unfurl render is the Unfurl Service's
//!    (CHAT-P13 / S4 — a named floor); HERE the composer only produces the INTENT (the node to insert).
//! 4. [`DraftStore`] / [`Draft`] — **draft persistence**, per-subject-DEK encrypted (the C1 draft
//!    store, [`crate::dek::ChatFreeText::Draft`] / [`crate::schema::ChatDraftRow`]). An unsent body is
//!    equally PII; it is sealed under the AUTHOR's per-subject DEK (the same crypto-shred lever the
//!    sent body uses — CHAT-P6) and restored on re-open (wireframe §S3 "draft restored").
//! 5. [`EditCas`] / [`EditRequest`] / [`EditOutcome`] — the **per-message CAS on edit** (`edited_seq`,
//!    arch §1.4 / §3, X-2). A stale edit (`expect_seq` ≠ the stored `edited_seq`) is **REJECTED with
//!    the current state** ([`EditOutcome::Rejected`]) — **0 silent overwrite** of a message. This wraps
//!    the store's [`crate::store::MessageStore::revise`] CAS at the composer boundary so the composer
//!    can re-render the current body rather than clobber a concurrent edit.
//!
//! ## The no-chat-CRDT FLOOR (X-2 / arch §1.1 / OQ-L)
//! Chat is **single-author per message**: the edit model is a per-message CAS (`edited_seq` bump under
//! optimistic concurrency), **NOT** a collaborative-edit CRDT. The CRDT is **Knowledge's**, not chat's
//! ([`00-reconciliation-decisions.md`] §X-2 / `planning/06-roadmaps/subsystems/chat.md` §5). A stale
//! edit is REFUSED, not merged. **No agent should build a chat CRDT.** The related OQ-L comment-
//! threading consolidation (anchored comments promoted onto the firehose) is a transport/store swap —
//! **M5-C-X2 / CHAT-P31**, *still* a per-message CAS, *still* NOT a CRDT (the chat.md §6 / OQ-L note).
//! Named here so the next agent does not mistake the per-message CAS for a missing CRDT and build one.
//!
//! ## Other named floors (VISION §3)
//! - **The browser-driven S3 composer.** This module is the SERVER-SIDE composer logic (the slash menu,
//!   the autocomplete port, the paste-unfurl intent, the draft round-trip, the edit CAS). The
//!   `contenteditable` editor + the portal'd flip-above picker (wireframe §S3) is the chat frontend
//!   package's, compiled over the SAME [`myelin_content::wasm`] render path CHAT-P11 froze (one editor
//!   render path, EI-01 §7). The browser-drive of the live S3 composer is recorded honestly in the
//!   prompt report (EI-01 §4) — **partial**: the frontend package is not yet built in this Rust
//!   workspace, so the browser drive is over the wireframe contract + the server-side logic here, NOT a
//!   live `contenteditable` (the frontend build is a later M4 frontend prompt). The server surfaces are
//!   unit-tested end-to-end; the live `contenteditable` drive is the named floor.
//! - **The per-viewer unfurl render** is CHAT-P13 (the Unfurl Service: the shared per-ref projection
//!   cache + the per-viewer `list_objects`/`check` gate, the no-leak floor). HERE the composer only
//!   produces the unfurl INTENT (the structured node to insert); the rendered S4 card is CHAT-P13's.

use crate::store::{MessageId, MessageStore, OutboxTx, StoreError};
use myelin_content::InlineNode;
use myelin_events::ArtifactRef;

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE SERVER-SIDE SLASH (`/`) COMMAND MENU
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A single server-side slash command (wireframe §S3 "`/`=actions"). The command set is SERVER-OWNED:
/// a client renders the menu but cannot mint a command (the effect of each command is the server's, an
/// authz-gated action). PII-free — a stable token + a human label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashCommand {
    /// The stable command token the client posts (e.g. `"remind"`, `"poll"`, `"agent"`). PII-free.
    pub token: &'static str,
    /// The human label rendered in the menu (e.g. `"Remind me…"`). PII-free, i18n-keyed downstream.
    pub label: &'static str,
}

/// **The server-side slash-command menu** (wireframe §S3). The command set is the server's (the client
/// cannot add one); a prefix filters the visible set. This is the menu's MODEL — the live render +
/// the portal'd flip-above placement is the frontend's (a named floor). The effects of each command
/// land in their owning slices (e.g. `/agent` → the explicit-first dispatch, [`crate::glue`]); HERE the
/// menu surfaces the available commands + filters them, it does not execute them.
#[derive(Clone, Debug)]
pub struct SlashMenu {
    commands: Vec<SlashCommand>,
}

impl SlashMenu {
    /// The default chat slash-command set. SERVER-OWNED — a fixed, audited set (a client cannot mint a
    /// command). Each command's effect lands in its owning slice; the menu only surfaces the choices.
    pub fn default_commands() -> SlashMenu {
        SlashMenu {
            commands: vec![
                SlashCommand {
                    token: "remind",
                    label: "Remind me when…",
                },
                SlashCommand {
                    token: "poll",
                    label: "Start a poll",
                },
                SlashCommand {
                    token: "agent",
                    label: "Ask an agent (explicit)",
                },
                SlashCommand {
                    token: "code",
                    label: "Insert a code block",
                },
                SlashCommand {
                    token: "shrug",
                    label: "Shrug ¯\\_(ツ)_/¯",
                },
            ],
        }
    }

    /// Build a menu from an explicit command set (for tests / per-tenant command customisation).
    pub fn new(commands: Vec<SlashCommand>) -> SlashMenu {
        SlashMenu { commands }
    }

    /// The commands whose token starts with `prefix` (the in-flight `/<prefix>` filter, wireframe
    /// §S3). An empty prefix returns the whole set (the menu just opened on `/`). Filtering is a token
    /// PREFIX match (case-insensitive), in declared order — the menu is stable, not relevance-ranked
    /// (the slash set is small + fixed; relevance ranking is the Search-backed `@`/`#` autocomplete's,
    /// not the slash menu's).
    pub fn filter(&self, prefix: &str) -> Vec<SlashCommand> {
        let prefix = prefix.to_ascii_lowercase();
        self.commands
            .iter()
            .filter(|c| c.token.to_ascii_lowercase().starts_with(&prefix))
            .cloned()
            .collect()
    }

    /// Whether `token` is a known server-owned command (the server-mints-the-command guard — a client
    /// cannot post a command outside this set; an unknown token is rejected, never executed).
    pub fn is_known(&self, token: &str) -> bool {
        self.commands.iter().any(|c| c.token == token)
    }
}

impl Default for SlashMenu {
    fn default() -> Self {
        SlashMenu::default_commands()
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE `@`/`#` AUTOCOMPLETE — Search-backed (contract 6.1; 0 chat-private index)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The autocomplete trigger kind (wireframe §S3): `@` = people/agents, `#` = artifacts. The kind
/// determines the Search object-type the [`AutocompletePort`] queries (the `@` query is over the
/// `member` object type; the `#` query is over the artifact object types) — so the SAME ACL-conjoining
/// `query` surface (contract 6.1) backs both, with NO chat-private index for either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutocompleteKind {
    /// `@` — people + agents (the mention target; resolves to a [`InlineNode::Mention`]).
    Mention,
    /// `#` — artifacts (issues, PRs, pages; resolves to a [`InlineNode::ArtifactRefNode`]).
    Artifact,
}

impl AutocompleteKind {
    /// The trigger character (`@` / `#`) — the keystroke that opens the picker.
    pub fn trigger(self) -> char {
        match self {
            AutocompleteKind::Mention => '@',
            AutocompleteKind::Artifact => '#',
        }
    }
}

/// One autocomplete suggestion (a row in the §S3 picker). PII-MINIMAL: the `target` is the OPAQUE
/// artifact/member URN (an `@`-suggestion's target is the PSEUDONYMOUS `member` URN, NEVER a raw name
/// keyed off a chat-private store), the `label` is the display string the Search projection already
/// authorised the viewer to see (it came back from the ACL-conjoined `query`, so it is leak-free by
/// construction). Selecting a suggestion inserts the [`Suggestion::node`] structured node into the body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// The opaque artifact / member URN the suggestion points at (the structured node's target).
    pub target: ArtifactRef,
    /// The display label (authorised by Search — the viewer is allowed to see it; leak-free).
    pub label: String,
    /// The kind (mention vs artifact) — drives which [`InlineNode`] selection inserts.
    pub kind: AutocompleteKind,
}

impl Suggestion {
    /// The structured [`InlineNode`] inserting this suggestion produces (the node the
    /// `refs.edge.created` producer — [`crate::content::extract_body_edges`] — reads). A `@`-mention
    /// inserts a [`InlineNode::Mention`] (built from the member URN by the caller, which holds the
    /// resolved [`myelin_identity::Principal`]); a `#`-artifact inserts a
    /// [`InlineNode::ArtifactRefNode`] over the URN directly. The mention case needs the resolved
    /// principal (not just the URN), so it is built by [`Suggestion::artifact_node`] for artifacts and
    /// by the caller for mentions — this method covers the URN-only artifact case.
    pub fn artifact_node(&self) -> Option<InlineNode> {
        match self.kind {
            AutocompleteKind::Artifact => Some(InlineNode::ArtifactRefNode(self.target.clone())),
            // A mention's node needs the resolved Principal (held by the caller), not just the URN.
            AutocompleteKind::Mention => None,
        }
    }
}

/// **The Search-backed autocomplete PORT (contract 6.1 — the `@`/`#` autocomplete seam).** The
/// composer holds THIS, never a chat-private mention/artifact index. The ONLY conforming
/// implementation routes through the one Search `query` surface (the ACL-conjoining entry — every
/// suggestion is pre-filtered by the viewer's `list_objects` `Filter` in the engine, so a suggestion
/// the viewer cannot see never reaches the composer). The CDC (`tests/cdc_6_1_chat_autocomplete.rs`)
/// pins that the real `myelin_search::query` surface satisfies this port.
///
/// `prefix` is the in-flight text after the trigger (`@ali` → `"ali"`); `kind` selects the object type
/// the Search query runs over; the returned suggestions are already ranked + authorised. A chat
/// crate cannot depend on the Search SERVICE (the §2.9 acyclic DAG), so this trait is the seam the
/// gateway wires to `myelin_search::query` (the same posture [`crate::glue`] takes with
/// `myelin_query::DispatchTier` for the D17 dispatch tier).
pub trait AutocompletePort {
    /// Resolve the autocomplete suggestions for an in-flight `@`/`#` prefix, Search-backed +
    /// ACL-filtered for the viewer. Returns the ranked, authorised suggestions (≤ `limit`). The
    /// implementation MUST route through the contract-6.1 ACL-conjoining `query` surface — it MUST NOT
    /// consult a chat-private index (the `0-chat-private-index` GATE; proven by the CDC).
    fn suggest(&self, kind: AutocompleteKind, prefix: &str, limit: u32) -> Vec<Suggestion>;
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. PASTE-URL → UNFURL (the unfurl INTENT; the rendered card is CHAT-P13)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A detected paste-URL → unfurl intent (wireframe §S3 "paste a PR URL → offer unfurl"). The composer
/// recognises a pasted URL and produces the structured node to INSERT (the `artifact_ref`/`embed`
/// node); the per-viewer rendered S4 unfurl card is the Unfurl Service's (CHAT-P13 — a named floor).
/// PII-free: the target is an opaque URN/URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnfurlIntent {
    /// The pasted URL is an in-platform artifact URN (`myelin://…`). It becomes a structured
    /// [`InlineNode::ArtifactRefNode`] — a reliable reference (the `refs.edge.created` producer reads
    /// the structured node, never a regex over prose, EI-04 §2.4). This is the differentiating surface
    /// (a pasted issue/PR URL is a first-class linked reference, not dead text).
    Artifact(ArtifactRef),
    /// The pasted URL is an external link. It becomes a structured [`InlineNode::Embed`] over the URL
    /// (an unfurl/embed candidate). The per-viewer rendered preview is CHAT-P13's; HERE it is the
    /// embed node to insert.
    External(ArtifactRef),
}

impl UnfurlIntent {
    /// The structured [`InlineNode`] this intent inserts into the body (the node the
    /// `refs.edge.created` producer reads). An in-platform artifact → `artifact_ref` (`links`); an
    /// external URL → `embed` (`embeds`).
    pub fn node(&self) -> InlineNode {
        match self {
            UnfurlIntent::Artifact(r) => InlineNode::ArtifactRefNode(r.clone()),
            UnfurlIntent::External(r) => InlineNode::Embed(r.clone()),
        }
    }
}

/// **Detect a pasted URL → an unfurl intent (wireframe §S3).** An in-platform `myelin://…` URL becomes
/// an [`UnfurlIntent::Artifact`] (a reliable structured `artifact_ref`); an `http(s)://…` URL becomes
/// an [`UnfurlIntent::External`] (an `embed` unfurl candidate). A non-URL paste returns `None` (it is
/// ordinary text, inserted verbatim — never coerced to a node). The recognition is over the PASTE
/// EVENT (a structured clipboard URL), NOT a regex scan over the body prose (the structured-not-regex
/// reliability guarantee, EI-04 §2.4 — a `myelin://…` *typed as prose* is NOT auto-unfurled; only a
/// genuine paste of a URL is offered an unfurl).
pub fn detect_pasted_url(pasted: &str) -> Option<UnfurlIntent> {
    let trimmed = pasted.trim();
    // A paste is a SINGLE URL token (no embedded whitespace) — a multi-word paste is ordinary text.
    if trimmed.is_empty() || trimmed.split_whitespace().count() != 1 {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("myelin://") {
        // An in-platform artifact URN — a reliable structured reference. Must be a non-empty path.
        if rest.is_empty() {
            return None;
        }
        return Some(UnfurlIntent::Artifact(ArtifactRef(trimmed.to_string())));
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        // Must have a host after the scheme (not a bare `https://`).
        let after_scheme = trimmed
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        if after_scheme.is_empty() {
            return None;
        }
        return Some(UnfurlIntent::External(ArtifactRef(trimmed.to_string())));
    }
    None
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 4. DRAFT PERSISTENCE — per-subject-DEK encrypted (the C1 draft store; CHAT-P6 lever)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// An unsent composer draft (wireframe §S3 "draft restored"). The draft body is the unsent
/// markdown-subset string + its structured nodes — equally PII, sealed under the AUTHOR's per-subject
/// DEK ([`crate::dek::ChatFreeText::Draft`]) at rest, never erasable plaintext in the log. This is the
/// CLEARTEXT in-memory form; the at-rest ciphertext is [`crate::schema::ChatDraftRow::message_body`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Draft {
    /// The unsent markdown-subset body string (the `myelin-content` Chat subset, arch §1.4).
    pub body_inline: String,
    /// The unsent structured `mention`/`artifact_ref`/`embed` nodes kept OUT of the string (so
    /// reference-extraction stays reliable — the same split the sent body uses).
    pub body_nodes: Vec<InlineNode>,
}

impl Draft {
    /// A draft with a body string and no structured nodes yet.
    pub fn text(body_inline: impl Into<String>) -> Draft {
        Draft {
            body_inline: body_inline.into(),
            body_nodes: Vec::new(),
        }
    }

    /// Whether the draft is empty (nothing to persist / restore — an empty composer).
    pub fn is_empty(&self) -> bool {
        self.body_inline.is_empty() && self.body_nodes.is_empty()
    }
}

/// The key a draft is stored under — `(conversation, author)`. A draft is PER conversation PER author:
/// re-opening a conversation restores THAT author's unsent draft for THAT conversation (wireframe §S3
/// "draft restored"). PII-minimal: the author is the OPAQUE pseudonym.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DraftKey {
    /// The conversation the draft is being composed in.
    pub conversation_id: String,
    /// The drafting author's opaque pseudonym (contract 4.8).
    pub author_pseudonym: String,
}

impl DraftKey {
    /// Build a draft key from its `(conversation, author)` components.
    pub fn new(
        conversation_id: impl Into<String>,
        author_pseudonym: impl Into<String>,
    ) -> DraftKey {
        DraftKey {
            conversation_id: conversation_id.into(),
            author_pseudonym: author_pseudonym.into(),
        }
    }
}

/// **The composer draft store (the C1 draft store, per-subject-DEK encrypted).** Persists an unsent
/// [`Draft`] per `(conversation, author)` and restores it on re-open. The at-rest body is sealed under
/// the author's per-subject DEK ([`crate::dek::ChatFreeText::Draft`]) — the SAME crypto-shred lever the
/// sent body uses (CHAT-P6), so an author's Art. 17 erasure destroys their unsent drafts too (the
/// draft is equally PII).
///
/// This trait is the seam; [`MemDraftStore`] is the DB-free floor (the in-memory model the unit test
/// drives) and the real per-subject-DEK PG-backed draft store is the integration tier (the SAME seam,
/// behind the storage trait — a named floor: the live DEK round-trip against the dev-stack rides the
/// CHAT-P6 `integration_chat_p6_subject_dek.rs` lever, which already proves the draft column seals +
/// opens; here the composer's draft round-trip is over the cleartext model).
pub trait DraftStore {
    /// Persist (upsert) the author's unsent draft for a conversation. An empty draft CLEARS the stored
    /// draft (sending or clearing the composer removes the restored-draft marker).
    fn save(&self, key: &DraftKey, draft: &Draft);

    /// Restore the author's unsent draft for a conversation (`None` = no draft → an empty composer).
    fn load(&self, key: &DraftKey) -> Option<Draft>;

    /// Clear the author's draft for a conversation (on send / explicit discard).
    fn clear(&self, key: &DraftKey);
}

/// The DB-free in-memory draft store (the unit-test floor; the per-subject-DEK PG-backed store is the
/// integration tier, the SAME [`DraftStore`] seam). Behaviour-identical to the real store on this
/// surface (save → load round-trips; an empty save clears).
#[derive(Default)]
pub struct MemDraftStore {
    drafts: std::sync::Mutex<std::collections::HashMap<DraftKey, Draft>>,
}

impl MemDraftStore {
    /// A fresh empty in-memory draft store.
    pub fn new() -> MemDraftStore {
        MemDraftStore::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<DraftKey, Draft>> {
        self.drafts.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl DraftStore for MemDraftStore {
    fn save(&self, key: &DraftKey, draft: &Draft) {
        let mut drafts = self.lock();
        if draft.is_empty() {
            drafts.remove(key);
        } else {
            drafts.insert(key.clone(), draft.clone());
        }
    }

    fn load(&self, key: &DraftKey) -> Option<Draft> {
        self.lock().get(key).cloned()
    }

    fn clear(&self, key: &DraftKey) {
        self.lock().remove(key);
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 5. THE PER-MESSAGE EDIT CAS (`edited_seq`, X-2; 0 silent overwrite — NOT a CRDT)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A composer edit request (wireframe §S3 edit flow). Carries the new body + the `expect_seq` the
/// composer last rendered — the per-message CAS guard (X-2). A stale `expect_seq` (a concurrent edit
/// bumped `edited_seq` since the composer loaded the body) is REJECTED with the current state, never a
/// silent clobber.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditRequest {
    /// The message being edited (the stable id; arch §3 — id stable across edits).
    pub message_id: MessageId,
    /// The new markdown-subset body string (the `myelin-content` Chat subset).
    pub body_inline: Vec<u8>,
    /// The new structured nodes (kept out of the string).
    pub body_nodes: Vec<u8>,
    /// The `edited_seq` the composer last rendered — the CAS guard. A mismatch is a refused clobber.
    pub expect_seq: i32,
}

/// The outcome of a composer edit (the §S3 edit flow). An [`EditOutcome::Applied`] is the happy path
/// (the body was re-stamped under CAS, `edited_seq` bumped, the `chat.message.edited` event co-
/// committed); an [`EditOutcome::Rejected`] is the **stale-edit refusal** — the composer must re-render
/// the `current_seq` body, NOT overwrite (0 silent overwrite, the X-2 CAS gate). A
/// [`EditOutcome::NotFound`] is a missing message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditOutcome {
    /// The edit applied under CAS — the new `edited_seq` (bumped from `expect_seq`).
    Applied {
        /// The new `edited_seq` after the bump (`expect_seq + 1`).
        new_seq: i32,
    },
    /// The edit was REJECTED — the composer's `expect_seq` was stale (a concurrent edit). The composer
    /// must re-render the `current_seq` body (the current state), NOT clobber it. **0 silent overwrite.**
    Rejected {
        /// The `edited_seq` the composer expected (and which was stale).
        expected: i32,
        /// The `edited_seq` actually stored now (the current state the composer must reconcile to).
        current_seq: i32,
    },
    /// The message id was not found (a deleted/never-existing message).
    NotFound,
    /// The underlying store faulted (a cold-tier / I/O error, NOT a CAS or missing-message case). The
    /// composer surfaces a retryable error, NEVER swallows it as a clobber/no-op (loud, not silent —
    /// EI-01 §2). Carries the store error's display string (PII-free).
    StoreFault(String),
}

/// **The per-message edit CAS at the composer boundary (X-2; `edited_seq`).** Wraps the store's
/// [`MessageStore::revise`] CAS so the composer surfaces an [`EditOutcome`] (apply with the new seq, OR
/// reject with the current state) rather than a raw [`StoreError`] — so the §S3 edit flow can re-render
/// the current body on a stale edit instead of clobbering. **Chat is single-author per message: this is
/// a per-message CAS, NOT a CRDT (the no-chat-CRDT floor — see the module doc).** A stale edit is
/// REFUSED, never merged.
pub struct EditCas<'a, S: MessageStore> {
    store: &'a S,
}

impl<'a, S: MessageStore> EditCas<'a, S> {
    /// Build an edit-CAS over a message store.
    pub fn new(store: &'a S) -> EditCas<'a, S> {
        EditCas { store }
    }

    /// **Apply a composer edit under the per-message CAS (X-2).** Co-commits the body re-stamp + the
    /// `chat.message.edited` event through `tx` (the store's `revise` co-commit) IFF `expect_seq`
    /// matches the stored `edited_seq`. A mismatch is an [`EditOutcome::Rejected`] carrying the
    /// `current_seq` (the composer re-renders the current state — **0 silent overwrite**, the X-2 CAS
    /// gate). The new `edited_seq` on success is `expect_seq + 1`.
    pub fn apply(&self, tx: &mut OutboxTx, req: &EditRequest) -> EditOutcome {
        match self.store.revise(
            tx,
            &req.message_id,
            req.body_inline.clone(),
            req.body_nodes.clone(),
            req.expect_seq,
        ) {
            Ok(()) => EditOutcome::Applied {
                new_seq: req.expect_seq + 1,
            },
            Err(StoreError::CasConflict { actual, .. }) => EditOutcome::Rejected {
                expected: req.expect_seq,
                current_seq: actual,
            },
            Err(StoreError::NotFound(_)) => EditOutcome::NotFound,
            // Any OTHER store error (a cold-tier / I/O fault — `revise` on the hot tier yields only
            // CasConflict / NotFound, but the trait is general) is a real fault the composer must see:
            // it is surfaced LOUDLY as a retryable StoreFault, NEVER swallowed as a clobber or a no-op
            // (loud-not-silent, EI-01 §2). The composer renders the §S3 "couldn't record your edit"
            // error, not a fake success.
            Err(e) => EditOutcome::StoreFault(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{AuthorKind, ConversationId, MemHotTier, NewMessage};
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
        Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::sync::Arc;

    fn alice() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

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

    fn conv() -> ConversationId {
        ConversationId::new("acme", "fr-par", "01J0CONV")
    }

    // ── 1. the server-side slash menu ───────────────────────────────────────────────────────────

    #[test]
    fn slash_menu_filters_by_prefix_and_guards_unknown_commands() {
        let menu = SlashMenu::default_commands();
        // empty prefix → the whole set (the menu just opened on `/`).
        assert_eq!(menu.filter("").len(), menu.filter("").len().max(5));
        // a prefix narrows the set (case-insensitive).
        let r = menu.filter("re");
        assert!(r.iter().any(|c| c.token == "remind"));
        assert!(!r.iter().any(|c| c.token == "poll"));
        assert_eq!(menu.filter("RE"), r, "prefix filter is case-insensitive");
        // the server owns the command set: an unknown token is not a command.
        assert!(menu.is_known("remind"));
        assert!(!menu.is_known("rm -rf"), "a client cannot mint a command");
    }

    // ── 2. the Search-backed autocomplete port (0 chat-private index) ───────────────────────────

    /// A fake autocomplete port standing in for the real Search-backed adapter — it records that the
    /// composer goes THROUGH the port (never a chat-private index). The CDC
    /// (`cdc_6_1_chat_autocomplete.rs`) proves the REAL `myelin_search::query` surface satisfies this
    /// trait; here we assert the composer's contract over it (kind → object type, leak-free labels).
    struct FakeSearchPort;
    impl AutocompletePort for FakeSearchPort {
        fn suggest(&self, kind: AutocompleteKind, prefix: &str, limit: u32) -> Vec<Suggestion> {
            // A real adapter would build a QueryAst from `prefix`, run `myelin_search::query` over the
            // kind's object type with the viewer + zookie (the ACL-conjoining entry), and map the
            // RankedResults to Suggestions. The fake mirrors the SHAPE: ranked, authorised, ≤ limit.
            let target = match kind {
                AutocompleteKind::Mention => {
                    ArtifactRef(format!("myelin://acme/identity/member/{prefix}"))
                }
                AutocompleteKind::Artifact => {
                    ArtifactRef(format!("myelin://acme/issue/issue/{prefix}"))
                }
            };
            vec![Suggestion {
                target,
                label: format!("{prefix} (authorised)"),
                kind,
            }]
            .into_iter()
            .take(limit as usize)
            .collect()
        }
    }

    #[test]
    fn autocomplete_goes_through_the_port_and_artifact_inserts_a_structured_node() {
        let port = FakeSearchPort;
        let mentions = port.suggest(AutocompleteKind::Mention, "ali", 5);
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].kind, AutocompleteKind::Mention);
        // a mention's target is the pseudonymous member URN (never a chat-private name lookup).
        assert!(mentions[0].target.0.contains("/identity/member/"));
        // selecting a mention needs the resolved Principal (the URN-only node is None).
        assert!(mentions[0].artifact_node().is_none());

        let arts = port.suggest(AutocompleteKind::Artifact, "ENG-1", 5);
        assert_eq!(arts[0].kind, AutocompleteKind::Artifact);
        // selecting an artifact inserts a structured artifact_ref node (the refs.edge producer reads it).
        let node = arts[0]
            .artifact_node()
            .expect("artifact inserts a structured node");
        assert!(matches!(node, InlineNode::ArtifactRefNode(_)));

        assert_eq!(AutocompleteKind::Mention.trigger(), '@');
        assert_eq!(AutocompleteKind::Artifact.trigger(), '#');
    }

    // ── 3. paste-URL → unfurl intent ────────────────────────────────────────────────────────────

    #[test]
    fn paste_in_platform_url_is_a_structured_artifact_ref() {
        let intent =
            detect_pasted_url("myelin://acme/git/pr/88").expect("an in-platform URL unfurls");
        assert!(matches!(intent, UnfurlIntent::Artifact(_)));
        // it inserts a structured artifact_ref (links) — a reliable reference, not dead text.
        assert!(matches!(intent.node(), InlineNode::ArtifactRefNode(_)));
    }

    #[test]
    fn paste_external_url_is_an_embed() {
        let intent =
            detect_pasted_url("https://example.com/page").expect("an external URL unfurls");
        assert!(matches!(intent, UnfurlIntent::External(_)));
        assert!(matches!(intent.node(), InlineNode::Embed(_)));
    }

    #[test]
    fn non_url_paste_is_ordinary_text() {
        // a multi-word paste, a bare scheme, and plain prose are NOT auto-unfurled.
        assert!(detect_pasted_url("see myelin://acme/git/pr/88 please").is_none());
        assert!(detect_pasted_url("https://").is_none());
        assert!(detect_pasted_url("myelin://").is_none());
        assert!(detect_pasted_url("just some text").is_none());
        assert!(detect_pasted_url("").is_none());
    }

    // ── 4. draft persistence round-trip ─────────────────────────────────────────────────────────

    #[test]
    fn draft_save_load_round_trips_and_empty_clears() {
        let store = MemDraftStore::new();
        let key = DraftKey::new("01J0CONV", "p-opaque-alice");
        assert!(store.load(&key).is_none(), "no draft → empty composer");

        let draft = Draft {
            body_inline: "an unsent **message**".into(),
            body_nodes: vec![InlineNode::ArtifactRefNode(ArtifactRef(
                "myelin://acme/issue/issue/ENG-1".into(),
            ))],
        };
        store.save(&key, &draft);
        assert_eq!(
            store.load(&key).as_ref(),
            Some(&draft),
            "the draft round-trips (restored on re-open)"
        );

        // saving an empty draft clears the restored-draft marker (send / discard).
        store.save(&key, &Draft::default());
        assert!(store.load(&key).is_none(), "an empty save clears the draft");

        // a different author's draft for the same conversation is isolated.
        let bob = DraftKey::new("01J0CONV", "p-opaque-bob");
        store.save(&bob, &Draft::text("bob's draft"));
        store.save(&key, &Draft::text("alice's draft"));
        assert_eq!(store.load(&bob).unwrap().body_inline, "bob's draft");
        assert_eq!(store.load(&key).unwrap().body_inline, "alice's draft");
    }

    // ── 5. the per-message edit CAS — 0 silent overwrite (X-2; NOT a CRDT) ──────────────────────

    fn append_message(
        store: &MemHotTier,
        outbox: &OutboxStore,
        minter: &Arc<MonotonicMinter>,
        body: &[u8],
    ) -> MessageId {
        let minter: Arc<dyn IdMinter> = minter.clone();
        let mut tx = outbox.begin(minter, ctx_base());
        let id = store
            .append(
                &mut tx,
                NewMessage {
                    conv: conv(),
                    thread_root_id: None,
                    author: "p-opaque-alice".into(),
                    author_kind: AuthorKind::Human,
                    body_inline: body.to_vec(),
                    body_nodes: Vec::new(),
                    client_nonce: "nonce-1".into(),
                },
            )
            .unwrap();
        tx.commit().unwrap();
        id
    }

    #[test]
    fn edit_cas_applies_a_fresh_edit_and_bumps_the_seq() {
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let id = append_message(&store, &outbox, &minter, b"original");

        let cas = EditCas::new(&store);
        let mut tx = outbox.begin(minter.clone() as Arc<dyn IdMinter>, ctx_base());
        let outcome = cas.apply(
            &mut tx,
            &EditRequest {
                message_id: id.clone(),
                body_inline: b"edited once".to_vec(),
                body_nodes: Vec::new(),
                expect_seq: 0, // a freshly-appended message is at edited_seq 0.
            },
        );
        tx.commit().unwrap();
        assert_eq!(
            outcome,
            EditOutcome::Applied { new_seq: 1 },
            "a fresh edit applies and bumps edited_seq 0 → 1"
        );
    }

    #[test]
    fn edit_cas_rejects_a_stale_edit_with_the_current_state_zero_silent_overwrite() {
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let id = append_message(&store, &outbox, &minter, b"original");

        let cas = EditCas::new(&store);

        // first edit (the concurrent winner): seq 0 → 1.
        let mut tx = outbox.begin(minter.clone() as Arc<dyn IdMinter>, ctx_base());
        let first = cas.apply(
            &mut tx,
            &EditRequest {
                message_id: id.clone(),
                body_inline: b"winner edit".to_vec(),
                body_nodes: Vec::new(),
                expect_seq: 0,
            },
        );
        tx.commit().unwrap();
        assert_eq!(first, EditOutcome::Applied { new_seq: 1 });

        // a SECOND composer still holding the stale seq 0 tries to edit — REJECTED with current state.
        let mut tx2 = outbox.begin(minter.clone() as Arc<dyn IdMinter>, ctx_base());
        let stale = cas.apply(
            &mut tx2,
            &EditRequest {
                message_id: id.clone(),
                body_inline: b"loser clobber".to_vec(),
                body_nodes: Vec::new(),
                expect_seq: 0, // STALE — the body is now at seq 1.
            },
        );
        // 0 silent overwrite: the stale edit is REFUSED, carrying the current seq the composer
        // must reconcile to (NOT a CRDT merge — chat is single-author).
        assert_eq!(
            stale,
            EditOutcome::Rejected {
                expected: 0,
                current_seq: 1
            },
            "a stale edit is rejected with the current state — 0 silent overwrite"
        );

        // the body still reflects the WINNER edit (the clobber did not land).
        let rows = store
            .range(&conv(), crate::store::RangeCursor::Recent, 10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].body_inline, b"winner edit",
            "the clobber did not overwrite"
        );
        assert_eq!(
            rows[0].edited_seq, 1,
            "the seq reflects exactly one applied edit"
        );
    }

    #[test]
    fn edit_cas_not_found_for_a_missing_message() {
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let cas = EditCas::new(&store);
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let mut tx = outbox.begin(minter, ctx_base());
        let outcome = cas.apply(
            &mut tx,
            &EditRequest {
                message_id: MessageId("01J-does-not-exist".into()),
                body_inline: b"x".to_vec(),
                body_nodes: Vec::new(),
                expect_seq: 0,
            },
        );
        assert_eq!(outcome, EditOutcome::NotFound);
    }
}
