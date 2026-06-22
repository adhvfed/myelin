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
//! ## EXTENDED IN PLACE by KN-P06 / P-296 (the OWNER registration — coherence, EI-01 §7)
//! EB-26 / P-246 registered a REPRESENTATIVE `knowledge.*` slice here (page/doc/block/row pointers
//! + the snapshot/erased cross-cutting names). KN-P06 is the OWNER prompt that COMPLETES the list to
//! the full architecture-03 §1 taxonomy (the page/block/database/view/row/comment/mention lifecycle
//! + access/DSR + the cross-cutting `knowledge.*.erased`/`knowledge.*.snapshot`). Per the coherence
//! rule (never a parallel second list, never a re-defined constant), KN-P06 **extends this one list
//! in place** — every EB-26 constant is UNCHANGED (so `myelin-refs-service`'s `KNOWLEDGE_PAGE_CREATED`
//! / `KNOWLEDGE_PAGE_MOVED` consumers keep resolving) and the new lifecycle/DSR tokens are ADDED to
//! [`KNOWLEDGE_DURABLE_TOKENS`]. The list lives in `myelin-content` (the frozen content/taxonomy
//! crate every KN producer + consumer depends on) — NOT in the `myelin-knowledge` service crate —
//! so a Search/Refs/Notif consumer can name a token without a dependency on the service binary. The
//! Knowledge service's OUTBOX EMIT SEAM (the `OutboxTx::emit`-per-state-change body these tokens ride)
//! is the genuinely-new KN-P06 work in `myelin-knowledge::emit` (`emit-iff-committed`, no-raw-publish).
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

// --- page / doc lifecycle (arch 03 §1.1; aggregate = the page) ------------------

/// A page (or sub-page/folder) was created. `subject` = the page ArtifactRef.
pub const KNOWLEDGE_PAGE_CREATED: &str = "knowledge.page.created";
/// A **coalesced** semantic page change (debounced; never per-keystroke — carries a changed-summary,
/// not raw ops). The durable pointer the index re-reads; the per-keystroke body ops ride the firehose
/// — see [`KNOWLEDGE_FIREHOSE_TOKENS`].
pub const KNOWLEDGE_PAGE_UPDATED: &str = "knowledge.page.updated";
/// A page was moved within the tree (re-parented) — emits a `page_parent` typed-edge change (§3).
pub const KNOWLEDGE_PAGE_MOVED: &str = "knowledge.page.moved";
/// A page was archived (soft-delete).
pub const KNOWLEDGE_PAGE_ARCHIVED: &str = "knowledge.page.archived";
/// A page was restored from archive (the inverse of [`KNOWLEDGE_PAGE_ARCHIVED`]).
pub const KNOWLEDGE_PAGE_RESTORED: &str = "knowledge.page.restored";
/// A page was hard-deleted — tombstones inbound edges (§6).
pub const KNOWLEDGE_PAGE_DELETED: &str = "knowledge.page.deleted";
/// A page was public-published to the web — **security-relevant + audit** (GDPR-flagged).
pub const KNOWLEDGE_PAGE_PUBLISHED: &str = "knowledge.page.published";
/// A page was unpublished from the web — **security-relevant + audit** (GDPR-flagged).
pub const KNOWLEDGE_PAGE_UNPUBLISHED: &str = "knowledge.page.unpublished";
/// The **pointer** event for live-embed invalidation (rides the durable bus as a pointer; the
/// op-stream is on the firehose, scope `doc:<id>`). The coalesced durable pointer after a collab session.
pub const KNOWLEDGE_DOC_UPDATED: &str = "knowledge.doc.updated";
/// The page-tree parent typed-edge event Refs mirrors as a `parent` lifecycle edge (TE-7, §3).
pub const KNOWLEDGE_PAGE_PARENT_SET: &str = "knowledge.page.parent_set";

// --- block-level (higher volume, opt-in; arch 03 §1.2) --------------------------

/// A block was created (internal/opt-in due to volume; agents subscribe to coarser
/// [`KNOWLEDGE_PAGE_UPDATED`]). Drives block-level Search reindex.
pub const KNOWLEDGE_BLOCK_CREATED: &str = "knowledge.block.created";
/// A block's content settled (the durable pointer at BLOCK granularity — the index re-derives the
/// block subtree; the per-keystroke ops are firehose). Internal/opt-in.
pub const KNOWLEDGE_BLOCK_UPDATED: &str = "knowledge.block.updated";
/// A block was deleted (internal/opt-in).
pub const KNOWLEDGE_BLOCK_DELETED: &str = "knowledge.block.deleted";

// --- database / schema / view / row (arch 03 §1.3) ------------------------------

/// A `db_collection` instance was created.
pub const KNOWLEDGE_DATABASE_CREATED: &str = "knowledge.database.created";
/// A `FieldType` def was added/removed → triggers the derived-projection feeder
/// (expand→backfill→contract; the >5% measured-hot threshold, contract 6.3).
pub const KNOWLEDGE_DATABASE_SCHEMA_CHANGED: &str = "knowledge.database.schema_changed";
/// A saved `ViewSpec` was created.
pub const KNOWLEDGE_VIEW_CREATED: &str = "knowledge.view.created";
/// A saved `ViewSpec` was updated.
pub const KNOWLEDGE_VIEW_UPDATED: &str = "knowledge.view.updated";
/// A database row was created.
pub const KNOWLEDGE_ROW_CREATED: &str = "knowledge.row.created";
/// A database row was updated — carries a **changed-property delta** (feeds Search + rollup deltas).
pub const KNOWLEDGE_ROW_UPDATED: &str = "knowledge.row.updated";
/// A database row was deleted.
pub const KNOWLEDGE_ROW_DELETED: &str = "knowledge.row.deleted";
/// A database row was moved (board-column / manual-order change — the LexoRank `order_key`).
pub const KNOWLEDGE_ROW_MOVED: &str = "knowledge.row.moved";
/// A `db_relation` typed-edge was created (TE-7 source of truth = Knowledge; Refs projects, §3).
pub const KNOWLEDGE_RELATION_CREATED: &str = "knowledge.relation.created";
/// A `db_relation` typed-edge was removed (TE-7, §3).
pub const KNOWLEDGE_RELATION_REMOVED: &str = "knowledge.relation.removed";

// --- comments / mentions (→ Notifications; arch 03 §1.5) ------------------------

/// An inline comment was created (anchored to a block/range; a `#comment-<id>` sub-artifact, X-4).
pub const KNOWLEDGE_COMMENT_CREATED: &str = "knowledge.comment.created";
/// An inline comment was resolved.
pub const KNOWLEDGE_COMMENT_RESOLVED: &str = "knowledge.comment.resolved";
/// A `mention(Principal)` node → Notifications (the inbox) + a `refs.edge.created`.
pub const KNOWLEDGE_MENTION_CREATED: &str = "knowledge.mention.created";

// --- permissions / GDPR / audit (arch 03 §1.6) ---------------------------------

/// A page-tree ACL change granted access → writes ReBAC tuples via Id `write_tuples` (returns a
/// zookie stamped on `page.acl_zookie`); **security-relevant + audit**.
pub const KNOWLEDGE_ACCESS_GRANTED: &str = "knowledge.access.granted";
/// A page-tree ACL change revoked access; **security-relevant + audit**.
pub const KNOWLEDGE_ACCESS_REVOKED: &str = "knowledge.access.revoked";
/// A DSR export was requested (the GDPR holder lifecycle).
pub const KNOWLEDGE_SUBJECT_EXPORT_REQUESTED: &str = "knowledge.subject.export_requested";
/// A DSR export completed.
pub const KNOWLEDGE_SUBJECT_EXPORT_COMPLETED: &str = "knowledge.subject.export_completed";
/// A DSR erasure was requested.
pub const KNOWLEDGE_SUBJECT_ERASURE_REQUESTED: &str = "knowledge.subject.erasure_requested";
/// A DSR erasure completed.
pub const KNOWLEDGE_SUBJECT_ERASURE_COMPLETED: &str = "knowledge.subject.erasure_completed";

// --- cross-cutting *.erased tombstones (contract 2.7) ---------------------------

/// The `*.erased` tombstone for a page (crypto-shred the page's per-subject DEK, contract 2.7).
pub const KNOWLEDGE_PAGE_ERASED: &str = "knowledge.page.erased";
/// The `*.erased` tombstone for a row (contract 2.7).
pub const KNOWLEDGE_ROW_ERASED: &str = "knowledge.row.erased";
/// The `*.erased` tombstone for a comment (contract 2.7).
pub const KNOWLEDGE_COMMENT_ERASED: &str = "knowledge.comment.erased";

// --- cross-cutting *.snapshot reindex-from-source events (contract 2.6) ----------

/// The `*.snapshot` reindex event for a page (contract 2.6 — `replay`). Cold == live.
pub const KNOWLEDGE_PAGE_SNAPSHOT: &str = "knowledge.page.snapshot";
/// The `*.snapshot` reindex event at BLOCK granularity (the page-subtree replay re-emits one per
/// block, contract 2.6 — "KN page-subtree at block granularity").
pub const KNOWLEDGE_BLOCK_SNAPSHOT: &str = "knowledge.block.snapshot";
/// The `*.snapshot` reindex event for a row (contract 2.6 — Search re-indexes, Refs re-derives edges).
pub const KNOWLEDGE_ROW_SNAPSHOT: &str = "knowledge.row.snapshot";

/// The complete DURABLE `knowledge.*` list (the outbox-emitted pointer + lifecycle events the
/// derived stores index — the full architecture-03 §1 owner taxonomy). The Bus taxonomy (contract
/// 2.9) admits exactly these under §6.1; each is PROVEN grammatical below.
pub const KNOWLEDGE_DURABLE_TOKENS: &[&str] = &[
    // page / doc lifecycle (§1.1)
    KNOWLEDGE_PAGE_CREATED,
    KNOWLEDGE_PAGE_UPDATED,
    KNOWLEDGE_PAGE_MOVED,
    KNOWLEDGE_PAGE_ARCHIVED,
    KNOWLEDGE_PAGE_RESTORED,
    KNOWLEDGE_PAGE_DELETED,
    KNOWLEDGE_PAGE_PUBLISHED,
    KNOWLEDGE_PAGE_UNPUBLISHED,
    KNOWLEDGE_DOC_UPDATED,
    KNOWLEDGE_PAGE_PARENT_SET,
    // block-level (§1.2)
    KNOWLEDGE_BLOCK_CREATED,
    KNOWLEDGE_BLOCK_UPDATED,
    KNOWLEDGE_BLOCK_DELETED,
    // database / view / row (§1.3)
    KNOWLEDGE_DATABASE_CREATED,
    KNOWLEDGE_DATABASE_SCHEMA_CHANGED,
    KNOWLEDGE_VIEW_CREATED,
    KNOWLEDGE_VIEW_UPDATED,
    KNOWLEDGE_ROW_CREATED,
    KNOWLEDGE_ROW_UPDATED,
    KNOWLEDGE_ROW_DELETED,
    KNOWLEDGE_ROW_MOVED,
    KNOWLEDGE_RELATION_CREATED,
    KNOWLEDGE_RELATION_REMOVED,
    // comments / mentions (§1.5)
    KNOWLEDGE_COMMENT_CREATED,
    KNOWLEDGE_COMMENT_RESOLVED,
    KNOWLEDGE_MENTION_CREATED,
    // permissions / GDPR / audit (§1.6)
    KNOWLEDGE_ACCESS_GRANTED,
    KNOWLEDGE_ACCESS_REVOKED,
    KNOWLEDGE_SUBJECT_EXPORT_REQUESTED,
    KNOWLEDGE_SUBJECT_EXPORT_COMPLETED,
    KNOWLEDGE_SUBJECT_ERASURE_REQUESTED,
    KNOWLEDGE_SUBJECT_ERASURE_COMPLETED,
    // cross-cutting *.erased (contract 2.7)
    KNOWLEDGE_PAGE_ERASED,
    KNOWLEDGE_ROW_ERASED,
    KNOWLEDGE_COMMENT_ERASED,
    // cross-cutting *.snapshot (contract 2.6)
    KNOWLEDGE_PAGE_SNAPSHOT,
    KNOWLEDGE_BLOCK_SNAPSHOT,
    KNOWLEDGE_ROW_SNAPSHOT,
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
