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
//!   (co-committed with the outbox event) is the CHAT-P6 floor.
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

pub mod events;
pub mod glue;
pub mod rebac_fragment;
pub mod replay;
pub mod store;
pub mod subs;

pub use replay::{ChatReindexSource, ChatReplayKind};
pub use store::{
    AuthorKind, ColdSegments, ConversationId, MemHotTier, Message, MessageId, MessageState,
    MessageStore, MonotonicUlidSource, NewMessage, OutboxTx, RangeCursor, StoreError,
    SystemUlidSource, TombstoneReason, UlidSource,
};
