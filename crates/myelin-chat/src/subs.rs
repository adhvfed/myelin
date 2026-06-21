//! # `subs` — chat's `#sub` mints registered with Refs (CHAT-P2 / P-244, contract 5.7)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md` §2 (the
//! `ArtifactRef` + the frozen `#sub` scheme — chat mints `myelin://<tenant>/chat/{channel|message|
//! thread}/<id>`; the frozen Chat `#sub` kinds are `message-<opaqueid>` for a single message and
//! `thread-<opaqueid>` for a thread root; the `<opaqueid>` is the immutable `message_id` /
//! `thread_root_id` ULID — **a stable opaque id, not a positional index**, stable across edits) +
//! `01-tech-and-data-model.md` (the message/thread identity the `#sub` is minted from).
//! **Reconciliation:**
//! `05-refined-shared-systems-architecture/00-reconciliation-decisions.md` X-4 (the unified `#sub`
//! grammar + the one resolution ladder, frozen — `thread-`/`message-` are the Chat kinds). **Contract:**
//! `contract-index.md` row 5.7 — the unified `#sub` grammar; **chat owns the `message-` / `thread-`
//! mints** (Refs owns the grammar + the ladder; the shared `thread-` kind, OQ-L).
//!
//! ## The seam this module pins (chat mints; Refs owns the grammar)
//! Refs ([`myelin_refs`]) owns ONE `#sub` URN grammar + ONE resolution ladder (recon X-4). Each
//! subsystem owns the **stable opaque mint** of its declared kinds. This module is chat's side:
//!
//! - [`register_chat_sub_kinds`] — chat's REGISTRATION with Refs: it DECLARES that chat owns (mints +
//!   will resolve) the [`SubKind::Message`] / [`SubKind::Thread`] kinds. The registration is validated
//!   by Refs ([`SubKindRegistration::validate`]) and ACCEPTED. This is the deliverable of CHAT-P2: the
//!   `#sub` grammar (message-/thread-) is frozen + registered.
//! - [`mint_message`] / [`mint_thread`] — chat's typed `#sub` mints. They build chat's **canonical
//!   root** (`chat/message/<message_id>`, `chat/thread/<thread_root_id>`) and attach the stable opaque
//!   sub-id through the one Refs codec ([`myelin_refs::mint`]), so every minted ref is grammatical **by
//!   construction** (0 ungrammatical — `mint` re-parses through the frozen grammar and rejects a
//!   malformed opaque body LOUDLY). Refs stores both the full sub-URN AND the
//!   [`myelin_refs::strip_sub`] root.
//!
//! ## The stability obligation is chat's (architecture §2 / contract 5.7)
//! The `<opaqueid>` is the **immutable `message_id` / `thread_root_id` ULID** — a stable opaque id,
//! never a positional index. An edited message keeps its `message_id` and thus its `#sub`, so embeds
//! and references **don't dangle** across edits. Refs validates the grammar SHAPE only; the opacity +
//! immutability of the id is chat's contract. Refs stores the full sub-URN AND the `#sub`-stripped
//! root, so a broken sub-anchor still resolves to the parent via the one 4-step tombstone ladder
//! (live / gone / erased for chat — a message has no moved/outdated state; it is content-addressed by
//! stable id, §2).
//!
//! ## FLOORS named (EI-01 §1 — this is the REGISTRATION + the GRAMMAR, NOT the minting/resolution)
//! Only the kind REGISTRATION + the grammatical mint codecs ship here. The named follow-ons:
//! - **The runtime `#sub` MINTING** (the message/thread create path that calls [`mint_message`] /
//!   [`mint_thread`] with a real persisted `message_id` / `thread_root_id`, co-committed with the
//!   `chat.message.created` / `chat.thread.created` outbox event) is the **CHAT-P6 follow-on** — this
//!   prompt freezes the grammar + ships the mint CODEC; the mint SITE lands in CHAT-P6.
//! - **The `message-` / `thread-` `project(ref, viewer)` sub-anchor resolver** (the Refs ladder calls)
//!   lands with the Chat projection spine (the per-viewer permission-gated projection, §3).
//!
//! So this module is the contract-5.7 mint half (chat-owned), not the working mint site or resolver.

use myelin_refs::{mint, ArtifactRef, ParseError, Sub, SubKind, SubKindRegistration};

/// The canonical Bus §6.2 subsystem token chat owns (§2 — `myelin://<tenant>/chat/<type>/<id>`).
pub const CHAT_SUBSYSTEM: &str = "chat";

/// The `#sub` kinds chat is the mint + resolver owner of (contract 5.7 / architecture §2): a single
/// chat message (`message-<message_id>`) and a thread root (`thread-<thread_root_id>`, the kind
/// shared with Git review threads, OQ-L). Chat does NOT mint `comment-` (that is Git/Issues/Knowledge)
/// nor any line-range / block / field kind.
pub const CHAT_OWNED_SUB_KINDS: &[SubKind] = &[SubKind::Message, SubKind::Thread];

/// Chat's registration of its owned `#sub` kinds WITH Refs (contract 5.7, the deliverable of CHAT-P2 /
/// P-244). Returns the [`SubKindRegistration`] that Refs **accepts** (validated against the frozen
/// grammar + the Bus token table). This DECLARES the kinds chat mints; it does NOT install a resolver
/// (the resolver is the named follow-on — the Chat projection spine).
///
/// # Errors
/// Returns a [`myelin_refs::RegistrationError`] if the registration is not accepted — by construction
/// it always is (the subsystem token is canonical, the kinds are a non-empty, duplicate-free subset of
/// the frozen vocabulary); the fallible signature is the honest contract surface (Refs is the
/// authority that accepts, chat does not get to assert acceptance).
pub fn register_chat_sub_kinds() -> Result<SubKindRegistration, myelin_refs::RegistrationError> {
    SubKindRegistration {
        subsystem: CHAT_SUBSYSTEM.to_string(),
        kinds: CHAT_OWNED_SUB_KINDS.to_vec(),
    }
    .validate()
}

/// Build chat's canonical **message root** `myelin://<tenant>/chat/message/<message_id>` (architecture
/// §2 — the `message_id` ULID is chat's stable mintable key, NOT a positional index). This is the root
/// a `message-` sub attaches to.
fn message_root(tenant: &str, message_id: &str) -> Result<ArtifactRef, ParseError> {
    myelin_refs::parse(&format!("myelin://{tenant}/chat/message/{message_id}"))
}

/// Build chat's canonical **thread root** `myelin://<tenant>/chat/thread/<thread_root_id>`
/// (architecture §2/§3 — `chat/thread → sub_anchor: thread-<root>`; the `thread_root_id` ULID is the
/// immutable root). This is the root a `thread-` sub attaches to.
fn thread_root(tenant: &str, thread_root_id: &str) -> Result<ArtifactRef, ParseError> {
    myelin_refs::parse(&format!("myelin://{tenant}/chat/thread/{thread_root_id}"))
}

/// Mint a **single-message** sub-URN `…/chat/message/<message_id>#message-<message_id>` (contract 5.7,
/// `message-` kind). The opaque body is chat's **immutable `message_id` ULID** (the stability
/// obligation is chat's, §2 — the id does not change when the message is edited, so the `#sub` survives
/// edits). The result is grammatical by construction (it round-trips the frozen grammar); an empty
/// `message_id` is rejected LOUDLY.
///
/// The same `message_id` is both the root `<id>` and the `#sub` opaque body — the `#sub` is the
/// stable sub-anchor onto the message artifact itself (so a reference like "see this message" pins the
/// exact message even as the surrounding timeline mutates).
///
/// **FLOOR:** this MINTS the stable ref; the runtime mint SITE (co-committed with
/// `chat.message.created`) is CHAT-P6; the `project(ref, viewer)` resolver is the Chat projection spine.
pub fn mint_message(tenant: &str, message_id: &str) -> Result<ArtifactRef, ParseError> {
    let root = message_root(tenant, message_id)?;
    mint(&root, Sub::Message(message_id.to_string()))
}

/// Mint a **thread-root** sub-URN `…/chat/thread/<thread_root_id>#thread-<thread_root_id>` (contract
/// 5.7, `thread-` kind — the kind shared with Git review threads, OQ-L). The opaque body is chat's
/// **immutable `thread_root_id` ULID** (the stability obligation is chat's, §2). The result is
/// grammatical by construction; an empty `thread_root_id` is rejected LOUDLY.
///
/// **FLOOR:** the runtime mint SITE (co-committed with `chat.thread.created`) is CHAT-P6; the
/// `project(ref, viewer)` resolver is the Chat projection spine.
pub fn mint_thread(tenant: &str, thread_root_id: &str) -> Result<ArtifactRef, ParseError> {
    let root = thread_root(tenant, thread_root_id)?;
    mint(&root, Sub::Thread(thread_root_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_refs::{strip_sub, sub_kind};

    /// The registration is ACCEPTED by Refs (the CHAT-P2 GATE) and declares exactly the two kinds chat
    /// owns — message-/thread- — and NO foreign kind.
    #[test]
    fn chat_sub_kind_registration_is_accepted_and_declares_only_chat_owned_kinds() {
        let reg = register_chat_sub_kinds().expect("Refs must accept chat's #sub registration");
        assert_eq!(reg.subsystem, "chat");
        assert_eq!(reg.kinds, vec![SubKind::Message, SubKind::Thread]);
        // chat does NOT register the Git/Issues/Knowledge/CI-owned kinds (architecture §2).
        for foreign in [
            SubKind::Comment,
            SubKind::LineRange,
            SubKind::Block,
            SubKind::Heading,
            SubKind::Row,
            SubKind::Field,
            SubKind::Check,
            SubKind::Step,
        ] {
            assert!(
                !reg.kinds.contains(&foreign),
                "chat must not claim the foreign kind {foreign:?}"
            );
        }
    }

    /// Every chat mint produces a GRAMMATICAL sub-URN (0 ungrammatical): it round-trips the frozen
    /// grammar, classifies to the right [`SubKind`], and its `strip_sub` root is chat's canonical root.
    #[test]
    fn chat_mints_produce_grammatical_round_tripping_sub_urns() {
        // message- on a message root (the message_id is both the root id and the #sub opaque body).
        let m = mint_message("acme-eu", "01J0MSGULID").unwrap();
        assert_eq!(
            myelin_refs::format(&m),
            "myelin://acme-eu/chat/message/01J0MSGULID#message-01J0MSGULID"
        );
        assert_eq!(sub_kind(&m).map(|s| s.kind()), Some(SubKind::Message));
        assert_eq!(
            myelin_refs::format(&strip_sub(&m)),
            "myelin://acme-eu/chat/message/01J0MSGULID"
        );

        // thread- on a thread root.
        let t = mint_thread("acme-eu", "01J0THRROOT").unwrap();
        assert_eq!(
            myelin_refs::format(&t),
            "myelin://acme-eu/chat/thread/01J0THRROOT#thread-01J0THRROOT"
        );
        assert_eq!(sub_kind(&t).map(|s| s.kind()), Some(SubKind::Thread));
        assert_eq!(
            myelin_refs::format(&strip_sub(&t)),
            "myelin://acme-eu/chat/thread/01J0THRROOT"
        );
    }

    /// The `#sub` survives an edit by construction: re-minting with the SAME immutable `message_id`
    /// produces the IDENTICAL sub-URN (the stability obligation, §2 — an edited message keeps its id
    /// and thus its `#sub`, so embeds don't dangle).
    #[test]
    fn the_sub_is_stable_across_edits_because_the_id_is_immutable() {
        let before = mint_message("acme", "01J0STABLE").unwrap();
        // an edit changes the message BODY but never the message_id; re-minting yields the same #sub.
        let after = mint_message("acme", "01J0STABLE").unwrap();
        assert_eq!(before, after, "the #sub is stable across edits (the id is immutable)");
    }

    /// An empty opaque message / thread id is rejected LOUDLY at mint time (the stable-id obligation is
    /// chat's, but the GRAMMAR refuses an empty body — a malformed mint never reaches Refs as a
    /// sub-URN, 0 ungrammatical by construction).
    #[test]
    fn empty_opaque_id_is_rejected_at_mint_time() {
        assert!(matches!(
            mint_message("acme", ""),
            Err(ParseError::EmptySegment { .. } | ParseError::UnknownSubKind { .. })
        ));
        assert!(matches!(
            mint_thread("acme", ""),
            Err(ParseError::EmptySegment { .. } | ParseError::UnknownSubKind { .. })
        ));
    }
}
