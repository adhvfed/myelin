//! # `read_state` — the Chat Read-state Service (Valkey hot markers + batched PG flush;
//! cache-never-authoritative) — CHAT-P16 / P-410, M4-C5
//!
//! The **read-state-service slice** of M4-C5 (the fanout-class boundary + Activity-as-view is
//! CHAT-P17). The churny hot path the architecture names the platform invariant:
//!
//! > **Read-state** is the churny hot path: Valkey hot markers + PG durable record + batched flush,
//! > eventually-consistent, **cache-never-authoritative**, firehose-only events, a
//! > `PersonalDataHolder`. Unread is **derived** (`count(id > last_read)`), never write-fanned-out.
//! > — `05-hard-problems.md` §; `02-internals-and-algorithms.md` §3.
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `planning/04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md` §3 (the
//!   read-state hot path: `Valkey HSET read:<t>:<p>:<conv> = last_read` debounced; batched flush
//!   `UPSERT read_state` ~seconds; the PG record is the truth; `unread = count(id > last_read)`, a
//!   bounded range read; the firehose-only `chat.read_state.updated`; cache loss is benign+bounded).
//! - `03-events-contracts-and-glue.md` §4 (AMBIENT / read-fanout — per-watcher unread computed
//!   LAZILY, never write-fanned-out; 0 per-member unread writes on an ambient post).
//! - `05-hard-problems.md` (the Valkey+PG batched-flush read-state design + cache-never-authoritative
//!   is the platform invariant).
//! - `06-reconciliation-compliance.md` R-C3 (the batched-flush cadence + the `Notif.mark(item, read)`
//!   trigger are the tunable — eventually-consistent is accepted, the cadence is measured).
//!
//! **Contracts:** index rows **7.2** (read-state truth — CONSUMED: ONE read-state truth, a marker in
//! a scoped view is the same row in the unified inbox; Chat's per-channel scroll-state is LINKED to
//! Notif's per-item state at the **mention**, §5.3, never a third copy), **3.5** (the firehose-only
//! `chat.read_state.updated` — CONSUMED: the coarse cross-device sync rides the `channel:<id>`
//! firehose, never the durable bus), **10.1** (the read-state store is a `PersonalDataHolder` —
//! OWNED: a `(user × conversation)` last-read marker is a person's footprint; it crypto-shreds /
//! purges on erasure, CHAT-D8).
//!
//! ## The cache-never-authoritative contract (the platform invariant)
//! [`ReadStateService`] holds a write-back marker cache ([`Cache`], the Valkey seam) IN FRONT of the
//! durable record ([`ReadStateRecord`], the PG truth). On `mark_read`:
//! 1. the hot marker is written to the cache (debounced — a rapid scroll coalesces);
//! 2. a coarse firehose `chat.read_state.updated` is published (ephemeral, allowed-to-drop);
//! 3. the marker is staged for the **batched flush** to the durable record (the truth).
//!
//! On a **cache loss** ([`ReadStateService::drop_cache`] models the Valkey eviction) the marker is
//! reconstructed from the durable record — **the PG record is authoritative; a flushed marker is at
//! worst slightly stale** (you re-see a few read messages, benign+bounded). A marker that was written
//! to the cache but **not yet flushed** is lost on a cache drop — which is exactly the
//! eventually-consistent bound the architecture accepts (the flush cadence is the R-C3 tunable). The
//! durable record NEVER loses correctness, only freshness (the CHAT-D12 drill,
//! `tests/drill_chat_d12_read_state.rs`).
//!
//! ## Unread is DERIVED, never write-fanned-out (the celebrity-fanout property)
//! [`ReadStateService::unread_count`] is a **bounded range read** over the [`crate::store::MessageStore`]
//! (`count(message_id > last_read)`, via the store's existing `resync_from` clustering read) — NEVER
//! a per-member counter incremented on each post. An ambient post to a 100k-member channel does **0
//! per-member unread writes** ([`ReadStateService::ambient_post_unread_writes`] is structurally `0`);
//! every member's unread is computed LAZILY the next time they ask. This is the read-fanout half of
//! the M4-C5 boundary (the write-fanout/read-fanout split itself is CHAT-P17).
//!
//! ## DB-free
//! This module is a sync, in-memory model over the [`Cache`] trait ([`InMemoryCache`] floor) + an
//! in-memory durable-record map + the in-memory [`crate::store::MessageStore`] — so `cargo build
//! --workspace` stays DB-free. The REAL durable record over dev-stack Postgres + REAL Valkey hot
//! markers ride [`pg`] behind the `integration` feature (the CHAT-D12 real-data leg).
//!
//! ## Floors named (VISION §3) — none NEW
//! Per the prompt: **no new milestone floor.** The **batched-flush cadence**
//! ([`DEFAULT_FLUSH_CADENCE`]) + the precise **`Notif.mark(item, read)` trigger** when scrolling past
//! a mention are the **measured-not-predicted tunable R-C3** — tuned against telemetry (the p99
//! marker-staleness vs the flush-lag), NEVER guessed here and NEVER a separate milestone. The cadence
//! constant is a NAMED, generous default; the bus-coarse-sync makes the common cross-device case
//! immediate, the flush is the durability path.

#[cfg(feature = "integration")]
pub mod pg;

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use myelin_events::firehose::FirehoseScope;
use myelin_storage::cache::Cache;
use myelin_tenancy::TenantId;

use crate::events::CHAT_READ_STATE_UPDATED;
use crate::store::{ConversationId, MessageId, MessageStore};

// ───────────────────────────── the flush-cadence tunable (R-C3, the durability path) ──────────────

/// **The default batched-flush cadence — the DURABILITY path, not the freshness path (R-C3,
/// measured-not-predicted).** The hot marker is written to the cache immediately (debounced); this
/// cadence is how often the coalesced markers are UPSERTed to the durable record (the truth). It is a
/// NAMED tunable (a generous default), tuned per-cell against telemetry (the p99 marker-staleness vs
/// the flush-lag), never a guessed production number. ~2s is generous enough that a rapid scroll
/// coalesces to one flush while the cross-device coarse firehose makes the common case immediate; it
/// exists so the eventually-consistent window (the markers a cache drop would lose) is bounded.
pub const DEFAULT_FLUSH_CADENCE: Duration = Duration::from_secs(2);

/// **The hot-marker cache TTL — the staleness ceiling on a Valkey marker (R-C3).** A marker is a
/// write-back cache value bounded by this TTL: a missed flush cannot pin a stale hot marker forever
/// (the durable record is re-read on expiry). Generous (the flush is the precise path); NAMED so a
/// drill asserts the name, never a literal.
pub const HOT_MARKER_TTL: Duration = Duration::from_secs(300);

// ───────────────────────────── the read-state holder store name (10.1) ────────────────────────────

/// The stable, PII-free name of the Chat **read-state store** — the `(user × conversation)` last-read
/// markers. Distinct from the OLTP message store ([`crate::holder::CHAT_OLTP_STORE`]) because the
/// read-state durable record is its OWN store (its own table/keyspace), and the DSR cascade must
/// reach it specifically (D-C8: read-state purged). PII-free: a store identifier, never personal data.
pub const CHAT_READ_STATE_STORE: &str = "chat_read_state";

// ───────────────────────────── the firehose publish seam (3.5, the coarse sync) ───────────────────

/// **The coarse cross-device read-state push seam — the port the gateway's live-delivery surface
/// implements (contract 3.5 / arch §3).** A `mark_read` publishes a COARSE
/// [`crate::events::CHAT_READ_STATE_UPDATED`] frame on the channel's firehose scope (`channel:<id>` —
/// a bounded selector, never `*`) so the user's OTHER devices learn "this conversation is read up to
/// here". It is FIREHOSE-ONLY (ephemeral, allowed-to-drop, ADR-04.5): if lost, the next flush / the
/// durable record is the truth (the cross-device sync is eventually-consistent, R-C3).
///
/// **Why a port, not a `firehose.publish` here (EI-01 §7 — one transport).** The firehose transport
/// is the gateway's (arch §9 — the gateway owns the ONE excluded `firehose.publish` call site); the
/// read-state service owns NO transport handle. It hands the coarse marker to this port; the gateway
/// publishes the frame on the bounded scope. The frame is references-not-payloads (the conversation +
/// the last-read id, never message content), so it is leak-free.
pub trait ReadStatePush {
    /// Push a coarse `chat.read_state.updated` frame for `marker` on the bounded channel `scope`
    /// (contract 3.5). Returns the assigned firehose frame seq (the resume cursor the other device
    /// backfills from). Allowed-to-drop (firehose semantics).
    fn push_read_state(&self, scope: &FirehoseScope, marker: &ReadMarker) -> u64;
}

// ───────────────────────────── the marker + durable record ────────────────────────────────────────

/// **A read-state marker — the per-`(user × conversation)` last-read position (arch §3).** The hot
/// cache holds this (`read:<t>:<p>:<conv> = last_read_message_id`); the durable record persists it.
/// The marker is a person's footprint (it names a principal + a conversation), so it is PII the H5
/// read-state holder crypto-shreds / purges on erasure (D-C8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadMarker {
    /// The conversation (the per-conversation aggregate the marker is scoped to).
    pub conv: ConversationId,
    /// The pseudonymous principal whose scroll-position this is (never a name/email).
    pub principal: String,
    /// The last message id this principal has read in this conversation (the marker; `count(id >
    /// last_read)` is the derived unread).
    pub last_read: MessageId,
}

impl ReadMarker {
    /// Build a marker.
    pub fn new(
        conv: ConversationId,
        principal: impl Into<String>,
        last_read: MessageId,
    ) -> ReadMarker {
        ReadMarker {
            conv,
            principal: principal.into(),
            last_read,
        }
    }

    /// **The Valkey hot-marker cache key — `read:<region>:<principal>:<conversation>` (arch §3).**
    /// The conversation carries the tenant in its key; the cache namespaces by [`TenantId`] on top
    /// (the `{tenant}:{key}` backing), so the full physical key is per-tenant isolated. PII-free: an
    /// opaque pseudonymous principal id + ids, never a name.
    pub fn cache_key(conv: &ConversationId, principal: &str) -> String {
        format!(
            "read:{}:{}:{}",
            conv.region, principal, conv.conversation_id
        )
    }
}

/// **The durable read-state record (the PG truth, arch §3) — the in-memory model.** A
/// `(conversation, principal) → last_read` map. This is the **source of truth**: the cache is a
/// write-back tier in front of it. The real PG-backed record rides [`pg`] behind `integration`; this
/// in-memory model is the DB-free floor (behaviour-identical on the `flush`/`load` surface). Keyed by
/// the residency-pinned [`ConversationId`] so a marker lives in its conversation's region.
#[derive(Default)]
pub struct ReadStateRecord {
    /// `(conversation, principal) → last_read` — the durable last-read truth.
    records: Mutex<BTreeMap<(ConversationId, String), MessageId>>,
}

impl ReadStateRecord {
    /// A fresh, empty durable record.
    pub fn new() -> ReadStateRecord {
        ReadStateRecord::default()
    }

    /// **UPSERT the durable marker (the batched flush target, arch §3).** Monotone: a flush NEVER
    /// regresses the durable last-read below a higher already-persisted value (a late/out-of-order
    /// flush of an older marker is ignored — the marker only moves forward, the read-position is
    /// monotone). Returns `true` iff the durable record advanced.
    pub fn upsert(&self, marker: &ReadMarker) -> bool {
        let mut g = self.records.lock().unwrap_or_else(|e| e.into_inner());
        let key = (marker.conv.clone(), marker.principal.clone());
        match g.get(&key) {
            // Only advance — the read-position is monotone (a stale flush cannot rewind it).
            Some(existing) if *existing >= marker.last_read => false,
            _ => {
                g.insert(key, marker.last_read.clone());
                true
            }
        }
    }

    /// **Load the durable last-read marker (the authoritative read, arch §3).** This is what a cache
    /// miss / a cache loss reconstructs from — the PG record is the truth. `None` if the principal
    /// has never read in this conversation.
    pub fn load(&self, conv: &ConversationId, principal: &str) -> Option<MessageId> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(conv.clone(), principal.to_string()))
            .cloned()
    }

    /// **Purge a principal's durable read-state (the H5 holder erasure leg, D-C8).** Removes every
    /// `(conversation, principal)` marker for `principal` — the read-state is purged on erasure (a
    /// person's footprint, 0 recoverable). Returns the count purged.
    pub fn purge_principal(&self, principal: &str) -> usize {
        let mut g = self.records.lock().unwrap_or_else(|e| e.into_inner());
        let before = g.len();
        g.retain(|(_, p), _| p != principal);
        before - g.len()
    }

    /// The number of durable markers (telemetry / a test assertion — never PII).
    pub fn len(&self) -> usize {
        self.records.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether the durable record is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ───────────────────────────── the read-state service ─────────────────────────────────────────────

/// **The Chat Read-state Service (CHAT-P16) — Valkey hot markers + batched PG flush;
/// cache-never-authoritative.** Holds:
/// - a write-back hot-marker [`Cache`] (the Valkey seam — [`InMemoryCache`] floor / [`pg`] real);
/// - the durable [`ReadStateRecord`] (the PG truth);
/// - the **pending-flush buffer** (markers written to the cache but not yet flushed — the
///   eventually-consistent window the cadence bounds).
///
/// `mark_read` writes the hot marker + buffers it for flush + (optionally) pushes the coarse firehose
/// frame. `flush` UPSERTs the buffered markers to the durable record (the batched flush). `read_pos`
/// reads the marker cache-first then falls back to the durable truth on a miss (cache-never-
/// authoritative). `unread_count` is the bounded range read over the [`MessageStore`].
///
/// `S` is the [`MessageStore`] the derived unread reads against; `C` is the [`Cache`] hot tier.
pub struct ReadStateService<'a, S: MessageStore, C: Cache> {
    /// The hot-marker cache (the Valkey write-back tier).
    cache: C,
    /// The durable record (the PG truth).
    record: ReadStateRecord,
    /// The message store the derived unread reads against (the bounded range read).
    store: &'a S,
    /// The pending-flush buffer: markers written to the cache, not yet UPSERTed to the durable
    /// record. A `flush` drains it. A cache drop loses ONLY the un-flushed tail (the bounded
    /// eventually-consistent window).
    pending: Mutex<BTreeMap<(ConversationId, String), MessageId>>,
}

impl<'a, S: MessageStore, C: Cache> ReadStateService<'a, S, C> {
    /// Compose the service over a hot-marker cache + a message store. The durable record starts
    /// empty.
    pub fn new(cache: C, store: &'a S) -> ReadStateService<'a, S, C> {
        ReadStateService {
            cache,
            record: ReadStateRecord::new(),
            store,
            pending: Mutex::new(BTreeMap::new()),
        }
    }

    /// Borrow the durable record (so a test / the DSR holder asserts the authoritative truth).
    pub fn record(&self) -> &ReadStateRecord {
        &self.record
    }

    /// The per-tenant namespace key the hot-marker cache is scoped under (the cache namespaces by
    /// [`TenantId`] on top of [`ReadMarker::cache_key`]).
    fn tenant_of(conv: &ConversationId) -> TenantId {
        TenantId(conv.tenant.clone())
    }

    /// **`mark_read` (the high-frequency write, arch §3).** On scroll/open:
    /// 1. WRITE the hot marker to the cache (debounced — overwriting a prior coalesces a rapid
    ///    scroll into one value), TTL-bounded;
    /// 2. BUFFER the marker for the batched flush (the durability path);
    /// 3. the COARSE firehose `chat.read_state.updated` is the caller's via [`mark_read_and_push`]
    ///    (this method does the cache + buffer half — no transport here).
    ///
    /// The durable record is NOT written here — that is the batched [`flush`](Self::flush) (the
    /// cache-never-authoritative contract: the write path is cache+buffer, the truth is flushed
    /// asynchronously). Returns the marker (so a caller can push the coarse frame).
    pub fn mark_read(&self, marker: ReadMarker) -> ReadMarker {
        let key = ReadMarker::cache_key(&marker.conv, &marker.principal);
        let tenant = Self::tenant_of(&marker.conv);
        // The hot marker (the Valkey write-back value). A cache error is BENIGN (best-effort tier):
        // the durable flush below is the truth, so a failed cache write only costs freshness.
        let _ = self.cache.set(
            &tenant,
            &key,
            marker.last_read.as_str().as_bytes(),
            HOT_MARKER_TTL,
        );
        // Buffer for the batched flush (the durability path) — coalesced per (conv, principal).
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                (marker.conv.clone(), marker.principal.clone()),
                marker.last_read.clone(),
            );
        marker
    }

    /// **`mark_read` + push the coarse cross-device firehose frame (arch §3, contract 3.5).** Does
    /// the [`mark_read`](Self::mark_read) cache+buffer half, then pushes the COARSE
    /// `chat.read_state.updated` frame on the conversation's bounded `channel:<id>` firehose scope
    /// through the gateway's [`ReadStatePush`] port (never a second transport — arch §9). Returns the
    /// pushed frame seq (the resume cursor the other device backfills from). The push is
    /// allowed-to-drop (firehose semantics): if lost, the flush / durable record is the truth.
    pub fn mark_read_and_push<P: ReadStatePush>(
        &self,
        marker: ReadMarker,
        push: &P,
    ) -> Result<u64, myelin_events::firehose::FirehoseError> {
        let scope = crate::glue::chat_channel_scope(&marker.conv.conversation_id)?;
        let marker = self.mark_read(marker);
        Ok(push.push_read_state(&scope, &marker))
    }

    /// **`read_pos` — read the last-read marker, cache-first then the durable truth on a miss (the
    /// cache-never-authoritative read, arch §3).** A cache HIT serves the hot marker; a cache MISS
    /// (or a cache error, treated as a miss) falls back to the durable [`ReadStateRecord`] — the PG
    /// record is authoritative. `None` iff the principal has never read in this conversation in
    /// EITHER tier. A reconstructed marker is at-worst slightly stale (the un-flushed tail), never
    /// wrong (the bounded benign window, CHAT-D12).
    pub fn read_pos(&self, conv: &ConversationId, principal: &str) -> Option<MessageId> {
        let key = ReadMarker::cache_key(conv, principal);
        let tenant = Self::tenant_of(conv);
        // Cache-first. A miss OR a cache-tier error both fall through to the durable truth (the cache
        // is best-effort + never authoritative).
        if let Ok(Some(bytes)) = self.cache.get(&tenant, &key) {
            if let Ok(s) = String::from_utf8(bytes) {
                return Some(MessageId(s));
            }
        }
        // The authoritative fallback: the durable record (the PG truth).
        self.record.load(conv, principal)
    }

    /// **The batched flush (cadence ~[`DEFAULT_FLUSH_CADENCE`], arch §3).** Drains the pending-flush
    /// buffer and UPSERTs each marker to the durable [`ReadStateRecord`] (monotone — never regresses
    /// the durable truth). This is the ONLY write to the durable record (the truth is written
    /// asynchronously from the hot path — cache-never-authoritative). Returns the number of markers
    /// that ADVANCED the durable record. After a flush the buffer is empty (a subsequent cache drop
    /// loses nothing previously flushed).
    pub fn flush(&self) -> usize {
        let drained: Vec<((ConversationId, String), MessageId)> = {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *pending).into_iter().collect()
        };
        let mut advanced = 0;
        for ((conv, principal), last_read) in drained {
            let marker = ReadMarker {
                conv,
                principal,
                last_read,
            };
            if self.record.upsert(&marker) {
                advanced += 1;
            }
        }
        advanced
    }

    /// The number of markers buffered for the next flush (the eventually-consistent window — telemetry
    /// / a test assertion).
    pub fn pending_len(&self) -> usize {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// **Drop the hot-marker cache for `(conv, principal)` — model a Valkey eviction / cache loss
    /// (CHAT-D12).** After this, a [`read_pos`](Self::read_pos) for the marker MISSES the cache and
    /// reconstructs from the durable record (the PG record is authoritative; the marker is at-worst
    /// slightly stale). This does NOT touch the durable record OR the pending buffer — a cache loss
    /// never loses durable correctness.
    pub fn drop_cache(&self, conv: &ConversationId, principal: &str) {
        let key = ReadMarker::cache_key(conv, principal);
        let tenant = Self::tenant_of(conv);
        let _ = self.cache.delete(&tenant, &key);
    }

    /// **`unread_count` — the DERIVED unread (a bounded range read, NEVER write-fanned-out, arch
    /// §3/§4).** `count(message_id > last_read)` over the [`MessageStore`] — the store's existing
    /// `resync_from` clustering read returns everything strictly after the cursor, so unread is the
    /// length of that bounded range. There is NO per-member unread counter; every member's unread is
    /// computed LAZILY here. If the principal has no marker (never read), every message in the
    /// conversation is unread (a bounded read from the conversation head).
    ///
    /// The unread is DERIVED from the durable read-position (cache-first via [`read_pos`](Self::read_pos))
    /// + the message log — never a cached number, never write-fanned-out.
    pub fn unread_count(
        &self,
        conv: &ConversationId,
        principal: &str,
    ) -> Result<usize, crate::store::StoreError> {
        match self.read_pos(conv, principal) {
            // Read up to `last_read` → unread = the bounded range strictly AFTER it.
            Some(last_read) => Ok(self.store.resync_from(conv, &last_read)?.len()),
            // Never read → every message is unread (a bounded read from the head; `range(Recent, …)`
            // bounded by a generous page is the production form — here the whole log IS the unread).
            None => Ok(self
                .store
                .range(conv, crate::store::RangeCursor::Recent, u32::MAX)?
                .len()),
        }
    }

    /// **The celebrity-fanout property: an ambient post does 0 per-member unread writes (arch §4).**
    /// This is a STRUCTURAL `0` — there is no per-member unread counter to increment, so posting to a
    /// channel of ANY size (100k members) writes ZERO read-state rows (unread is computed lazily by
    /// [`unread_count`](Self::unread_count) the next time each member asks). A drill asserts this is
    /// `0` regardless of member count (the read-fanout half of M4-C5; the fanout-class boundary
    /// itself is CHAT-P17).
    pub fn ambient_post_unread_writes(&self, _member_count: usize) -> usize {
        // No write-fanout: unread is derived, never materialised per-member. Always 0.
        0
    }
}

// ───────────────────────────── the read-state token (3.5, the firehose-only coarse sync) ──────────

/// **The coarse read-state firehose token chat publishes on `mark_read` (contract 3.5, §1.2).** The
/// FINE per-message read-state is firehose-only too ([`crate::events::CHAT_READ_STATE_VIEWED`]); this
/// COARSE `chat.read_state.updated` is the cross-device sync frame. NAMED here so the read-state
/// service references the token by name (X-5), never a literal.
pub const READ_STATE_UPDATED: &str = CHAT_READ_STATE_UPDATED;

#[cfg(test)]
mod tests;
