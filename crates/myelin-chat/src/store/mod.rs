//! # `store` — the Chat message store (CHAT-P4 / P-398, M4-C1): the `MessageStore` trait + the
//! partitioned hot tier + the fs-backed cold-segment tier (the swap seam).
//!
//! This is the **storage-tier slice** of M4-C1 (the durable message store), conformed to the
//! frozen data model in
//! [`01-tech-and-data-model.md`](../../../../planning/04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md)
//! §3 (the message log) + §3.1 (the `MessageStore` trait) and the tiering lifecycle in
//! [`02-internals-and-algorithms.md`](../../../../planning/04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md)
//! §2. It ships:
//!
//! - **[`MessageStore`]** — the only interface the rest of Chat sees: `append` / `range` /
//!   `revise` / `tombstone` / `resync_from`. The trait is the **hot-engine swap seam** (arch §3.1):
//!   PostgreSQL-partitioned is the v1 hot tier, ScyllaDB is the named measured promotion (the M5
//!   floor, CHAT-P28 / P-502). The cold tier + the trait are identical under either hot engine, so
//!   the promotion is a swap, not a redesign.
//! - **[`MemHotTier`]** — the DB-free, behaviour-identical hot tier model (partitioned by
//!   `(tenant, region)` + conversation, residency-pinned), the unit-test floor. The PostgreSQL hot
//!   tier ([`pg::PgMessageStore`]) implements the SAME trait against real Postgres behind the
//!   `integration` feature, and the unit + integration suites assert **0 behavioural divergence**
//!   on the trait's surface.
//! - **[`ColdSegments`]** — the fs-backed cold-segment tier: an archived `(conversation, range)`
//!   seals to a content-addressed [`myelin_storage::BlobStore`] segment (the 11.2 fs floor), still
//!   range-readable. `range` / `resync_from` fetch the hot partition or the cold segment behind the
//!   SAME interface (transparent cold reads, arch §2.1).
//!
//! ## The ULID message id — intrinsic per-conversation order
//! [`MessageId`] is a k-sortable ULID ([`ulid`]): per-conversation order is INTRINSIC to the id,
//! never wall-clock-derived at read time. The aggregate is `conversation_id` — the keying the
//! CHAT-P5 outbox `UNIQUE(aggregate, seq)` and the CHAT-D2 total-order property build on.
//!
//! ## Named floors (VISION §3 name-your-floors) — the CHAT-P28 / P-502 promotion split
//! This prompt (CHAT-P28 / P-502) is a TRIGGERED M5 promotion. One leg is built (the object-store
//! swap is a one-line, behaviour-preserving backing change provable now); the other stays a named
//! floor because its measured trigger has NOT fired (EI-04 §4 — the gap is VISIBLE, in code):
//! - **The fs-backed `BlobStore` → object-store swap (contract 11.2) is RESOLVED for the cold
//!   segments (CHAT-P28 / P-502).** [`ColdSegments`] is now generic over `B: BlobStore` (defaulting
//!   to the DB-free [`myelin_storage::FsBlobStore`] floor so the build stays DB-free); production
//!   seals to the object-store [`myelin_storage::s3blob::S3BlobStore`] via
//!   [`ColdSegments::with_blob_store`] — a CONSTRUCTION-TIME backing change, NOT a code change.
//!   [`chat_cold_blob_store_parity`] proves the seal/read is byte-identical fs↔object (the CI proof
//!   runs fs↔fs; the `--features integration` proof runs fs↔S3 against the live dev-stack RustFS).
//! - **The ScyllaDB hot tier remains a NAMED FLOOR — the trigger has NOT fired**
//!   ([`SCYLLA_HOT_TIER_PROMOTED`]` == false`; M5-C-S2 / CHAT-P28 / P-502). It is taken ONLY on
//!   [`SCYLLA_PROMOTION_TRIGGER`] (measured per-cell write/partition volume crossing the hot-tier
//!   budget, R-C6/R-5); the CHAT-P26/P-500 surge family measured the gateway SHED budgets, not the
//!   message-store write/partition volume, so the v1 Postgres-partitioned hot tier is RETAINED
//!   (measure-before-shard, ADR-10). The [`MessageStore`] trait makes the eventual promotion a SWAP
//!   (the cold tier + trait identical either way; residency-pinned + crypto-shred-capable per cell),
//!   and CHAT-D2 + CHAT-D8 re-run across it; landing at [`SCYLLA_PROMOTION_LANDING`].
//! - **The outbox co-commit + `chat.message.created` emit LANDED in CHAT-P5** (P-399). The
//!   [`MessageStore`] threads the [`OutboxTx`] seam so the append's state change AND its
//!   `chat.message.created` event commit in ONE transaction (BUS-2, emit-iff-committed): `append`
//!   stages the state change AND emits the real `chat.message.created` envelope (references-only
//!   payload, `aggregate = conversation_id`) through `tx`, so an aborted transaction writes NEITHER
//!   the message NOR its event (CHAT-D13), a retried `client_nonce` co-commits exactly ONE message +
//!   ONE event (CHAT-D14), and a burst from many gateways keeps per-conversation total order
//!   (CHAT-D2). `revise` emits `chat.message.edited`; `tombstone` emits `chat.message.erased`.
//! - **The per-subject-DEK encryption of `body_inline` / `body_nodes` is CHAT-P6** (the columns
//!   exist here; the DEK round-trip is CHAT-P6's, arch §3 + the prompt's DELIVERABLE).

pub mod ulid;

#[cfg(feature = "integration")]
pub mod pg;

use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_events::{
    AggregateKey, DataRole, EventDraft, EventType, OutboxTransaction, OutboxTx as OutboxTxTrait,
    Visibility,
};
use myelin_storage::{BlobStore, ContentHash, FsBlobStore};
use myelin_tenancy::TenantId;

use crate::events::{CHAT_MESSAGE_CREATED, CHAT_MESSAGE_EDITED, CHAT_MESSAGE_ERASED};

pub use ulid::{MessageId, MonotonicUlidSource, SystemUlidSource, UlidSource};

/// The `(tenant, region)` partition + conversation key — the residency-pinned shard key every
/// message row carries (arch §3; contract 12.1/12.4). `region` is in the key, so a write lands in
/// its region's partition (0 cross-region rows; the residency-pin holds structurally — the hot
/// tier indexes by this key and the integration PG tier RLS-policies on `(tenant, region)`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConversationId {
    /// The partition + residency key (ADR-11) — first-class, never optional.
    pub tenant: String,
    /// The residency pin: `== cell.region` (the residency-pin lint, arch §3).
    pub region: String,
    /// The conversation this message log belongs to (the aggregate, ULID-ordered within it).
    pub conversation_id: String,
}

impl ConversationId {
    /// Construct a conversation key from its three components.
    pub fn new(
        tenant: impl Into<String>,
        region: impl Into<String>,
        conversation_id: impl Into<String>,
    ) -> ConversationId {
        ConversationId {
            tenant: tenant.into(),
            region: region.into(),
            conversation_id: conversation_id.into(),
        }
    }
}

/// The kind of actor that authored a message (arch §3 `author_kind`) — provenance + agent
/// treatment. Aligned to the frozen envelope actor kind `{human | agent | service}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorKind {
    /// A human principal.
    Human,
    /// An agent principal (provenance popover; explicit-first dispatch, CHAT-1).
    Agent,
    /// A service principal.
    Service,
}

/// The lifecycle state of a message (arch §3 `msg_state`). `Tombstoned` is the erasure end-state:
/// the RECORD survives (conversation structure/order/causality intact for others) while the body is
/// crypto-shredded ("delete the content, keep the fact"; the 4-step ladder's `erased` outcome,
/// contract 5.7 — the crypto-shred itself is the GDPR holder's job, wired in CHAT-P6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageState {
    /// The message is live.
    Active,
    /// The message was edited (a new version under CAS; `edited_seq` bumped, id stable).
    Edited,
    /// The message was soft-deleted by its author (still visible-as-deleted, body present).
    Deleted,
    /// The record survives, the body is crypto-shredded (the erasure end-state).
    Tombstoned,
}

/// Why a message was tombstoned (arch §3.1 `TombstoneReason`). Carried so the audit fact records
/// the cause; the body crypto-shred is the GDPR holder's job (CHAT-P6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    /// A GDPR erasure of the author subject reached this body (crypto-shred the per-subject DEK).
    SubjectErased,
    /// A per-channel retention purge reached this message.
    RetentionPurge,
    /// A moderation / admin removal.
    Moderation,
}

/// A new message to append (arch §3 `NewMessage`). The body is the PII (`body_inline` +
/// `body_nodes`, the `myelin-content` split, arch §1.4); the per-subject-DEK encryption of these
/// bytes is wired in CHAT-P6 — HERE the columns exist and carry the bytes verbatim (the DEK
/// round-trip is CHAT-P6's, the prompt's DELIVERABLE).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewMessage {
    /// The conversation (aggregate) this message appends to.
    pub conv: ConversationId,
    /// `None` = top-level; else the thread this reply belongs to (arch §3 `thread_root_id`).
    pub thread_root_id: Option<MessageId>,
    /// The pseudonymous author principal id (erasure-safe; arch §3 `author`).
    pub author: String,
    /// The author kind (human | agent | service) — provenance + agent treatment.
    pub author_kind: AuthorKind,
    /// The markdown-subset body string (the `myelin-content` Chat subset, arch §1.4). The DEK
    /// envelope-encryption of these bytes is CHAT-P6; here the column carries them.
    pub body_inline: Vec<u8>,
    /// The structured `mention` / `artifact_ref` / `embed` nodes, kept OUT of the markdown string
    /// so reference-extraction is reliable (the `refs.edge.created` producer, contract 5.4). DEK
    /// encryption is CHAT-P6.
    pub body_nodes: Vec<u8>,
    /// The idempotency nonce — a retried send (flaky mobile/agent) dedups to ONE message (arch §3
    /// `UNIQUE(tenant, conversation_id, client_nonce)`). The idempotent-send GATE lands in CHAT-P5;
    /// here the column + the per-conversation uniqueness invariant exist.
    pub client_nonce: String,
}

/// A stored message (arch §3 `message` row). The body bytes round-trip verbatim through the store
/// (the per-subject-DEK encryption is CHAT-P6; here the store is body-opaque).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// The k-sortable ULID — intrinsic per-conversation order; the stable id behind the `#sub`
    /// anchor `message-<message_id>` (stable across edits).
    pub message_id: MessageId,
    /// The conversation (aggregate) this message belongs to.
    pub conv: ConversationId,
    /// `None` = top-level; else the thread root.
    pub thread_root_id: Option<MessageId>,
    /// The pseudonymous author.
    pub author: String,
    /// The author kind.
    pub author_kind: AuthorKind,
    /// The (CHAT-P6-encrypted) markdown-subset body string.
    pub body_inline: Vec<u8>,
    /// The (CHAT-P6-encrypted) structured nodes.
    pub body_nodes: Vec<u8>,
    /// The idempotency nonce.
    pub client_nonce: String,
    /// The per-message CAS-on-edit counter (arch §3; bumped by [`MessageStore::revise`], id stable).
    pub edited_seq: i32,
    /// The lifecycle state.
    pub state: MessageState,
}

/// A range-read cursor (arch §3.1 `RangeCursor`). A read is either the recent tail (open a channel),
/// a scroll-back page (paginate before a cursor), or the resume-gap read (everything after a
/// cursor) — [`MessageStore::resync_from`] is the dedicated resume-gap form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RangeCursor {
    /// The recent-N tail (most recent `limit` messages, ascending). Open-a-channel.
    Recent,
    /// Scroll-back: the page of `limit` messages strictly BEFORE this id (ascending). Paginate.
    Before(MessageId),
    /// The resume gap: everything strictly AFTER this id (ascending). The resume-cursor backbone.
    After(MessageId),
}

/// A store error — a typed, loud surface (a store failure is a value, never a silent fallthrough).
#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The CAS `expect_seq` did not match the stored `edited_seq` (a concurrent edit; arch §3 X-2).
    CasConflict {
        /// The id whose CAS failed.
        message_id: MessageId,
        /// The `edited_seq` the caller expected.
        expected: i32,
        /// The `edited_seq` actually stored.
        actual: i32,
    },
    /// A message id was referenced that the store does not hold.
    NotFound(MessageId),
    /// A duplicate `client_nonce` within a conversation (the idempotent-send invariant). The full
    /// idempotent-send GATE is CHAT-P5; the store surfaces the conflict here.
    DuplicateNonce {
        /// The conversation the duplicate landed in.
        conversation_id: String,
        /// The nonce that already exists.
        client_nonce: String,
    },
    /// The cold-segment tier (`BlobStore`) failed.
    Cold(String),
}

impl core::fmt::Display for StoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StoreError::CasConflict {
                message_id,
                expected,
                actual,
            } => write!(
                f,
                "CAS conflict on {message_id:?}: expected edited_seq {expected}, found {actual}"
            ),
            StoreError::NotFound(id) => write!(f, "message {id:?} not found"),
            StoreError::DuplicateNonce {
                conversation_id,
                client_nonce,
            } => write!(
                f,
                "duplicate client_nonce {client_nonce} in conversation {conversation_id}"
            ),
            StoreError::Cold(e) => write!(f, "cold-segment tier error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// The store result alias.
pub type Result<T> = core::result::Result<T, StoreError>;

/// The outbox co-commit seam (arch §3.1 / §9; contract 2.2). `append` / `revise` / `tombstone` take
/// the transaction so the state change AND its `chat.*` event are ONE transaction — the no-dual-write
/// guarantee is STRUCTURAL (BUS-2). **CHAT-P5 (P-399):** the event EMIT through this transaction is
/// real — `append` emits `chat.message.created`, `revise` emits `chat.message.edited`, `tombstone`
/// emits `chat.message.erased` (`aggregate = conversation_id`), so an aborted transaction writes
/// NEITHER the message NOR its event (emit-iff-committed, BUS-D4 / CHAT-D13).
pub type OutboxTx = OutboxTransaction;

/// **The `chat.message.*` co-commit emit (CHAT-P5, contract 2.2 / arch §9).** Build the
/// references-only [`EventDraft`] for a message lifecycle event and emit it through the SAME
/// transaction the state change is staged onto — the BUS-2 co-commit. `aggregate = conversation_id`
/// (contract 2.3; the CHAT-D2 per-conversation total-order keying); `subject` is the stable
/// `message-<id>` `#sub` anchor (contract 5.7, minted via [`crate::subs::mint_message`]); the
/// payload is references-only (the message/conversation refs + the author principal id — NEVER the
/// body bytes: references-not-payloads, the body is per-subject-DEK-encrypted at rest and is
/// CHAT-P6's concern, never on the bus). The emit is a causal ROOT (`cause = None`): a user/agent
/// send is the head of its own causal chain (a reaction to an inbound event would pass that cause,
/// the gateway's concern in CHAT-P9).
///
/// Returns the minted `event_id` (the broker-side dedup key) so a caller/test can assert the
/// co-committed event exists for this message.
fn emit_message_event(
    tx: &mut OutboxTx,
    event_type: &str,
    conv: &ConversationId,
    message_id: &MessageId,
    author: &str,
    thread_root_id: Option<&MessageId>,
) -> Result<()> {
    // The stable `message-<id>` #sub anchor as the envelope subject (contract 5.7). It is
    // grammatical by construction; a mint failure is a programming error (the message_id is a
    // minted ULID), surfaced LOUDLY as a Cold-class store error rather than a silent drop.
    let subject = crate::subs::mint_message(&conv.tenant, message_id.as_str())
        .map_err(|e| StoreError::Cold(format!("mint message #sub anchor: {e}")))?;
    let draft = EventDraft {
        type_: EventType(event_type.to_string()),
        subject,
        // The per-conversation aggregate (contract 2.3) — every message event for one conversation
        // is per-aggregate ordered, the CHAT-D2 / D-9 total-order property.
        aggregate: AggregateKey(conv.conversation_id.clone()),
        // References-not-payloads (arch §1.1 / §9): IDs + the author principal, NEVER the body.
        payload: serde_json::json!({
            "conversation_id": conv.conversation_id,
            "message_id": message_id.as_str(),
            "author": author,
            "thread_root_id": thread_root_id.map(|t| t.as_str().to_string()),
        }),
        // Chat is the CONTROLLER of its message content (arch §9 / ADR-04.4).
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        // The body is NOT inline on the event (references-only); the per-subject-DEK PII lives at
        // rest in the store, so no inline-PII envelope key is needed here (CHAT-P6 owns the DEK).
        contains_personal_data: false,
        pii_key_ref: None,
    };
    // The co-commit: emit derives the envelope + buffers the row into THIS transaction (durable iff
    // the transaction commits — emit-iff-committed). A root send carries no `cause`.
    tx.emit(draft, None)
        .map_err(|e| StoreError::Cold(format!("outbox emit {event_type}: {e:?}")))?;
    Ok(())
}

/// **Emit a `chat.message.erased` tombstone for a record the GDPR erase fan-out reached (CHAT-P22 /
/// P-411, contract 2.7 / 10.4).** The cascade ([`crate::erase::ChatErasureCascade`]) calls this to put
/// the cross-cutting `*.erased` tombstone on the OUTBOX for EVERY record the erased subject authored —
/// the bus + DSR cascade Search/Refs/Notif consume. This is decoupled from the hot-tier
/// [`MessageStore::tombstone`] record-mutation so the cascade reaches records that have already been
/// sealed to a COLD segment too (the body is already crypto-shredded by the per-subject-DEK destroy
/// regardless of tier; the tombstone records the FACT + drives the derivative cascade). The author is
/// the erased subject's pseudonym (references-only — never the body, which is shredded).
pub fn emit_erased_tombstone(
    tx: &mut OutboxTx,
    conv: &ConversationId,
    message_id: &MessageId,
    author: &str,
) -> Result<()> {
    tx.stage_state_change(format!("chat.message.erased:{}", message_id.as_str()));
    emit_message_event(tx, CHAT_MESSAGE_ERASED, conv, message_id, author, None)
}

/// **The `MessageStore` trait — the hot-engine swap seam (arch §3.1).** The only interface the rest
/// of Chat sees. PostgreSQL-partitioned is the v1 hot tier ([`pg::PgMessageStore`], `integration`);
/// the in-memory [`MemHotTier`] is the DB-free behaviour-identical floor; ScyllaDB is the named
/// measured promotion (CHAT-P28 / P-502). Cold reads are transparent — `range` / `resync_from`
/// fetch the hot partition or the cold object segment behind THIS interface.
///
/// The trait is sync over an in-memory model here (the PG impl wraps its async store on the
/// harness runtime, the same posture other subsystems take); behaviour across the two tiers is
/// asserted identical by the unit + integration suites (0 divergence on this surface — the GATE).
pub trait MessageStore {
    /// Persist a message (and, in CHAT-P5, its `chat.message.created` outbox row) in ONE
    /// transaction (BUS-2). The store assigns a k-sortable [`MessageId`] (intrinsic
    /// per-conversation order). Idempotent on `client_nonce` within a conversation — a retried send
    /// returns the EXISTING id (the idempotent-send invariant; the full GATE is CHAT-P5). The state
    /// change is staged onto `tx`; the `chat.message.created` EMIT is the CHAT-P5 floor.
    fn append(&self, tx: &mut OutboxTx, msg: NewMessage) -> Result<MessageId>;

    /// Ordered range read (arch §3.1): recent-N (open a channel) | scroll-back (paginate) |
    /// resume-gap ("after cursor X"). Always ascending by `message_id` (per-conversation total
    /// order). Cold reads are transparent (the hot partition or a cold segment, same interface).
    fn range(&self, conv: &ConversationId, cursor: RangeCursor, limit: u32)
        -> Result<Vec<Message>>;

    /// Edit-as-new-version under CAS (arch §3.1, X-2): the `message_id` is STABLE, `edited_seq`
    /// bumps from `expect_seq`. A CAS mismatch is a [`StoreError::CasConflict`] (a concurrent edit
    /// clobber is refused, never silent). The `chat.message.edited` emit is CHAT-P5.
    fn revise(
        &self,
        tx: &mut OutboxTx,
        msg_id: &MessageId,
        body_inline: Vec<u8>,
        body_nodes: Vec<u8>,
        expect_seq: i32,
    ) -> Result<()>;

    /// Tombstone the record (keep the fact; arch §3.1): the row survives, `state` → `Tombstoned`.
    /// The body crypto-shred is the GDPR holder's job (CHAT-P6); this records the erasure FACT. The
    /// `chat.message.tombstoned` emit is CHAT-P5.
    fn tombstone(
        &self,
        tx: &mut OutboxTx,
        msg_id: &MessageId,
        reason: TombstoneReason,
    ) -> Result<()>;

    /// **The resume correctness backbone (arch §1.3 / §3.1; contract 3.5):** everything in `conv`
    /// strictly after `cursor`, gap-free, ordered. This is what the gateway's frozen
    /// `resume(stream, scope, last_seq)` backfills from when the firehose retention window is
    /// exceeded (`resync_required`). A clustering-range read (cold reads transparent).
    fn resync_from(&self, conv: &ConversationId, cursor: &MessageId) -> Result<Vec<Message>>;
}

// ---------------------------------------------------------------------------------------------
// The in-memory hot tier — the DB-free, behaviour-identical floor (the unit-test tier).
// ---------------------------------------------------------------------------------------------

/// The in-memory hot tier: partitioned by `(tenant, region)` + conversation, residency-pinned, the
/// per-conversation log a `BTreeMap<MessageId, _>` so the keys (ULIDs) are kept in intrinsic order.
/// This is the DB-free behaviour-identical floor — the [`MessageStore`] surface here and the PG
/// surface ([`pg::PgMessageStore`]) are asserted to round-trip IDENTICALLY (the 0-divergence GATE).
///
/// The store is residency-pinned STRUCTURALLY: the partition key includes `region`, so a write for
/// `(tenant, region)` lands ONLY in that region's partition — a read for a different region sees 0
/// rows (the partition/residency-pin GATE; the PG tier enforces the same at the DB via RLS).
pub struct MemHotTier {
    /// The minter for k-sortable message ids (the ordering source — monotone).
    minter: Box<dyn UlidSource>,
    /// The partitioned log: `(tenant, region, conversation)` → ordered `message_id` → row.
    partitions: Mutex<BTreeMap<ConversationId, BTreeMap<MessageId, Message>>>,
    /// The fs-backed cold-segment tier (the 11.2 fs floor); sealed archived ranges live here.
    cold: ColdSegments,
}

impl MemHotTier {
    /// A fresh hot tier with the deterministic monotone ULID source (the test/floor source) and a
    /// fresh fs-backed cold tier.
    pub fn new() -> MemHotTier {
        MemHotTier::with_source(Box::new(MonotonicUlidSource::new()))
    }

    /// A fresh hot tier with an explicit ULID source (e.g. the wall-clock [`SystemUlidSource`] in
    /// production, or a `starting_at` source so two conversations get disjoint ordered id ranges).
    pub fn with_source(minter: Box<dyn UlidSource>) -> MemHotTier {
        MemHotTier {
            minter,
            partitions: Mutex::new(BTreeMap::new()),
            cold: ColdSegments::new(),
        }
    }

    /// The cold-segment tier (for the seal/restore lifecycle the detach job drives, arch §2.1).
    pub fn cold(&self) -> &ColdSegments {
        &self.cold
    }

    /// **Seal an old `(conversation, range)` to the cold tier (the detach job, arch §2.1).** Every
    /// message in `conv` strictly BEFORE `up_to` (exclusive) is moved from the hot partition to a
    /// content-addressed cold segment. Subsequent `range` / `resync_from` reads fetch it back
    /// transparently — the trait surface is IDENTICAL whether a message is hot or cold (the cold
    /// transparency property, arch §2.1). Returns the count sealed.
    pub fn seal_before(&self, conv: &ConversationId, up_to: &MessageId) -> Result<usize> {
        let mut parts = self.lock();
        let log = match parts.get_mut(conv) {
            Some(log) => log,
            None => return Ok(0),
        };
        let to_seal: Vec<MessageId> = log
            .range(..up_to.clone())
            .map(|(id, _)| id.clone())
            .collect();
        let mut sealed = Vec::with_capacity(to_seal.len());
        for id in &to_seal {
            if let Some(msg) = log.remove(id) {
                sealed.push(msg);
            }
        }
        if sealed.is_empty() {
            return Ok(0);
        }
        let n = sealed.len();
        self.cold.seal(conv, sealed)?;
        Ok(n)
    }

    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<ConversationId, BTreeMap<MessageId, Message>>> {
        self.partitions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The merged, ordered view of a conversation: cold segments (older) followed by the hot tail.
    /// This is the transparent-cold-read primitive `range` / `resync_from` read against.
    fn merged_log(&self, conv: &ConversationId) -> Result<Vec<Message>> {
        let mut out = self.cold.read(conv)?;
        let parts = self.lock();
        if let Some(log) = parts.get(conv) {
            out.extend(log.values().cloned());
        }
        // cold is strictly older than hot (the seal moves a prefix), and each tier is already
        // ULID-ordered, so the concatenation is globally ordered. Re-sort defensively (cheap) so
        // the total-order invariant holds even if a caller seals out of order.
        out.sort_by(|a, b| a.message_id.cmp(&b.message_id));
        Ok(out)
    }
}

impl Default for MemHotTier {
    fn default() -> Self {
        MemHotTier::new()
    }
}

impl MessageStore for MemHotTier {
    fn append(&self, tx: &mut OutboxTx, msg: NewMessage) -> Result<MessageId> {
        let mut parts = self.lock();
        let log = parts.entry(msg.conv.clone()).or_default();
        // Idempotent-send: a retried send (same nonce in the same conversation) returns the
        // EXISTING id — a no-op, not a second row (arch §3; the full GATE is CHAT-P5).
        if let Some(existing) = log
            .values()
            .find(|m| m.client_nonce == msg.client_nonce)
            .map(|m| m.message_id.clone())
        {
            return Ok(existing);
        }
        let message_id = self.minter.mint();
        // The conversation key (the aggregate) + author for the co-committed event — captured
        // before `msg.conv` / `msg.author` move into the stored row.
        let conv_key = msg.conv.clone();
        let author = msg.author.clone();
        let thread_root_id = msg.thread_root_id.clone();
        let stored = Message {
            message_id: message_id.clone(),
            conv: msg.conv,
            thread_root_id: msg.thread_root_id,
            author: msg.author,
            author_kind: msg.author_kind,
            body_inline: msg.body_inline,
            body_nodes: msg.body_nodes,
            client_nonce: msg.client_nonce,
            edited_seq: 0,
            state: MessageState::Active,
        };
        log.insert(message_id.clone(), stored);
        // The outbox CO-COMMIT (CHAT-P5, BUS-2): the message persist (above, staged for durability)
        // AND the `chat.message.created` event emit share THIS transaction. Stage the state change
        // (the "state" half) and emit the real event (the "event" half) — both durable iff the
        // transaction commits (emit-iff-committed; an abort writes NEITHER — CHAT-D13). Drop the
        // partitions lock FIRST so the emit (which takes the outbox store lock) cannot deadlock
        // against a concurrent committer.
        drop(parts);
        tx.stage_state_change(format!("chat.message.created:{}", message_id.as_str()));
        emit_message_event(
            tx,
            CHAT_MESSAGE_CREATED,
            &conv_key,
            &message_id,
            &author,
            thread_root_id.as_ref(),
        )?;
        Ok(message_id)
    }

    fn range(
        &self,
        conv: &ConversationId,
        cursor: RangeCursor,
        limit: u32,
    ) -> Result<Vec<Message>> {
        let all = self.merged_log(conv)?;
        let limit = limit as usize;
        let out = match cursor {
            RangeCursor::Recent => {
                let start = all.len().saturating_sub(limit);
                all[start..].to_vec()
            }
            RangeCursor::Before(id) => {
                let before: Vec<Message> = all.into_iter().filter(|m| m.message_id < id).collect();
                let start = before.len().saturating_sub(limit);
                before[start..].to_vec()
            }
            RangeCursor::After(id) => all
                .into_iter()
                .filter(|m| m.message_id > id)
                .take(limit)
                .collect(),
        };
        Ok(out)
    }

    fn revise(
        &self,
        tx: &mut OutboxTx,
        msg_id: &MessageId,
        body_inline: Vec<u8>,
        body_nodes: Vec<u8>,
        expect_seq: i32,
    ) -> Result<()> {
        let mut parts = self.lock();
        let mut found: Option<(ConversationId, String, Option<MessageId>)> = None;
        for log in parts.values_mut() {
            if let Some(msg) = log.get_mut(msg_id) {
                // The per-message CAS (X-2): a stale `expect_seq` is a refused clobber, not a
                // silent overwrite.
                if msg.edited_seq != expect_seq {
                    return Err(StoreError::CasConflict {
                        message_id: msg_id.clone(),
                        expected: expect_seq,
                        actual: msg.edited_seq,
                    });
                }
                msg.body_inline = body_inline;
                msg.body_nodes = body_nodes;
                msg.edited_seq += 1;
                msg.state = MessageState::Edited;
                found = Some((
                    msg.conv.clone(),
                    msg.author.clone(),
                    msg.thread_root_id.clone(),
                ));
                break;
            }
        }
        let (conv, author, thread_root_id) = match found {
            Some(f) => f,
            None => return Err(StoreError::NotFound(msg_id.clone())),
        };
        // CO-COMMIT the `chat.message.edited` event (CHAT-P5). Drop the partitions lock first so the
        // outbox-store lock the emit takes cannot deadlock a concurrent committer.
        drop(parts);
        tx.stage_state_change(format!("chat.message.edited:{}", msg_id.as_str()));
        emit_message_event(
            tx,
            CHAT_MESSAGE_EDITED,
            &conv,
            msg_id,
            &author,
            thread_root_id.as_ref(),
        )?;
        Ok(())
    }

    fn tombstone(
        &self,
        tx: &mut OutboxTx,
        msg_id: &MessageId,
        _reason: TombstoneReason,
    ) -> Result<()> {
        let mut parts = self.lock();
        let mut found: Option<(ConversationId, String, Option<MessageId>)> = None;
        for log in parts.values_mut() {
            if let Some(msg) = log.get_mut(msg_id) {
                msg.state = MessageState::Tombstoned;
                // Keep the fact, drop the body. The crypto-shred of the per-subject DEK is the
                // GDPR holder's job (CHAT-P6); here the body bytes are cleared and the record
                // survives (order/causality intact for others).
                msg.body_inline.clear();
                msg.body_nodes.clear();
                found = Some((
                    msg.conv.clone(),
                    msg.author.clone(),
                    msg.thread_root_id.clone(),
                ));
                break;
            }
        }
        let (conv, author, thread_root_id) = match found {
            Some(f) => f,
            None => return Err(StoreError::NotFound(msg_id.clone())),
        };
        // CO-COMMIT the `chat.message.erased` event (CHAT-P5, the `*.erased` cross-cutting token,
        // contract 2.7 — the crypto-shred tombstone FACT that drives the Search/Refs/Notif erasure
        // cascade). Drop the partitions lock first (deadlock-free emit).
        drop(parts);
        tx.stage_state_change(format!("chat.message.erased:{}", msg_id.as_str()));
        emit_message_event(
            tx,
            CHAT_MESSAGE_ERASED,
            &conv,
            msg_id,
            &author,
            thread_root_id.as_ref(),
        )?;
        Ok(())
    }

    fn resync_from(&self, conv: &ConversationId, cursor: &MessageId) -> Result<Vec<Message>> {
        // The resume-gap read: everything strictly after `cursor`, gap-free, ordered. A clustering
        // range read across the merged (cold + hot) log.
        let all = self.merged_log(conv)?;
        Ok(all.into_iter().filter(|m| &m.message_id > cursor).collect())
    }
}

// ---------------------------------------------------------------------------------------------
// The fs-backed cold-segment tier (contract 11.2 — the BlobStore fs floor).
// ---------------------------------------------------------------------------------------------

/// **The cold-segment tier (arch §2.1; contract 11.2).** A sealed archived `(conversation, range)`
/// serialises to a content-addressed [`myelin_storage::BlobStore`] segment (the fs floor); a cold
/// read = segment fetch + decode. Still range-readable, still crypto-shreddable (destroy the
/// per-tenant/per-subject DEK — the holder's job). The fs-backed `BlobStore` → object-store swap is
/// the named M5 follow-on (CHAT-P28 / P-502); the trait makes it a one-line backing swap.
///
/// The store keeps the segment hashes per conversation (the PG `(conversation, range → segment)`
/// index the arch describes) so a cold read fetches the right segments; here that index is an
/// in-process map (the fs floor models the layout, not a shortcut).
///
/// **Generic over `B: BlobStore` — the object-store swap seam (contract 11.2, CHAT-P28 / P-502).**
/// The backing defaults to the fs floor [`FsBlobStore`] (the DB-free unit-test tier, so
/// [`MemHotTier`] and `cargo build --workspace` stay DB-free) but is ANY [`BlobStore`]: in
/// production the cold segments seal to the object-store [`myelin_storage::s3blob::S3BlobStore`]
/// (RustFS in dev, Scaleway Object Storage in prod). Because the content address is BLAKE3 of the
/// PLAINTEXT (backing-independent), the swap is a CONSTRUCTION-TIME backing change, NOT a code
/// change — the seal/read logic here is identical under either backing, and
/// [`chat_cold_blob_store_parity`] proves the put/get is byte-identical fs↔object. The residency
/// pin holds because the BlobStore is per-tenant-keyed (`<tenant>/…`); the crypto-shred-for-erasure
/// is the per-tenant/per-subject DEK destroy (the GDPR holder's job, CHAT-P6), which operates at the
/// key layer and is therefore preserved across the backing swap.
pub struct ColdSegments<B: BlobStore = FsBlobStore> {
    blob: B,
    /// `conversation` → the content addresses of its sealed segments, oldest-first (the
    /// `(conversation, range → segment)` index, arch §2.1).
    index: Mutex<BTreeMap<ConversationId, Vec<ContentHash>>>,
}

impl ColdSegments<FsBlobStore> {
    /// A fresh cold tier over a new fs-backed `BlobStore` (the DB-free floor backing).
    pub fn new() -> ColdSegments<FsBlobStore> {
        ColdSegments::with_blob_store(FsBlobStore::new())
    }
}

impl<B: BlobStore> ColdSegments<B> {
    /// A fresh cold tier over an EXPLICIT [`BlobStore`] backing — the object-store swap entry point
    /// (contract 11.2, CHAT-P28 / P-502). Pass [`myelin_storage::s3blob::S3BlobStore`] to seal cold
    /// segments to the real object store (a one-line backing change; the seal/read code is unchanged).
    pub fn with_blob_store(blob: B) -> ColdSegments<B> {
        ColdSegments {
            blob,
            index: Mutex::new(BTreeMap::new()),
        }
    }

    /// Seal a batch of messages (already ULID-ordered) to a content-addressed cold segment. The
    /// segment is stored in the conversation's tenant keyspace (the per-tenant isolation, §3.2) and
    /// its address recorded in the cold index. The bytes are the message rows serialised — still
    /// range-readable on a cold read.
    fn seal(&self, conv: &ConversationId, messages: Vec<Message>) -> Result<()> {
        let bytes = encode_segment(&messages);
        let tenant = TenantId(conv.tenant.clone());
        let hash = self
            .blob
            .put(&tenant, &bytes)
            .map_err(|e| StoreError::Cold(e.to_string()))?;
        self.index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(conv.clone())
            .or_default()
            .push(hash);
        Ok(())
    }

    /// Read all of a conversation's cold segments back, decoded and ULID-ordered (the transparent
    /// cold read, arch §2.1). Re-hash-on-read integrity is the `BlobStore`'s (a corrupt segment is
    /// refused, never silently served).
    fn read(&self, conv: &ConversationId) -> Result<Vec<Message>> {
        let hashes = {
            let index = self.index.lock().unwrap_or_else(|e| e.into_inner());
            match index.get(conv) {
                Some(h) => h.clone(),
                None => return Ok(Vec::new()),
            }
        };
        let tenant = TenantId(conv.tenant.clone());
        let mut out = Vec::new();
        for hash in &hashes {
            let bytes = self
                .blob
                .get(&tenant, hash)
                .map_err(|e| StoreError::Cold(e.to_string()))?;
            out.extend(decode_segment(&bytes)?);
        }
        out.sort_by(|a, b| a.message_id.cmp(&b.message_id));
        Ok(out)
    }
}

impl Default for ColdSegments<FsBlobStore> {
    fn default() -> Self {
        ColdSegments::new()
    }
}

// ---------------------------------------------------------------------------------------------
// The object-store BlobStore swap parity (contract 11.2; CHAT-P28 / P-502) — the cold-segment
// backing swap is behaviour-preserving.
// ---------------------------------------------------------------------------------------------

/// The verdict of [`chat_cold_blob_store_parity`] — whether the cold-segment object-store swap is
/// byte-identical to the fs floor (contract 11.2). Carried (not just a `bool`) so a test/log row can
/// assert the content address matched AND name the two addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColdBlobParityVerdict {
    /// The content address the fs floor assigned the sealed cold segment.
    pub fs_address: ContentHash,
    /// The content address the object store assigned the SAME segment bytes (must equal `fs_address`
    /// — BLAKE3-of-plaintext is backing-independent).
    pub object_address: ContentHash,
    /// `true` iff the address matched AND both backings round-tripped the EXACT segment bytes (the
    /// swap preserved both the content address AND the bytes — STOR-D7 0-silent-serve).
    pub byte_identical: bool,
}

/// **Prove the cold-segment object-store BlobStore swap is behaviour-preserving (contract 11.2;
/// CHAT-P28 / P-502 — the fs floor for chat cold segments RESOLVED).** Seals the SAME batch of
/// `messages` under the SAME `tenant` keyspace to BOTH the fs floor and the object store and
/// asserts: (1) the content address is IDENTICAL (BLAKE3-of-the-encoded-segment is
/// backing-independent), and (2) the bytes read back from BOTH stores decode to the SAME message
/// rows the input encodes (re-hash-on-read integrity holds in both — STOR-D7 0-silent-serve). This
/// is the behaviour-preserving check the one-line backing swap rests on; [`ColdSegments`] is already
/// generic over `B: BlobStore`, so the swap is a CONSTRUCTION-TIME backing change, NOT a code change
/// to the cold tier (EI-01 §7 — one cold-tier encoder, two backings).
///
/// Generic over two [`BlobStore`]s so the CI parity proof runs fs↔fs (deterministic, DB-free) and
/// the `--features integration` proof runs fs↔[`myelin_storage::s3blob::S3BlobStore`] against the
/// LIVE object store (the real artifact that flips the gate green — the cold tier seals exactly
/// these bytes either way).
pub fn chat_cold_blob_store_parity<F, O>(
    fs: &F,
    object: &O,
    tenant: &TenantId,
    messages: &[Message],
) -> Result<ColdBlobParityVerdict>
where
    F: BlobStore,
    O: BlobStore,
{
    // The cold tier's private wire — encoded ONCE, sealed to both backings (the segment a real seal
    // would write). BLAKE3 of these exact bytes is the content address either backing computes.
    let bytes = encode_segment(messages);
    let fs_address = fs
        .put(tenant, &bytes)
        .map_err(|e| StoreError::Cold(e.to_string()))?;
    let object_address = object
        .put(tenant, &bytes)
        .map_err(|e| StoreError::Cold(e.to_string()))?;
    let fs_back = fs
        .get(tenant, &fs_address)
        .map_err(|e| StoreError::Cold(e.to_string()))?;
    let object_back = object
        .get(tenant, &object_address)
        .map_err(|e| StoreError::Cold(e.to_string()))?;
    // Decode each backing's read-back so the parity is asserted on the DOMAIN rows (a cold read is a
    // decode), not just the wire bytes — the cold tier is range-readable identically either way.
    let fs_rows = decode_segment(&fs_back)?;
    let object_rows = decode_segment(&object_back)?;
    let address_identical = fs_address == object_address;
    let fs_roundtrip_ok = fs_rows == messages;
    let object_roundtrip_ok = object_rows == messages;
    let byte_identical = address_identical && fs_roundtrip_ok && object_roundtrip_ok;
    Ok(ColdBlobParityVerdict {
        fs_address,
        object_address,
        byte_identical,
    })
}

// ---------------------------------------------------------------------------------------------
// The ScyllaDB hot-tier promotion — a NAMED FLOOR (the trigger has NOT fired). VISION §3 /
// EI-04 §4: the gap must be VISIBLE, with its measured trigger signal + landing prompt.
// ---------------------------------------------------------------------------------------------

/// **The ScyllaDB hot-tier promotion is a NAMED FLOOR — the trigger has NOT fired (M5-C-S2 /
/// CHAT-P28 / P-502; the named M4-C1 floor, R-C6/R-5).**
///
/// The promotion is **TRIGGERED, not unconditional** (architecture 05 §2 "ScyllaDB the named
/// measured promotion"; roadmap chat §5; the measure-before-shard mandate ADR-10): it is taken ONLY
/// when [`SCYLLA_PROMOTION_TRIGGER`] fires — measured per-cell write/partition volume crossing the
/// hot-tier budget. No such signal has been measured (the CHAT-P26 / P-500 surge family measured the
/// gateway SHED budgets, never the message-store hot-tier write/partition volume crossing a budget),
/// so the v1 Postgres-partitioned hot tier ([`pg::PgMessageStore`]) is RETAINED. This constant is
/// `false` so the gap is VISIBLE in code, not implied. When the trigger fires, the promotion is a
/// [`MessageStore`]-trait SWAP (the cold tier + trait are identical under either hot engine —
/// residency-pinned + crypto-shred-capable per cell), and CHAT-D2 (per-conversation total order) +
/// CHAT-D8 (0 recoverable PII) re-run across the swap (they were written to survive it).
pub const SCYLLA_HOT_TIER_PROMOTED: bool = false;

/// **The measured trigger that would fire the ScyllaDB hot-tier promotion** (R-C6/R-5; the honest
/// named-floor signal, EI-04 §4). Recorded so the floor is not a silent gap: the promotion lands at
/// CHAT-P28 / P-502 the moment a cell's measured message-store write/partition volume crosses the
/// hot-tier budget. Until then the Postgres-partitioned hot tier is correct (the cell bounds the
/// scale — a cell is one region's tenants, ADR-11, not the planet).
pub const SCYLLA_PROMOTION_TRIGGER: &str =
    "measured per-cell message-store write/partition volume crossing the hot-tier budget (R-C6/R-5)";

/// **Where the ScyllaDB hot-tier promotion lands when [`SCYLLA_PROMOTION_TRIGGER`] fires** — the
/// landing prompt id, so the named floor points at its filler (the gap is traceable, EI-04 §4).
pub const SCYLLA_PROMOTION_LANDING: &str = "CHAT-P28 / P-502";

/// Serialise a cold segment: a length-prefixed line-delimited JSON encoding of the message rows.
/// The content address the `BlobStore` computes is over THESE bytes (plaintext-addressed; the DEK
/// wrap is CHAT-P6 / P-ST-08, transparent to the address).
fn encode_segment(messages: &[Message]) -> Vec<u8> {
    let mut buf = Vec::new();
    for m in messages {
        let line = serde_json::to_string(&SegmentRow::from(m)).expect("segment row serialises");
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
    }
    buf
}

/// Decode a cold segment back to message rows.
fn decode_segment(bytes: &[u8]) -> Result<Vec<Message>> {
    let mut out = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let row: SegmentRow = serde_json::from_slice(line)
            .map_err(|e| StoreError::Cold(format!("decode segment row: {e}")))?;
        out.push(row.into());
    }
    Ok(out)
}

// A serde mirror of `Message` for the cold-segment encoding (the domain types deliberately do NOT
// derive serde — the store is body-opaque; the segment format is the cold tier's private wire).
#[derive(serde::Serialize, serde::Deserialize)]
struct SegmentRow {
    message_id: String,
    tenant: String,
    region: String,
    conversation_id: String,
    thread_root_id: Option<String>,
    author: String,
    author_kind: u8,
    body_inline: Vec<u8>,
    body_nodes: Vec<u8>,
    client_nonce: String,
    edited_seq: i32,
    state: u8,
}

impl SegmentRow {
    fn from(m: &Message) -> SegmentRow {
        SegmentRow {
            message_id: m.message_id.0.clone(),
            tenant: m.conv.tenant.clone(),
            region: m.conv.region.clone(),
            conversation_id: m.conv.conversation_id.clone(),
            thread_root_id: m.thread_root_id.as_ref().map(|t| t.0.clone()),
            author: m.author.clone(),
            author_kind: author_kind_code(m.author_kind),
            body_inline: m.body_inline.clone(),
            body_nodes: m.body_nodes.clone(),
            client_nonce: m.client_nonce.clone(),
            edited_seq: m.edited_seq,
            state: state_code(m.state),
        }
    }
}

impl From<SegmentRow> for Message {
    fn from(r: SegmentRow) -> Message {
        Message {
            message_id: MessageId(r.message_id),
            conv: ConversationId {
                tenant: r.tenant,
                region: r.region,
                conversation_id: r.conversation_id,
            },
            thread_root_id: r.thread_root_id.map(MessageId),
            author: r.author,
            author_kind: author_kind_from_code(r.author_kind),
            body_inline: r.body_inline,
            body_nodes: r.body_nodes,
            client_nonce: r.client_nonce,
            edited_seq: r.edited_seq,
            state: state_from_code(r.state),
        }
    }
}

fn author_kind_code(k: AuthorKind) -> u8 {
    match k {
        AuthorKind::Human => 0,
        AuthorKind::Agent => 1,
        AuthorKind::Service => 2,
    }
}

fn author_kind_from_code(c: u8) -> AuthorKind {
    match c {
        1 => AuthorKind::Agent,
        2 => AuthorKind::Service,
        _ => AuthorKind::Human,
    }
}

fn state_code(s: MessageState) -> u8 {
    match s {
        MessageState::Active => 0,
        MessageState::Edited => 1,
        MessageState::Deleted => 2,
        MessageState::Tombstoned => 3,
    }
}

fn state_from_code(c: u8) -> MessageState {
    match c {
        1 => MessageState::Edited,
        2 => MessageState::Deleted,
        3 => MessageState::Tombstoned,
        _ => MessageState::Active,
    }
}

#[cfg(test)]
mod tests;
