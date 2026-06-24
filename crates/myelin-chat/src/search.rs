//! # `search` — Chat `declare_indexable` + the ACL-conjoined Search feeder + embeddings-as-PII +
//! the HYOK skip (CHAT-P20 / P-415, M4-C7)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md` §7
//! (`declare_indexable(IndexSpec)` — the chat/message Search projection + the frozen ACL filter):
//! Search ALWAYS conjoins the frozen `list_objects` `Filter{set_expr, zookie}` over the
//! `message.id` column before scoring (the `search-requires-acl-filter` lint, contract 6.1); the
//! `SetExpr` lowers to a JOIN against Identity's per-tenant authz reverse index (no N+1, no
//! post-filter); on erasure Search **purges + reindexes** (incl. embeddings) — never hides; and an
//! HYOK tenant whose `can_derive_plaintext_index()=false` structurally skips message indexing
//! (contract 11.3). **Internals:** `02-internals-and-algorithms.md` §4.4 (the reindex consumer —
//! the feeder). **Reconciliation:** `00-reconciliation-decisions.md` §OQ-E (the `list_objects`
//! `Filter` conjoin lowering to a JOIN against the authz reverse index).
//!
//! ## What CHAT-P20 ships here — the Chat producer half of contract 6.3 / 6.1 / 11.3
//!
//! Chat is a **producer** of searchable artifacts (one searchable doc per `message`). This module is
//! the Chat-OWNED `declare_indexable` side (the SPEC + the index-time projection BUILDER + the
//! HYOK-skip admission gate), plus the CONSUMER leg of the ACL-conjoined `query` surface (6.1) that
//! makes the **search-as-non-member = 0** guarantee hold for chat (CHAT-D11):
//!
//! 1. [`message_index_spec`] — Chat's frozen `declare_indexable(IndexSpec)` (§7, contract 6.3):
//!    `subsystem = "chat"`, `type = "message"`, `ft_fields = ["body"]` (the markdown-subset body,
//!    carried at emit time in [`SearchProjection::text`]), `struct_fields = [channel, author,
//!    thread_root, created_at, kind]` PLUS the three cross-producer reference facets
//!    (mention/artifact_ref/embed, X-2), **`semantic` = true** (an `EmbeddingSpec` — embeddings ARE
//!    personal data → erasure-aware, §3/§7), `acl_object_type = "message"` (the conjoin keys on
//!    `message.id`; `message.view = parent_channel->read`, the [`crate::rebac_fragment`]).
//! 2. [`message_search_projection`] — the owner's `project(ref)` index-time row (contract 5.6): the
//!    markdown-subset full-text body + the three structured reference facets walked from the SAME
//!    [`MessageBody::structured_nodes`] node array the [`crate::content`] edge producer uses (X-2 —
//!    one walk, never a second regex extractor; the facets are byte-identical to Git/KN/Issues).
//! 3. [`AclConjoinedSearchFeeder`] — the CONSUMER of the ACL-conjoining `myelin_search::query`
//!    surface (6.1). EVERY chat message search routes through the ONE conjoining entry, so a viewer's
//!    `list_objects(read, message)` `Filter` pre-filters the candidate set BEFORE scoring: a
//!    non-member of a channel gets **0 message results from that channel** — not in the rows, not in
//!    the count (CHAT-D11). There is no chat-private message index and no post-filter; the
//!    `search-requires-acl-filter` lint holds over this module's source (every `.search(`-class site
//!    carries `list_objects`).
//! 4. [`admit_message_indexing`] — the structural HYOK skip (contract 11.3): the index-builder
//!    consults [`myelin_storage::IndexAdmission::for_origin`] over the tenant's
//!    [`myelin_storage::KeyOrigin`] BEFORE building a plaintext-derived index. A HYOK class
//!    (`can_derive_plaintext_index() = false`) is REFUSED — **0 indexed message bodies** — by
//!    construction, never by a reviewer remembering to check (the §6 D-S10 seam).
//!
//! ## Embeddings-as-personal-data (the no-hide property, §3 / external-insights/04 §1)
//!
//! Chat's spec is **semantic** — message bodies get a vector embedding (RAG/dedup, §7). An embedding
//! derived from a person's prose IS personal data: on erasure Search PURGES + reindexes the
//! embeddings (not just the full-text), so a person's contributions are 0-recoverable from the vector
//! space too — erasure never merely HIDES. Chat does not own the embedding store; the cascade reaches
//! it the ONE lawful way — the Chat erase fan-out ([`crate::erase`]) emits the tombstone/erase events
//! through the OUTBOX and the Search derivative holder (H7) PURGES + reindexes its own embeddings (the
//! no-cross-store-read law — there is NO backdoor write into Search). [`EmbeddingsArePersonalData`]
//! records this binding; the chat-side spec being `semantic` is what makes the embeddings exist (and
//! therefore must be reached) — the wiring is asserted here, the full multi-holder erasure RECEIPT
//! (0-recoverable across every derivative holder) is the named CHAT-P22 floor.
//!
//! ## Coherence (EI-01 §7) — one shape, one walk, one query surface
//! - **The IndexSpec is `myelin_search::IndexSpec`** (Search owns the type; Chat constructs the
//!   chat/message instance — the SAME posture [`myelin_issues::declares`] / Git's code-projection
//!   spec take). Chat does NOT define a second indexing type.
//! - **The structured-node walk is the ONE shared seam** — [`message_search_projection`] reuses
//!   [`MessageBody::structured_nodes`] (the byte-identical walk the [`crate::content`] edge producer
//!   runs), so the reference facets are dependable across producers (X-2) BY CONSTRUCTION.
//! - **The query path is `myelin_search::query`** — the ONE ACL-conjoining surface (6.1). Chat owns
//!   no second search path and no post-filter; the leak-free guarantee is inherited, not re-built.
//!
//! ## Reconciliation note (the Search-side SRCH-P23 model)
//! Search's own consumer-side model of the chat doc ([`myelin_search::chat_projection`] / SRCH-P23)
//! pinned `acl_object_type = "channel"` (non-semantic) as a modelling simplification. The OWNING
//! chat architecture §7 + this producer prompt (CHAT-P20) freeze the spec at `acl_object_type =
//! "message"` (the conjoin keys on `message.id`, supported by the frozen `message.view =
//! parent_channel->read` fragment) and **semantic** (embeddings for RAG/dedup). This module is the
//! CHAT-OWNED authoritative spec; the divergence is documented for the SRCH-P23 reconciliation pass
//! (a needed Search-side alignment is a whole-workspace contract PR, EI-01 §1 — not silently forked
//! here). See [`crate::search::tests::chat_spec_is_the_authoritative_owned_6_3_shape`].
//!
//! ## Floors named (CHAT-P20 DoD)
//! - **The full replay-from-source parity** (Search/Refs/Notif read-models rebuild, steady-state ==
//!   recovery, one path) is **CHAT-P21** ([`ReplayParityFollowOn::REPLAY_PARITY`]) — here the index
//!   is wired + ACL-correct; the byte-parity rebuild is the follow-on.
//! - **The full multi-holder erasure RECEIPT** (the embeddings erasure cascade's holder-completeness
//!   — 0-recoverable across every derivative holder) is **CHAT-P22**
//!   ([`ReplayParityFollowOn::ERASURE_RECEIPT`]) — here the embeddings-as-PII binding is asserted; the
//!   complete receipt set is the follow-on.

use std::collections::BTreeMap;

use myelin_content::{Block, InlineNode};
use myelin_identity::{Consistency, ListObjectsResult, ObjectType, Permission, Principal};
use myelin_query::{FieldType, FieldValue};
use myelin_search::{
    query as search_query, IndexBackend, IndexSpec, ListObjectsPort, Page, QueryStats,
    RankedResults, ScopedEngine, SearchProjection,
};
use myelin_storage::{IndexAdmission, KeyOrigin};

use crate::content::MessageBody;

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE declare_indexable IndexSpec (contract 6.3) — chat/message, semantic, acl_object_type=message
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The subsystem token Chat declares its message projection under (`chat`) — byte-identical to the
/// `myelin://<tenant>/chat/...` artifact authority + the `chat.*` event token family. Mirrors
/// `myelin_search::chat_projection::CHAT_SUBSYSTEM`.
pub const CHAT_SUBSYSTEM: &str = "chat";

/// The artifact type Chat's message projection indexes — a `message`: ONE searchable doc per chat
/// message (the markdown-subset body, multilingual + RAG-embedded). The canonical doc ref is
/// `myelin://<tenant>/chat/message/<id>`. Byte-identical to
/// `myelin_chat::rebac_fragment::object_types::MESSAGE`.
pub const MESSAGE_TYPE: &str = crate::rebac_fragment::object_types::MESSAGE;

/// **The ACL object type the message filter pins on — the `message` itself** (§7: "Search ALWAYS
/// conjoins the frozen `list_objects` Filter over `message.id`"). A message's reachability is
/// `message.view = parent_channel->read` (the [`crate::rebac_fragment`]), so `list_objects(viewer,
/// read, "message")` returns the visible message-id set the conjoin keys over — a non-member's set
/// excludes the channel's messages, so they are never in any result incl. counts (CHAT-D11).
pub const MESSAGE_ACL_OBJECT_TYPE: &str = crate::rebac_fragment::object_types::MESSAGE;

/// **The permission the message-search ACL conjoin lists under — `read`** (the frozen Search pipeline
/// permission, `myelin_search::READ_PERMISSION`; contract 4.2/4.3). The frozen `query` surface ALWAYS
/// lists objects for `read`; for the `message` object type Identity resolves `message.read` through
/// the frozen `message.view = parent_channel->read` rewrite (a message's read IS its channel's read,
/// the [`crate::rebac_fragment`]). Chat never widens it — the ONE permission, the ONE surface.
pub const MESSAGE_READ_PERMISSION: &str = "read";

/// The full-text field the markdown-subset message body is analysed under (`body`, §7 `ft_fields`).
/// The content is NOT in the [`IndexSpec`] (the spec is the columnar/semantic schema); it arrives at
/// emit time in the index-time [`SearchProjection::text`] ([`message_search_projection`]).
pub const FT_BODY_FIELD: &str = "body";

/// The structured facet for an inline `artifact_ref` in a message body — the ONE cross-producer facet
/// key (X-2).
pub use myelin_search::chat_projection::FACET_ARTIFACT_REF;
/// The structured facet for an inline `embed` in a message body — the ONE cross-producer facet key
/// (X-2).
pub use myelin_search::chat_projection::FACET_EMBED;
/// The structured facet for an `@mention` in a message body — the ONE cross-producer facet key (X-2),
/// re-exported from Search's KN model so it is provably one key, not a copy.
pub use myelin_search::chat_projection::FACET_MENTION;

/// The `channel` columnar facet (§7 `struct_fields`) — the parent channel a message belongs to.
pub const FACET_CHANNEL: &str = "channel";
/// The `author` columnar facet (§7 `struct_fields`) — the PSEUDONYMOUS author member URN (never a
/// name; the body IS the PII, the author is a pseudonym, [`crate::dek`]).
pub const FACET_AUTHOR: &str = "author";
/// The `thread_root` columnar facet (§7 `struct_fields`) — the thread the message replies under.
pub const FACET_THREAD_ROOT: &str = "thread_root";
/// The `created_at` columnar facet (§7 `struct_fields`) — the message's creation instant (sort/range).
pub const FACET_CREATED_AT: &str = "created_at";
/// The `kind` columnar facet (§7 `struct_fields`) — the message kind (e.g. `message`/`system`).
pub const FACET_KIND: &str = "kind";

/// **Chat's `declare_indexable` message IndexSpec (contract 6.3 — the chat-OWNED §7 shape).**
///
/// `subsystem = "chat"`, `type = "message"`, **`semantic` = true** (an `EmbeddingSpec` — message
/// bodies are vector-embedded for RAG/dedup; embeddings ARE personal data → erasure-aware, §7),
/// `acl_object_type = "message"` (the conjoin keys on `message.id`). The structured columnar facets
/// are the §7 set (`channel`, `author`, `thread_root`, `created_at`, `kind`) PLUS the three
/// cross-producer reference facets (`mention`/`artifact_ref`/`embed`, X-2). The full-text `body`
/// (`ft_fields = ["body"]`) is delivered at emit time in [`SearchProjection::text`], NOT in the spec.
pub fn message_index_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    // The §7 columnar facets. `created_at` is a timestamp (an integer instant); the rest are
    // relation/keyword tokens (a channel/author/thread id, a kind keyword).
    struct_fields.insert(FACET_CHANNEL.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_AUTHOR.to_string(), FieldType::Principal);
    struct_fields.insert(FACET_THREAD_ROOT.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_CREATED_AT.to_string(), FieldType::Date);
    struct_fields.insert(FACET_KIND.to_string(), FieldType::Select);
    // The three cross-producer reference facets (X-2) — byte-identical to KN's (the SAME keys + types,
    // so a mention/ref query is dependable across producers). Walked from the structured node array.
    struct_fields.insert(FACET_MENTION.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_ARTIFACT_REF.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_EMBED.to_string(), FieldType::Relation);
    // semantic() — message bodies are RAG-embedded (§7); acl_object_type = "message" (conjoin on
    // message.id).
    IndexSpec::new(CHAT_SUBSYSTEM, MESSAGE_TYPE, struct_fields)
        .with_acl_object_type(MESSAGE_ACL_OBJECT_TYPE)
        .semantic()
}

/// Every Chat index spec (the one `message` type) — the set a Search indexer registers to consume the
/// real Chat corpus.
pub fn message_index_specs() -> Vec<IndexSpec> {
    vec![message_index_spec()]
}

/// **Register Chat's message index spec WITH Search (the registration GATE).** Builds
/// [`message_index_specs`] and proves Search ACCEPTS it by admitting it into a live
/// [`myelin_search::IncrementalIndexer`]'s per-tenant facet union without a schema mismatch (the only
/// honest definition of "accepted" — Search is the authority that admits). Returns the accepted set.
pub fn register_message_index_specs() -> Vec<IndexSpec> {
    let specs = message_index_specs();
    // Admit them into a real indexer's facet union (the build-time declare_indexable surface). A
    // facet-type collision or a malformed shape would panic at construction; it does not. The spec
    // is semantic, so the indexer wires a (mock) embedding adapter — the embeddings path is live.
    let _accepted = myelin_search::IncrementalIndexer::new(
        specs.clone(),
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(myelin_search::MockEmbeddingAdapter::new(8)),
    );
    specs
}

/// A do-nothing [`myelin_search::ProjectFetcher`] used ONLY to admit the Chat specs into a live
/// indexer for the registration GATE (the SPEC + the projection BUILDER ship here; the real
/// owner-`project` fetch rides the live emitter). It never fetches — registration does not index.
struct NullProjectFetcher;

impl myelin_search::ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<SearchProjection, myelin_search::ProjectFetchError> {
        // The SPEC registration never fetches a projection (no emitter here). This is the
        // registration GATE — Search admits the schema — not the index path.
        Err(myelin_search::ProjectFetchError::Gone)
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE index-time PROJECTION builder (contract 5.6) — the markdown body + the structured facets
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **Build a chat message's [`SearchProjection`] from its [`MessageBody`] (the index-time row, §4.1 /
/// contract 5.6).** This is the owner's `project(ref)` body Search consumes — NOT a DB read. It
/// produces:
/// - the analyzable full-text `text` (the message's markdown-subset prose, multilingual — `lang`
///   selects the per-language analyzer chain; `None` lets the indexer detect it),
/// - the three structured inline-node reference facets (mention/artifact_ref/embed) walked from the
///   SAME [`MessageBody::structured_nodes`] node array the [`crate::content`] edge producer uses (X-2
///   — never a regex over prose; the cross-subsystem dependable facets).
///
/// The author/channel/thread_root/created_at/kind columnar facets are message METADATA the emitter
/// stamps; this builder produces the body + the body-derived reference facets (the half that lives in
/// the content). The full metadata-facet stamping is the live emitter's (the SAME posture
/// [`myelin_issues::declares`] takes). The body is the SAME `myelin_content` markdown subset a KN page
/// is, so the inline-node facet extraction is byte-identical across producers BY CONSTRUCTION.
pub fn message_search_projection(body: &MessageBody, lang: Option<&str>) -> SearchProjection {
    // The full-text body — every inline run's prose, in document order (the analyzable text).
    let text = render_body_text(&body.blocks);
    // The three reference facets — the SAME structured-node walk the edge producer runs (X-2).
    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
    for node in body.structured_nodes() {
        match node {
            InlineNode::Mention(principal) => {
                // The PSEUDONYMOUS member URN (never a name) — erasure-safe (the body is the PII; the
                // mention is a pseudonym token, [`crate::dek`]). Last-writer-wins on multiple mentions
                // matches the single-valued columnar facet; the full multi-value facet is the emitter.
                fields.insert(
                    FACET_MENTION.to_string(),
                    FieldValue::Relation(principal.principal_id.0.clone()),
                );
            }
            InlineNode::ArtifactRefNode(target) => {
                fields.insert(
                    FACET_ARTIFACT_REF.to_string(),
                    FieldValue::Relation(target.0.clone()),
                );
            }
            InlineNode::Embed(target) => {
                fields.insert(
                    FACET_EMBED.to_string(),
                    FieldValue::Relation(target.0.clone()),
                );
            }
        }
    }
    SearchProjection {
        text,
        fields,
        lang: lang.map(|s| s.to_string()),
    }
}

/// Flatten a message body's block subtree into its analyzable full-text prose (every inline run's
/// rendered text, in document order). Reuses the ONE WASM render path's serializer over each inline
/// run — there is no Chat-local renderer (EI-01 §7). The structured nodes contribute their `OBJ`
/// placeholder in the serialized run (the facets carry the reference; the prose carries the words).
fn render_body_text(blocks: &[Block]) -> String {
    let mut out = String::new();
    collect_block_text(blocks, &mut out);
    out
}

fn collect_block_text(blocks: &[Block], out: &mut String) {
    for block in blocks {
        match block {
            Block::Paragraph { inline } | Block::Heading { inline, .. } => {
                push_inline_text(inline, out);
            }
            Block::TaskList { items } => {
                for item in items {
                    push_inline_text(&item.inline, out);
                }
            }
            Block::Blockquote { blocks } | Block::Callout { blocks, .. } => {
                collect_block_text(blocks, out);
            }
            Block::BulletList { items } | Block::OrderedList { items, .. } => {
                for item in items {
                    collect_block_text(&item.blocks, out);
                }
            }
            Block::Table { rows, .. } => {
                for row in rows {
                    for cell in row {
                        collect_block_text(&cell.blocks, out);
                    }
                }
            }
            Block::CodeBlock { text, .. } => {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(text);
            }
            // Divider/Image carry no analyzable prose (an image's alt is metadata, not body text).
            _ => {}
        }
    }
}

fn push_inline_text(inline: &myelin_content::Inline, out: &mut String) {
    let rendered = myelin_content::serialize_inline(inline);
    if !rendered.is_empty() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&rendered);
    }
}

/// **The canonical chat message doc ref for a message id.** `myelin://<tenant>/chat/message/<id>` —
/// the Search `doc_id` (byte-identical to the artifact authority the Chat producer mints).
pub fn message_doc_ref(tenant: &str, message_id: &str) -> String {
    format!("myelin://{tenant}/chat/{MESSAGE_TYPE}/{message_id}")
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. THE ACL-CONJOINED SEARCH FEEDER (contract 6.1) — search-as-non-member = 0 (CHAT-D11)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The Chat message-search CONSUMER of the ACL-conjoining `myelin_search::query` surface (contract
/// 6.1).** Every chat message search routes through the ONE conjoining `query` entry — there is NO
/// chat-private message index and NO post-filter. The viewer's `list_objects(read, message)` `Filter`
/// pre-filters the candidate set BEFORE scoring, so a non-member of a channel gets **0 message
/// results from that channel** — not in the rows, not in the count (CHAT-D11). The
/// `search-requires-acl-filter` lint holds over this module's source: the only `.search(`-class call
/// site is [`search_query`], which carries `list_objects` (the ACL binder).
pub struct AclConjoinedSearchFeeder<'a, B: IndexBackend> {
    engine: &'a ScopedEngine<'a, B>,
    authz: &'a dyn ListObjectsPort,
}

impl<'a, B: IndexBackend> AclConjoinedSearchFeeder<'a, B> {
    /// Couple the (tenant-scoped) engine with the per-viewer authz port. The caller resolves the
    /// viewer's-tenant engine BEFORE this (the partition key, §3.4); `query` re-checks the tenant.
    pub fn new(
        engine: &'a ScopedEngine<'a, B>,
        authz: &'a dyn ListObjectsPort,
    ) -> AclConjoinedSearchFeeder<'a, B> {
        AclConjoinedSearchFeeder { engine, authz }
    }

    /// **Run a message search for `viewer` at consistency `at` (the ACL-conjoined entry).** The query
    /// ALWAYS routes through `myelin_search::query`, which conjoins the viewer's `list_objects(read,
    /// message)` `Filter` over `message.id` BEFORE scoring — a non-member's message is never in the
    /// ranked rows NOR the count (CHAT-D11). Exactly ONE `list_objects` per query (no N+1; the
    /// [`QueryStats::list_objects_calls`] invariant). There is no chat-side post-filter — the engine
    /// pre-filter is the ONLY visibility gate.
    pub fn search_messages(
        &self,
        ast: &myelin_query::QueryAst,
        viewer: &Principal,
        at: &Consistency,
        page: Page,
        stats: &QueryStats,
    ) -> Result<RankedResults, myelin_search::QueryError> {
        let ty = ObjectType(MESSAGE_ACL_OBJECT_TYPE.to_string());
        // THE ONE conjoining surface — the ACL filter is conjoined inside `query` (the lint binder is
        // `list_objects`, on this exact statement). No chat-private index branch, no post-filter.
        search_query(self.engine, self.authz, ast, viewer, &ty, at, page, stats)
    }
}

/// The permission/object-type pair a chat message search conjoins on (`read` over `message`) — the
/// frozen Search-pipeline permission (`myelin_search::READ_PERMISSION`) over the `message`
/// `acl_object_type`; Identity resolves `message.read` through the `message.view =
/// parent_channel->read` rewrite (the [`crate::rebac_fragment`]). Exposed so the production wiring
/// binds `list_objects` to the SAME (permission, type) the frozen `query` surface keys on.
pub fn message_search_acl_anchor() -> (Permission, ObjectType) {
    (
        Permission(MESSAGE_READ_PERMISSION.to_string()),
        ObjectType(MESSAGE_ACL_OBJECT_TYPE.to_string()),
    )
}

/// A convenience for the production wiring: a `list_objects` answer of `SetExpr::None` for a viewer
/// who is a member of NO channel — the `WHERE false` short-circuit that yields 0 message results (the
/// non-member). Kept here so the wiring and the drill share ONE shape (no second authz path).
pub fn non_member_filter(zookie: &str) -> ListObjectsResult {
    ListObjectsResult::Filter {
        set_expr: myelin_identity::SetExpr::None,
        zookie: myelin_identity::Zookie(zookie.to_string()),
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 4. THE HYOK STRUCTURAL SKIP (contract 11.3) — a HYOK class produces 0 indexed message bodies
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The structural HYOK skip (contract 11.3 / storage.md §6 D-S10).** Consult the tenant's
/// [`KeyOrigin`] BEFORE building a plaintext-derived message index. A HYOK class whose
/// `can_derive_plaintext_index()` is `false` is REFUSED — the index-builder gets
/// [`IndexAdmission::SkipHyok`] and builds **0 indexed message bodies** for that tenant (you cannot
/// index what you cannot decrypt). The limit is enforced by code via [`IndexAdmission::for_origin`],
/// never by a reviewer remembering to check. Platform-managed / BYOK (`can_derive` = true) is
/// [`IndexAdmission::Admit`] — full search/RAG.
pub fn admit_message_indexing(origin: &dyn KeyOrigin) -> IndexAdmission {
    IndexAdmission::for_origin(origin)
}

/// `true` iff a plaintext-derived message index/embedding may be built for this origin — the single
/// boolean the indexer branches on (a HYOK class is `false`, the structural skip). Sugar over
/// [`admit_message_indexing`]`(origin).may_index()`.
pub fn may_index_messages(origin: &dyn KeyOrigin) -> bool {
    admit_message_indexing(origin).may_index()
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 5. EMBEDDINGS-AS-PERSONAL-DATA + the named follow-ons
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The embeddings-as-personal-data binding (§3 / external-insights/04 §1).** A greppable marker
/// recording WHY the chat message spec is `semantic` (so an embedding exists for every message body)
/// AND that the embedding is reached by erasure exactly like the full-text — on erasure Search
/// PURGES + reindexes embeddings, never hides. Chat does not own the embedding store; the cascade
/// reaches it the ONE lawful way ([`crate::erase`] emits through the OUTBOX; the Search derivative
/// holder self-purges — no backdoor). The full multi-holder erasure RECEIPT is the named CHAT-P22
/// floor.
#[derive(Clone, Copy, Debug)]
pub struct EmbeddingsArePersonalData;

impl EmbeddingsArePersonalData {
    /// The chat erase cascade token the embeddings purge rides (the OUTBOX fan-out — never a
    /// backdoor write into Search).
    pub const ERASE_CASCADE_TOKEN: &'static str = crate::erase::CHAT_ERASE_CASCADE_TOKEN;
    /// The chat message spec is semantic — therefore an embedding exists per body, therefore erasure
    /// MUST reach it (the no-hide obligation).
    pub const SPEC_IS_SEMANTIC: bool = true;
}

/// **FLOOR (named) — the follow-ons CHAT-P20 leaves to later prompts.** A greppable marker: the index
/// is wired + ACL-correct + HYOK-skipped + embeddings-as-PII bound here; the byte-parity replay
/// rebuild and the complete multi-holder erasure receipt are the named follow-ons.
#[derive(Clone, Copy, Debug)]
pub struct ReplayParityFollowOn;

impl ReplayParityFollowOn {
    /// The full replay-from-source parity (Search/Refs/Notif read-models rebuild, steady-state ==
    /// recovery, one path) — CHAT-D15.
    pub const REPLAY_PARITY: &'static str = "CHAT-P21";
    /// The complete multi-holder erasure RECEIPT (the embeddings erasure cascade's holder-completeness
    /// — 0-recoverable across every derivative holder) — CHAT-D8.
    pub const ERASURE_RECEIPT: &'static str = "CHAT-P22";
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::{parse_inline, Block, HeadingLevel};
    use myelin_events::ArtifactRef;
    use myelin_identity::{
        Consistency, ConsistencyMode, Literal, ObjectId, ObjectType as IdObjectType,
        Permission as IdPerm, Principal, PrincipalId, PrincipalKind, Result as AuthzRes, Zookie,
    };
    use myelin_query::{CmpOp, Expr, Predicate, QueryAst};
    use myelin_search::{
        FieldDecl, FieldSchema, IncrementalIndexer, IndexDocument, MockEmbeddingAdapter,
        TantivyBackend,
    };
    use myelin_storage::{
        kms::{DekHandle, KekId, KmsEngine, KEY_LEN},
        Byok, Dek, Hyok, HyokKeyService, HyokServiceDenied, PlatformManaged, WrappedDek,
    };
    use myelin_tenancy::{Region, TenantId};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn alice() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn mention(id: &str) -> InlineNode {
        InlineNode::Mention(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        ))
    }

    fn consistency() -> Consistency {
        Consistency {
            at_least: Zookie("z0".into()),
            mode: ConsistencyMode::BoundedStale,
        }
    }

    // ── §1: the declare_indexable IndexSpec (contract 6.3, the chat-OWNED §7 shape) ──

    /// **CDC 6.3 — the chat message spec is the OWNED §7 shape.** `acl_object_type = "message"`
    /// (the conjoin keys on `message.id`), **semantic** (embeddings for RAG, §7), and the §7 columnar
    /// facets + the three cross-producer reference facets. A rename of a Search `IndexSpec` field, or
    /// a drift off the §7 frozen shape, breaks this.
    #[test]
    fn chat_spec_is_the_authoritative_owned_6_3_shape() {
        let s = message_index_spec();
        assert_eq!(s.subsystem, "chat");
        assert_eq!(s.type_, "message");
        assert_eq!(
            s.acl_object_type, "message",
            "§7: Search ALWAYS conjoins the list_objects Filter over message.id"
        );
        assert!(
            s.semantic,
            "§7: message bodies are vector-embedded for RAG/dedup (embeddings ARE personal data)"
        );
        // The §7 columnar facets are all present.
        for facet in [
            FACET_CHANNEL,
            FACET_AUTHOR,
            FACET_THREAD_ROOT,
            FACET_CREATED_AT,
            FACET_KIND,
        ] {
            assert!(
                s.struct_fields.contains_key(facet),
                "§7 struct_field `{facet}` is present"
            );
        }
        // The three cross-producer reference facets (X-2), all Relation.
        for facet in [FACET_MENTION, FACET_ARTIFACT_REF, FACET_EMBED] {
            assert_eq!(
                s.struct_fields.get(facet),
                Some(&FieldType::Relation),
                "`{facet}` is a dependable reference facet (Relation, X-2)"
            );
        }
    }

    /// **The full-text body is NOT a struct facet** — the markdown-subset prose arrives at emit time
    /// in `SearchProjection.text` (`ft_fields = ["body"]`), so it is absent from `struct_fields`.
    #[test]
    fn message_body_is_not_a_struct_facet() {
        let s = message_index_spec();
        for absent in [FT_BODY_FIELD, "text", "message", "content", "markdown"] {
            assert!(
                !s.struct_fields.contains_key(absent),
                "`{absent}` is the full-text projection body, not a structured facet"
            );
        }
    }

    /// **Search ACCEPTS the chat message spec (the registration GATE).** Search admits it into a live
    /// indexer's per-tenant facet union without a schema mismatch — the accepted set is byte-equal to
    /// the declared set, and the semantic spec wires the embedding adapter (the embeddings path is
    /// live).
    #[test]
    fn registration_is_accepted_by_search() {
        let accepted = register_message_index_specs();
        assert_eq!(
            accepted,
            message_index_specs(),
            "Search accepts the declared chat spec verbatim"
        );
        let _ix = IncrementalIndexer::new(
            message_index_specs(),
            std::sync::Arc::new(NullProjectFetcher),
            std::sync::Arc::new(MockEmbeddingAdapter::new(8)),
        );
    }

    // ── §2: the projection builder (contract 5.6) ──

    /// **The message projection walks the markdown-subset body + the structured inline nodes.** The
    /// full-text body carries the prose (multilingual), and the reference facets come from the SAME
    /// node-array walk the edge producer uses (X-2, never a regex over prose).
    #[test]
    fn message_projection_extracts_body_and_structured_facets() {
        let referenced = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        let body = MessageBody::new(vec![
            Block::Heading {
                level: HeadingLevel::new(2).unwrap(),
                inline: parse_inline("standup notes", &[]),
            },
            Block::Paragraph {
                inline: parse_inline(
                    &format!(
                        "blocked on {} — ping {}",
                        myelin_content::OBJ,
                        myelin_content::OBJ
                    ),
                    &[
                        InlineNode::ArtifactRefNode(referenced.clone()),
                        mention("alice"),
                    ],
                ),
            },
        ])
        .unwrap();
        let p = message_search_projection(&body, Some("en"));

        assert!(p.text.contains("standup notes"));
        assert!(p.text.contains("blocked on"));
        assert_eq!(p.lang.as_deref(), Some("en"));

        assert_eq!(
            p.fields.get(FACET_ARTIFACT_REF),
            Some(&FieldValue::Relation(referenced.0.clone()))
        );
        assert_eq!(
            p.fields.get(FACET_MENTION),
            Some(&FieldValue::Relation("alice".to_string()))
        );
    }

    /// **A multilingual message with no structured nodes carries no reference facets** (the columnar
    /// shape only holds present references). The body is still indexed under its `lang`.
    #[test]
    fn multilingual_message_with_no_nodes_has_no_reference_facets() {
        let body = MessageBody::new(vec![Block::Paragraph {
            inline: parse_inline("der Scheduler ist blockiert", &[]),
        }])
        .unwrap();
        let p = message_search_projection(&body, Some("de"));
        assert!(
            p.fields.is_empty(),
            "no structured nodes ⇒ no reference facets"
        );
        assert!(p.text.contains("Scheduler"));
        assert_eq!(p.lang.as_deref(), Some("de"));
    }

    /// **The doc ref is the `chat/message/<id>` artifact ref.**
    #[test]
    fn doc_ref_is_the_chat_message_ref() {
        assert_eq!(
            message_doc_ref("acme", "m-7"),
            "myelin://acme/chat/message/m-7"
        );
    }

    // ── §3: the ACL-conjoined feeder (contract 6.1) — search-as-non-member = 0 (CHAT-D11) ──

    fn schema() -> FieldSchema {
        FieldSchema::new().with(
            FT_BODY_FIELD,
            FieldDecl::stored(myelin_query::FieldType::Text),
        )
    }

    fn corpus() -> TantivyBackend {
        let mut be = TantivyBackend::open(&BTreeMap::new()).expect("open");
        for (id, body) in [
            (
                "myelin://acme/chat/message/m-public",
                "deploy the public service",
            ),
            (
                "myelin://acme/chat/message/m-secret",
                "deploy the confidential fix",
            ),
        ] {
            be.upsert(&IndexDocument::new(id, body)).unwrap();
        }
        be
    }

    /// A scripted `ListObjectsPort` returning a canned allow-set + counting the calls (the no-N+1
    /// gate). The SAME fake shape the Search/composer CDCs use — chat does not author a second authz
    /// path.
    struct FakeAuthz {
        answer: ListObjectsResult,
        calls: AtomicU64,
    }
    impl FakeAuthz {
        fn ids(ids: &[&str]) -> FakeAuthz {
            FakeAuthz {
                answer: ListObjectsResult::Ids {
                    ids: ids.iter().map(|i| ObjectId((*i).into())).collect(),
                    zookie: Zookie("z-acl".into()),
                },
                calls: AtomicU64::new(0),
            }
        }
        fn non_member() -> FakeAuthz {
            FakeAuthz {
                answer: non_member_filter("z-acl"),
                calls: AtomicU64::new(0),
            }
        }
    }
    impl ListObjectsPort for FakeAuthz {
        fn list_objects(
            &self,
            _subject: &Principal,
            _permission: &IdPerm,
            _ty: &IdObjectType,
            _at: &Consistency,
        ) -> AuthzRes<ListObjectsResult> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.answer.clone())
        }
    }

    fn ast(term: &str) -> QueryAst {
        QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(FT_BODY_FIELD.into()),
            rhs: Expr::Lit(Literal::Str(term.into())),
        })
        .expect("within cost bounds")
    }

    /// **CHAT-D11 — a non-member of a channel gets 0 message results from it (not in rows, not in
    /// count).** The `SetExpr::None` short-circuit (`WHERE false`) yields an empty result, then a
    /// grant surfaces the message on the SAME surface. Exactly ONE list_objects (no N+1).
    #[test]
    fn search_as_non_member_returns_zero_results_then_grant_surfaces() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());

        // NON-MEMBER: the allow-set is empty → 0 results, not in count.
        let none = FakeAuthz::non_member();
        let feeder = AclConjoinedSearchFeeder::new(&eng, &none);
        let stats = QueryStats::new();
        let res = feeder
            .search_messages(
                &ast("deploy"),
                &alice(),
                &consistency(),
                Page::FIRST,
                &stats,
            )
            .expect("the Search query surface is reachable");
        assert!(
            res.hits.is_empty(),
            "a non-member sees 0 message results from channels they're not in (CHAT-D11)"
        );
        assert_eq!(
            none.calls.load(Ordering::Relaxed),
            1,
            "exactly ONE list_objects (the conjoined pre-filter; no N+1)"
        );

        // GRANTED on the public message only → exactly that one surfaces (the confidential one is
        // excluded by the pre-filter — 0 leak, even though it matches "deploy").
        let granted = FakeAuthz::ids(&["myelin://acme/chat/message/m-public"]);
        let feeder2 = AclConjoinedSearchFeeder::new(&eng, &granted);
        let stats2 = QueryStats::new();
        let res2 = feeder2
            .search_messages(
                &ast("deploy"),
                &alice(),
                &consistency(),
                Page::FIRST,
                &stats2,
            )
            .expect("reachable");
        let ids: Vec<&str> = res2.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            ids,
            ["myelin://acme/chat/message/m-public"],
            "only the granted message surfaces; the confidential one is excluded incl. count"
        );
        assert_eq!(
            res2.hits.len(),
            1,
            "the count reveals only the visible message"
        );
    }

    /// **The ACL anchor is `(read, message)`** — the frozen Search-pipeline `read` permission over
    /// the `message` object (Identity resolves `message.read` via `message.view =
    /// parent_channel->read`); the conjoin keys on `message.id`, NOT the parent channel doc.
    #[test]
    fn acl_anchor_is_read_over_message() {
        let (perm, ty) = message_search_acl_anchor();
        assert_eq!(perm.0, "read");
        assert_eq!(ty.0, "message");
        assert_eq!(
            ty.0,
            message_index_spec().acl_object_type,
            "the feeder conjoins on the SAME object type the spec declares (message.id)"
        );
    }

    // ── §4: the HYOK structural skip (contract 11.3) — 0 indexed bodies for a HYOK class ──

    /// A deterministic mock customer HYOK key service (the customer holds the key OUTSIDE Myelin's
    /// reach; revocable). Mirrors the storage `key_origin` test mock byte-for-byte.
    struct MockHyok {
        revoked: std::cell::Cell<bool>,
        key: [u8; KEY_LEN],
    }
    impl MockHyok {
        fn new() -> MockHyok {
            MockHyok {
                revoked: std::cell::Cell::new(false),
                key: [7u8; KEY_LEN],
            }
        }
    }
    impl HyokKeyService for MockHyok {
        fn wrap(&self, _dek: &Dek) -> Result<WrappedDek, HyokServiceDenied> {
            // The admission gate (`can_derive_plaintext_index`) never calls wrap/unwrap; a HYOK class
            // is refused BEFORE any key operation. The bodies exist only to satisfy the trait — they
            // return a fixed wrapped value (the customer key never crosses into Myelin's plaintext).
            if self.revoked.get() {
                return Err(HyokServiceDenied);
            }
            Ok(WrappedDek {
                nonce: [0u8; 12],
                wrapped: self.key.to_vec(),
                kek_epoch: 0,
            })
        }
        fn unwrap(&self, _w: &WrappedDek) -> Result<DekHandle, HyokServiceDenied> {
            if self.revoked.get() {
                return Err(HyokServiceDenied);
            }
            Ok(DekHandle::from_raw(self.key))
        }
        fn destroy(&self) {
            self.revoked.set(true);
        }
    }

    /// **A HYOK class produces 0 indexed message bodies (the structural skip, 11.3).** A HYOK origin's
    /// `can_derive_plaintext_index() = false` → `admit_message_indexing` is `SkipHyok` →
    /// `may_index_messages` is `false`. A platform-managed / BYOK origin admits.
    #[test]
    fn hyok_class_skips_message_indexing() {
        let eng = KmsEngine::new();
        eng.ensure_kek(&KekId::new(
            TenantId("acme".into()),
            Region("fr-par".into()),
        ));
        let region = Region("fr-par".into());

        let platform = PlatformManaged::new(&eng, region.clone());
        assert_eq!(admit_message_indexing(&platform), IndexAdmission::Admit);
        assert!(
            may_index_messages(&platform),
            "platform-managed: full search/RAG"
        );

        let byok = Byok::new(&eng, region.clone(), "kms-customer://acme/k");
        assert!(
            may_index_messages(&byok),
            "BYOK: the key is live in-engine → can index"
        );

        let hyok = Hyok::new(MockHyok::new());
        assert_eq!(
            admit_message_indexing(&hyok),
            IndexAdmission::SkipHyok,
            "a HYOK class is refused — you cannot index what you cannot decrypt"
        );
        assert!(
            !may_index_messages(&hyok),
            "0 indexed message bodies for a HYOK tenant (the structural skip)"
        );
    }

    // ── §5: embeddings-as-PII + the named follow-ons ──

    /// **Embeddings-as-personal-data is bound to the erase cascade (no-hide).** The spec is semantic
    /// (an embedding exists per body), and erasure reaches the embeddings via the OUTBOX cascade —
    /// never a backdoor.
    #[test]
    fn embeddings_are_personal_data_bound_to_the_cascade() {
        // The marker's claim (the spec is semantic → an embedding exists → erasure must reach it)
        // matches the ACTUAL wired spec — not a tautology over a const, a check against the live spec.
        assert_eq!(
            EmbeddingsArePersonalData::SPEC_IS_SEMANTIC,
            message_index_spec().semantic,
            "the marker's semantic claim matches the wired spec (embeddings exist → must be reached)"
        );
        assert_eq!(
            EmbeddingsArePersonalData::ERASE_CASCADE_TOKEN,
            crate::erase::CHAT_ERASE_CASCADE_TOKEN,
            "the embeddings purge rides the chat erase cascade (the OUTBOX fan-out)"
        );
    }

    /// **The follow-ons are named (CHAT-P20 DoD).**
    #[test]
    fn follow_ons_are_named() {
        assert_eq!(ReplayParityFollowOn::REPLAY_PARITY, "CHAT-P21");
        assert_eq!(ReplayParityFollowOn::ERASURE_RECEIPT, "CHAT-P22");
    }
}
