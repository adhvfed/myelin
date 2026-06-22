//! # `chat_projection` — Chat indexing: message bodies (the markdown subset, multilingual) +
//! the channel ReBAC anchor + the cross-subsystem structured-node facets (SRCH-P23 / P-341, M4)
//!
//! **Owning architecture doc:** `search-and-indexing.md` §4.7 (the multilingual analyzers — Chat
//! message bodies are EU-multilingual prose; the per-language analyzer chain selects on `lang`), §3.1
//! (the structured inline nodes mention/artifact_ref/embed are dependable facets, **uniform across all
//! five producers**). **Reconciliation:** `00-reconciliation-decisions.md` X-2 (the three content
//! nodes byte-identical — mention/ref facets reliable across Git/KN/Issues/Chat). **Contracts:** 6.3
//! (consume Chat's IndexSpec — message bodies as the markdown subset), 4.3/4.9 (the channel ReBAC
//! fragment `channel.read = member + parent_project->read` flowing through `list_objects`).
//!
//! ## What SRCH-P23 ships here — the Chat slice of contract 6.3 (the engine is UNCHANGED)
//!
//! The Chat producer crate (`myelin-chat`) already owns its frozen ReBAC fragment
//! (`channel.read = member + parent_project->read`, `message.view = parent_channel->read`,
//! `rebac_fragment.rs`) and its `chat.*` event taxonomy. SRCH-P23 is the **consumer side**: Search
//! models the Chat searchable doc (one doc per message — `chat`/`message`) + ships the index-time
//! **projection BUILDER** ([`message_search_projection`]) that turns a message's
//! [`myelin_content::Block`] body (the markdown subset, multilingual) + its structured inline nodes
//! into the [`SearchProjection`] Search indexes, so the real Chat corpus is searchable end-to-end and
//! a **search-as-non-member returns 0 results** (the channel ReBAC fragment flows through
//! `list_objects` — the CHAT-D11 analog, proven by SRCH-D1 on the Chat corpus).
//!
//! Chat completes the FIVE-producer corpus (Git + KN + Issues + CI logs + Chat). The cross-subsystem
//! mention/ref facets are now dependable because **all five producers emit the same three structured
//! inline nodes** (X-2): a `mention(@alice)` / `artifact_ref(this issue)` facet query is reliable
//! across Git/KN/Issues/Chat because every producer walks the SAME
//! [`myelin_content::Inline::structured_nodes`] node array — there is no second extraction path (the
//! exact posture [`crate::kn_projection::page_search_projection`] takes; Chat reuses it).
//!
//! ## Coherence (EI-01 §7) — Search models the doc, it does NOT re-define the Chat fragment
//! - **The channel/message ReBAC fragment is the producer's** (`myelin_chat::rebac_fragment`). Search
//!   does NOT re-declare `channel.read`; it CONSUMES it — a message's reachability is decided by its
//!   parent CHANNEL's `read` (member + parent_project), so [`message_index_spec`]'s
//!   `acl_object_type = "channel"` while `type_ = "message"`. A viewer who is not a channel member (and
//!   whose project does not grant read) gets `list_objects(channel) = {}` for that channel, so the
//!   message's ACL pre-filter excludes it from EVERY result incl. counts — the search-as-non-member = 0
//!   guarantee (the CHAT-D11 analog). This is the SAME parent-anchored posture
//!   [`crate::ci_log_projection`] (CI log → parent run) and [`crate::git_code_projection`] (git blob →
//!   parent repo) take. Search cannot import `myelin-chat` (Chat is a producer above the Search
//!   consumer in the §2.9 DAG — `myelin-chat` depends on `myelin-search`, never the reverse), so the
//!   subsystem/type/acl-anchor tokens are modelled byte-identically here ([`CHAT_SUBSYSTEM`] /
//!   [`MESSAGE_TYPE`] / [`MESSAGE_ACL_OBJECT_TYPE`] mirror `myelin_chat::rebac_fragment::object_types`).
//! - **The structured-inline-node walk is the ONE shared seam** — Chat reuses the byte-identical
//!   `structured_nodes()` walk KN/Refs use (X-2). Search does NOT invent a second mention/ref extractor
//!   for Chat; [`message_search_projection`] delegates to the shared
//!   [`crate::kn_projection::page_search_projection`] block walk, so the cross-subsystem facets are
//!   dependable BY CONSTRUCTION (the same code path produces the same facet for every producer).
//!
//! ## Floors named (SRCH-P23 DoD)
//! - **Chat completes the five-producer corpus — it is NOT world-scale-hardened.** The M5 world-scale
//!   hardening (surge SRCH-P25, freshness SRCH-P24, restore SRCH-P28, HYOK SRCH-P29, cross-cell
//!   SRCH-P31) is the FOLLOW-ON band. Recorded so the full deterministic-correct corpus is not mistaken
//!   for world-scale-hardened. Greppable as [`ChatFiveProducerCorpusNotWorldScaleFloor`].
//! - **The real Chat projection EMITTER** (the live `project(ref, viewer)` that walks a message's body +
//!   emits the per-message projection through the outbox) is the Chat producer's M4 emitter prompt.
//!   Here Search ships the SPEC model + the projection BUILDER the emitter feeds; the integration test
//!   drives the genuine builder over a real Chat corpus.
//! - **No new mutation-core module** — the SRCH-P09/P11/P15 mutation floors (the SetExpr ACL conjoin
//!   decision logic) still hold on the full five-producer corpus; this slice is producer-corpus WIRING,
//!   the engine decision logic is unchanged.

use myelin_content::Block;

use crate::indexer::{IndexSpec, SearchProjection};

/// The subsystem token Chat declares its message projection under (`chat`) — byte-identical to the
/// `myelin://<tenant>/chat/...` artifact authority + the `chat.*` event token family the indexer
/// whitelists ([`crate::indexer::INDEXER_SUBJECT_PREFIXES`]). Search models it here because
/// `myelin-chat` depends on `myelin-search`, never the reverse (the [`crate::kn_projection`] posture).
pub const CHAT_SUBSYSTEM: &str = "chat";

/// The artifact type Chat's message projection indexes — a `message`: ONE searchable doc per chat
/// message (the markdown-subset body, multilingual). The canonical doc ref is
/// `myelin://<tenant>/chat/message/<id>`. Byte-identical to
/// `myelin_chat::rebac_fragment::object_types::MESSAGE`.
pub const MESSAGE_TYPE: &str = "message";

/// The ACL object type a message doc's reachability filter pins on — the parent **`channel`** (there
/// is NO per-message ACL object: `message.view = parent_channel->read`, so the channel decides
/// reachability — `channel.read = member + parent_project->read`). Byte-identical to
/// `myelin_chat::rebac_fragment::object_types::CHANNEL`. This is what makes the search-as-non-member
/// = 0 guarantee hold: a non-member's `list_objects(channel)` excludes the channel, so its messages
/// are never in any result incl. counts (the CHAT-D11 analog, the SRCH-D1 instance on Chat).
pub const MESSAGE_ACL_OBJECT_TYPE: &str = "channel";

/// The structured facet for an inline `artifact_ref` in a message body — re-exported from
/// [`crate::kn_projection::FACET_ARTIFACT_REF`] (the ONE cross-producer facet key, X-2).
pub use crate::kn_projection::FACET_ARTIFACT_REF;
/// The structured facet for an inline `embed` in a message body — re-exported from
/// [`crate::kn_projection::FACET_EMBED`] (the ONE cross-producer facet key, X-2).
pub use crate::kn_projection::FACET_EMBED;
/// The structured facet for an `@mention` in a message body — re-exported from
/// [`crate::kn_projection::FACET_MENTION`] (the ONE cross-producer facet key, X-2). A message
/// mentioning `@alice` is filterable by exactly the same facet key a KN page / an Issue / a Git
/// commit message uses — the cross-subsystem facet dependability.
pub use crate::kn_projection::FACET_MENTION;

/// **Chat's `declare_indexable` message IndexSpec (contract 6.3 — the Search-side consumed model).**
/// `subsystem = "chat"`, `type = "message"`, the three structured inline-node reference facets
/// (mention/artifact_ref/embed — byte-identical to KN's, X-2; the cross-subsystem facet keys),
/// **non-semantic** in v1 (Chat is multilingual full-text + the dependable reference facets; semantic
/// vector indexing of chat is a post-v1 follow-on — message bodies are short conversational prose, not
/// the long-form KN documents the v1 vector surface targets), `acl_object_type = "channel"` (the
/// parent channel decides reachability — `message.view = parent_channel->read`).
///
/// The full-text message body (the markdown subset, multilingual) is NOT in the spec — it arrives at
/// emit time in the index-time [`SearchProjection::text`] ([`message_search_projection`]). The spec is
/// the columnar schema; the projection is the row.
pub fn message_index_spec() -> IndexSpec {
    // The three structured inline-node reference facets (§3.1) — byte-identical to KN's (X-2): the
    // SAME facet keys + types, so a mention/ref query is dependable ACROSS producers. Built from KN's
    // page spec's struct_fields so the cross-producer facet shape is provably one shape, not a copy.
    let struct_fields = crate::kn_projection::kn_page_index_spec().struct_fields;
    // A message's reachability is its parent channel's `read` (there is no per-message ACL object,
    // UNLIKE an issue whose ACL is its own object — like git's blob→repo / CI's log→run). Non-semantic
    // in v1 (multilingual full-text + reference facets; semantic chat is the post-v1 follow-on).
    IndexSpec::new(CHAT_SUBSYSTEM, MESSAGE_TYPE, struct_fields)
        .with_acl_object_type(MESSAGE_ACL_OBJECT_TYPE)
}

/// Every Chat index spec (the one `message` type) — the set a Search indexer registers to consume the
/// real Chat corpus. Mirrors [`crate::ci_log_projection::ci_log_index_specs`].
pub fn message_index_specs() -> Vec<IndexSpec> {
    vec![message_index_spec()]
}

/// **Register Chat's message index spec WITH Search (the GATE).** Builds [`message_index_specs`] and
/// proves Search **accepts** it by admitting it into a live
/// [`IncrementalIndexer`](crate::indexer::IncrementalIndexer)'s per-tenant facet union without a
/// schema mismatch (the only honest definition of "accepted" — Search is the authority that admits).
/// Returns the specs that were accepted. Mirrors
/// [`crate::ci_log_projection::register_ci_log_index_specs`].
pub fn register_message_index_specs() -> Vec<IndexSpec> {
    let specs = message_index_specs();
    // Admit them into a real indexer's facet union (the build-time declare_indexable surface). A
    // facet-type collision or a malformed shape would panic at construction; it does not.
    let _accepted = crate::indexer::IncrementalIndexer::new(
        specs.clone(),
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
    );
    specs
}

/// A do-nothing [`ProjectFetcher`](crate::indexer::ProjectFetcher) used ONLY to admit the Chat specs
/// into a live indexer for the registration GATE (the SPEC half + the projection BUILDER ship here;
/// the real owner-`project` fetch is the Chat producer's emitter). It never fetches — registration
/// does not index. Mirrors CI's / Issues' `NullProjectFetcher`.
struct NullProjectFetcher;

impl crate::indexer::ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<SearchProjection, crate::indexer::ProjectFetchError> {
        // The SPEC registration never fetches a projection (no emitter here). This is the registration
        // GATE — Search admits the schema — not the index path.
        Err(crate::indexer::ProjectFetchError::Gone)
    }
}

/// **Build a chat message's [`SearchProjection`] from its [`Block`] body (the index-time row, §4.1).**
/// This is the owner's `project(ref, viewer)` body Search consumes (contract 5.6) — NOT a DB read. It
/// produces:
/// - the analyzable full-text `text` (the message's markdown-subset prose, multilingual — the `lang`
///   tag selects the per-language analyzer chain, §4.7; `None` lets the indexer detect it),
/// - the three structured inline-node reference facets (mention/artifact_ref/embed) walked from the
///   SAME [`myelin_content::Inline::structured_nodes`] node array KN/Refs use (X-2, the cross-subsystem
///   dependable facets — never a regex over prose).
///
/// A chat message body is the SAME `myelin_content::Block` markdown subset a KN page / an issue comment
/// is, so the projection DELEGATES to the shared
/// [`crate::kn_projection::page_search_projection`] block walk — the cross-subsystem facets are
/// dependable BY CONSTRUCTION (one code path, one facet shape for every producer). In production the
/// Chat service builds the `Block` body from a message's stored content; here the builder takes it
/// directly (the projection is the row, the store is the source).
pub fn message_search_projection(body: &[Block], lang: Option<&str>) -> SearchProjection {
    // Reuse the ONE shared block/inline walk (X-2): the same `structured_nodes()` extraction every
    // producer uses, so a mention/ref facet is byte-identical across Git/KN/Issues/Chat. Chat is
    // non-semantic in v1 (the page projection's vector field is only embedded for a semantic spec; the
    // message spec is non-semantic, so the indexer skips embedding — see `message_index_spec`).
    crate::kn_projection::page_search_projection(body, lang)
}

/// **The canonical chat message doc ref for a message id.** `myelin://<tenant>/chat/message/<id>` —
/// the Search `doc_id` for the message (byte-identical to the `myelin://<tenant>/chat/message/...`
/// artifact authority the Chat producer mints).
pub fn message_doc_ref(tenant: &str, message_id: &str) -> String {
    format!("myelin://{tenant}/chat/{MESSAGE_TYPE}/{message_id}")
}

/// **FLOOR (named) — Chat completes the five-producer corpus; it is NOT world-scale-hardened.** A
/// greppable zero-sized marker: with Chat the deterministic-correct five-producer corpus (Git + KN +
/// Issues + CI logs + Chat) is searchable and the SRCH-D1/D3 leak/IDOR invariants hold across it. The
/// M5 world-scale hardening (surge, freshness, restore, HYOK, cross-cell) is the FOLLOW-ON band.
/// Recorded so the full deterministic-correct corpus is not mistaken for world-scale-hardened.
#[derive(Clone, Copy, Debug)]
pub struct ChatFiveProducerCorpusNotWorldScaleFloor;

impl ChatFiveProducerCorpusNotWorldScaleFloor {
    /// The M5 follow-on that hardens the freshness budget under load (SRCH-D7 full-scale).
    pub const FRESHNESS_FOLLOW_ON: &'static str = "SRCH-P24";
    /// The M5 follow-on that tunes the 30x agent/CI query surge + the protected-human-lane shed order.
    pub const SURGE_FOLLOW_ON: &'static str = "SRCH-P25";
    /// The M5 follow-on that proves restore + cross-seam + re-erase at scale (SRCH-D9).
    pub const RESTORE_FOLLOW_ON: &'static str = "SRCH-P28";
    /// The Chat producer emitter that ships the live `project(ref)` feeding this builder.
    pub const CHAT_EMITTER_FOLLOW_ON: &'static str = "the Chat M4 emitter prompt";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::IncrementalIndexer;
    use myelin_content::{parse_inline, Block, HeadingLevel, InlineNode};
    use myelin_events::ArtifactRef;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_query::{FieldType, FieldValue};
    use myelin_tenancy::TenantId;

    fn mention(id: &str) -> InlineNode {
        InlineNode::Mention(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        ))
    }

    /// **The Chat message spec is the consumed 6.3 shape.** Pins every facet + type + the
    /// acl_object_type. A rename of a Search `IndexSpec` field, or a drift in the facet set/types,
    /// breaks the registrant.
    #[test]
    fn message_spec_is_the_consumed_6_3_shape() {
        let s = message_index_spec();
        assert_eq!(s.subsystem, "chat");
        assert_eq!(s.type_, "message");
        assert_eq!(
            s.acl_object_type, "channel",
            "a message's reachability is its parent channel's `read` (message.view = parent_channel->read)"
        );
        assert!(
            !s.semantic,
            "Chat is multilingual full-text + reference facets in v1, not vector-embedded"
        );
        // The three structured inline-node reference facets (§3.1), all Relation — byte-identical to KN.
        assert_eq!(
            s.struct_fields.len(),
            3,
            "exactly the three reference facets"
        );
        for facet in [FACET_MENTION, FACET_ARTIFACT_REF, FACET_EMBED] {
            assert_eq!(
                s.struct_fields.get(facet),
                Some(&FieldType::Relation),
                "`{facet}` is a dependable reference facet (Relation)"
            );
        }
    }

    /// **The acl_object_type is the parent channel, not the message itself (the blob→repo / log→run
    /// analog).** This is what makes the search-as-non-member = 0 guarantee hold (CHAT-D11) — pinned so
    /// a future edit can not drift the ACL anchor off the parent `channel`.
    #[test]
    fn acl_object_is_the_parent_channel_not_the_message() {
        let s = message_index_spec();
        assert_eq!(s.acl_object_type, "channel");
        assert_ne!(
            s.acl_object_type, s.type_,
            "the ACL anchor is the parent channel, NOT the per-message doc (no per-message ACL object)"
        );
    }

    /// **The cross-subsystem facet keys are byte-identical to KN's (X-2).** The Chat message spec
    /// declares EXACTLY the same three structured-node facet keys + types as the KN page spec — so a
    /// mention/ref facet query is dependable across producers (the SAME columnar key, never two
    /// extraction paths). Pins the X-2 uniformity at the plan layer.
    #[test]
    fn cross_subsystem_facets_are_byte_identical_to_kn() {
        let chat = message_index_spec();
        let kn = crate::kn_projection::kn_page_index_spec();
        assert_eq!(
            chat.struct_fields, kn.struct_fields,
            "Chat's reference facets are byte-identical to KN's (X-2 — one cross-producer facet shape)"
        );
        // And the facet KEYS are the one shared set (re-exported, not re-declared).
        assert_eq!(FACET_MENTION, crate::kn_projection::FACET_MENTION);
        assert_eq!(FACET_ARTIFACT_REF, crate::kn_projection::FACET_ARTIFACT_REF);
        assert_eq!(FACET_EMBED, crate::kn_projection::FACET_EMBED);
    }

    /// **The full-text message body is NOT a structured facet.** The markdown-subset prose arrives at
    /// emit time in `SearchProjection.text`, so it must be absent from `struct_fields` (the schema is
    /// the columnar reference-facet half, not the body).
    #[test]
    fn message_body_is_not_a_struct_facet() {
        let s = message_index_spec();
        for absent in ["body", "text", "message", "content", "markdown"] {
            assert!(
                !s.struct_fields.contains_key(absent),
                "`{absent}` is full-text projection body, not a structured facet"
            );
        }
    }

    /// **Search ACCEPTS the Chat message spec (the GATE).** Search admits it into a live indexer's
    /// per-tenant facet union without a schema mismatch — the accepted set is byte-equal to the
    /// declared set.
    #[test]
    fn registration_is_accepted_by_search() {
        let accepted = register_message_index_specs();
        assert_eq!(
            accepted,
            message_index_specs(),
            "Search accepts the declared Chat spec verbatim"
        );
        let _ix = IncrementalIndexer::new(
            message_index_specs(),
            std::sync::Arc::new(NullProjectFetcher),
            std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
        );
    }

    /// **The message projection walks the markdown-subset body + the structured inline nodes.** The
    /// full-text body carries the prose (multilingual), and the three reference facets are extracted
    /// via the SAME node-array walk every producer uses (X-2, never a regex over prose).
    #[test]
    fn message_projection_extracts_body_and_structured_facets() {
        let referenced = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        let body = vec![
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
        ];
        let p = message_search_projection(&body, Some("en"));

        // The full-text body carries the message prose.
        assert!(p.text.contains("standup notes"));
        assert!(p.text.contains("blocked on"));
        assert_eq!(p.lang.as_deref(), Some("en"));

        // The structured reference facets are extracted via the shared node-array walk (X-2).
        assert_eq!(
            p.fields.get(FACET_ARTIFACT_REF),
            Some(&FieldValue::Relation(referenced.0.clone()))
        );
        assert_eq!(
            p.fields.get(FACET_MENTION),
            Some(&FieldValue::Relation("alice".to_string()))
        );
    }

    /// **A message with no structured nodes carries no reference facets** (the columnar shape only
    /// holds present references — an absent facet is not indexed as empty). The multilingual body is
    /// still indexed.
    #[test]
    fn message_with_no_nodes_has_no_reference_facets() {
        // A non-English (German) message body — multilingual full-text (§4.7).
        let body = vec![Block::Paragraph {
            inline: parse_inline("der Scheduler ist blockiert", &[]),
        }];
        let p = message_search_projection(&body, Some("de"));
        assert!(
            p.fields.is_empty(),
            "no structured nodes ⇒ no reference facets"
        );
        assert!(p.text.contains("Scheduler"));
        assert_eq!(
            p.lang.as_deref(),
            Some("de"),
            "the multilingual lang is carried"
        );
    }

    /// **The doc ref is the `chat/message/<id>` artifact ref.**
    #[test]
    fn doc_ref_is_the_chat_message_ref() {
        assert_eq!(
            message_doc_ref("acme", "m-7"),
            "myelin://acme/chat/message/m-7"
        );
    }

    /// **The floor marker names the M5 world-scale follow-ons + the Chat emitter.**
    #[test]
    fn floor_marker_names_the_follow_ons() {
        assert_eq!(
            ChatFiveProducerCorpusNotWorldScaleFloor::FRESHNESS_FOLLOW_ON,
            "SRCH-P24"
        );
        assert_eq!(
            ChatFiveProducerCorpusNotWorldScaleFloor::SURGE_FOLLOW_ON,
            "SRCH-P25"
        );
        assert_eq!(
            ChatFiveProducerCorpusNotWorldScaleFloor::RESTORE_FOLLOW_ON,
            "SRCH-P28"
        );
    }
}
