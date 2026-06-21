//! # `events` — the complete `knowledge.*` event-token list (EB-26 / P-246, M3)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §6.1 (the dotted-name grammar —
//! the AUTHORITY), §6.4 (the seed `knowledge.*` names), §4.9 (the `*.snapshot` reindex events).
//! **Contract:** index row **2.9** ("each subsystem completes its list" — the Bus owns the grammar;
//! Knowledge COMPLETES its `knowledge.*` list, validated against the one grammar) + row **2.6**
//! (the `*.snapshot` reindex-from-source events Knowledge re-emits, page-subtree at block
//! granularity). **Doctrine:** `external-insights/01-process-and-quality-doctrine.md` §7 (one
//! grammar, no per-subsystem drift — the X-5 names anchor).
//!
//! ## What this is (KN COMPLETES its list — the M3 owner registration)
//! The Bus owns the §6.1 grammar + the seed; Knowledge OWNS + completes its full `knowledge.*`
//! dotted-name list HERE and REGISTERS it against the Bus grammar ([`register_knowledge_tokens`] +
//! the EB-26 [`myelin_events::TokenListHarness`]). Knowledge registers; it does NOT author the
//! grammar (the validator is `myelin_events::validate_event_type` — one grammar, no drift).
//!
//! ## The durable / firehose split (arch — collab op-streams are firehose)
//! Knowledge's COLLAB op-streams (the per-keystroke CRDT ops, KN-D1) ride the **firehose** over the
//! EB-21 transport — the durable bus carries only the pointer events (`knowledge.page.updated`,
//! `knowledge.block.updated`, …) the derived stores index. The fine-grained collab op-stream tokens
//! are firehose-only ([`KNOWLEDGE_FIREHOSE_TOKENS`]); the durable set is [`KNOWLEDGE_DURABLE_TOKENS`].
//! Every token — durable or firehose — obeys the ONE §6.1 grammar (proven below).

use myelin_events::validate_event_type;

// =================================================================================================
// §1 — the DURABLE `knowledge.*` tokens (via the OUTBOX — the derived-store-indexed pointer events)
// =================================================================================================

/// A page was created.
pub const KNOWLEDGE_PAGE_CREATED: &str = "knowledge.page.created";
/// A page's metadata/title was updated (the durable pointer the index re-reads; the per-keystroke
/// body ops ride the firehose — see [`KNOWLEDGE_FIREHOSE_TOKENS`]).
pub const KNOWLEDGE_PAGE_UPDATED: &str = "knowledge.page.updated";
/// A page was moved within the tree (re-parented).
pub const KNOWLEDGE_PAGE_MOVED: &str = "knowledge.page.moved";
/// A page was archived/trashed.
pub const KNOWLEDGE_PAGE_ARCHIVED: &str = "knowledge.page.archived";
/// A doc-level structural update settled (the coalesced durable pointer after a collab session).
pub const KNOWLEDGE_DOC_UPDATED: &str = "knowledge.doc.updated";
/// A block's content settled (the durable pointer at BLOCK granularity — the index re-derives the
/// block subtree; the per-keystroke ops are firehose).
pub const KNOWLEDGE_BLOCK_UPDATED: &str = "knowledge.block.updated";
/// A database-view row settled.
pub const KNOWLEDGE_ROW_UPDATED: &str = "knowledge.row.updated";

// --- cross-cutting *.erased tombstones (contract 2.7) ---------------------------

/// The `*.erased` tombstone for a page (crypto-shred the page's per-subject DEK, contract 2.7).
pub const KNOWLEDGE_PAGE_ERASED: &str = "knowledge.page.erased";

// --- cross-cutting *.snapshot reindex-from-source events (contract 2.6) ----------

/// The `*.snapshot` reindex event for a page (contract 2.6 — `replay`). Cold == live.
pub const KNOWLEDGE_PAGE_SNAPSHOT: &str = "knowledge.page.snapshot";
/// The `*.snapshot` reindex event at BLOCK granularity (the page-subtree replay re-emits one per
/// block, contract 2.6 — "KN page-subtree at block granularity").
pub const KNOWLEDGE_BLOCK_SNAPSHOT: &str = "knowledge.block.snapshot";

/// The complete DURABLE `knowledge.*` list (the outbox-emitted pointer events the derived stores
/// index). The Bus taxonomy (contract 2.9) admits exactly these under §6.1; each is PROVEN
/// grammatical below.
pub const KNOWLEDGE_DURABLE_TOKENS: &[&str] = &[
    KNOWLEDGE_PAGE_CREATED,
    KNOWLEDGE_PAGE_UPDATED,
    KNOWLEDGE_PAGE_MOVED,
    KNOWLEDGE_PAGE_ARCHIVED,
    KNOWLEDGE_DOC_UPDATED,
    KNOWLEDGE_BLOCK_UPDATED,
    KNOWLEDGE_ROW_UPDATED,
    // cross-cutting *.erased + *.snapshot
    KNOWLEDGE_PAGE_ERASED,
    KNOWLEDGE_PAGE_SNAPSHOT,
    KNOWLEDGE_BLOCK_SNAPSHOT,
];

// =================================================================================================
// §2 — the FIREHOSE-only `knowledge.*` tokens (the collab op-streams — NEVER the durable bus)
// =================================================================================================

/// A per-keystroke collab CRDT op on a block (the firehose op-stream, KN-D1 — `resume(scope=doc,
/// last_seq)` loses 0 ops). High-volume, ephemeral — firehose only, NEVER the durable bus.
pub const KNOWLEDGE_BLOCK_OP: &str = "knowledge.block.op";
/// A live presence/cursor update in a doc (ephemeral — firehose only).
pub const KNOWLEDGE_PRESENCE_UPDATED: &str = "knowledge.presence.updated";

/// The complete FIREHOSE-only `knowledge.*` list (the collab op-streams — NEVER the durable bus,
/// ADR-04.5). These come online over the EB-21 transport (KN-D1); the durable bus carries only the
/// pointer events above.
pub const KNOWLEDGE_FIREHOSE_TOKENS: &[&str] =
    &[KNOWLEDGE_BLOCK_OP, KNOWLEDGE_PRESENCE_UPDATED];

/// Register Knowledge's complete `knowledge.*` list (durable + firehose) against the Bus grammar
/// (contract 2.9). Returns `Ok(())` iff EVERY registered token parses the §6.1 grammar via the one
/// Bus validator; otherwise the first offending token + its [`myelin_events::TaxonomyError`] (LOUD).
/// Knowledge REGISTERS its list against the grammar it does not own.
pub fn register_knowledge_tokens() -> Result<(), (&'static str, myelin_events::TaxonomyError)> {
    for &tok in KNOWLEDGE_DURABLE_TOKENS.iter().chain(KNOWLEDGE_FIREHOSE_TOKENS) {
        validate_event_type(tok).map_err(|e| (tok, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **THE GATE (contract 2.9): 0 ungrammatical tokens.** Every registered `knowledge.*` token
    /// (durable + firehose) parses the Bus §6.1/§6.2 grammar via the one Bus validator.
    #[test]
    fn every_knowledge_token_parses_the_bus_grammar() {
        for &tok in KNOWLEDGE_DURABLE_TOKENS.iter().chain(KNOWLEDGE_FIREHOSE_TOKENS) {
            assert!(
                validate_event_type(tok).is_ok(),
                "registered knowledge token `{tok}` is UNGRAMMATICAL: {:?}",
                validate_event_type(tok)
            );
        }
        assert!(register_knowledge_tokens().is_ok());
    }

    /// Every registered token carries the canonical `knowledge` subsystem prefix (§6.2).
    #[test]
    fn every_knowledge_token_carries_the_knowledge_prefix() {
        for &tok in KNOWLEDGE_DURABLE_TOKENS.iter().chain(KNOWLEDGE_FIREHOSE_TOKENS) {
            assert_eq!(tok.split('.').next().unwrap(), "knowledge");
        }
        assert!(myelin_events::SUBSYSTEM_TOKENS.contains(&"knowledge"));
    }

    /// The durable + firehose sets are DISJOINT (a token is either durable-via-outbox OR
    /// firehose-only, never both — the structural split).
    #[test]
    fn durable_and_firehose_sets_are_disjoint() {
        for d in KNOWLEDGE_DURABLE_TOKENS {
            assert!(
                !KNOWLEDGE_FIREHOSE_TOKENS.contains(d),
                "`{d}` cannot be both durable and firehose"
            );
        }
    }

    /// No duplicates across the whole `knowledge.*` registry.
    #[test]
    fn the_knowledge_list_has_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for &tok in KNOWLEDGE_DURABLE_TOKENS.iter().chain(KNOWLEDGE_FIREHOSE_TOKENS) {
            assert!(seen.insert(tok), "knowledge token `{tok}` registered more than once");
        }
    }

    /// Knowledge registers no foreign-subsystem token (the acyclic-producer invariant, EI-02 §3).
    #[test]
    fn knowledge_registers_no_foreign_subsystem_tokens() {
        for &tok in KNOWLEDGE_DURABLE_TOKENS.iter().chain(KNOWLEDGE_FIREHOSE_TOKENS) {
            assert!(tok.starts_with("knowledge."), "foreign-subsystem token `{tok}`");
        }
    }
}
