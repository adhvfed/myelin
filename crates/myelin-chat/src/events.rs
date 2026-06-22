//! # `events` — the complete `chat.*` event token registration, split durable vs firehose
//! (CHAT-P1 / P-128)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md`
//! §1 (the COMPLETE `chat.*` taxonomy under the Bus §6 grammar — canonical subsystem token `chat`;
//! canonical types `channel`/`message`/`thread` plus `reaction`/`presence`/`typing`/`read_state`),
//! §1.1 (the **durable** set via the OUTBOX — the only emit path, BUS-2 / contract 2.2), §1.2 (the
//! **firehose-only** set — never the durable bus, ADR-04.5, over the frozen resume-cursor protocol
//! contract 3.5), §4 (the fanout-class declaration the tokens carry).
//!
//! **Contract-index rows (registered here — against the frozen Bus grammar):**
//! - **2.9** Event taxonomy + token table — `<subsystem>.<artifact_type>.<event_name>`. The Bus
//!   owns the **grammar + the seed**; **each subsystem completes its own list** (contract 2.9
//!   text). This module is Chat COMPLETING its `chat.*` list — chat **registers**, it does **not**
//!   author the grammar. Every token below is validated against the one Bus validator
//!   ([`myelin_events::validate_event_type`], EB-02 / P-042) — there is no second token language.
//! - **2.1** `EventEnvelope` — the canonical envelope every `chat.*` event aligns to (the `type`
//!   field is one of the tokens below; the names/units anchor X-5). Referenced, not re-defined.
//!
//! ## The durable / firehose split is STRUCTURAL (arch §1.1 vs §1.2)
//! The tokens fall into two **disjoint** classes the architecture freezes:
//!
//! - **DURABLE (via the OUTBOX)** — [`CHAT_DURABLE_TOKENS`]. The ONLY set that may ride
//!   `OutboxTx::emit` (BUS-2 / contract 2.2). Per-aggregate ordering is `aggregate = conversation_id`
//!   (arch §1.1; contract 2.3 — the D-9 / CHAT-D2 per-conversation total-order property). These
//!   drive live delivery, unfurl edges, the Search index, the read/write fanout, and the
//!   reindex-from-source rebuild (`*.snapshot`, contract 2.6).
//! - **FIREHOSE-ONLY (never the durable bus)** — [`CHAT_FIREHOSE_TOKENS`]. Ephemeral,
//!   allowed-to-drop frames over the frozen `subscribe/resume/scope` resume-cursor protocol
//!   (contract 3.5); if lost, the durable record (or the final message) is the truth. The
//!   `no-raw-publish` lint + the firehose seam keep these off the durable bus STRUCTURALLY (the
//!   enforcement lands with the emit behaviour in CHAT-P9/P10).
//!
//! [`split_is_disjoint_and_total`] / the unit tests below prove the classification is **total**
//! (every registered token is in exactly one class) and **disjoint** (no token is in both) — the
//! "0 misclassified tokens" gate.
//!
//! ## What this prompt (CHAT-P1 / P-128) ships — and what it deliberately does NOT
//! **Ships:** the complete v1 `chat.*` token registration (arch §1.1/§1.2) as named `&'static str`
//! constants + the two class tables + the [`chat_event_tokens`] union, each PROVEN grammatical
//! against the Bus §6.1/§6.2 grammar (0 ungrammatical — the gate), the split disjoint+total.
//!
//! **Does NOT ship (FLOORS named — VISION §3 name-your-floors):** these tokens are **registered**
//! here but **actually EMITTED only later** — there is no emit body in this prompt:
//! - the **DURABLE set's emit behaviour begins in CHAT-P5** (the message persist +
//!   `chat.message.created` outbox row in ONE PG transaction via `OutboxTx::emit` — the
//!   silent-data-loss floor; per-conversation total order, idempotent send);
//! - the **FIREHOSE set's behaviour begins in CHAT-P10** (the live delivery transport over the
//!   frozen resume-cursor protocol, contract 3.5).
//!
//! ## A note on the Agent-owned firehose frames (NOT registered under `chat.*`)
//! Architecture §1.2 names `agent.message.partial` (streaming) and the "live message delivery
//! frame" as firehose frames Chat participates in. Those carry the **`agent`** subsystem prefix (an
//! Agent-Fabric-owned token, registered by the agent subsystem) and the live-delivery frame is a
//! transport frame, not a `chat.*` taxonomy token — Chat does NOT register foreign-subsystem tokens
//! (the acyclic-producer invariant, EI-02 §3, same discipline as `myelin-git::events`). Chat's
//! firehose-only `chat.*` tokens are the presence/typing/fine-grained-read_state set below.

use myelin_events::validate_event_type;

// ===========================================================================
// §1.1 — the DURABLE `chat.*` tokens (via the OUTBOX — the only emit path, BUS-2 / contract 2.2)
//
// Names are taken verbatim from arch 03 §1.1 (the durable taxonomy table). Per-aggregate ordering
// is `aggregate = conversation_id` (arch §1.1 / contract 2.3). Every constant is asserted
// grammatical against the Bus validator in `tests` below (0 ungrammatical). Emitted only from the
// outbox in CHAT-P5 (message family) / CHAT-P6 (erased, replay snapshots) / CHAT-P8 (channel
// membership family) — registered here, no emit body.
// ===========================================================================

// --- message lifecycle (aggregate: conversation_id) ------------------------

/// A message was created — drives live delivery, unfurl edges (`refs.edge.created`), the Search
/// index, read-fanout. Emit body: **CHAT-P5** (persist + outbox co-commit in one PG tx).
pub const CHAT_MESSAGE_CREATED: &str = "chat.message.created";
/// A message was edited (new `edited_seq` per-message CAS, changed nodes) — re-index/re-unfurl.
pub const CHAT_MESSAGE_EDITED: &str = "chat.message.edited";
/// A message was deleted (timeline removal; derived-store update).
pub const CHAT_MESSAGE_DELETED: &str = "chat.message.deleted";
/// A message was erased (crypto-shred tombstone; the cross-cutting `*.erased`, contract 2.7) —
/// drives the Search/Refs/Notif erasure cascade. Emit body: **CHAT-P6** (the holder erasure path).
pub const CHAT_MESSAGE_ERASED: &str = "chat.message.erased";
/// A principal was mentioned in a message — **the write-fanout / notify-reason producer** (arch §4
/// / Notif): the frozen `mention(Principal)` node → a Signal → Notif write-fanout.
pub const CHAT_MESSAGE_MENTIONED: &str = "chat.message.mentioned";

// --- reaction (aggregate: conversation_id) ---------------------------------

/// A reaction was added (lightweight signal; a ✅ may be an *explicit* approve-action).
pub const CHAT_REACTION_ADDED: &str = "chat.reaction.added";
/// A reaction was removed.
pub const CHAT_REACTION_REMOVED: &str = "chat.reaction.removed";

// --- thread (aggregate: conversation_id) -----------------------------------

/// A thread was created (the frozen `thread-<root>` `#sub` kind) — drives thread watch/read-fanout.
pub const CHAT_THREAD_CREATED: &str = "chat.thread.created";
/// A reply was posted to a thread — drives thread watch/read-fanout.
pub const CHAT_THREAD_REPLIED: &str = "chat.thread.replied";

// --- channel lifecycle + membership (aggregate: conversation_id) -----------

/// A channel was created — lifecycle; ReBAC tuple write. Emit body: **CHAT-P8**.
pub const CHAT_CHANNEL_CREATED: &str = "chat.channel.created";
/// A channel was archived — lifecycle.
pub const CHAT_CHANNEL_ARCHIVED: &str = "chat.channel.archived";
/// A member was added — membership/visibility/unfurl recompute; **ReBAC tuple write**
/// (`write_tuples`, returns zookie — the new-enemy guard). Emit body: **CHAT-P8**.
pub const CHAT_CHANNEL_MEMBER_ADDED: &str = "chat.channel.member_added";
/// A member was removed — membership/visibility/unfurl recompute; **ReBAC tuple write**.
pub const CHAT_CHANNEL_MEMBER_REMOVED: &str = "chat.channel.member_removed";
/// A channel was linked to an `ArtifactRef` → `refs.edge.created` ("discussed in").
pub const CHAT_CHANNEL_LINKED: &str = "chat.channel.linked";

// --- read-state (COARSE only — the fine grain is firehose, §1.2) -----------

/// A read-state coarse summary was updated (optional cross-device coarse sync). The **fine-grained**
/// read-state is firehose-only ([`CHAT_READ_STATE_VIEWED`]); this coarse `*.updated` is durable.
pub const CHAT_READ_STATE_UPDATED: &str = "chat.read_state.updated";

// --- cross-cutting `*.snapshot` reindex-from-source events (contract 2.6) ---

/// The reindex-from-source projection for a channel (sub-artifact-granular) — Search/Refs/Notif/
/// OLAP rebuild via `replay(scope, since)` (arch §6). Emitted through the OUTBOX, never a direct
/// publish. Replay skeleton: **CHAT-P6**; full parity: **CHAT-P21**.
pub const CHAT_CHANNEL_SNAPSHOT: &str = "chat.channel.snapshot";
/// The reindex-from-source projection for a message (sub-artifact-granular).
pub const CHAT_MESSAGE_SNAPSHOT: &str = "chat.message.snapshot";
/// The reindex-from-source projection for a thread (sub-artifact-granular).
pub const CHAT_THREAD_SNAPSHOT: &str = "chat.thread.snapshot";

/// The complete **durable** `chat.*` token set (arch §1.1) — the ONLY set that may ride
/// `OutboxTx::emit` (BUS-2 / contract 2.2). Per-aggregate ordering `aggregate = conversation_id`.
pub const CHAT_DURABLE_TOKENS: &[&str] = &[
    // message lifecycle
    CHAT_MESSAGE_CREATED,
    CHAT_MESSAGE_EDITED,
    CHAT_MESSAGE_DELETED,
    CHAT_MESSAGE_ERASED,
    CHAT_MESSAGE_MENTIONED,
    // reaction
    CHAT_REACTION_ADDED,
    CHAT_REACTION_REMOVED,
    // thread
    CHAT_THREAD_CREATED,
    CHAT_THREAD_REPLIED,
    // channel lifecycle + membership
    CHAT_CHANNEL_CREATED,
    CHAT_CHANNEL_ARCHIVED,
    CHAT_CHANNEL_MEMBER_ADDED,
    CHAT_CHANNEL_MEMBER_REMOVED,
    CHAT_CHANNEL_LINKED,
    // read-state (coarse)
    CHAT_READ_STATE_UPDATED,
    // cross-cutting *.snapshot (contract 2.6)
    CHAT_CHANNEL_SNAPSHOT,
    CHAT_MESSAGE_SNAPSHOT,
    CHAT_THREAD_SNAPSHOT,
];

// ===========================================================================
// §1.2 — the FIREHOSE-only `chat.*` tokens (NEVER the durable bus — ADR-04.5; over the frozen
// resume-cursor protocol, contract 3.5)
//
// Ephemeral, allowed-to-drop frames. If lost, the durable record (or the final message) is the
// truth. The `no-raw-publish` lint + the firehose seam keep these off the durable bus structurally
// when the emit behaviour lands (CHAT-P10). The Agent-owned `agent.message.partial` + the live
// delivery transport frame are firehose frames Chat participates in but does NOT register here —
// they are not `chat.*` taxonomy tokens (the acyclic-producer invariant).
// ===========================================================================

/// A principal's presence changed (online/away/offline; incl. **agent presence** classes
/// available/busy/rate-limited/offline). High-volume, ephemeral — firehose only.
pub const CHAT_PRESENCE_CHANGED: &str = "chat.presence.changed";
/// A principal started typing — ephemeral, firehose only.
pub const CHAT_TYPING_STARTED: &str = "chat.typing.started";
/// A principal stopped typing — ephemeral, firehose only.
pub const CHAT_TYPING_STOPPED: &str = "chat.typing.stopped";
/// A **fine-grained** read-state frame (e.g. a per-message viewed marker) — high-volume, ephemeral,
/// firehose only. The COARSE summary ([`CHAT_READ_STATE_UPDATED`]) is the durable counterpart.
pub const CHAT_READ_STATE_VIEWED: &str = "chat.read_state.viewed";

/// The complete **firehose-only** `chat.*` token set (arch §1.2) — NEVER the durable bus
/// (ADR-04.5); over the frozen resume-cursor protocol (contract 3.5). Allowed-to-drop.
pub const CHAT_FIREHOSE_TOKENS: &[&str] = &[
    CHAT_PRESENCE_CHANGED,
    CHAT_TYPING_STARTED,
    CHAT_TYPING_STOPPED,
    CHAT_READ_STATE_VIEWED,
];

/// The complete `chat.*` token registry = the DURABLE set ∪ the FIREHOSE set. The union the
/// grammar gate runs over (0 ungrammatical) and the no-duplicate / chat-prefix checks assert.
pub fn chat_event_tokens() -> Vec<&'static str> {
    CHAT_DURABLE_TOKENS
        .iter()
        .chain(CHAT_FIREHOSE_TOKENS.iter())
        .copied()
        .collect()
}

/// Register the complete `chat.*` list against the Bus grammar (contract 2.9). Returns `Ok(())` iff
/// **every** registered token (durable ∪ firehose) parses the §6.1 grammar via the one Bus
/// validator ([`myelin_events::validate_event_type`]); otherwise the first offending token + its
/// [`myelin_events::TaxonomyError`] (LOUD, never silently coerced). This is the registration check
/// the GATE asserts (0 ungrammatical tokens) — chat REGISTERS its list against the grammar it does
/// not own.
pub fn register_chat_tokens() -> Result<(), (&'static str, myelin_events::TaxonomyError)> {
    for tok in chat_event_tokens() {
        validate_event_type(tok).map_err(|e| (tok, e))?;
    }
    Ok(())
}

/// The durable/firehose classification of a registered `chat.*` token (arch §1.1 vs §1.2). The
/// STRUCTURAL split the `no-raw-publish` lint enforces when the emit paths land (CHAT-P5/P10): the
/// durable class is the ONLY class that may ride `OutboxTx::emit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryClass {
    /// Via the OUTBOX — the only durable emit path (BUS-2 / contract 2.2). `aggregate =
    /// conversation_id`.
    Durable,
    /// Firehose only — never the durable bus (ADR-04.5); over the resume-cursor protocol (3.5).
    Firehose,
}

/// Classify a `chat.*` token into its [`DeliveryClass`], or `None` if it is not a registered chat
/// token. **Total over the registry** (every registered token classifies) and **disjoint** (a token
/// is durable XOR firehose) — see [`split_is_disjoint_and_total`].
pub fn delivery_class(token: &str) -> Option<DeliveryClass> {
    if CHAT_DURABLE_TOKENS.contains(&token) {
        Some(DeliveryClass::Durable)
    } else if CHAT_FIREHOSE_TOKENS.contains(&token) {
        Some(DeliveryClass::Firehose)
    } else {
        None
    }
}

/// The structural-split gate (contract 2.9 / arch §1.1-§1.2): the durable and firehose sets are
/// **disjoint** (no token in both) AND **total** (every token in [`chat_event_tokens`] classifies).
/// Returns `true` iff the split is well-formed (0 misclassified tokens) — the "0 misclassified"
/// gate, as a callable invariant (not only a test assertion).
pub fn split_is_disjoint_and_total() -> bool {
    // Disjoint: no durable token is also a firehose token.
    let disjoint = !CHAT_DURABLE_TOKENS
        .iter()
        .any(|d| CHAT_FIREHOSE_TOKENS.contains(d));
    // Total: every union member classifies into exactly one side.
    let total = chat_event_tokens()
        .iter()
        .all(|t| delivery_class(t).is_some());
    disjoint && total
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **THE GATE (contract 2.9): 0 ungrammatical tokens.** Every registered `chat.*` token (durable
    /// ∪ firehose) parses the Bus §6.1/§6.2 grammar via the one Bus validator — chat registers
    /// against the grammar it does not author. The parse is the green artifact.
    #[test]
    fn every_chat_token_parses_the_bus_grammar() {
        for tok in chat_event_tokens() {
            assert!(
                validate_event_type(tok).is_ok(),
                "registered chat token `{tok}` is UNGRAMMATICAL: {:?}",
                validate_event_type(tok)
            );
        }
        // The whole-list registration helper agrees (0 ungrammatical).
        assert!(
            register_chat_tokens().is_ok(),
            "register_chat_tokens() must succeed: {:?}",
            register_chat_tokens()
        );
    }

    /// Every registered token carries the canonical `chat` subsystem prefix (§6.2 — `chat` is the
    /// canonical subsystem token; CLI aliases are render-time only and never the stored token).
    #[test]
    fn every_chat_token_carries_the_chat_subsystem_prefix() {
        for tok in chat_event_tokens() {
            let head = tok.split('.').next().expect("non-empty token");
            assert_eq!(
                head, "chat",
                "token `{tok}` must carry the `chat` subsystem prefix"
            );
        }
        // ...and `chat` is the canonical subsystem token the Bus knows.
        assert!(
            myelin_events::SUBSYSTEM_TOKENS.contains(&"chat"),
            "`chat` must be a canonical Bus subsystem token"
        );
    }

    /// The registry has **no duplicates** across the union — a token registered twice (or in both
    /// classes) is a contract smell (each name is minted once).
    #[test]
    fn the_chat_token_registry_has_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for tok in chat_event_tokens() {
            assert!(
                seen.insert(tok),
                "chat token `{tok}` is registered more than once"
            );
        }
        assert_eq!(seen.len(), chat_event_tokens().len());
    }

    /// **THE SPLIT GATE (0 misclassified tokens): the durable/firehose classification is DISJOINT +
    /// TOTAL.** No firehose-only token is in the durable set and vice versa; every registered token
    /// classifies into exactly one side. This is the structural-separation contract the
    /// `no-raw-publish` lint enforces when the emit paths land (CHAT-P5/P10).
    #[test]
    fn the_durable_firehose_split_is_disjoint_and_total() {
        // The callable invariant agrees.
        assert!(
            split_is_disjoint_and_total(),
            "the durable/firehose split must be disjoint AND total"
        );
        // Disjoint, spelled out: 0 tokens appear in both classes.
        for d in CHAT_DURABLE_TOKENS {
            assert!(
                !CHAT_FIREHOSE_TOKENS.contains(d),
                "token `{d}` is in BOTH the durable and firehose sets (misclassified)"
            );
        }
        // Total: every union member classifies into exactly one side, and the two sides partition
        // the union (sizes add up — no token is lost or double-counted).
        for t in chat_event_tokens() {
            assert!(
                delivery_class(t).is_some(),
                "token `{t}` does not classify into a delivery class (split not total)"
            );
        }
        assert_eq!(
            CHAT_DURABLE_TOKENS.len() + CHAT_FIREHOSE_TOKENS.len(),
            chat_event_tokens().len(),
            "the durable + firehose sizes must partition the union exactly"
        );
    }

    /// The durable set classifies as [`DeliveryClass::Durable`] and the firehose set as
    /// [`DeliveryClass::Firehose`] — the classification matches the arch §1.1/§1.2 tables, token by
    /// token. A non-chat / unregistered token classifies as `None`.
    #[test]
    fn delivery_class_matches_the_architecture_tables() {
        for d in CHAT_DURABLE_TOKENS {
            assert_eq!(
                delivery_class(d),
                Some(DeliveryClass::Durable),
                "`{d}` must be Durable"
            );
        }
        for f in CHAT_FIREHOSE_TOKENS {
            assert_eq!(
                delivery_class(f),
                Some(DeliveryClass::Firehose),
                "`{f}` must be Firehose"
            );
        }
        // An unregistered token (a foreign-subsystem name) does not classify.
        assert_eq!(delivery_class("git.pr.opened"), None);
        assert_eq!(delivery_class("chat.message.nonexistent"), None);
    }

    /// The load-bearing chat tokens are present under their NAMED constants (the names anchor X-5 —
    /// Search/Refs/Notif/the live-delivery firehose consume these by name, never by literal). A
    /// rename/drop here is a contract change every consumer must reconcile.
    #[test]
    fn the_load_bearing_chat_tokens_are_registered() {
        // the durable message family (CHAT-P5 emit follow-on)
        assert!(CHAT_DURABLE_TOKENS.contains(&CHAT_MESSAGE_CREATED));
        assert!(CHAT_DURABLE_TOKENS.contains(&CHAT_MESSAGE_MENTIONED)); // the write-fanout producer
                                                                        // the membership family (CHAT-P8 emit follow-on — the ReBAC tuple write)
        assert!(CHAT_DURABLE_TOKENS.contains(&CHAT_CHANNEL_MEMBER_ADDED));
        assert!(CHAT_DURABLE_TOKENS.contains(&CHAT_CHANNEL_MEMBER_REMOVED));
        // the cross-cutting *.erased + *.snapshot tokens (contracts 2.7 / 2.6)
        assert!(CHAT_DURABLE_TOKENS.contains(&CHAT_MESSAGE_ERASED));
        assert!(CHAT_DURABLE_TOKENS.contains(&CHAT_MESSAGE_SNAPSHOT));
        // the firehose-only set (CHAT-P10 transport follow-on)
        assert!(CHAT_FIREHOSE_TOKENS.contains(&CHAT_PRESENCE_CHANGED));
        assert!(CHAT_FIREHOSE_TOKENS.contains(&CHAT_READ_STATE_VIEWED));
    }

    /// Chat does NOT register a foreign-subsystem token — no `agent.*` (the Agent-owned
    /// `agent.message.partial` firehose frame is registered by the agent subsystem, not Chat), no
    /// `refs.*` / `ci.*` echoes. No registered token leaves the `chat` prefix. The in-crate proof of
    /// the acyclic-producer invariant (EI-02 §3), matching `myelin-git::events`.
    #[test]
    fn chat_registers_no_foreign_subsystem_tokens() {
        for tok in chat_event_tokens() {
            assert!(
                tok.starts_with("chat."),
                "chat must not register the foreign-subsystem token `{tok}`"
            );
        }
    }

    /// The COARSE read-state (`chat.read_state.updated`) is DURABLE while the FINE read-state
    /// (`chat.read_state.viewed`) is FIREHOSE — the arch §1.1/§1.2 split of the high-volume
    /// read-state stream (coarse cross-device sync is durable; per-message viewed markers are not).
    #[test]
    fn read_state_coarse_is_durable_fine_is_firehose() {
        assert_eq!(
            delivery_class(CHAT_READ_STATE_UPDATED),
            Some(DeliveryClass::Durable)
        );
        assert_eq!(
            delivery_class(CHAT_READ_STATE_VIEWED),
            Some(DeliveryClass::Firehose)
        );
    }
}
