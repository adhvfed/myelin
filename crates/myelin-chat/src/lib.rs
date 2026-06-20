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
//!   its dotted-name list, each token validated against the one Bus grammar. The ReBAC fragment +
//!   `#sub` grammar are CHAT-P2; the humanise/notif/fanout slice is CHAT-P3.
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
