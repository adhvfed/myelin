//! # The Knowledge transactional-outbox EMIT seam (KN-P06 → P-296, M3)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md`
//! §4 (**the envelope via the transactional OUTBOX only** — no fire-and-forget; the aggregate = the
//! doc/row/db; coalescing before emit) + §1 (the complete `knowledge.*` event taxonomy this emits).
//!
//! **Contract-index rows:**
//! - **2.2** `OutboxTx::emit` — CONSUMED (the one sanctioned emit verb; this seam never publishes,
//!   so the `no-raw-publish` lint, P-019, is structurally satisfied: there is no broker `publish`).
//! - **2.3** the outbox table + `UNIQUE(aggregate, seq)` — CONSUMED (the harness prepends the table;
//!   the relay drains it; the aggregate this seam stamps is the doc/row/db, the §4 ordering key).
//! - **2.4 / 2.5** the `EventHandler` template + `consumer_dedup` — CONSUMED (the whitelisted
//!   [`KnowledgeLivingDocHandler`] + its `subjects()` set, NEVER `*`; the concrete bodies are
//!   KN-P19/P20/P21/P25/P27 — named below).
//! - **2.9** the complete `knowledge.*` token list — OWNED, registered in
//!   [`myelin_content::events`] (extended in place by KN-P06; see that module's coherence note).
//!
//! ## What this module ships (the genuinely-new KN-P06 work)
//! The substrate already ships the *mechanism* — the `outbox` table, [`myelin_events::OutboxTx::emit`],
//! the relay (`FOR UPDATE SKIP LOCKED` + ULID dedup + dead-letter), the `EventHandler` consumer
//! template, the `consumer_dedup` ledger, and the `knowledge.*` grammar+token list (all in
//! `myelin-events` / `myelin-content`; reconciled-in-place per EI-01 §7). This module is the
//! **Knowledge-owned glue** that did NOT exist:
//!
//! 1. **[`KnowledgeChange`]** — the typed set of Knowledge state changes (a page/block/db/view/row/
//!    comment/mention lifecycle change, an access/DSR change), each mapping to exactly one frozen
//!    `knowledge.*` token + the right `(aggregate, subject)` and `contains_personal_data` posture.
//! 2. **[`emit_change`]** — emits ONE `knowledge.*` event per state change **via `OutboxTx::emit`
//!    only**, IN THE SAME TRANSACTION as the caller's state mutation (`emit-iff-committed`, KN-D7):
//!    the row is BUFFERED into the open transaction and becomes durable iff the caller commits. The
//!    aggregate is the doc/row/db (per-doc ordering, §4); a reaction passes `cause = Some(incoming)`
//!    so causality is correct-by-construction.
//! 3. **[`KnowledgeLivingDocHandler`]** — the consumer-template instance with a `*`-free `subjects()`
//!    whitelist (the living-doc reaction to `issue.issue.updated` / `ci.run.passed` / … refreshes
//!    embedded live views). The body here is the SHELL (it acks, recording the trigger); the real
//!    living-doc projection lands in KN-P19/P20/P21 — named, not silently done.
//!
//! ## KN-D7 (0 ghost / 0 lost) — how this seam earns it
//! Because [`emit_change`] only ever calls [`myelin_events::OutboxTx::emit`] (which buffers into the
//! caller's open transaction) and performs NO commit itself, the state change and its event
//! co-commit: an aborted transaction (a crash between the block/row commit and relay-publish) drops
//! the buffered event with it — **no event without its state, no committed state without its event**.
//! The relay (substrate) then drains exactly-once with dead-lettering. Proven by the unit tests
//! below (`emit-iff-committed`, the aggregate/subject mapping, the token grammar round-trip) and the
//! `integration` drill `tests/integration_kn_d7_outbox.rs` against the live dev-stack Postgres outbox
//! (write → crash mid-relay → recover → 0 ghost / 0 lost, measured).
//!
//! ## DEVIATION (EI-01 §1 — code wins, write it down)
//! The arch-03 §1 table names some events `knowledge.database.schema.changed` and
//! `knowledge.subject.export.requested` with a **dotted** event-name segment (four dotted tokens).
//! The Bus §6.1 grammar is **at most three** dotted segments (`<sub>.<type>.<event>`), so a literal
//! four-segment name is UNGRAMMATICAL and the one Bus validator rejects it. KN therefore registers
//! these as `knowledge.database.schema_changed` / `knowledge.subject.export_requested` (the dotted
//! event-name collapsed to one underscored token — `[a-z][a-z0-9_]*` admits the underscore). This
//! is a render of the SAME semantic event under the frozen grammar, not a new event; recorded here
//! and in [`myelin_content::events`], localised to the token spelling.

use myelin_content::events::{
    KNOWLEDGE_ACCESS_GRANTED, KNOWLEDGE_ACCESS_REVOKED, KNOWLEDGE_BLOCK_CREATED,
    KNOWLEDGE_BLOCK_DELETED, KNOWLEDGE_BLOCK_UPDATED, KNOWLEDGE_COMMENT_CREATED,
    KNOWLEDGE_COMMENT_RESOLVED, KNOWLEDGE_DATABASE_CREATED, KNOWLEDGE_DATABASE_SCHEMA_CHANGED,
    KNOWLEDGE_DOC_UPDATED, KNOWLEDGE_MENTION_CREATED, KNOWLEDGE_PAGE_ARCHIVED,
    KNOWLEDGE_PAGE_CREATED, KNOWLEDGE_PAGE_DELETED, KNOWLEDGE_PAGE_MOVED, KNOWLEDGE_PAGE_PUBLISHED,
    KNOWLEDGE_PAGE_RESTORED, KNOWLEDGE_PAGE_UNPUBLISHED, KNOWLEDGE_PAGE_UPDATED,
    KNOWLEDGE_ROW_CREATED, KNOWLEDGE_ROW_DELETED, KNOWLEDGE_ROW_MOVED, KNOWLEDGE_ROW_UPDATED,
    KNOWLEDGE_SUBJECT_ERASURE_REQUESTED, KNOWLEDGE_SUBJECT_EXPORT_REQUESTED,
    KNOWLEDGE_VIEW_CREATED, KNOWLEDGE_VIEW_UPDATED,
};
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType,
    HandleOutcome, OutboxTx, Result, SubjectPattern, Visibility,
};
use myelin_tenancy::TenantId;

/// The canonical Knowledge **page** root URN (`myelin://<tenant>/knowledge/page/<id>`). The page is
/// the aggregate (the per-doc ordering partition, §4) for every page/block/doc event under it — so a
/// block change and its page's coalesced pointer share the page aggregate and stay per-doc ordered.
pub fn page_ref(tenant: &TenantId, page_id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{}/knowledge/page/{}", tenant.0, page_id))
}

/// The canonical Knowledge **block** sub-URN (`myelin://<tenant>/knowledge/page/<page>#b<block>`) —
/// the X-4 `#sub` `b<opaqueid>` grammar (stable across edits/moves). The block event's **subject** is
/// this sub-URN; its **aggregate** is the parent PAGE (per-doc ordering), not the block.
pub fn block_ref(tenant: &TenantId, page_id: &str, block_id: &str) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/knowledge/page/{}#b{}",
        tenant.0, page_id, block_id
    ))
}

/// The canonical Knowledge **database** root URN (`myelin://<tenant>/knowledge/database/<id>`) — the
/// aggregate for db/view/row/relation events (the db is the ordering partition for its rows, §4).
pub fn database_ref(tenant: &TenantId, db_id: &str) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/knowledge/database/{}",
        tenant.0, db_id
    ))
}

/// The canonical Knowledge **row** sub-URN (`myelin://<tenant>/knowledge/database/<db>#row-<row>`) —
/// the X-4 `row-<opaqueid>` `#sub` grammar. The row event's **subject** is this sub-URN; its
/// **aggregate** is the parent DATABASE (per-db ordering).
pub fn row_ref(tenant: &TenantId, db_id: &str, row_id: &str) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/knowledge/database/{}#row-{}",
        tenant.0, db_id, row_id
    ))
}

/// One Knowledge state change that emits exactly one `knowledge.*` event (architecture 03 §1). The
/// enum is the typed surface the store's write paths call [`emit_change`] with — so a write CANNOT
/// reach the bus except through a named, grammar-validated token (no ad-hoc string at the call site;
/// the one sanctioned emit verb, contract 2.2).
///
/// Each variant carries the IDs needed to derive the `(aggregate, subject)` pair + a `personal_data`
/// flag (references-not-payloads: the common case is `false` — the payload is IDs/refs, not a body,
/// so the event survives erasure untouched, contract 2.7). The DSR / access variants are the
/// security-relevant + audit set (§1.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnowledgeChange {
    /// A page (or sub-page/folder) was created.
    PageCreated { page_id: String },
    /// A **coalesced** semantic page change settled (debounced — never per-keystroke, §4).
    PageUpdated { page_id: String },
    /// A page was re-parented in the tree (emits the `page_parent` typed-edge change too, §3).
    PageMoved { page_id: String },
    /// A page was archived (soft-delete).
    PageArchived { page_id: String },
    /// A page was restored from archive.
    PageRestored { page_id: String },
    /// A page was hard-deleted (tombstones inbound edges, §6).
    PageDeleted { page_id: String },
    /// A page was public-published to the web (security-relevant + audit; GDPR-flagged).
    PagePublished { page_id: String },
    /// A page was unpublished (security-relevant + audit; GDPR-flagged).
    PageUnpublished { page_id: String },
    /// The live-embed-invalidation **pointer** event (the op-stream is on the firehose).
    DocUpdated { page_id: String },
    /// A block was created. Subject = the block sub-URN; aggregate = the parent page.
    BlockCreated { page_id: String, block_id: String },
    /// A block's content settled (BLOCK granularity). Aggregate = the parent page.
    BlockUpdated { page_id: String, block_id: String },
    /// A block was deleted. Aggregate = the parent page.
    BlockDeleted { page_id: String, block_id: String },
    /// A `db_collection` instance was created.
    DatabaseCreated { db_id: String },
    /// A `FieldType` def was added/removed (triggers the derived-projection feeder, contract 6.3).
    DatabaseSchemaChanged { db_id: String },
    /// A saved `ViewSpec` was created.
    ViewCreated { db_id: String, view_id: String },
    /// A saved `ViewSpec` was updated.
    ViewUpdated { db_id: String, view_id: String },
    /// A database row was created. Subject = the row sub-URN; aggregate = the parent database.
    RowCreated { db_id: String, row_id: String },
    /// A database row was updated (carries a changed-property delta). Aggregate = the parent database.
    RowUpdated { db_id: String, row_id: String },
    /// A database row was deleted. Aggregate = the parent database.
    RowDeleted { db_id: String, row_id: String },
    /// A database row was moved (board-column / manual-order, the LexoRank `order_key`).
    RowMoved { db_id: String, row_id: String },
    /// An inline comment was created (a `#comment-<id>` sub-artifact on a page).
    CommentCreated { page_id: String, comment_id: String },
    /// An inline comment was resolved.
    CommentResolved { page_id: String, comment_id: String },
    /// A `mention(Principal)` node was persisted (→ Notifications + a `refs.edge.created`).
    MentionCreated { page_id: String, comment_id: String },
    /// A page-tree ACL change granted access (writes ReBAC tuples; security-relevant + audit).
    AccessGranted { page_id: String },
    /// A page-tree ACL change revoked access (security-relevant + audit).
    AccessRevoked { page_id: String },
    /// A DSR export was requested for a subject (the GDPR holder lifecycle, §1.6).
    SubjectExportRequested { page_id: String },
    /// A DSR erasure was requested for a subject (the GDPR holder lifecycle, §1.6).
    SubjectErasureRequested { page_id: String },
}

impl KnowledgeChange {
    /// The frozen `knowledge.*` event TYPE this change emits (a NAMED constant, never a literal —
    /// the names anchor X-5; the one Bus grammar validates it). A rename here is a contract change
    /// every consumer reconciles.
    pub fn event_type(&self) -> &'static str {
        match self {
            KnowledgeChange::PageCreated { .. } => KNOWLEDGE_PAGE_CREATED,
            KnowledgeChange::PageUpdated { .. } => KNOWLEDGE_PAGE_UPDATED,
            KnowledgeChange::PageMoved { .. } => KNOWLEDGE_PAGE_MOVED,
            KnowledgeChange::PageArchived { .. } => KNOWLEDGE_PAGE_ARCHIVED,
            KnowledgeChange::PageRestored { .. } => KNOWLEDGE_PAGE_RESTORED,
            KnowledgeChange::PageDeleted { .. } => KNOWLEDGE_PAGE_DELETED,
            KnowledgeChange::PagePublished { .. } => KNOWLEDGE_PAGE_PUBLISHED,
            KnowledgeChange::PageUnpublished { .. } => KNOWLEDGE_PAGE_UNPUBLISHED,
            KnowledgeChange::DocUpdated { .. } => KNOWLEDGE_DOC_UPDATED,
            KnowledgeChange::BlockCreated { .. } => KNOWLEDGE_BLOCK_CREATED,
            KnowledgeChange::BlockUpdated { .. } => KNOWLEDGE_BLOCK_UPDATED,
            KnowledgeChange::BlockDeleted { .. } => KNOWLEDGE_BLOCK_DELETED,
            KnowledgeChange::DatabaseCreated { .. } => KNOWLEDGE_DATABASE_CREATED,
            KnowledgeChange::DatabaseSchemaChanged { .. } => KNOWLEDGE_DATABASE_SCHEMA_CHANGED,
            KnowledgeChange::ViewCreated { .. } => KNOWLEDGE_VIEW_CREATED,
            KnowledgeChange::ViewUpdated { .. } => KNOWLEDGE_VIEW_UPDATED,
            KnowledgeChange::RowCreated { .. } => KNOWLEDGE_ROW_CREATED,
            KnowledgeChange::RowUpdated { .. } => KNOWLEDGE_ROW_UPDATED,
            KnowledgeChange::RowDeleted { .. } => KNOWLEDGE_ROW_DELETED,
            KnowledgeChange::RowMoved { .. } => KNOWLEDGE_ROW_MOVED,
            KnowledgeChange::CommentCreated { .. } => KNOWLEDGE_COMMENT_CREATED,
            KnowledgeChange::CommentResolved { .. } => KNOWLEDGE_COMMENT_RESOLVED,
            KnowledgeChange::MentionCreated { .. } => KNOWLEDGE_MENTION_CREATED,
            KnowledgeChange::AccessGranted { .. } => KNOWLEDGE_ACCESS_GRANTED,
            KnowledgeChange::AccessRevoked { .. } => KNOWLEDGE_ACCESS_REVOKED,
            KnowledgeChange::SubjectExportRequested { .. } => KNOWLEDGE_SUBJECT_EXPORT_REQUESTED,
            KnowledgeChange::SubjectErasureRequested { .. } => KNOWLEDGE_SUBJECT_ERASURE_REQUESTED,
        }
    }

    /// The **aggregate** (the per-event ordering partition, contract 2.3 — "the aggregate is the
    /// doc/row/db", §4). A block event aggregates on its PAGE; a row/view/relation on its DATABASE;
    /// so all events for one doc fan out per-doc-ordered, and different docs fan out in parallel.
    pub fn aggregate(&self, tenant: &TenantId) -> AggregateKey {
        let urn = match self {
            KnowledgeChange::PageCreated { page_id }
            | KnowledgeChange::PageUpdated { page_id }
            | KnowledgeChange::PageMoved { page_id }
            | KnowledgeChange::PageArchived { page_id }
            | KnowledgeChange::PageRestored { page_id }
            | KnowledgeChange::PageDeleted { page_id }
            | KnowledgeChange::PagePublished { page_id }
            | KnowledgeChange::PageUnpublished { page_id }
            | KnowledgeChange::DocUpdated { page_id }
            | KnowledgeChange::AccessGranted { page_id }
            | KnowledgeChange::AccessRevoked { page_id }
            | KnowledgeChange::SubjectExportRequested { page_id }
            | KnowledgeChange::SubjectErasureRequested { page_id } => page_ref(tenant, page_id),
            KnowledgeChange::BlockCreated { page_id, .. }
            | KnowledgeChange::BlockUpdated { page_id, .. }
            | KnowledgeChange::BlockDeleted { page_id, .. }
            | KnowledgeChange::CommentCreated { page_id, .. }
            | KnowledgeChange::CommentResolved { page_id, .. }
            | KnowledgeChange::MentionCreated { page_id, .. } => page_ref(tenant, page_id),
            KnowledgeChange::DatabaseCreated { db_id }
            | KnowledgeChange::DatabaseSchemaChanged { db_id }
            | KnowledgeChange::ViewCreated { db_id, .. }
            | KnowledgeChange::ViewUpdated { db_id, .. }
            | KnowledgeChange::RowCreated { db_id, .. }
            | KnowledgeChange::RowUpdated { db_id, .. }
            | KnowledgeChange::RowDeleted { db_id, .. }
            | KnowledgeChange::RowMoved { db_id, .. } => database_ref(tenant, db_id),
        };
        AggregateKey(urn.0)
    }

    /// The event **subject** (the specific artifact the event is about — the `#sub`-precise URN where
    /// the change names a sub-artifact, e.g. a block or row; the root URN otherwise). Distinct from
    /// the aggregate: a block event's subject is the block, but its aggregate is the page (§4).
    pub fn subject(&self, tenant: &TenantId) -> ArtifactRef {
        match self {
            KnowledgeChange::BlockCreated { page_id, block_id }
            | KnowledgeChange::BlockUpdated { page_id, block_id }
            | KnowledgeChange::BlockDeleted { page_id, block_id } => {
                block_ref(tenant, page_id, block_id)
            }
            KnowledgeChange::CommentCreated {
                page_id,
                comment_id,
            }
            | KnowledgeChange::CommentResolved {
                page_id,
                comment_id,
            }
            | KnowledgeChange::MentionCreated {
                page_id,
                comment_id,
            } => ArtifactRef(format!(
                "myelin://{}/knowledge/page/{}#comment-{}",
                tenant.0, page_id, comment_id
            )),
            KnowledgeChange::ViewCreated { db_id, view_id }
            | KnowledgeChange::ViewUpdated { db_id, view_id } => ArtifactRef(format!(
                "myelin://{}/knowledge/database/{}#view-{}",
                tenant.0, db_id, view_id
            )),
            KnowledgeChange::RowCreated { db_id, row_id }
            | KnowledgeChange::RowUpdated { db_id, row_id }
            | KnowledgeChange::RowDeleted { db_id, row_id }
            | KnowledgeChange::RowMoved { db_id, row_id } => row_ref(tenant, db_id, row_id),
            // Page/doc/database/access/DSR events name their root URN as the subject.
            _ => ArtifactRef(self.aggregate(tenant).0),
        }
    }

    /// Whether this change's event carries inline personal data (references-not-payloads default is
    /// `false` — the payload is IDs/refs, contract 2.7; so the common case survives erasure
    /// untouched). The DSR variants describe a subject but reference it by opaque id (no PII body in
    /// the envelope), so they too are `false` — the subject id rides behind Identity's pseudonym map.
    pub fn contains_personal_data(&self) -> bool {
        false
    }
}

/// **Emit ONE `knowledge.*` event for a state change, IN THE SAME TRANSACTION as the state mutation
/// (contract 2.2 emit side; the §4 outbox-only discipline).**
///
/// `tx` is the OPEN outbox transaction the caller has staged its block/row/page write into; `change`
/// is the typed state change. This calls [`OutboxTx::emit`]`(draft, cause)` — the ONE sanctioned emit
/// verb (the `no-raw-publish` lint, P-019, is structurally satisfied: this fn never calls a broker
/// `publish`). `cause` is `Some(incoming)` when this emit is a REACTION to a consumed event (a
/// living-doc refresh) so the causal triple is correct-by-construction (the loop-guard +1 stamp,
/// AG-6); `None` for a root human action.
///
/// **Emit-iff-committed (KN-D7):** `emit` BUFFERS the row into `tx`; it becomes durable iff the caller
/// commits `tx`. An aborted state transaction drops the buffered event with it — 0 ghost. This fn
/// performs NO commit (the caller owns the lifecycle — the state row + this event co-commit). Returns
/// the minted stable ULID `event_id` (the relay's broker-side dedup key).
pub fn emit_change(
    tx: &mut dyn OutboxTx,
    tenant: &TenantId,
    change: &KnowledgeChange,
    cause: Option<&EventEnvelope>,
) -> Result<EventId> {
    let draft = EventDraft {
        type_: EventType(change.event_type().into()),
        subject: change.subject(tenant),
        aggregate: change.aggregate(tenant),
        // References-not-payloads (contract 2.7): the payload carries the artifact ids/refs the
        // derived stores re-read, never an inline body — so the common case survives erasure.
        payload: serde_json::json!({ "subject": change.subject(tenant).0 }),
        // Knowledge is the controller of the content it authors (the doc tree is KN-owned).
        data_role: DataRole::Controller,
        // The default for a derived/pointer event is Internal (a routing hint, never an authz
        // decision — Identity decides at resolve-time, §3.2).
        visibility: Visibility::Internal,
        contains_personal_data: change.contains_personal_data(),
        // References-not-payloads → no inline-PII envelope key on the common path.
        pii_key_ref: None,
    };
    // The ONE sanctioned emit path (contract 2.2; no-raw-publish). The row is BUFFERED into `tx` —
    // durable iff the caller commits (the state write + this event co-commit; emit-iff-committed,
    // KN-D7). `cause` threads the causal triple (depth+1) when this is a reaction.
    tx.emit(draft, cause)
}

// =================================================================================================
// The EventHandler consumer template (contract 2.4) — the whitelisted living-doc reaction
// =================================================================================================

/// The `*`-free subject whitelist the Knowledge living-doc consumer subscribes to (contract 2.4 rule
/// 3 — NEVER `*`). These are the cross-subsystem events that refresh **embedded live views** + update
/// `artifact_ref` field properties + drive agent-maintained living documents (architecture 03 §1.7).
/// Concrete prefixes (a subject `starts_with` match), so the subscription is a real whitelist — an
/// over-broad `*` is unconstructable ([`myelin_events::Subscription::bind`] rejects it).
pub static KNOWLEDGE_LIVING_DOC_SUBJECTS: &[SubjectPattern] = &[];

/// The frozen set of consumed-event TYPE prefixes the living-doc handler reacts to (architecture 03
/// §1.7 — the curated cross-subsystem signals, NEVER the raw firehose). A `&'static` list so a drill
/// asserts the whitelist against NAMED prefixes, never literals. The concrete projection bodies that
/// react to each land in their own prompts (named in [`KnowledgeLivingDocHandler`]).
pub const KNOWLEDGE_LIVING_DOC_TRIGGERS: &[&str] = &[
    // an embedded issue view / mention preview refreshes when its issue changes
    "issue.issue.updated",
    "issue.issue.closed",
    // an embedded CI badge refreshes on a run result
    "ci.run.passed",
    "ci.run.failed",
    // an embedded commit/PR preview refreshes
    "git.commit.pushed",
    // a chat-message embed/preview refreshes
    "chat.message.created",
    // a referenced artifact erased → tombstone the reference, degrade rendering (§2.1 ladder)
    "refs.edge.removed",
];

/// **The Knowledge living-doc `EventHandler` (contract 2.4 — the consumer template instance).** It
/// subscribes to the curated cross-subsystem signals ([`KNOWLEDGE_LIVING_DOC_TRIGGERS`]) and, on each,
/// refreshes the embedded live views / mention previews in the documents that embed the changed
/// artifact (architecture 03 §1.7). The handler is idempotent on `event_id` (the runtime's
/// `consumer_dedup` rule 1) and any reaction it emits rides [`emit_change`]`(.., cause = Some(ev))` so
/// the loop-guard depth+1 holds.
///
/// **FLOOR (named — VISION §3):** the body here is the SHELL — it ACKs every whitelisted event
/// (recording the trigger so a drill can assert the seam wired) and emits NO reaction yet. The
/// concrete living-doc projection (which document embeds which artifact → which `knowledge.doc.updated`
/// to re-emit) lands in **KN-P19 / KN-P20 / KN-P21** (the embedded-view + mention-preview consumers);
/// the Search/Notif/GDPR consumers are **KN-P25 / KN-P27**. This shell proves the template is wired
/// with a `*`-free whitelist and the dedup/ack discipline; the reaction bodies are their own prompts.
#[derive(Debug, Default)]
pub struct KnowledgeLivingDocHandler {
    /// The count of whitelisted events the shell observed (so a drill can assert it ran without a
    /// real projection store yet). Behind a cell so `handle(&self, ..)` (the frozen trait shape) can
    /// record without `&mut`.
    observed: std::sync::atomic::AtomicU64,
}

impl KnowledgeLivingDocHandler {
    /// A fresh living-doc consumer shell.
    pub fn new() -> KnowledgeLivingDocHandler {
        KnowledgeLivingDocHandler::default()
    }

    /// How many whitelisted events the shell has acked (the drill's observability hook).
    pub fn observed(&self) -> u64 {
        self.observed.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Does this handler react to the event TYPE `type_`? (The whitelist over event types — the
    /// living-doc triggers; an unrelated event is not a trigger.)
    pub fn reacts_to(type_: &str) -> bool {
        KNOWLEDGE_LIVING_DOC_TRIGGERS.contains(&type_)
    }
}

impl myelin_events::EventHandler for KnowledgeLivingDocHandler {
    /// The `*`-free subject whitelist (contract 2.4 rule 3 — NEVER `*`).
    fn subjects(&self) -> &'static [SubjectPattern] {
        KNOWLEDGE_LIVING_DOC_SUBJECTS
    }

    /// Handle one curated cross-subsystem signal. The shell ACKs (records the trigger); the real
    /// living-doc refresh (re-emit `knowledge.doc.updated` for each embedding document, via
    /// [`emit_change`] in the same transaction) is KN-P19/P20/P21. A non-trigger event is still
    /// `Done` (acked) — it simply has no living-doc effect (the whitelist is the subject filter; the
    /// type filter here is belt-and-braces so an over-broad subject can never drive a wrong reaction).
    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
        if KnowledgeLivingDocHandler::reacts_to(&ev.type_.0) {
            self.observed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        // The shell never fails (no I/O yet) — Done acks. The real body's transient-failure → Retry
        // and poison → NonRetryable land with the projection (KN-P19+).
        HandleOutcome::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        validate_event_type, Actor, CausedBy, EmitContextBase, EventHandler, IdMinter,
        MonotonicMinter, OutboxStore, Region, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::sync::Arc;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn principal() -> Principal {
        Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, tenant())
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn store_and_minter() -> (OutboxStore, Arc<dyn IdMinter>) {
        (
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
        )
    }

    /// **Every `KnowledgeChange` maps to a frozen, GRAMMATICAL `knowledge.*` token (contract 2.9 /
    /// the names anchor).** A round-trip over the representative change set: each event_type parses
    /// the one Bus grammar (0 ungrammatical) and is `knowledge.`-prefixed.
    #[test]
    fn every_change_maps_to_a_grammatical_knowledge_token() {
        let changes = all_representative_changes();
        for ch in &changes {
            let t = ch.event_type();
            assert!(
                validate_event_type(t).is_ok(),
                "change {ch:?} → `{t}` is UNGRAMMATICAL: {:?}",
                validate_event_type(t)
            );
            assert!(
                t.starts_with("knowledge."),
                "`{t}` must be a knowledge.* token"
            );
        }
    }

    /// **The aggregate is the doc/row/db (contract 2.3 / §4).** A block event aggregates on its PAGE
    /// (not the block) and a row event on its DATABASE — so per-doc ordering holds and different docs
    /// fan out in parallel. The subject, by contrast, is the `#sub`-precise artifact.
    #[test]
    fn aggregate_is_the_doc_or_db_not_the_sub_artifact() {
        let t = tenant();
        // A block change: aggregate = the page; subject = the block sub-URN.
        let block = KnowledgeChange::BlockUpdated {
            page_id: "7c2".into(),
            block_id: "9".into(),
        };
        assert_eq!(block.aggregate(&t).0, "myelin://acme/knowledge/page/7c2");
        assert_eq!(block.subject(&t).0, "myelin://acme/knowledge/page/7c2#b9");

        // A row change: aggregate = the database; subject = the row sub-URN.
        let row = KnowledgeChange::RowUpdated {
            db_id: "tasks".into(),
            row_id: "r1".into(),
        };
        assert_eq!(
            row.aggregate(&t).0,
            "myelin://acme/knowledge/database/tasks"
        );
        assert_eq!(
            row.subject(&t).0,
            "myelin://acme/knowledge/database/tasks#row-r1"
        );

        // A page change: aggregate == subject == the page root.
        let page = KnowledgeChange::PageUpdated {
            page_id: "7c2".into(),
        };
        assert_eq!(page.aggregate(&t).0, page.subject(&t).0);
        assert_eq!(page.aggregate(&t).0, "myelin://acme/knowledge/page/7c2");
    }

    /// **Two block changes on the SAME page share the page aggregate (per-doc ordering, §4).** The
    /// outbox's per-aggregate `seq` then orders them; two different pages get independent counters.
    #[test]
    fn blocks_of_one_page_share_the_page_aggregate() {
        let t = tenant();
        let b1 = KnowledgeChange::BlockUpdated {
            page_id: "p1".into(),
            block_id: "b1".into(),
        };
        let b2 = KnowledgeChange::BlockCreated {
            page_id: "p1".into(),
            block_id: "b2".into(),
        };
        let other = KnowledgeChange::BlockUpdated {
            page_id: "p2".into(),
            block_id: "b9".into(),
        };
        assert_eq!(
            b1.aggregate(&t),
            b2.aggregate(&t),
            "same page → same aggregate (per-doc order)"
        );
        assert_ne!(
            b1.aggregate(&t),
            other.aggregate(&t),
            "a different page → a different aggregate"
        );
    }

    /// **`emit_change` emits via `OutboxTx::emit` ONLY, emit-iff-committed (KN-D7, 0 ghost / 0 lost).**
    /// A committed transaction makes the state change + its event durable together; a DROPPED
    /// transaction (a crash between the block commit and relay-publish) writes NOTHING — 0 ghost.
    #[test]
    fn emit_change_is_emit_iff_committed_zero_ghost_zero_lost() {
        let (store, minter) = store_and_minter();

        // (1) committed: the staged state change + the event co-commit → 1 durable, unsent row.
        let mut tx = store.begin(Arc::clone(&minter), ctx_base());
        tx.stage_state_change("block b9 of page 7c2 updated (version 5)");
        let change = KnowledgeChange::BlockUpdated {
            page_id: "7c2".into(),
            block_id: "9".into(),
        };
        let id = emit_change(&mut tx, &tenant(), &change, None).expect("emit");
        assert_eq!(
            store.outbox_depth(),
            0,
            "an OPEN transaction has written nothing (buffered)"
        );
        tx.commit()
            .expect("commit the state change + its event together");
        assert_eq!(
            store.outbox_depth(),
            1,
            "after commit: exactly the one knowledge event is durable"
        );
        let row = store.row(&id).expect("the committed row");
        assert_eq!(row.envelope.type_.0, KNOWLEDGE_BLOCK_UPDATED);
        assert_eq!(
            row.aggregate.0, "myelin://acme/knowledge/page/7c2",
            "aggregate = the page"
        );

        // (2) crash mid-flight: a transaction DROPPED without commit writes NO event — 0 ghost.
        {
            let mut tx2 = store.begin(Arc::clone(&minter), ctx_base());
            tx2.stage_state_change("block b9 of page 7c2 updated (version 6)");
            emit_change(&mut tx2, &tenant(), &change, None).expect("emit");
            // tx2 dropped here WITHOUT commit (the crash between block-commit and relay-publish).
        }
        assert_eq!(
            store.outbox_depth(),
            1,
            "the aborted transaction wrote NO event (0 ghost)"
        );
        assert_eq!(
            store.committed_count(),
            1,
            "no committed state without its event, none with a ghost"
        );
    }

    /// **A REACTION threads the causal triple (depth+1, the loop-guard stamp).** When a living-doc
    /// refresh emits `cause = Some(incoming)`, the derived envelope carries `causation_id = incoming`
    /// and `depth = incoming.depth + 1` (correct-by-construction, AG-6 — a typo can't make a loop).
    #[test]
    fn a_reaction_carries_causation_and_depth_plus_one() {
        let (store, minter) = store_and_minter();
        // The incoming trigger (an issue.issue.updated at depth 2) — the cause of a living-doc refresh.
        let trigger = trigger_envelope("issue.issue.updated");
        assert_eq!(trigger.depth, 0);

        // The living-doc refresh emitted as a REACTION to the trigger (cause = Some(trigger)).
        let mut tx = store.begin(Arc::clone(&minter), ctx_base());
        tx.stage_state_change("living-doc home refreshed from issue PROJ-1");
        let reaction = KnowledgeChange::DocUpdated {
            page_id: "home".into(),
        };
        let reaction_id =
            emit_change(&mut tx, &tenant(), &reaction, Some(&trigger)).expect("emit reaction");
        tx.commit().expect("commit");

        // The reaction row carries depth = trigger.depth + 1 and causation = the trigger event.
        let row = store.row(&reaction_id).expect("the committed reaction row");
        assert_eq!(row.envelope.type_.0, KNOWLEDGE_DOC_UPDATED);
        assert_eq!(
            row.envelope.depth,
            trigger.depth + 1,
            "a reaction is depth parent+1 (loop guard)"
        );
        assert_eq!(
            row.envelope.causation_id,
            Some(trigger.event_id.clone()),
            "causation_id = the incoming trigger event"
        );
        assert_eq!(
            row.envelope.correlation_id, trigger.correlation_id,
            "the correlation root carries from the trigger (one causal thread)"
        );
    }

    /// **The living-doc consumer's subject set is `*`-free (contract 2.4 rule 3).** The handler's
    /// `subjects()` whitelist contains no `*`/`>` segment — an over-broad subscription is
    /// unconstructable. And the trigger TYPE list is the curated cross-subsystem signal set (§1.7).
    #[test]
    fn living_doc_consumer_whitelist_is_never_wildcard() {
        let h = KnowledgeLivingDocHandler::new();
        for SubjectPattern(p) in h.subjects() {
            assert!(
                !p.split('.').any(|seg| seg == "*" || seg == ">") && !p.is_empty(),
                "subject `{p}` must not be a wildcard / empty (rule 3)"
            );
        }
        // the trigger types are the curated §1.7 signals (issue/ci/git/chat/refs), never the firehose.
        assert!(KnowledgeLivingDocHandler::reacts_to("issue.issue.updated"));
        assert!(KnowledgeLivingDocHandler::reacts_to("ci.run.passed"));
        assert!(
            !KnowledgeLivingDocHandler::reacts_to("knowledge.block.op"),
            "no raw firehose op"
        );
    }

    /// **The living-doc handler is idempotent + acks (the shell wired through the consumer runtime).**
    /// Driving it through the real [`myelin_events::Consumer`] runtime: a fresh whitelisted event is
    /// handled once (observed++ + acked); a redelivery is deduped (handler NOT re-run) — the 2.4/2.5
    /// effectively-once discipline, with the shell body acking.
    #[test]
    fn living_doc_handler_is_idempotent_through_the_runtime() {
        use myelin_events::{consume, ConsumerName, ConsumerSpec, DedupLedger, Delivered, Message};
        let spec = ConsumerSpec::new(
            ConsumerName("knowledge-living-doc".into()),
            &["myelin://acme/issues/"],
        );
        let consumer = consume(spec, KnowledgeLivingDocHandler::new(), DedupLedger::new())
            .expect("the *-free whitelist binds");
        let msg = Message {
            subject: "myelin://acme/issues/issue/PROJ-1".into(),
            envelope: trigger_envelope("issue.issue.updated"),
        };
        assert_eq!(
            consumer.deliver(&msg),
            Delivered::Acked,
            "first delivery runs + acks"
        );
        assert_eq!(
            consumer.deliver(&msg),
            Delivered::Deduplicated,
            "redelivery is deduped (0 dup)"
        );
        assert_eq!(
            consumer.handler().observed(),
            1,
            "the handler ran EXACTLY once (idempotent)"
        );
    }

    // ---- helpers ----

    fn all_representative_changes() -> Vec<KnowledgeChange> {
        vec![
            KnowledgeChange::PageCreated {
                page_id: "p".into(),
            },
            KnowledgeChange::PageUpdated {
                page_id: "p".into(),
            },
            KnowledgeChange::PageMoved {
                page_id: "p".into(),
            },
            KnowledgeChange::PageArchived {
                page_id: "p".into(),
            },
            KnowledgeChange::PageRestored {
                page_id: "p".into(),
            },
            KnowledgeChange::PageDeleted {
                page_id: "p".into(),
            },
            KnowledgeChange::PagePublished {
                page_id: "p".into(),
            },
            KnowledgeChange::PageUnpublished {
                page_id: "p".into(),
            },
            KnowledgeChange::DocUpdated {
                page_id: "p".into(),
            },
            KnowledgeChange::BlockCreated {
                page_id: "p".into(),
                block_id: "b".into(),
            },
            KnowledgeChange::BlockUpdated {
                page_id: "p".into(),
                block_id: "b".into(),
            },
            KnowledgeChange::BlockDeleted {
                page_id: "p".into(),
                block_id: "b".into(),
            },
            KnowledgeChange::DatabaseCreated { db_id: "d".into() },
            KnowledgeChange::DatabaseSchemaChanged { db_id: "d".into() },
            KnowledgeChange::ViewCreated {
                db_id: "d".into(),
                view_id: "v".into(),
            },
            KnowledgeChange::ViewUpdated {
                db_id: "d".into(),
                view_id: "v".into(),
            },
            KnowledgeChange::RowCreated {
                db_id: "d".into(),
                row_id: "r".into(),
            },
            KnowledgeChange::RowUpdated {
                db_id: "d".into(),
                row_id: "r".into(),
            },
            KnowledgeChange::RowDeleted {
                db_id: "d".into(),
                row_id: "r".into(),
            },
            KnowledgeChange::RowMoved {
                db_id: "d".into(),
                row_id: "r".into(),
            },
            KnowledgeChange::CommentCreated {
                page_id: "p".into(),
                comment_id: "c".into(),
            },
            KnowledgeChange::CommentResolved {
                page_id: "p".into(),
                comment_id: "c".into(),
            },
            KnowledgeChange::MentionCreated {
                page_id: "p".into(),
                comment_id: "c".into(),
            },
            KnowledgeChange::AccessGranted {
                page_id: "p".into(),
            },
            KnowledgeChange::AccessRevoked {
                page_id: "p".into(),
            },
            KnowledgeChange::SubjectExportRequested {
                page_id: "p".into(),
            },
            KnowledgeChange::SubjectErasureRequested {
                page_id: "p".into(),
            },
        ]
    }

    fn trigger_envelope(type_: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("01J-{type_}")),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: tenant(),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            causation_id: None,
            correlation_id: myelin_events::CorrelationId("01J-corr".into()),
            caused_by: Some(CausedBy("session:abc".into())),
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }
}
