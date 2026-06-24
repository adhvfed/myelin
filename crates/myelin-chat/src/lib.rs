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
//!   `restrict` wired, `erase` stubbed to crypto-shred naming its CHAT-P22 fan-out.
//! - [`replay`] — **EB-27 / P-327** (skeleton confirmed at CHAT-P6): chat's `replay(scope, since)`
//!   re-emits `chat.{channel,message,thread}.snapshot` through the OUTBOX (contract 2.6); full
//!   Search/Refs/Notif replay PARITY is the CHAT-P21 floor.
//!
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

pub mod conversation;
pub mod dek;
pub mod events;
pub mod glue;
pub mod holder;
pub mod membership;
pub mod rebac_fragment;
pub mod replay;
pub mod schema;
pub mod store;
pub mod subs;

pub use conversation::{
    Conversation, ConversationError, ConversationKind, ConversationStore, MemConversationStore,
    Membership, MembershipRole,
};
pub use dek::{decrypt_body, encrypt_body, plaintext_at_rest, subject_dek_erasure, ChatFreeText};
pub use holder::{
    chat_store_classifier, register_chat_holders, ChatHolder, ChatStoreClass, RestrictionFlag,
    CHAT_OLTP_STORE, CHAT_RESIDUAL_POSTURE_REF,
};
pub use membership::{MembershipError, MembershipGate, MembershipService, MembershipTupleWriter};
pub use replay::{ChatReindexSource, ChatReplayKind};
pub use store::{
    AuthorKind, ColdSegments, ConversationId, MemHotTier, Message, MessageId, MessageState,
    MessageStore, MonotonicUlidSource, NewMessage, OutboxTx, RangeCursor, StoreError,
    SystemUlidSource, TombstoneReason, UlidSource,
};
