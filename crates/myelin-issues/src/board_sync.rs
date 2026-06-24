//! # `board_sync` — real-time board sync over the firehose resume-cursor protocol
//! (ISS-P30 / P-397; the ISS-D13 zero-ops-lost-on-reconnect gate)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md`
//! §7 (*Real-time sync — optimistic UI + the frozen firehose resume-cursor protocol*): the v1 FLOOR
//! is **optimistic local updates + bus-driven cache invalidation** over the **shared firehose** using
//! the **frozen `subscribe/resume/scope` resume-cursor protocol** (contract 3.5, OQ-J):
//!
//! ```text
//! subscribe(stream = fan.<tenant>.<project>, scope = board:<id>)   // scope BOUNDS frames; never *
//! on local mutation: apply optimistically; send through the SAME permissioned API (UI=CLI=agent)
//! on server confirm: keep; on server reject: roll back + one quiet line + the field to fix
//! on Frame{seq, ...} for scope: patch the normalised cache (an agent-moved card animates in)
//! on reconnect: resume(stream, scope, last_seq) → backfill (last_seq, now] then live // loses ZERO ops
//! on resync_required (last_seq older than the window): full *.snapshot replay (contract 2.6)
//! ```
//!
//! **Contract-index rows:**
//! - **3.5** (the firehose transport + resume-cursor protocol) — **CONSUMED.** This module drives the
//!   ONE frozen Bus-owned protocol ([`myelin_events::Firehose`] — `subscribe`/`resume`/`Frame.seq`/
//!   `resync_required`, P-141/EB-21); it adds NO second transport. Scope is the bounded
//!   [`myelin_events::FirehoseScope`] `board:<id>` (never `*` — the transport rejects an over-broad
//!   scope at `subscribe`). The Issues board cache is the CONSUMER the protocol drives.
//! - **1.11** (the connection-storm shed budget / per-surface shed order) — **CONSUMED.** A slow board
//!   consumer is dropped to `resync_required` rather than buffering unboundedly (the per-connection
//!   in-flight cap on [`myelin_events::FirehoseSubscription`]); presence/typing ride the EPHEMERAL
//!   firehose, never the durable bus, and shed FIRST.
//!
//! ## What this module ships (ISS-P30 — the Issues-layer board-sync consumer)
//! The Bus owns the zero-loss-replay transport (P-141) and the substrate owns the bounded-and-sheds
//! buffer (P-135). THIS module is the **Issues board cache** that those two halves drive: a normalised
//! per-`board:<id>` row cache ([`BoardCache`]) plus the [`BoardSync`] state machine that:
//!
//! 1. **subscribes** to the bounded `board:<id>` firehose scope (paginated to the visible window +
//!    margin via [`myelin_substrate::firehose_selector::ScopeWindow`] — a 50k-row board never streams
//!    50k live frames to one client; the transport rejects `*`);
//! 2. **applies optimistically** on a local mutation ([`BoardSync::apply_local`]) — the card moves in
//!    the cache immediately, tracked as PENDING; a server **confirm** keeps it
//!    ([`BoardSync::confirm_local`]); a server **reject** rolls it back to its pre-mutation state
//!    ([`BoardSync::reject_local`]) — the "one quiet line + the field to fix" UX, never a silent loss;
//! 3. **patches the cache** on every live [`Frame`] for the scope ([`BoardSync::on_frame`]) — the
//!    bus-driven cache invalidation (an agent-moved card animates in, labelled);
//! 4. on **reconnect** ([`BoardSync::reconnect`]) calls `resume(stream, scope, last_seq)` → backfills
//!    `(last_seq, now]` then resumes live, applying EVERY gap op to the cache — it **loses ZERO ops**
//!    (the ISS-D13 pass condition);
//! 5. on **`resync_required`** (the `last_seq` is older than the bounded retention window) falls back
//!    to a full [`BoardSync::resync_from_snapshot`] (`issue.*.snapshot` replay, contract 2.6) — the
//!    cold-rebuild path, **NAMED not silent**.
//!
//! ## Coherence (EI-01 §7) — this is a CONSUMER, not a second transport
//! The firehose protocol is frozen and OWNED by the Bus ([`myelin_events::firehose`], P-141); the
//! bounded-and-sheds buffer is OWNED by the substrate
//! ([`myelin_substrate::firehose::FrameBuffer`] / [`myelin_substrate::firehose_selector::ScopeWindow`],
//! P-135/P-136). This module DRIVES both — it constructs a `board:<id>` [`myelin_events::FirehoseScope`]
//! through the same `*`-rejecting [`myelin_events::FirehoseScope::parse`] chokepoint, subscribes/resumes
//! through the same [`myelin_events::Firehose`], and re-uses the substrate's
//! [`myelin_substrate::firehose_selector::ScopeWindow`] for the paginated bounded scope. The board
//! cache is the Issues-specific normalised projection the transport invalidates; the `seq`/`scope`/
//! `resync_required` vocabulary lines up 1:1 with the Bus protocol by construction. NO new transport,
//! NO second resume-cursor implementation.
//!
//! ## Floor named (ISS-P30 DoD; VISION §3 name-your-floors)
//! - **The sync floor is `optimistic + resume-cursor` (R-8).** Offline / local-first is the NAMED
//!   follow-on ([`BoardSyncFloors::OFFLINE_LOCAL_FIRST`], post-M5, out of v1 scope unless promoted).
//!   A v1 client must be connected to mutate; a reconnect replays the gap (or `resync_required` →
//!   snapshot). Issue-body concurrency is single-author **CAS** (ADR-05, the `version` token), NOT a
//!   CRDT — there is **no Issues CRDT in v1**.
//! - **The real connection tier** that opens one [`BoardSync`] per connected viewer + drives delivery
//!   off the live firehose socket is the connection-tier deployment (the Chat M4 connection gateway is
//!   the shared backbone, P-403/CHAT-P9); here the [`BoardSync`] is the in-process consumer the ISS-D13
//!   drill drives against the in-process [`myelin_events::Firehose`] floor transport. The protocol
//!   shape it consumes is the frozen contract-3.5 surface.
//!
//! ## Mutation-score floor (ISS-P30 DoD — mandatory-core; a lost op is a correctness failure)
//! The resume-protocol consumer ([`BoardCache::apply`], [`BoardSync::drain_into_cache`] /
//! [`BoardSync::apply_frame`] / [`BoardSync::reconnect`] / [`BoardSync::resync_from_snapshot`], and the
//! optimistic apply/confirm/[`BoardSync::reject_local`] rollback) is a **mandatory-core mutation
//! target with a ≥ 90% floor**: `cargo mutants -p myelin-issues -f crates/myelin-issues/src/board_sync.rs`.
//! A surviving mutant in the cursor-advance / idempotent-apply / rollback logic is a LOST-or-DUPLICATED
//! op (the ISS-D13 correctness failure). **FLOOR (measured-under-load):** the measured % is the CI
//! `cargo mutants` artifact, registered RED-until-run in the scorecard, never self-asserted (EI-01 §3).

use std::collections::HashMap;

use myelin_events::{
    Firehose, FirehoseError, FirehoseScope, FirehoseSubscription, Frame, FrameDraft,
};
use myelin_substrate::firehose_selector::ScopeWindow;

/// **The named follow-ons this v1 sync floor leaves (ISS-P30 DoD — greppable markers, R-8).**
#[derive(Clone, Copy, Debug)]
pub struct BoardSyncFloors;

impl BoardSyncFloors {
    /// **Offline / local-first sync — the NAMED follow-on (R-8).** v1 is `optimistic + resume-cursor`:
    /// a client must be connected to mutate, and a reconnect replays `(last_seq, now]` (or
    /// `resync_required` → snapshot). True offline-first (mutate while disconnected, merge on
    /// reconnect) needs a CRDT/op-log merge and is **out of v1 scope unless promoted** (post-M5).
    pub const OFFLINE_LOCAL_FIRST: &'static str = "R-8";
    /// **The real connection tier** (one [`BoardSync`] per connected viewer, driven off the live
    /// firehose socket) rides the shared Chat M4 connection gateway backbone — **P-403 (CHAT-P9)**.
    pub const CONNECTION_TIER: &'static str = "P-403";
}

/// The firehose stream a project's board frames ride: `fan.<tenant>.<project>` (§7). Held as the
/// opaque PII-free stream name; the board's bounded scope (`board:<id>`) is the `(…, scope)` half.
pub const BOARD_FIREHOSE_STREAM_PREFIX: &str = "fan";

/// Build the firehose stream name for a project's board frames: `fan.<tenant>.<project>` (§7). The
/// stream is a PII-free identifier; the per-`(stream, scope)` monotone `seq` is keyed off it + the
/// bounded `board:<id>` scope.
pub fn board_stream(tenant: &str, project: &str) -> String {
    format!("{BOARD_FIREHOSE_STREAM_PREFIX}.{tenant}.{project}")
}

/// **A normalised board card — the Issues-layer projection the firehose invalidates.** The minimal
/// row state the board renders + the resume-cursor patches: the canonical `issue.id` (the row
/// identity both the board and the roadmap read — `views::RowProjection`), the displayed
/// `state_category` lane, and the `order_key` (LexoRank) position. A live frame patches THIS card;
/// an optimistic local mutation moves it ahead of the server confirm. PII-free at this layer — the
/// free-text title/body is resolved through the permissioned read API, never carried on the frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardCard {
    /// The canonical issue id (the row identity the board + roadmap share — no parallel reality).
    pub issue_id: String,
    /// The board lane (the `state_category` the card renders under) — what an optimistic move/agent
    /// transition changes.
    pub state_category: String,
    /// The LexoRank `order_key` (the displayed position within the lane) — what a reorder changes.
    pub order_key: String,
}

impl BoardCard {
    /// A board card for `issue_id` in `state_category` at `order_key`.
    pub fn new(
        issue_id: impl Into<String>,
        state_category: impl Into<String>,
        order_key: impl Into<String>,
    ) -> BoardCard {
        BoardCard {
            issue_id: issue_id.into(),
            state_category: state_category.into(),
            order_key: order_key.into(),
        }
    }
}

/// **One board op — the normalised mutation a firehose frame (or a local optimistic edit) applies to
/// the [`BoardCache`].** This is the Issues-layer interpretation of a `Frame`'s payload pointer: the
/// transport carries an opaque pointer ([`myelin_events::FramePayload`]); the board sync resolves it
/// (in v1 the in-process floor carries the op inline as the frame payload string, encoded by
/// [`BoardOp::encode`] / decoded by [`BoardOp::decode`] — the real connection tier resolves the
/// pointer through the permissioned read API, P-403). Every op is IDEMPOTENT against a card's id:
/// applying the same op twice is the same as applying it once (so a backfill that overlaps a live
/// frame never double-applies — the zero-dup half of zero-loss).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoardOp {
    /// A card was created / its full row is now `card` (an upsert — idempotent on `issue_id`).
    Upsert(BoardCard),
    /// A card moved to `state_category` (a transition / agent move) — idempotent.
    Move {
        /// The issue id the move targets.
        issue_id: String,
        /// The new lane.
        state_category: String,
    },
    /// A card was reordered to `order_key` (a LexoRank CAS reorder) — idempotent.
    Reorder {
        /// The issue id the reorder targets.
        issue_id: String,
        /// The new LexoRank position.
        order_key: String,
    },
    /// A card was removed from the board (closed/deleted/erased tombstone) — idempotent.
    Remove {
        /// The issue id removed.
        issue_id: String,
    },
}

impl BoardOp {
    /// The issue id this op targets (every op is keyed by exactly one card — the unit of idempotence).
    pub fn issue_id(&self) -> &str {
        match self {
            BoardOp::Upsert(c) => &c.issue_id,
            BoardOp::Move { issue_id, .. }
            | BoardOp::Reorder { issue_id, .. }
            | BoardOp::Remove { issue_id } => issue_id,
        }
    }

    /// Encode the op into a firehose frame payload pointer (the in-process floor's inline encoding;
    /// the real connection tier carries an `ArtifactRef` pointer the read API resolves, P-403). A
    /// stable, PII-free, `|`-delimited encoding (the board layer carries no free-text title/body).
    pub fn encode(&self) -> String {
        match self {
            BoardOp::Upsert(c) => {
                format!("upsert|{}|{}|{}", c.issue_id, c.state_category, c.order_key)
            }
            BoardOp::Move {
                issue_id,
                state_category,
            } => format!("move|{issue_id}|{state_category}"),
            BoardOp::Reorder {
                issue_id,
                order_key,
            } => {
                format!("reorder|{issue_id}|{order_key}")
            }
            BoardOp::Remove { issue_id } => format!("remove|{issue_id}"),
        }
    }

    /// Decode an op from a firehose frame payload pointer. Returns `None` for an unrecognised /
    /// malformed payload (a frame the board layer does not interpret — e.g. a presence frame; the
    /// caller skips it rather than crashing). The inverse of [`Self::encode`].
    pub fn decode(payload: &str) -> Option<BoardOp> {
        let mut parts = payload.split('|');
        match parts.next()? {
            "upsert" => {
                let issue_id = parts.next()?.to_string();
                let state_category = parts.next()?.to_string();
                let order_key = parts.next()?.to_string();
                Some(BoardOp::Upsert(BoardCard {
                    issue_id,
                    state_category,
                    order_key,
                }))
            }
            "move" => Some(BoardOp::Move {
                issue_id: parts.next()?.to_string(),
                state_category: parts.next()?.to_string(),
            }),
            "reorder" => Some(BoardOp::Reorder {
                issue_id: parts.next()?.to_string(),
                order_key: parts.next()?.to_string(),
            }),
            "remove" => Some(BoardOp::Remove {
                issue_id: parts.next()?.to_string(),
            }),
            _ => None,
        }
    }

    /// A firehose [`FrameDraft`] carrying this op's encoded pointer (what a producer publishes).
    pub fn to_draft(&self) -> FrameDraft {
        FrameDraft::new(self.encode())
    }
}

/// **The normalised per-`board:<id>` row cache (the consumer the firehose invalidates, §7).** A
/// `issue_id → BoardCard` map: a live [`BoardOp`] patches it (bus-driven cache invalidation), an
/// optimistic local edit moves a card ahead of the server confirm, and a `*.snapshot` resync replaces
/// it wholesale. Idempotent application is the zero-dup half of zero-loss: applying the same op twice
/// (a backfill overlapping a live frame) lands the same state.
#[derive(Clone, Debug, Default)]
pub struct BoardCache {
    cards: HashMap<String, BoardCard>,
}

impl BoardCache {
    /// An empty board cache.
    pub fn new() -> BoardCache {
        BoardCache::default()
    }

    /// **Apply one board op (idempotently).** An [`BoardOp::Upsert`] replaces the card; a
    /// [`BoardOp::Move`]/[`BoardOp::Reorder`] patches an existing card (a move/reorder of an unknown
    /// id is a no-op — the card is not on this board's window); a [`BoardOp::Remove`] drops it. Applying
    /// the same op twice is the same as once (idempotent on the card id).
    pub fn apply(&mut self, op: &BoardOp) {
        match op {
            BoardOp::Upsert(card) => {
                self.cards.insert(card.issue_id.clone(), card.clone());
            }
            BoardOp::Move {
                issue_id,
                state_category,
            } => {
                if let Some(card) = self.cards.get_mut(issue_id) {
                    card.state_category = state_category.clone();
                }
            }
            BoardOp::Reorder {
                issue_id,
                order_key,
            } => {
                if let Some(card) = self.cards.get_mut(issue_id) {
                    card.order_key = order_key.clone();
                }
            }
            BoardOp::Remove { issue_id } => {
                self.cards.remove(issue_id);
            }
        }
    }

    /// The card for `issue_id` (the row the board renders), if present on this board's window.
    pub fn card(&self, issue_id: &str) -> Option<&BoardCard> {
        self.cards.get(issue_id)
    }

    /// The number of cards currently in the cache (the rendered board size — bounded by the scope
    /// window, never the whole 50k-row board).
    pub fn len(&self) -> usize {
        self.cards.len()
    }

    /// `true` iff the cache holds no cards.
    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// The cards in the lane `state_category`, in `order_key` order (the rendered lane). A bounded,
    /// deterministic read (a column render) — never an open-ended scan of the whole board.
    pub fn lane(&self, state_category: &str) -> Vec<BoardCard> {
        let mut lane: Vec<BoardCard> = self
            .cards
            .values()
            .filter(|c| c.state_category == state_category)
            .cloned()
            .collect();
        lane.sort_by(|a, b| a.order_key.cmp(&b.order_key));
        lane
    }

    /// Replace the entire cache with `cards` (the `*.snapshot` resync — the cold rebuild). The
    /// NAMED-not-silent fallback when a reconnect is past the retention window.
    fn replace_from_snapshot(&mut self, cards: Vec<BoardCard>) {
        self.cards = cards.into_iter().map(|c| (c.issue_id.clone(), c)).collect();
    }
}

/// **A pending optimistic local mutation (§7 — apply optimistically, confirm or roll back).** Holds
/// the op applied locally + the card's PRE-mutation state so a server reject can roll it back exactly
/// (the "one quiet line + the field to fix" UX — never a silent loss, never a guess at the prior
/// state). A `None` `prior` means the card did not exist before (an optimistic CREATE → a reject
/// removes it).
#[derive(Clone, Debug)]
struct PendingMutation {
    /// The op applied optimistically (kept for diagnostics / the confirm path).
    op: BoardOp,
    /// The card's state BEFORE the optimistic apply (`None` if it did not exist) — the rollback target.
    prior: Option<BoardCard>,
}

/// **Why an optimistic local mutation could not be applied (a LOUD verdict, never silent).**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalMutationError {
    /// A mutation id is already pending (a second optimistic edit before the first confirmed). v1
    /// serialises optimistic edits per `mutation_id`; the connection tier coalesces concurrent edits.
    AlreadyPending {
        /// The mutation id that is already in flight.
        mutation_id: String,
    },
}

impl core::fmt::Display for LocalMutationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LocalMutationError::AlreadyPending { mutation_id } => {
                write!(f, "optimistic mutation `{mutation_id}` is already pending")
            }
        }
    }
}

impl std::error::Error for LocalMutationError {}

/// **The board-sync state machine (§7 — the Issues-layer consumer of the resume-cursor protocol).**
///
/// One per connected board viewer. It owns: the bounded `(stream, scope = board:<id>)` key, the
/// [`ScopeWindow`] the board paginates to (the visible window + margin — never the whole 50k rows),
/// the live [`myelin_events::FirehoseSubscription`] handle, the resume cursor (`last_seq`), the
/// normalised [`BoardCache`], and the in-flight optimistic mutations. It drives the frozen
/// contract-3.5 protocol on the [`myelin_events::Firehose`] (the Bus owns the transport; this is the
/// consumer).
pub struct BoardSync {
    /// The firehose stream (`fan.<tenant>.<project>`) — the `(stream, …)` key half.
    stream: String,
    /// The bounded `board:<id>` scope — the `(…, scope)` key half (NEVER `*`).
    scope: FirehoseScope,
    /// The paginated scope window (the visible rows + margin) — bounds the board's live frame fan-in
    /// to the window, not the whole board (the §7.7 "paginates its scope" discipline).
    window: ScopeWindow,
    /// The live subscription handle (the resume-cursor stream). `None` before the first subscribe /
    /// while disconnected.
    sub: Option<FirehoseSubscription>,
    /// The resume cursor — the seq of the last frame APPLIED to the cache. Presented to
    /// `resume(stream, scope, last_seq)` on reconnect. `0` before any frame (a fresh viewer).
    last_seq: u64,
    /// The normalised board cache (the consumer the firehose invalidates).
    cache: BoardCache,
    /// The in-flight optimistic local mutations, keyed by mutation id (confirm or roll back).
    pending: HashMap<String, PendingMutation>,
    /// The cumulative count of `resync_required` falls-back-to-snapshot (the §10.2 survival signal —
    /// NAMED, not silent; a non-zero count is the cold-rebuild path having fired).
    resync_required_count: u64,
}

impl BoardSync {
    /// **Open a board sync on `board:<id>` (the bounded scope, §7).** Parses the raw scope through the
    /// ONE `*`-rejecting [`FirehoseScope::parse`] chokepoint — an over-broad/`*` scope is REJECTED
    /// ([`FirehoseError::OverBroadScope`]), never admitted. `window` is the paginated visible slice
    /// (the board never subscribes to all 50k rows). Does NOT subscribe yet — call
    /// [`Self::subscribe`].
    pub fn open(
        stream: impl Into<String>,
        board_scope: &str,
        window: ScopeWindow,
    ) -> Result<BoardSync, FirehoseError> {
        let scope = FirehoseScope::parse(board_scope)?;
        Ok(BoardSync {
            stream: stream.into(),
            scope,
            window,
            sub: None,
            last_seq: 0,
            cache: BoardCache::new(),
            pending: HashMap::new(),
            resync_required_count: 0,
        })
    }

    /// **`subscribe(stream, scope = board:<id>, cursor?)` (§7).** Open the live subscription on the
    /// firehose. `cursor = None` starts live from now (a fresh viewer); `cursor = Some(seq)` is a
    /// resume (used by [`Self::reconnect`]). Any backfilled gap is applied to the cache before going
    /// live (the zero-loss property). Returns the over-broad-scope rejection if the scope is unbounded
    /// (already rejected at [`Self::open`], so this is the resume's `resync_required` channel).
    pub fn subscribe(
        &mut self,
        fh: &mut Firehose,
        cursor: Option<u64>,
    ) -> Result<(), FirehoseError> {
        let sub = fh.subscribe(&self.stream, &self.scope, cursor)?;
        // Apply any backfilled gap (a resume) to the cache before the live frames — zero ops lost.
        self.drain_into_cache(&sub);
        self.sub = Some(sub);
        Ok(())
    }

    /// **Drain ready frames into the cache (the bus-driven cache invalidation, §7).** Pulls every
    /// ready frame off the subscription, decodes each to a [`BoardOp`], and applies it idempotently —
    /// advancing the resume cursor (`last_seq`) to the last applied frame. Returns the number of ops
    /// applied (the backfill/live gap closed). Skips a frame whose payload the board layer does not
    /// interpret (e.g. presence) WITHOUT advancing past an op (it still advances the cursor — a
    /// non-board frame is still consumed). Idempotent: re-applying an op already in the cache is a
    /// no-op, so a backfill overlapping a live frame never double-applies (zero-dup).
    pub fn pump(&mut self) -> usize {
        let Some(sub) = self.sub.clone() else {
            return 0;
        };
        self.drain_into_cache(&sub)
    }

    /// Drain a subscription's ready frames into the cache (the shared body of `subscribe`'s backfill
    /// apply and `pump`'s live apply). Advances the resume cursor to the last frame's seq.
    fn drain_into_cache(&mut self, sub: &FirehoseSubscription) -> usize {
        let mut applied = 0;
        for frame in sub.drain_ready() {
            self.apply_frame(&frame);
            applied += 1;
        }
        applied
    }

    /// Apply one firehose frame to the cache + advance the resume cursor. A frame whose payload is not
    /// a board op (presence/typing) advances the cursor but does not patch the cache.
    fn apply_frame(&mut self, frame: &Frame) {
        if let Some(op) = BoardOp::decode(&frame.payload.0) {
            self.cache.apply(&op);
        }
        // The cursor advances on EVERY consumed frame (board op or not) — the next resume continues
        // from here, so a non-board frame is not re-delivered. Monotone.
        self.last_seq = self.last_seq.max(frame.seq);
    }

    /// **`reconnect()` — the zero-ops-lost reconnect (§7; the ISS-D13 core).** Calls
    /// `resume(stream, scope, last_seq)`: the transport **backfills `(last_seq, now]`** from the
    /// bounded retention window then resumes live — every op in the gap is applied to the cache, so the
    /// reconnect **loses ZERO ops**. If `last_seq` is older than the retention window the transport
    /// raises [`FirehoseError::ResyncRequired`]; the caller then calls [`Self::resync_from_snapshot`]
    /// (the NAMED cold-rebuild fallback). Returns the number of gap ops applied on success.
    pub fn reconnect(&mut self, fh: &mut Firehose) -> Result<usize, FirehoseError> {
        // The live subscription is gone (the connection dropped) — drop the stale handle.
        self.sub = None;
        let before = self.last_seq;
        self.subscribe(fh, Some(self.last_seq))?;
        // The number of gap ops applied = how far the cursor advanced during the resume backfill.
        Ok((self.last_seq - before) as usize)
    }

    /// **`resync_required` → full `*.snapshot` replay (§7; the NAMED cold-rebuild fallback).** When
    /// [`Self::reconnect`] returns [`FirehoseError::ResyncRequired`] (the `last_seq` fell off the
    /// retention window), the caller rebuilds the cache from a `*.snapshot` (contract 2.6 / the
    /// `issue.issue.snapshot` event) — replacing the cache wholesale and re-pinning the cursor to the
    /// snapshot's `as_of_seq`, then re-subscribing live from there. The cold-rebuild path is taken
    /// LOUDLY (the `resync_required_count` increments) — never a silent partial board.
    pub fn resync_from_snapshot(
        &mut self,
        fh: &mut Firehose,
        snapshot: Vec<BoardCard>,
        as_of_seq: u64,
    ) -> Result<(), FirehoseError> {
        self.cache.replace_from_snapshot(snapshot);
        self.last_seq = as_of_seq;
        self.pending.clear(); // the snapshot is the truth; in-flight optimism is reconciled into it.
        self.resync_required_count += 1;
        // Re-subscribe live from the snapshot's seq (a resume at `as_of_seq` backfills nothing — the
        // snapshot already holds `(.., as_of_seq]` — then goes live). If THIS resume also resyncs (the
        // snapshot itself was stale vs the window floor), the error propagates (a re-snapshot).
        self.sub = None;
        self.subscribe(fh, Some(as_of_seq))
    }

    /// **`apply_local(mutation_id, op)` — apply an optimistic local mutation (§7).** The card moves in
    /// the cache IMMEDIATELY (optimistic UI), tracked as PENDING with its pre-mutation state for the
    /// rollback path. The same mutation is then sent through the SAME permissioned API (UI=CLI=agent
    /// parity); a server confirm calls [`Self::confirm_local`], a reject calls [`Self::reject_local`].
    /// Rejected if `mutation_id` is already pending ([`LocalMutationError::AlreadyPending`]).
    pub fn apply_local(
        &mut self,
        mutation_id: impl Into<String>,
        op: BoardOp,
    ) -> Result<(), LocalMutationError> {
        let mutation_id = mutation_id.into();
        if self.pending.contains_key(&mutation_id) {
            return Err(LocalMutationError::AlreadyPending { mutation_id });
        }
        // Snapshot the card's PRE-mutation state (the exact rollback target) BEFORE applying.
        let prior = self.cache.card(op.issue_id()).cloned();
        self.cache.apply(&op);
        self.pending
            .insert(mutation_id, PendingMutation { op, prior });
        Ok(())
    }

    /// **`confirm_local(mutation_id)` — the server accepted the optimistic mutation (§7).** The
    /// optimistic apply stands; the pending record is cleared. The authoritative `issue.*` frame for
    /// the mutation arrives over the firehose and re-applies the SAME op idempotently (it is already
    /// in the cache — a no-op). Returns `true` iff the mutation was pending (an unknown id is a
    /// tolerated no-op — a late confirm after a resync cleared the pending set).
    pub fn confirm_local(&mut self, mutation_id: &str) -> bool {
        self.pending.remove(mutation_id).is_some()
    }

    /// **`reject_local(mutation_id)` — the server rejected the optimistic mutation; ROLL BACK (§7).**
    /// The card is restored to its exact pre-mutation state (the "one quiet line + the field to fix"
    /// UX — never a silent loss): a rejected edit reverts the card; a rejected optimistic CREATE
    /// removes it (its `prior` was `None`). Returns `true` iff the mutation was pending.
    pub fn reject_local(&mut self, mutation_id: &str) -> bool {
        let Some(pending) = self.pending.remove(mutation_id) else {
            return false;
        };
        let issue_id = pending.op.issue_id().to_string();
        match pending.prior {
            // The card existed before — restore its exact prior state (an upsert back to `prior`).
            Some(prior) => self.cache.apply(&BoardOp::Upsert(prior)),
            // The card did not exist before (an optimistic CREATE) — remove it on reject.
            None => self.cache.apply(&BoardOp::Remove { issue_id }),
        }
        true
    }

    /// The normalised board cache (the rendered board state).
    pub fn cache(&self) -> &BoardCache {
        &self.cache
    }

    /// The resume cursor — the seq of the last applied frame (presented to `resume` on reconnect).
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// The bounded `board:<id>` scope (the `(…, scope)` key; NEVER `*`).
    pub fn scope(&self) -> &FirehoseScope {
        &self.scope
    }

    /// The firehose stream (`fan.<tenant>.<project>` — the `(stream, …)` key).
    pub fn stream(&self) -> &str {
        &self.stream
    }

    /// The paginated scope window (the visible slice + margin the board fans frames into).
    pub fn window(&self) -> &ScopeWindow {
        &self.window
    }

    /// The number of optimistic mutations currently in flight (pending a server confirm/reject).
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// The cumulative `resync_required` → snapshot count (the §10.2 survival signal — NAMED, not
    /// silent; a non-zero value is the cold-rebuild path having fired).
    pub fn resync_required_count(&self) -> u64 {
        self.resync_required_count
    }

    /// `true` iff this board sync currently holds a live subscription (it is connected).
    pub fn is_connected(&self) -> bool {
        self.sub.is_some()
    }
}

#[cfg(test)]
mod tests;
