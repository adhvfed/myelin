//! # `myelin-chat` — the Chat subsystem (the M2-C0 freeze-so-dependents-compile slice)
//!
//! Chat is the platform's **maximal-consumer** subsystem (architecture chat `00-overview.md`
//! §2.1): it owns four hot parts (the message log, the conversation/membership model, the
//! per-conversation read-state, the fanout-class decision) and CONSUMES everything else. Its
//! feature bulk lands in M4; this crate's M2 slice is the **contract-declaration** half of
//! milestone M2-C0 — the shapes dependents (the Bus seed, Identity's cell schema, Refs' frozen
//! `#sub` vocabulary, Notif's reason set) compile against ahead of the M4 build.
//!
//! - [`events`] — **CHAT-P1 / P-128**: the complete `chat.*` event-token registration (contract
//!   2.9) split into the **durable-via-outbox** set vs the **firehose-only** set — chat COMPLETES
//!   its dotted-name list, each token validated against the one Bus grammar.
//! - [`rebac_fragment`] — **CHAT-P2 / P-244**: the FROZEN Chat ReBAC namespace fragment (contract
//!   4.9) — the `channel` + `message` definitions (`channel.read = member + parent_project->read`,
//!   the `watcher` Notif read-fanout relation), the names-only [`myelin_identity::NamespaceFragment`]
//!   carriers Identity admits into the one cell schema. The runtime membership tuple writes are the
//!   CHAT-P8 floor.
//! - [`subs`] — **CHAT-P2 / P-244**: chat's `#sub` mints registered with Refs (contract 5.7) — the
//!   frozen `message-`/`thread-` grammar + the grammatical mint codecs. The runtime mint SITE
//!   (co-committed with the outbox event) is wired in [`store`] at CHAT-P5 (the `message-<id>` subject
//!   on every `chat.message.*` event); the mint codecs are exercised live there.
//!
//! - [`conversation`] — **CHAT-P7 / P-401**: the Conversation/Membership entity (one entity, many
//!   kinds — `channel`/`dm`/`group_dm`/`artifact_linked`/`announcement`, with `retention_days` +
//!   `linked_ref`) + the membership table + the `membership_by_principal` conversation-list index
//!   (S1 — the leak-free, no-N+1 "my conversations" candidate set the CHAT-P8/P13 `list_objects`
//!   gate joins against). The home cell is a settable VALUE (cross-org NON-FORECLOSURE; the single
//!   home-cell is the M4 floor, federation rides CHAT-P30 / 12.6). Contracts 11.1 (OLTP tier) / 12.1
//!   (partition key). The membership→`write_tuples`→zookie co-commit + new-enemy guard is CHAT-P8.
//! - [`membership`] — **CHAT-P8 / P-402**: membership→`write_tuples`→zookie in ONE transaction + the
//!   new-enemy guard + the send/membership `check` gate (contracts 4.6 / 4.10 / 4.9 / 4.2 / 2.2). A
//!   membership change writes the frozen `channel.member`/`watcher` tuples (returning the zookie),
//!   STAMPS the returned zookie on the conversation ([`conversation::Conversation::acl_zookie`]) in
//!   the SAME tx as the membership row + the `chat.channel.member_*` event — so a just-revoked grant
//!   cannot read stale (the new-enemy guard; a strong, stamped read denies post-revoke). The
//!   send/membership gate ([`membership::MembershipGate`]) is `Id.check`-backed + fail-closed; the
//!   channel lifecycle events (`created`/`archived`/`linked` → `refs.edge.created`) ride the outbox.
//!   The cross-org / federated channels follow-on (M5-C-X1 / CHAT-P30 / P-504) rides the 12.6 bridge.
//! - [`dek`] — **CHAT-P6 / P-400**: the per-subject-DEK encryption of the message bodies + drafts
//!   (contract 11.4 / GD-4). The `body_inline` / `body_nodes` / composer `draft` are sealed under the
//!   AUTHOR's per-subject DEK through the ONE shared `myelin_storage::encryption::ColumnCryptor` — the
//!   body IS the PII (arch 05 §5), and the per-subject DEK never bakes erasable plaintext into the
//!   immutable log (external-insights/04 §1). The crypto-shred erase BODY is the CHAT-P22 floor.
//! - [`schema`] — **CHAT-P6 / P-400**: the Chat OLTP row tag-carriers (`#[derive(PersonalData)]` +
//!   `#[personal_data(...)]`, contract 10.2) so the `no-untagged-personal-data` lint is green (0
//!   untagged PII fields) — the body fields are `Content`/`CryptoShred(subject_dek)`, the author is a
//!   pseudonymous `Identifier`/`Pseudonymise`.
//! - [`holder`] — **CHAT-P6 / P-400**: the Chat `PersonalDataHolder` (H5; contract 10.1 / 1.4),
//!   auto-registered over the Chat store through the harness ONE door; `locate`/`export` typed,
//!   `restrict` wired. The `erase` fan-out BODY is **CHAT-P22 / P-411** ([`erase`]).
//! - [`erase`] — **CHAT-P22 / P-411**: the Chat GDPR erase fan-out (the holder's erase BODY) — the
//!   author per-subject-DEK crypto-shred across hot/cold/backups (contract 11.4; 0 recoverable PII),
//!   the `chat.message.erased` tombstone cascade via the OUTBOX (2.7 / the DSR fan-out 10.4, never a
//!   backdoor), the read-state/drafts/unfurl-cache purge, the COMPLETE per-store holder-receipt set
//!   (10.1; 0 holders missed), and the destroyed-key epoch driving post-restore re-erasure (10.8) —
//!   CHAT-D8. The mention pseudonym-shred (→ `[erased user]`) + the Art. 18 restriction-flag
//!   suppression at every read path + the LEGAL free-text residual are CHAT-P23 / P-417 ([`restriction`]).
//! - [`restriction`] — **CHAT-P23 / P-417**: the second committable unit of M4-C8 — the mention
//!   pseudonym-shred ([`restriction::render_mention`] → `[erased user]` on next render via the 4.8
//!   pseudonym-map shred, FREE because the node is structured + pseudonymous, 0 recoverable
//!   mentioned-PII), the Art. 18 restriction flag honoured at EVERY read path
//!   ([`restriction::RestrictionGate`] — indexing / agent-use / notif-routing / analytics suppressed
//!   for a restricted subject, 0 processings, a distinct state from erasure, contract 10.1), and the
//!   LEGAL free-text residual ([`restriction::LEGAL_RESIDUAL_FLOOR`] — the `[OPEN — LEGAL]` floor BY
//!   REFERENCE to the ONE platform posture 10.9 / X-7, never a fifth chat-specific statement; the
//!   structural floor ships regardless). The named CHAT-P22 floor, filled.
//! - [`replay`] — **EB-27 / P-327** (skeleton at CHAT-P6) + **CHAT-P21 / P-416** (full parity): chat's
//!   `replay(scope, since)` re-emits `chat.{channel,message,thread}.snapshot` through the OUTBOX
//!   (contract 2.6), and the three Chat-fed read-models (Search/Refs/Notif) REBUILD from that re-emit
//!   through the SAME live consumer step ([`replay::ChatReadModelConsumer::ingest`] — steady-state and
//!   recovery share ONE path, 0 recovery-only code paths; the rebuild stays ACL-correct via the
//!   channel-keyed Filter conjoin; an erased subject emits a tombstone, X-7). CHAT-D15 greens on the
//!   [`replay::reindex_parity_hash`] (cold == live). The full multi-holder erasure RECEIPT remains the
//!   CHAT-P22 floor.
//!
//! - [`dispatch`] — **CHAT-P25 / P-419**: the second committable unit of M4-C9 (presence + streaming
//!   is CHAT-P24 [`presence`]) — **explicit-first agent dispatch** (no auto-spawn on mention;
//!   reserve-gated) + the **agent provenance popover** (S12). REUSES the explicit-first CLASS decision
//!   ([`glue::agent_dispatch_class`], NOTIF-P22) and wires it into the dispatch ORCHESTRATION: a casual
//!   `@agent` mention → [`dispatch::Disposition::NotifiedInbox`] (0 run, 0 reserve, contract 8.6 /
//!   CHAT-1); only an explicit action → [`dispatch::dispatch_explicit`] which reserves (11.7 — no
//!   balance → no run), mints a per-run token (4.7), and routes the run's chat output through
//!   [`myelin_agent::EffectApi`] (8.2 — the routing split). The structural
//!   [`dispatch::no_auto_spawn_path_is_wired`] proves 0 mention→run edges (CHAT-D17). The
//!   [`dispatch::agent_provenance`] popover answers "why did this agent post?" from `actor.on_behalf_of`
//!   / `causation_id` / `correlation_id` / `caused_by` (§7.5, the agent badge always set). FLOORS:
//!   the no-auto-spawn path is a DELIBERATE counsel-gated L-3 absence ([`dispatch::L3_AUTO_SPAWN_ABSENCE`],
//!   recon §6); the dispatched brain is the mock (--use-mock, the real `LlmAgentRuntime` post-M5). This
//!   COMPLETES the M4 chat surface.
//! - [`glue`] — **CHAT-P3 / P-245**: the M2-C0 humanise/notif/fanout-class + firehose-scope + TE-21
//!   slice (contracts 7.3 / 7.6 / 3.5 / 1.7). Chat REGISTERS its humanise template keys (card /
//!   agent-message / `chat.message.mentioned` — the ONE templating surface, OQ-L) + its
//!   `define_notif_rule` reason set (mentioned / replied / thread_watched / approval_requested) into
//!   Notif's frozen verbs, declares the fanout-class (write-fanout the bounded high-signal set,
//!   read-fanout the unbounded ambient set, arch §4), VALIDATES its `channel:<id>` firehose scope
//!   against the frozen resume-cursor protocol (contract 3.5, never `*`), and pins the TE-21
//!   connection-tier language call (Rust default; the BEAM hatch written-but-closed — a no-op against
//!   the 1.7 harness shim). This CLOSES the M2-C0 contract surface. The rules are USED in
//!   CHAT-P16/P18, the scope is IMPLEMENTED in CHAT-P9, the TE-21 hatch opens in CHAT-P26 (floors).
//!
//! - [`unfurl`] — **CHAT-P13 / P-407**: the Unfurl Service — the shared per-`ArtifactRef` projection
//!   cache (ONE entry per ref, viewer-INDEPENDENT) gated by a per-viewer `check`/`list_objects` (the
//!   no-leak floor, contracts 5.2 / 5.7 / 4.3 / 4.2). The gate runs BEFORE the cache/resolver is
//!   touched, so a denied viewer's title is NEVER fetched (CHAT-D5, 0 title leak); the SetExpr→JOIN
//!   class precompute lowers over the unfurl candidate id column (no N+1); the 4-step ladder
//!   (live/gone/erased) maps to leak-free cards. The Refs `resolve` chokepoint (REF-P10 / CHAT-P15)
//!   is the named floor; the canvas is an embedded Knowledge page, NOT a chat editor (M4-C4 lean).
//!   - **CHAT-P14 / P-408** ([`unfurl::invalidation`]): the bus-driven invalidation consumer
//!     (matching `*.updated`/`ci.check.updated`/`*.erased`, contracts 5.9 / 2.7, whitelisted-subject +
//!     idempotent) that busts the ONE shared cache + pushes a live firehose card-update frame
//!     (contract 3.5, `channel:<id>` scope) — CHAT-D7; the erasure-safe re-render (erase a third party
//!     → tombstone on next render, 0 recoverable PII, no durable snapshot, re-resolves live) — CHAT-D6;
//!     and the `#sub` anchor stability (an edited `message-<id>` embed stays live, a deleted one
//!     degrades to a root tombstone, never dangles) — CHAT-D18. The cache-TTL backstop is a
//!     measured-not-predicted tunable (R-C4), never a separate milestone.
//!
//! - [`project`] — **CHAT-P15 / P-409**: the producer half of `project(ref, viewer)` (contract 5.6) for
//!   chat/{channel,message,thread} — the ONLY way another subsystem reads about a chat artifact (no
//!   cross-DB). The Refs `resolve` chokepoint (the seam [`unfurl::RefsResolvePort`] CONSUMES) calls back
//!   into THIS `project()` for a per-viewer, pre-permission-checked `Projection | Tombstone` — NEVER the
//!   body (the frozen `{title, state, icon, render_hint, sub_anchor?}` shape; the title humanised via
//!   `humanise`, 7.3; per-type `render_hint` = ChannelChip/MessageChip/ThreadChip). Permission FIRST
//!   (a non-member viewer gets a `Denied` tombstone, 0 title leak), then erased/restricted, then the
//!   live-store root resolve. Resolution is ALWAYS cell-local (OQ-I — `project()` never reaches across
//!   cells; the cross-org follow-on CHAT-P30/12.6 consumes the bridge, not `project()`). Also asserts
//!   **chat the densest `refs.edge.created` producer** ([`project::densest_edge_producer`]) over the
//!   ALREADY-built edge machinery ([`content`] structured nodes + [`membership`] `chat.channel.linked`).
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md` §1 (the
//! COMPLETE `chat.*` taxonomy under the Bus §6 grammar), §1.1 (the **durable** set via the OUTBOX —
//! BUS-2 / contract 2.2), §1.2 (the **firehose-only** set — never the durable bus, ADR-04.5, over
//! the frozen resume-cursor protocol 3.5), §4 (the fanout-class declaration the tokens carry);
//! `00-overview.md` §2.1 (the four owned hot parts vs the consumed contracts).
//!
//! **Contract-index rows (registered/aligned here):**
//! - **2.9** Event taxonomy + token table — `<subsystem>.<artifact_type>.<event_name>`. The Bus
//!   owns the **grammar + the seed**; **each subsystem completes its own list** (contract 2.9
//!   text). [`events`] is Chat COMPLETING its `chat.*` list — chat **registers**, it does **not**
//!   author the grammar. Every token is validated against the one Bus validator
//!   ([`myelin_events::validate_event_type`], EB-02 / P-042) — there is no second token language.
//! - **2.1** `EventEnvelope` — the canonical envelope every `chat.*` event aligns to (the `type`
//!   field is one of the registered tokens; the names/units anchor X-5). Referenced, not
//!   re-defined (the envelope lives in `myelin-events`).
//!
//! ## What this prompt (CHAT-P1 / P-128) ships — and what it deliberately does NOT
//! **Ships:** the complete v1 `chat.*` event-token registration (arch §1.1/§1.2) as named
//! `&'static str` constants, partitioned into [`events::CHAT_DURABLE_TOKENS`] (the OUTBOX-only set)
//! and [`events::CHAT_FIREHOSE_TOKENS`] (the firehose-only set), each PROVEN grammatical against the
//! Bus §6.1/§6.2 grammar by [`myelin_events::validate_event_type`] (0 ungrammatical tokens), and the
//! durable/firehose split asserted **disjoint and total** (0 misclassified tokens). Chat REGISTERS
//! its list; the Bus owns the grammar.
//!
//! **Does NOT ship (FLOORS named — VISION §3 name-your-floors):** this prompt ships TOKENS, not a
//! working emit path. There is no message store, no outbox co-commit, no firehose transport here.
//! - **The DURABLE set's behaviour begins in CHAT-P5** (the message persist + `chat.message.created`
//!   outbox row in ONE PG transaction via `OutboxTx::emit` — the silent-data-loss floor). The
//!   durable set is the ONLY set that may ride `OutboxTx::emit`; the `no-raw-publish` lint (contract
//!   1.6) enforces this structurally when the behaviour lands.
//! - **The FIREHOSE set's behaviour begins in CHAT-P10** (the live delivery transport over the
//!   frozen resume-cursor protocol, contract 3.5). The firehose-only set NEVER touches the durable
//!   bus — it is allowed-to-drop, ephemeral; if lost, the durable record (or the final message) is
//!   the truth. The firehose seam keeps these off the durable bus structurally.
//!
//! ## Why this is data (a `&'static str` token table), not an emit seam
//! Registration at M2-C0 is a **names freeze** so dependents (the Bus seed, Search's indexer, Refs'
//! edge builder, Notif's router, the live-delivery firehose) compile against the NAMED chat tokens,
//! never literals (the names anchor, X-5). The durable/firehose CLASS of each token is also frozen
//! here as data — the structural separation the `no-raw-publish` lint enforces when the emit paths
//! attach to these constants in CHAT-P5 / CHAT-P10. One token language, no drift (EI-01 §7).

#![forbid(unsafe_code)]

pub mod composer;
pub mod content;
pub mod conversation;
pub mod dek;
pub mod dispatch;
pub mod e2e_wedge;
pub mod erase;
pub mod events;
pub mod fanout;
pub mod glue;
pub mod hitl;
pub mod holder;
pub mod membership;
pub mod presence;
pub mod project;
pub mod read_state;
pub mod rebac_fragment;
pub mod replay;
pub mod restriction;
pub mod schema;
pub mod scylla_followon;
pub mod search;
pub mod store;
pub mod subs;
pub mod tools;
pub mod unfurl;

pub use composer::{
    detect_pasted_url, AutocompleteKind, AutocompletePort, Draft, DraftKey, DraftStore, EditCas,
    EditOutcome, EditRequest, MemDraftStore, SlashCommand, SlashMenu, Suggestion, UnfurlIntent,
};
pub use content::{
    edge_aggregate_key, emit_body_edges, extract_body_edges, extract_message_edges, is_chat_block,
    paragraph_body, roundtrips_md, validate_subtree, BodyEdge, EdgeRel, MessageBody, SubsetError,
    CHAT_EXCLUDED_BLOCKS, REFS_EDGE_CREATED, REL_CLASS_REFERENCE,
};
pub use conversation::{
    Conversation, ConversationError, ConversationKind, ConversationStore, MemConversationStore,
    Membership, MembershipRole,
};
pub use dek::{decrypt_body, encrypt_body, plaintext_at_rest, subject_dek_erasure, ChatFreeText};
pub use dispatch::{
    agent_provenance, dispatch_disposition_class, dispatch_explicit, mention_is_always_notify_only,
    no_auto_spawn_path_is_wired, reserve_gate, AgentProvenance, DispatchOutcome, Disposition,
    L3_AUTO_SPAWN_ABSENCE, PROVENANCE_AUDIT_LINK_KIND,
};
pub use e2e_wedge::{run_chat_e2e_wedge, ChatE2eArtifact};
pub use erase::{
    aggregate_receipt, is_body_unrecoverable, ChatEraseReport, ChatErasureCascade, StoreReceipt,
    CHAT_ERASE_CASCADE_TOKEN,
};
pub use fanout::{
    activity, activity_filter, ambient_post_inbox_writes, fanout_behaviour,
    no_second_activity_store, resolve_watchers, write_fanout, AddressedRecipient, FanoutBehaviour,
    Signal, SignalSink, WatcherDirectory, WriteFanoutReason, WATCHER_RELATION,
};
pub use hitl::{
    approval_signal_name, auto_deny_on_timeout, build_card_signal, per_effect_idem_key,
    post_decision, render_card, run_object, CardClick, CardDecision, CardEffect, CardOutcome,
    CardSignal, ChatApprovalCard, ClickDenied, ClickGate, PostDecisionError, RenderedCardEffect,
    ResumeTokenMinter, SignalDelivery, SignalPort, SignalPostError, APPROVAL_SIGNAL_PREFIX,
    APPROVE_PERMISSION, DECLINE_MARKER, TIMEOUT_REASON,
};
pub use holder::{
    chat_store_classifier, register_chat_holders, ChatHolder, ChatStoreClass, RestrictionFlag,
    CHAT_OLTP_STORE, CHAT_RESIDUAL_POSTURE_REF,
};
pub use membership::{MembershipError, MembershipGate, MembershipService, MembershipTupleWriter};
pub use presence::{
    ag_d4_attestation_is_green, presence_and_partials_are_firehose_only, resume_view, run_streamed,
    AgD4Attestation, AgentPresence, FabricHealth, MockStreamRuntime, PartialFrame, PartialPush,
    PresencePush, ResumeView, StreamSession, StreamSessions, StreamState, AGENT_MESSAGE_PARTIAL,
    AGENT_STATUS_CHANGED,
};
pub use project::{
    densest_edge_producer, ChannelMeta, ChatProjectionSource, MessageMeta, ProjectError, Projected,
    Projection as ChatProjection, Projector, RenderHint, ThreadMeta, Tombstone as ProjectTombstone,
    TombstoneReason as ProjectTombstoneReason,
};
pub use read_state::{
    ReadMarker, ReadStatePush, ReadStateRecord, ReadStateService, CHAT_READ_STATE_STORE,
    DEFAULT_FLUSH_CADENCE, HOT_MARKER_TTL, READ_STATE_UPDATED,
};
pub use replay::{
    reindex_parity_hash, ChatReadModelConsumer, ChatReindexSource, ChatReplayKind,
    MessageProjectFetcher, MessageProjection, NOTIF_REASON_MENTIONED,
};
pub use restriction::{
    agent_may_read, analytics_eligible, index_projection_if_allowed, notif_may_route,
    render_body_mentions, render_mention, MentionRender, MentionResolver, ReadPath,
    RestrictionGate, ERASED_USER, LEGAL_RESIDUAL_FLOOR,
};
pub use scylla_followon::{
    scylla_floor_gap_report, FloorFollowOn, TriggerStatus, MEASURED_TRIGGER_FLOORS,
    SCYLLA_HOT_TIER_FLOOR,
};
pub use search::{
    admit_message_indexing, may_index_messages, message_doc_ref, message_index_spec,
    message_index_specs, message_search_acl_anchor, message_search_projection, non_member_filter,
    register_message_index_specs, AclConjoinedSearchFeeder, EmbeddingsArePersonalData,
    ReplayParityFollowOn, CHAT_SUBSYSTEM, FACET_ARTIFACT_REF, FACET_AUTHOR, FACET_CHANNEL,
    FACET_CREATED_AT, FACET_EMBED, FACET_KIND, FACET_MENTION, FACET_THREAD_ROOT, FT_BODY_FIELD,
    MESSAGE_ACL_OBJECT_TYPE, MESSAGE_READ_PERMISSION, MESSAGE_TYPE,
};
pub use store::{
    chat_cold_blob_store_parity, emit_erased_tombstone, AuthorKind, ColdBlobParityVerdict,
    ColdSegments, ConversationId, MemHotTier, Message, MessageId, MessageState, MessageStore,
    MonotonicUlidSource, NewMessage, OutboxTx, RangeCursor, StoreError, SystemUlidSource,
    TombstoneReason, UlidSource, SCYLLA_HOT_TIER_PROMOTED, SCYLLA_PROMOTION_LANDING,
    SCYLLA_PROMOTION_TRIGGER,
};
pub use unfurl::{
    filter_candidates_by_class, precompute_visibility_class, AuthzVisibleIndex, Card,
    LadderOutcome, LoweredFilter, Projection, RefsResolvePort, Tombstone as UnfurlTombstone,
    TombstoneReason as UnfurlTombstoneReason, UnfurlCache, UnfurlCandidate, UnfurlService,
};
