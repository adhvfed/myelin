#![forbid(unsafe_code)]

pub mod composer;
pub mod content;
pub mod conversation;
pub mod cross_org;
pub mod dek;
pub mod erase;
pub mod events;
pub mod fanout;
pub mod glue;
pub mod hitl;
pub mod holder;
pub mod membership;
mod mention_signal;
pub mod presence;
pub mod project;
pub mod provenance;
pub mod read_state;
pub mod rebac_fragment;
pub mod replay;
pub mod restriction;
pub mod schema;
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
pub use cross_org::{
    as_propagated, channel_ref, fanned_out_carried_fields, CellLocalChannelResolution,
    ChannelProjection, CrossOrgChannel, CrossOrgPointer, FederatedMember,
};
pub use dek::{
    decode_encrypted_body, decrypt_body, encode_encrypted_body, encrypt_body, plaintext_at_rest,
    subject_dek_erasure, ChatBodyEnvelopeError, ChatFreeText,
};
pub use erase::{
    aggregate_receipt, is_body_unrecoverable, ChatEraseError, ChatEraseReport, ChatErasureCascade,
    StoreReceipt, CHAT_ERASE_CASCADE_TOKEN,
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
pub use provenance::{agent_provenance, AgentProvenance, PROVENANCE_AUDIT_LINK_KIND};
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
    RestrictionGate, ERASED_USER,
};
pub use search::{
    admit_message_indexing, may_index_messages, message_doc_ref, message_index_spec,
    message_index_specs, message_search_acl_anchor, message_search_projection, non_member_filter,
    register_message_index_specs, AclConjoinedSearchFeeder, EmbeddingsArePersonalData,
    CHAT_SUBSYSTEM, FACET_ARTIFACT_REF, FACET_AUTHOR, FACET_CHANNEL, FACET_CREATED_AT, FACET_EMBED,
    FACET_KIND, FACET_MENTION, FACET_THREAD_ROOT, FT_BODY_FIELD, MESSAGE_ACL_OBJECT_TYPE,
    MESSAGE_READ_PERMISSION, MESSAGE_TYPE,
};
#[cfg(any(test, feature = "test-support"))]
pub use store::MemHotTier;
pub use store::{
    chat_cold_blob_store_parity, emit_erased_tombstone, AuthorKind, ColdBlobParityVerdict,
    ColdSegments, ConversationId, Message, MessageId, MessageState, MessageStore,
    MonotonicUlidSource, NewMessage, OutboxTx, RangeCursor, StoreError, SystemUlidSource,
    TombstoneReason, UlidSource,
};
pub use unfurl::{
    filter_candidates_by_class, precompute_visibility_class, AuthzVisibleIndex, Card,
    LadderOutcome, LoweredFilter, Projection, RefsResolvePort, Tombstone as UnfurlTombstone,
    TombstoneReason as UnfurlTombstoneReason, UnfurlCache, UnfurlCandidate, UnfurlService,
};
