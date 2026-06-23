//! # The firehose resume-cursor subscription protocol — the Bus-owned zero-loss-replay half
//! (P-141 / EB-21, built FIRST per EI-04 §2.2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md`
//! §4.3 (the firehose split + the resume-cursor protocol: `subscribe(stream, scope, cursor?)` /
//! `resume(stream, scope, last_seq)`; a per-`(stream, scope)` monotonic `seq`; `(last_seq, now]`
//! backfill on reconnect that loses ZERO ops; `resync_required → *.snapshot` fallback; scope a
//! bounded selector NEVER `*`), §5.5 (the firehose contract surface — `publish`/`tail`/`subscribe`/
//! `resume`).
//!
//! **Contract-index:** row 3.5 (the firehose transport + resume-cursor protocol — **owned-seam**;
//! KN owns the collab CRDT that slots INTO this transport, not the transport).
//!
//! **Reconciliation decision:** `00-reconciliation-decisions.md` OQ-J (the firehose protocol,
//! co-designed ONCE for a huge board / a hot doc / a hot channel — all three use it identically).
//!
//! ## What this module is (the Bus's stake in 3.5 — the zero-loss-replay half)
//! The architecture sites the firehose `publish`/`tail`/`subscribe`/`resume` API in `myelin-events`
//! (§5.5: `firehose::publish`, …). EI-04 §2.2 / KN-1 doctrine: **build the durable resume-cursor
//! transport FIRST**, then the CRDT slots into it. This module is that transport's protocol:
//!
//! - **`publish(stream, frame)`** — append a frame to a `(stream, scope)` log, assigning the
//!   per-`(stream, scope)` **monotonic `seq`** (the resume cursor). Returns the assigned [`Frame`].
//! - **`tail(stream, range)`** — a range-read / tail (the CI log viewer's `lines N..M`); reads
//!   whatever the retention window still holds for `(stream, scope)` in `[lo, hi]`.
//! - **`subscribe(stream, scope, cursor?)` → [`SubStream`]** — open a per-view subscription on a
//!   **BOUNDED** scope. `cursor = None` starts live from now; `cursor = Some(seq)` is the same as
//!   `resume(seq)`. The scope is REJECTED if it is `*` / unbounded / over-broad
//!   ([`FirehoseError::OverBroadScope`]) — the whitelist-not-`*` rule (BUS-3) generalised.
//! - **`resume(stream, scope, last_seq)` → [`SubStream`]** — reconnect: **backfill `(last_seq, now]`**
//!   from the bounded retention window, then go live. A reconnect **loses ZERO ops** (the D-10 pass
//!   condition). If `last_seq` is OLDER than the retention window floor, the client gets a
//!   **`resync_required`** verdict ([`FirehoseError::ResyncRequired`]) and falls back to a full
//!   `*.snapshot` replay (the cold-rebuild path — **NAMED, not silent**; the `*.snapshot` schema +
//!   the reindex-from-source seam that rebuilds it is **EB-22 / P-142**).
//!
//! A [`SubStream`] carries a per-connection in-flight cap; a consumer that falls behind the cap is
//! dropped to `resync_required` rather than buffering its gap unboundedly.
//!
//! ## Coherence (EI-01 §7) — the substrate's bounded-and-sheds half rides THIS protocol
//! The platform substrate (DOWNSTREAM of `myelin-events` in the §2.9 DAG) already shipped the
//! **bounded-and-sheds half** of contract 3.5: `myelin_substrate::firehose::FrameBuffer` (the
//! per-connection cap + the slow-consumer drop to `resync_required`, P-135/P-S28) and
//! `myelin_substrate::firehose_selector::BoundedSelector` (the `board:`/`doc:`/`channel:` scope
//! parse + `*`-rejection, P-136/P-S29). That is the §7.7 "substrate guarantees the bounded-and-sheds
//! half; the Bus guarantees the zero-loss-replay half" split. THIS module is the Bus half and is the
//! UPSTREAM authority for the protocol shape (`subscribe`/`resume`/`Frame.seq`/`resync_required`):
//!
//! - `myelin-events` CANNOT depend on `myelin-substrate` (it is downstream — the DAG forbids the
//!   edge). So the Bus protocol owns its OWN scope validator ([`FirehoseScope`]) and its OWN
//!   in-flight cap on the [`SubStream`]. These are NOT a second implementation of the substrate's
//!   `BoundedSelector`/`FrameBuffer` — they are the SAME rule enforced at the PROTOCOL seam (the
//!   Bus rejects an over-broad scope at `subscribe`; the substrate re-bounds + sheds at the
//!   connection tier). The two compose at the connection tier (Chat M4): the connection opens a Bus
//!   `subscribe`, feeds its frames into a substrate `FrameBuffer`, and turns the buffer's
//!   `resync_required` verdict into a Bus `resume`/`*.snapshot`. The scope strings + the `seq` +
//!   the `resync_required` vocabulary line up 1:1 across the seam by design.
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - **The retention WINDOW per stream class is NAMED-not-numbered.** [`RetentionWindow`] is the
//!   bounded ring; its capacity is a named floor — it MUST exceed the p99 reconnect gap, and is
//!   **MEASURED + tuned by D-10 in M5 (EB-30 / P-439)**. Here a generous default
//!   ([`RetentionWindow::DEFAULT_FRAMES`]) models a window large enough for the unit/drill proofs;
//!   the tuned per-stream-class number lands in EB-30. NAMED, never silently hardcoded as "the" size.
//! - **D-10 is written to re-run GREEN across the KN CAS→CRDT `engine_promote` boundary** (the
//!   collab transport swaps its apply engine in M5; the resume-cursor zero-loss property must not
//!   regress) — re-confirmed in **EB-30 / P-439**. The drill in `tests/drills_eb21_firehose_d10.rs`
//!   is engine-agnostic (it asserts the transport's zero-loss + resync-correct property, which is
//!   independent of the collab apply engine), so it re-runs unchanged across that boundary.
//! - **The `*.snapshot` event schema + the reindex-from-source seam that REBUILDS a
//!   `resync_required` client** is **EB-22 / P-142** (`cold == live`, proven by BUS-D5). Here D-10's
//!   `resync_required` leg asserts the SIGNAL is raised correctly (the over-window `last_seq` yields
//!   `resync_required`, named not silent); the rebuild itself is EB-22.
//! ## `no-raw-publish` lint note (EI-01 §1 / §5 — NAMED, not a silent skip)
//! The `no-raw-publish` lint (EB-07 / P-019) guards the DURABLE bus: no `.publish(` outside
//! `OutboxTx::emit` (an event must be emitted-iff-committed through the outbox; there is no
//! fire-and-forget). The firehose's frozen §5.5 method is NAMED `firehose::publish` — a `.publish(`
//! fingerprint that COLLIDES with the lint's pattern. But the firehose is a SEPARATE, ephemeral
//! transport BY DESIGN (§4.3: "the durable bus carries only pointer/summary events" — the firehose
//! carries the high-volume ephemeral frames over its OWN publish/subscribe/resume API; a frame is a
//! references-not-payloads [`FramePayload`] pointer, never an inline-PII durable event, and is NOT
//! routed through the outbox). So this ONE file is added to the lint-gate's NAMED, LOUD exclusion
//! list (`myelin-lints/src/bin/lint-gate.rs` + `tests/workspace_clean.rs`) — exactly the same
//! posture as `relay.rs` (the one legitimate broker-publish site). The lint stays fully live on
//! every durable-bus call site; this is a documented deviation, not a weakening.
//!
//! - **The real durable transport** (the JetStream-class broker the protocol rides in prod) is the
//!   Bus M0 deployment seam (`relay::BusTransport` + the `serve` lifecycle, P-S12). Here the
//!   in-process [`Firehose`] log is the unit/floor transport; the protocol shape it implements is
//!   the frozen §5.5 surface.

use std::collections::HashMap;

/// **A firehose scope — the BOUNDED selector that bounds which frames a subscription receives
/// (§4.3, contract 3.5).** Scope is a bounded selector, **never `*`**: `board:<id>` / `doc:<id>` /
/// `channel:<id>` / `inbox:<id>` (Notif's own-inbox slice, §7 / C4). The transport REJECTS an
/// unbounded/over-broad scope at `subscribe`/`resume` (the
/// whitelist-not-`*` rule, BUS-3, generalised to the firehose). A huge board paginates its scope to
/// the visible window + a margin (the windowing itself is the connection tier's / the substrate
/// `ScopeWindow`'s job; here the protocol enforces that the scope NAMES a single bounded resource).
///
/// Held as the raw selector string (`board:123`, `doc:abc`, `channel:eng`) — a PII-free identifier,
/// so a telemetry label built from it is `control-plane-pii-free` by construction. This is the
/// `(…, scope)` half of the per-`(stream, scope)` monotonic-seq key.
///
/// **Coherence:** this is the Bus-protocol-seam scope validator. The substrate's
/// `BoundedSelector::parse` (P-136) is the SAME rule applied at the connection tier; the two use the
/// identical `board:`/`doc:`/`channel:` grammar and the identical selector-string telemetry key, so
/// a scope validated here lowers 1:1 to the substrate scope key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FirehoseScope {
    kind: ScopeKind,
    /// The bounded resource id (the part after the prefix) — a PII-free identifier.
    id: String,
}

/// The bounded scope kinds the firehose admits (§4.3: `board:`/`doc:`/`channel:`/`inbox:`). There
/// is deliberately **no `All`/`*` variant** — the type cannot represent an unbounded subscription.
///
/// **Coherence (EI-01 §7 — the scope grammar is ONE set, extended in place not forked).** The OQ-J
/// protocol froze the resume-cursor `subscribe`/`resume`/`scope` *behaviour* and co-designed the
/// scope discipline once for the three storm surfaces named at the time (board / doc / channel). The
/// scope *kind set* is the bounded-selector whitelist, and it grows by ADMITTING a new bounded
/// resource kind — never by forking a second transport. Notif's `inbox watch` (architecture
/// `notifications.md` §7 / contract 3.5 C4) names a FOURTH bounded scope, `inbox:<principal>` (one
/// principal's own inbox slice, never `*`), and rides this SAME protocol — "there is no bespoke
/// Notif live transport" (§7). So `Inbox` is added here, to the one scope grammar, rather than
/// Notif inventing its own validator: the `*`-rejection, the per-`(stream, scope)` monotone seq, the
/// `(last_seq, now]` backfill, and the `resync_required` fallback all apply to `inbox:` identically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScopeKind {
    /// A board (Issues huge-board) — paginated to the visible window + margin (the 50k-row case).
    Board,
    /// A document (Knowledge hot-doc) — the collab op-stream + presence on one doc.
    Doc,
    /// A channel (Chat hot-channel) — live delivery + presence on one channel.
    Channel,
    /// One principal's inbox slice (Notif `inbox watch`, §7 / contract 3.5 C4) — a BOUNDED selector
    /// `inbox:<principal>`; a client gets only its own inbox's live frames, never the tenant firehose.
    Inbox,
    /// One CI run's live log tail (CI §7.1 / contract 3.5) — a BOUNDED selector `run:<run_id>`; a
    /// viewer gets only that run's live log frames, never the tenant firehose. **CI is the heaviest
    /// firehose producer** (event-bus §4.3): its log lines ride this transport keyed by the run, the
    /// resume-cursor protocol backfilling a reconnect's gap (CI-D11 / CI-P21). Added to the ONE scope
    /// grammar (coherence, EI-01 §7 — the bounded-selector whitelist grows by admitting a new bounded
    /// resource kind, never by forking a second transport): the `*`-rejection, the per-`(stream,
    /// scope)` monotone seq, the `(last_seq, now]` backfill, and the `resync_required` fallback all
    /// apply to `run:` identically.
    Run,
}

impl ScopeKind {
    /// The selector prefix (`board` / `doc` / `channel` / `inbox`) — the wire form before the `:`.
    pub fn prefix(self) -> &'static str {
        match self {
            ScopeKind::Board => "board",
            ScopeKind::Doc => "doc",
            ScopeKind::Channel => "channel",
            ScopeKind::Inbox => "inbox",
            ScopeKind::Run => "run",
        }
    }
}

impl FirehoseScope {
    /// **Parse a bounded scope — the `*`-rejection chokepoint (§4.3, contract 3.5).** Admits ONLY
    /// `board:<id>` / `doc:<id>` / `channel:<id>` (a non-empty id); REJECTS `*`, any `*`-containing
    /// form, the empty string, an un-prefixed bare id, and an unknown prefix — all as
    /// [`FirehoseError::OverBroadScope`]. There is no other way to construct a [`FirehoseScope`], so
    /// an unbounded subscription is unrepresentable (the transport "rejects an over-broad scope").
    pub fn parse(raw: &str) -> Result<FirehoseScope, FirehoseError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(FirehoseError::OverBroadScope {
                scope: String::new(),
                why: "an empty scope is unbounded (it names no resource)",
            });
        }
        // The headline rule: `*` (or any `*`-containing scope — `*`, `board:*`, `doc:a*`) is an
        // unbounded subscription and is REJECTED. Checked FIRST so `board:*` reads as over-broad,
        // not a "board id of `*`".
        if raw.contains('*') {
            return Err(FirehoseError::OverBroadScope {
                scope: raw.to_string(),
                why: "scope must be a bounded selector (board:/doc:/channel:), never `*`",
            });
        }
        let Some((prefix, id)) = raw.split_once(':') else {
            return Err(FirehoseError::OverBroadScope {
                scope: raw.to_string(),
                why: "scope must name its kind: board:/doc:/channel:",
            });
        };
        if id.is_empty() {
            return Err(FirehoseError::OverBroadScope {
                scope: raw.to_string(),
                why: "scope resource id must not be empty",
            });
        }
        let kind = match prefix {
            "board" => ScopeKind::Board,
            "doc" => ScopeKind::Doc,
            "channel" => ScopeKind::Channel,
            "inbox" => ScopeKind::Inbox,
            "run" => ScopeKind::Run,
            _ => {
                return Err(FirehoseError::OverBroadScope {
                    scope: raw.to_string(),
                    why: "unknown scope kind (only board:/doc:/channel:/inbox:/run:)",
                })
            }
        };
        Ok(FirehoseScope {
            kind,
            id: id.to_string(),
        })
    }

    /// The scope kind (`board`/`doc`/`channel`).
    pub fn kind(&self) -> ScopeKind {
        self.kind
    }

    /// The bounded resource id (the part after the prefix) — a PII-free identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The canonical selector string (`board:123`) — round-trips through [`Self::parse`] and is the
    /// `(…, scope)` half of the per-`(stream, scope)` survival-signal key.
    pub fn selector(&self) -> String {
        format!("{}:{}", self.kind.prefix(), self.id)
    }
}

/// **One firehose frame (§4.3 — `SubStream` yields `Frame { seq, ... }`).** A per-`(stream, scope)`
/// monotonic `seq` (the resume cursor the client sends on reconnect) plus an OPAQUE payload pointer
/// ([`FramePayload`]). The transport NEVER reads the payload body (PII stays out of the transport —
/// references-not-payloads, §2.1): a firehose frame carries an `ArtifactRef`/offset pointer to the
/// real content (the CI log byte-range, the collab op id, the chat message id), the durable bus
/// carries only the pointer EVENT (`ci.log.available` / `knowledge.doc.updated`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The per-`(stream, scope)` monotonic sequence (the resume cursor). `0` is the "before any
    /// frame" sentinel — the first published frame is `seq = 1`, so `resume(last_seq = 0)` backfills
    /// EVERYTHING the window holds (a fresh client that lost its cursor).
    pub seq: u64,
    /// The opaque payload pointer — the transport never reads its body.
    pub payload: FramePayload,
}

/// **An opaque firehose frame payload (references-not-payloads).** The transport treats it as an
/// opaque pointer; the connection tier resolves it to the real content. Held as a PII-free pointer
/// string (an `ArtifactRef`, a `(job, step, byte-range)` index key, an op id) — never an inline PII
/// body. (The concrete per-surface payload — CI log frame, collab op, chat message — is the
/// owning subsystem's; the transport is payload-agnostic.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FramePayload(pub String);

impl Frame {
    /// A frame at `seq` carrying the opaque pointer `payload`.
    pub fn new(seq: u64, payload: impl Into<String>) -> Frame {
        Frame {
            seq,
            payload: FramePayload(payload.into()),
        }
    }
}

/// **A draft frame a producer publishes (the `seq` is assigned BY the transport, not the producer).**
/// A producer cannot mint its own `seq` — the per-`(stream, scope)` monotonic `seq` is the
/// transport's invariant (a producer-chosen seq could collide / rewind and break the resume cursor).
/// [`Firehose::publish`] assigns the next seq and returns the assigned [`Frame`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameDraft {
    /// The opaque payload pointer (the transport never reads its body).
    pub payload: FramePayload,
}

impl FrameDraft {
    /// A draft carrying the opaque pointer `payload`.
    pub fn new(payload: impl Into<String>) -> FrameDraft {
        FrameDraft {
            payload: FramePayload(payload.into()),
        }
    }
}

/// **Why a firehose subscription / resume failed (the typed, LOUD verdicts — never a silent fail).**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirehoseError {
    /// The scope was `*` / unbounded / over-broad — the transport REJECTS it (the whitelist-not-`*`
    /// rule, BUS-3, generalised, §4.3). Carries the offending scope + the reason (a `400`/close at
    /// the connection tier, never a silent admit of an unbounded subscription).
    OverBroadScope {
        /// The offending scope string.
        scope: String,
        /// Why it is over-broad (the rule it broke).
        why: &'static str,
    },
    /// The reconnect `last_seq` is OLDER than the bounded retention window — the gap cannot be
    /// backfilled from the window, so the client MUST fall back to a full `*.snapshot` replay (the
    /// cold-rebuild path, **NAMED not silent**, §4.3). Carries the window floor for diagnostics.
    /// The `*.snapshot` rebuild itself is EB-22 / P-142.
    ResyncRequired {
        /// The `last_seq` the client presented.
        last_seq: u64,
        /// The OLDEST seq the retention window still holds (the window floor). `last_seq` is below
        /// `window_floor - 1`, so `(last_seq, now]` is not fully replayable from the window.
        window_floor: u64,
    },
}

impl FirehoseError {
    /// `true` iff this is the `resync_required` verdict (the §4.3 cold-rebuild fallback signal —
    /// the D-10 drill's resync leg asserts this is raised for an out-of-window `last_seq`).
    pub fn is_resync_required(&self) -> bool {
        matches!(self, FirehoseError::ResyncRequired { .. })
    }

    /// `true` iff this is the over-broad-scope rejection (the D-10 drill's `scope = *` leg).
    pub fn is_over_broad_scope(&self) -> bool {
        matches!(self, FirehoseError::OverBroadScope { .. })
    }
}

impl core::fmt::Display for FirehoseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FirehoseError::OverBroadScope { scope, why } => {
                write!(f, "firehose rejects over-broad scope `{scope}`: {why}")
            }
            FirehoseError::ResyncRequired { last_seq, window_floor } => write!(
                f,
                "resync_required: last_seq={last_seq} is older than the retention window (floor={window_floor}) \
                 → fall back to a *.snapshot replay (EB-22)"
            ),
        }
    }
}

impl std::error::Error for FirehoseError {}

/// **The bounded retention WINDOW for one `(stream, scope)` (§4.3 — "a bounded firehose retention
/// window").** A FIFO ring of the most-recent `capacity` frames: a `publish` appends (evicting the
/// oldest once full), `resume(last_seq)` reads `(last_seq, now]` from it, and an out-of-window
/// `last_seq` yields `resync_required`. The capacity is the **NAMED floor** (it must exceed the p99
/// reconnect gap) — MEASURED + tuned per stream class by D-10 in M5 (EB-30 / P-439); the default
/// here is generous enough for the unit/drill proofs and is NAMED, never silently "the" size.
///
/// Memory is bounded by `capacity` (Little's Law applied to retention) — the window never grows
/// unboundedly; an op older than the window is not in memory and is the `resync_required` case.
#[derive(Clone, Debug)]
pub struct RetentionWindow {
    /// The most-recent frames, oldest-first (a FIFO ring bounded at `capacity`).
    frames: std::collections::VecDeque<Frame>,
    /// The bound (the NAMED retention floor → EB-30 tunes per stream class).
    capacity: usize,
    /// The highest `seq` ever published to this `(stream, scope)` (monotone; survives eviction so a
    /// new frame's seq is `last_seq + 1` even after the window has rolled).
    last_seq: u64,
}

impl RetentionWindow {
    /// The default retention-window frame count (the **NAMED floor**, not a measured number). It is
    /// generous enough that the unit + D-10 drill proofs exercise both the in-window backfill AND
    /// the deliberate over-window `resync_required` path; the per-stream-class tuned number is
    /// measured by D-10 in M5 (EB-30 / P-439). NAMED, never "the" production size.
    pub const DEFAULT_FRAMES: usize = 4096;

    /// A retention window holding the most-recent `capacity` frames (`capacity` is raised to at
    /// least 1 so a window always holds the latest frame).
    pub fn new(capacity: usize) -> RetentionWindow {
        RetentionWindow {
            frames: std::collections::VecDeque::new(),
            capacity: capacity.max(1),
            last_seq: 0,
        }
    }

    /// Append a draft, assigning the next per-`(stream, scope)` monotonic `seq` (`last_seq + 1`).
    /// Evicts the oldest frame once at capacity (the window is bounded). Returns the assigned frame.
    fn publish(&mut self, draft: FrameDraft) -> Frame {
        self.last_seq += 1;
        let frame = Frame {
            seq: self.last_seq,
            payload: draft.payload,
        };
        if self.frames.len() == self.capacity {
            self.frames.pop_front();
        }
        self.frames.push_back(frame.clone());
        frame
    }

    /// The OLDEST seq the window still holds (the window floor). `0` when the window is empty.
    fn window_floor(&self) -> u64 {
        self.frames.front().map(|f| f.seq).unwrap_or(0)
    }

    /// The highest seq ever published (monotone, survives eviction).
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// Read `(last_seq, now]` — every frame the window holds with `seq > last_seq`, in order. The
    /// backfill that "loses zero ops": every op in the gap that the window still holds is returned.
    /// Returns `Err(ResyncRequired)` iff the gap reaches BEFORE the window floor (the oldest op the
    /// client needs has been evicted → it cannot be replayed from the window).
    fn backfill(&self, last_seq: u64) -> Result<Vec<Frame>, FirehoseError> {
        // The gap the client needs is `(last_seq, last_seq_now]`. The window can replay it iff every
        // op in the gap is still held — i.e. the first op the client is MISSING (`last_seq + 1`) is
        // still in the window. If `last_seq + 1 < window_floor`, the missing op was evicted → resync.
        //
        // The "fresh client" cases that are NOT a resync:
        //  - `last_seq >= self.last_seq`: the client is caught up (or ahead) → empty backfill, go live.
        //  - `last_seq == 0` with an empty-but-never-published window: nothing to replay, go live.
        if last_seq >= self.last_seq {
            return Ok(Vec::new());
        }
        let floor = self.window_floor();
        // The first op the client is missing:
        let first_missing = last_seq + 1;
        // If the window holds nothing at/after `first_missing` but the client is behind, the gap was
        // evicted. `floor == 0` means an empty window; a behind client then needs a resync too (the
        // ops it is missing are simply not held). `first_missing < floor` means the gap's head was
        // evicted.
        if floor == 0 || first_missing < floor {
            return Err(FirehoseError::ResyncRequired {
                last_seq,
                window_floor: floor,
            });
        }
        Ok(self
            .frames
            .iter()
            .filter(|f| f.seq > last_seq)
            .cloned()
            .collect())
    }

    /// Read the frames whose seq falls in `[lo, hi]` that the window still holds (the `tail(range)`
    /// read). Out-of-window frames are simply absent (a tail is best-effort over the live window; the
    /// durable record is the T3 log tier, contract 11.8 / EB seam).
    fn tail(&self, lo: u64, hi: u64) -> Vec<Frame> {
        self.frames
            .iter()
            .filter(|f| f.seq >= lo && f.seq <= hi)
            .cloned()
            .collect()
    }

    /// The number of frames the window currently holds (bounded by `capacity`).
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// `true` iff the window holds no frames.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// **The per-connection in-flight cap on a `SubStream` (§4.3 backpressure — the Bus-protocol half).**
/// A live subscription delivers frames into a bounded in-flight queue; a consumer that falls more
/// than `cap` frames behind is **dropped to `resync_required`** rather than the transport buffering
/// its gap unboundedly (the §4.3 "a slow consumer is dropped to `resync_required` rather than
/// buffering"). This is the Bus-protocol expression of the rule; the substrate's `FrameBuffer`
/// (P-135) is the per-frame-class shed half that rides it at the connection tier.
pub const DEFAULT_INFLIGHT_CAP: usize = 1024;

/// **A live subscription stream (§4.3 — `subscribe`/`resume` return a `SubStream`).** Carries the
/// backfilled gap (`(last_seq, now]`, empty for a `cursor = None` / caught-up subscribe) followed by
/// the live frames the producer publishes after the subscription opened. A reconnect's `SubStream`
/// is contiguous (backfill THEN live) with no gap and no duplicate — the zero-loss property.
///
/// The stream is a pull surface ([`Self::next`]) with a bounded in-flight cap: if the consumer lets
/// more than `inflight_cap` frames accumulate undelivered, the next live delivery drops it to
/// `resync_required` ([`Self::resync_required`]) instead of buffering unboundedly.
#[derive(Clone, Debug)]
pub struct SubStream {
    /// The `(stream, scope)` key this subscription is on (PII-free; the survival-signal key).
    stream: String,
    scope: FirehoseScope,
    /// The frames ready to be pulled (backfill first, then live frames as they are published),
    /// oldest-first. Bounded by `inflight_cap`.
    ready: std::collections::VecDeque<Frame>,
    /// The per-connection in-flight cap (§4.3) — undelivered frames may not exceed this; a slow
    /// consumer that exceeds it is dropped to `resync_required`.
    inflight_cap: usize,
    /// The seq of the last frame DELIVERED (pulled) — the resume cursor the client would present on
    /// the next reconnect. Monotone.
    delivered_seq: u64,
    /// `true` once this subscription was dropped to `resync_required` (a slow consumer). It holds no
    /// frames once dropped (memory released) — the consumer must `resume`/`*.snapshot` to recover.
    resync_required: bool,
    /// The highest seq enqueued onto `ready` (the producer side — for the gap check). Monotone.
    enqueued_seq: u64,
}

impl SubStream {
    /// Build a `SubStream` on `(stream, scope)` seeded with `backfill` frames and the in-flight cap.
    fn new(
        stream: String,
        scope: FirehoseScope,
        backfill: Vec<Frame>,
        inflight_cap: usize,
        start_seq: u64,
    ) -> SubStream {
        let enqueued_seq = backfill.last().map(|f| f.seq).unwrap_or(start_seq);
        SubStream {
            stream,
            scope,
            ready: backfill.into_iter().collect(),
            inflight_cap: inflight_cap.max(1),
            delivered_seq: start_seq,
            resync_required: false,
            enqueued_seq,
        }
    }

    /// Enqueue a freshly-published LIVE frame onto this subscription (the producer side). Frames are
    /// enqueued in seq order; a frame at or below the highest already-enqueued seq is ignored (no
    /// duplicate). If the consumer has let the in-flight queue reach `inflight_cap`, the subscription
    /// is **dropped to `resync_required`** (the slow-consumer drop) and the frame is NOT buffered —
    /// memory is released and stays bounded.
    fn enqueue_live(&mut self, frame: Frame) {
        if self.resync_required {
            return;
        }
        // No duplicate / no rewind: only strictly-newer frames advance the live stream.
        if frame.seq <= self.enqueued_seq {
            return;
        }
        if self.ready.len() >= self.inflight_cap {
            // The consumer is too slow — drop the whole subscription to resync_required rather than
            // buffer its gap unboundedly (§4.3). Release the buffer (memory → bounded).
            self.drop_to_resync();
            return;
        }
        self.enqueued_seq = frame.seq;
        self.ready.push_back(frame);
    }

    /// **Pull the next ready frame (the consumer side).** Returns `None` when the stream is caught up
    /// (no frames ready) or has been dropped to `resync_required` (the caller checks
    /// [`Self::resync_required`] to distinguish "caught up" from "must resync"). Advances the
    /// delivered cursor — the `last_seq` the client would present on the next reconnect. (Named
    /// `pull`, not `next`, to keep it a pull-cursor distinct from `Iterator::next`.)
    pub fn pull(&mut self) -> Option<Frame> {
        if self.resync_required {
            return None;
        }
        let frame = self.ready.pop_front()?;
        self.delivered_seq = frame.seq;
        Some(frame)
    }

    /// Drain every currently-ready frame, in order (the bounded, deterministic read a test / a batch
    /// delivery uses). Advances the delivered cursor to the last drained frame.
    pub fn drain_ready(&mut self) -> Vec<Frame> {
        let mut out = Vec::new();
        while let Some(f) = self.pull() {
            out.push(f);
        }
        out
    }

    /// The resume cursor — the seq of the last frame this subscription DELIVERED. The client presents
    /// this to `resume(stream, scope, last_seq)` on reconnect.
    pub fn last_seq(&self) -> u64 {
        self.delivered_seq
    }

    /// `true` iff this subscription was dropped to `resync_required` (a slow consumer fell past the
    /// in-flight cap). The consumer falls back to a `resume`/`*.snapshot` replay.
    pub fn resync_required(&self) -> bool {
        self.resync_required
    }

    /// The number of frames currently ready to pull (the in-flight count — bounded by the cap). `0`
    /// once dropped to `resync_required` (memory released).
    pub fn ready_len(&self) -> usize {
        self.ready.len()
    }

    /// The `(stream, …)` survival-signal key half.
    pub fn stream(&self) -> &str {
        &self.stream
    }

    /// The `(…, scope)` survival-signal key half.
    pub fn scope(&self) -> &FirehoseScope {
        &self.scope
    }

    /// Drop the subscription to `resync_required`: release every ready frame (memory → bounded) and
    /// set the flag. Idempotent.
    fn drop_to_resync(&mut self) {
        if self.resync_required {
            return;
        }
        self.ready.clear();
        self.resync_required = true;
    }
}

/// **The firehose transport (§5.5 — `firehose::publish`/`tail`/`subscribe`/`resume`).** Holds one
/// bounded [`RetentionWindow`] per `(stream, scope)` and tracks the open live [`SubStream`]s so a
/// `publish` fans out to them. This is the in-process floor transport; the protocol shape is the
/// frozen §5.5 surface (the real broker binding is the Bus M0 deployment, P-S12 — a named floor).
///
/// The publish side assigns the per-`(stream, scope)` monotonic `seq`; the subscribe/resume side
/// backfills `(last_seq, now]` from the window then goes live (losing zero ops) or raises
/// `resync_required` for an out-of-window cursor.
#[derive(Default)]
pub struct Firehose {
    /// One bounded retention window per `(stream, scope)`.
    windows: HashMap<(String, FirehoseScope), RetentionWindow>,
    /// The open live subscriptions, indexed by `(stream, scope)` — a `publish` fans out to them.
    /// (In the in-process floor a subscription is a shared handle the test drives; the real
    /// transport pushes to a connection's socket, P-S12.)
    subscribers: HashMap<(String, FirehoseScope), Vec<SubHandle>>,
    /// The retention-window capacity new `(stream, scope)` windows get (the NAMED floor → EB-30).
    window_capacity: usize,
    /// The per-connection in-flight cap new subscriptions get (§4.3 backpressure).
    inflight_cap: usize,
}

/// A shared handle to an open [`SubStream`] (the in-process floor's fan-out target). The real
/// transport (P-S12) fans out to a connection socket; here it is an `Rc<RefCell<…>>` so a `publish`
/// can enqueue onto every open subscription and a test can pull from the same handle.
#[derive(Clone)]
struct SubHandle(std::rc::Rc<std::cell::RefCell<SubStream>>);

/// A live subscription handle the caller holds (the in-process floor). Pull frames via [`Self::next`]
/// / [`Self::drain_ready`]; a `publish` on the same `(stream, scope)` enqueues onto it.
#[derive(Clone, Debug)]
pub struct Subscription(std::rc::Rc<std::cell::RefCell<SubStream>>);

impl Subscription {
    /// Pull the next ready frame (delegates to [`SubStream::pull`]).
    pub fn pull(&self) -> Option<Frame> {
        self.0.borrow_mut().pull()
    }

    /// Drain every currently-ready frame, in order (delegates to [`SubStream::drain_ready`]).
    pub fn drain_ready(&self) -> Vec<Frame> {
        self.0.borrow_mut().drain_ready()
    }

    /// The resume cursor (the last delivered seq).
    pub fn last_seq(&self) -> u64 {
        self.0.borrow().last_seq()
    }

    /// `true` iff this subscription was dropped to `resync_required` (a slow consumer).
    pub fn resync_required(&self) -> bool {
        self.0.borrow().resync_required()
    }

    /// The number of frames currently ready to pull (the in-flight count, bounded).
    pub fn ready_len(&self) -> usize {
        self.0.borrow().ready_len()
    }

    /// The `(stream, …)` key half.
    pub fn stream(&self) -> String {
        self.0.borrow().stream().to_string()
    }

    /// The `(…, scope)` key half.
    pub fn scope(&self) -> FirehoseScope {
        self.0.borrow().scope().clone()
    }
}

impl Firehose {
    /// A firehose with the default retention window ([`RetentionWindow::DEFAULT_FRAMES`]) + the
    /// default in-flight cap ([`DEFAULT_INFLIGHT_CAP`]).
    pub fn new() -> Firehose {
        Firehose::with_limits(RetentionWindow::DEFAULT_FRAMES, DEFAULT_INFLIGHT_CAP)
    }

    /// A firehose with an explicit retention-window capacity + in-flight cap (the D-10 drill drives a
    /// SMALL window to force the out-of-window `resync_required` path deterministically).
    pub fn with_limits(window_capacity: usize, inflight_cap: usize) -> Firehose {
        Firehose {
            windows: HashMap::new(),
            subscribers: HashMap::new(),
            window_capacity: window_capacity.max(1),
            inflight_cap: inflight_cap.max(1),
        }
    }

    /// **`publish(stream, scope, frame)` (§5.5).** Append a frame to the `(stream, scope)` retention
    /// window, assigning the per-`(stream, scope)` monotonic `seq`, and fan it out to every open live
    /// subscription on that key. Returns the assigned [`Frame`] (with its seq).
    ///
    /// `scope` MUST be a bounded [`FirehoseScope`] — the publish key is the same `(stream, scope)`
    /// the resume cursor is keyed by, so a producer publishes to a bounded resource, never `*`.
    pub fn publish(&mut self, stream: &str, scope: &FirehoseScope, draft: FrameDraft) -> Frame {
        let key = (stream.to_string(), scope.clone());
        let window = self
            .windows
            .entry(key.clone())
            .or_insert_with(|| RetentionWindow::new(self.window_capacity));
        let frame = window.publish(draft);
        // Fan out to open live subscriptions; prune any that have been dropped to resync_required.
        if let Some(subs) = self.subscribers.get_mut(&key) {
            for h in subs.iter() {
                h.0.borrow_mut().enqueue_live(frame.clone());
            }
            subs.retain(|h| !h.0.borrow().resync_required());
        }
        frame
    }

    /// **`tail(stream, scope, range)` (§5.5).** A range-read over the live retention window for
    /// `(stream, scope)`: the frames whose seq falls in `[lo, hi]` that the window still holds. The
    /// CI log viewer's `lines N..M`. (The DURABLE long-term tail is the T3 log tier, contract 11.8;
    /// here `tail` reads the live window.)
    pub fn tail(&self, stream: &str, scope: &FirehoseScope, lo: u64, hi: u64) -> Vec<Frame> {
        let key = (stream.to_string(), scope.clone());
        self.windows
            .get(&key)
            .map(|w| w.tail(lo, hi))
            .unwrap_or_default()
    }

    /// **`subscribe(stream, scope, cursor?)` → `SubStream` (§5.5, NEW).** Open a per-view subscription
    /// on a BOUNDED scope. `cursor = None` starts live from the current head (no backfill);
    /// `cursor = Some(seq)` is exactly `resume(seq)` (backfill `(seq, now]` then live).
    ///
    /// REJECTS the subscription with [`FirehoseError::OverBroadScope`] iff the scope is unbounded —
    /// but `scope` is already a typed [`FirehoseScope`] (unconstructable if unbounded), so the
    /// over-broad rejection happens at [`FirehoseScope::parse`]; this entry takes the parsed scope.
    /// Use [`Firehose::subscribe_raw`] to parse + subscribe in one step (the connection-tier entry
    /// that rejects `scope = *`).
    pub fn subscribe(
        &mut self,
        stream: &str,
        scope: &FirehoseScope,
        cursor: Option<u64>,
    ) -> Result<Subscription, FirehoseError> {
        match cursor {
            None => Ok(self.open_live(stream, scope, Vec::new())),
            Some(last_seq) => self.resume(stream, scope, last_seq),
        }
    }

    /// **`subscribe` from a RAW scope string — the connection-tier entry that REJECTS `scope = *`.**
    /// Parses the raw scope through [`FirehoseScope::parse`] (the `*`-rejection chokepoint) then
    /// subscribes. This is where `scope = *` / an over-broad scope is rejected
    /// ([`FirehoseError::OverBroadScope`]) — the D-10 "transport rejects an over-broad scope" leg.
    pub fn subscribe_raw(
        &mut self,
        stream: &str,
        raw_scope: &str,
        cursor: Option<u64>,
    ) -> Result<Subscription, FirehoseError> {
        let scope = FirehoseScope::parse(raw_scope)?;
        self.subscribe(stream, &scope, cursor)
    }

    /// **`resume(stream, scope, last_seq)` → `SubStream` (§5.5, NEW).** Reconnect: **backfill
    /// `(last_seq, now]`** from the bounded retention window, then go live. A reconnect **loses ZERO
    /// ops** — every op in the gap the window still holds is delivered before any new live frame.
    /// Returns [`FirehoseError::ResyncRequired`] iff `last_seq` is older than the window floor (the
    /// gap's head was evicted → fall back to a `*.snapshot` replay, EB-22 — NAMED not silent).
    pub fn resume(
        &mut self,
        stream: &str,
        scope: &FirehoseScope,
        last_seq: u64,
    ) -> Result<Subscription, FirehoseError> {
        let key = (stream.to_string(), scope.clone());
        let backfill = match self.windows.get(&key) {
            // A never-published (stream, scope): a fresh client at last_seq 0 just goes live; a
            // behind client (last_seq > 0 but nothing published) likewise has nothing to replay.
            None => Vec::new(),
            Some(window) => window.backfill(last_seq)?,
        };
        Ok(self.open_live(stream, scope, backfill))
    }

    /// Open a live subscription on `(stream, scope)` seeded with `backfill`, register it for fan-out,
    /// and return the caller handle. The subscription's start cursor is the last backfilled seq (or
    /// the window head if no backfill), so a subsequent `publish` enqueues only strictly-newer frames
    /// (no duplicate across the backfill→live boundary — the zero-loss, zero-dup property).
    fn open_live(
        &mut self,
        stream: &str,
        scope: &FirehoseScope,
        backfill: Vec<Frame>,
    ) -> Subscription {
        let key = (stream.to_string(), scope.clone());
        // The live start cursor: the head of the window (so a `None`-cursor subscribe starts live
        // from now and a backfilled resume continues from its last backfilled frame).
        let head = self.windows.get(&key).map(|w| w.last_seq()).unwrap_or(0);
        let start_seq = backfill.last().map(|f| f.seq).unwrap_or(head);
        let sub = SubStream::new(
            stream.to_string(),
            scope.clone(),
            backfill,
            self.inflight_cap,
            start_seq,
        );
        let rc = std::rc::Rc::new(std::cell::RefCell::new(sub));
        self.subscribers
            .entry(key)
            .or_default()
            .push(SubHandle(rc.clone()));
        Subscription(rc)
    }

    /// The highest seq published to `(stream, scope)` (the live head) — `0` if never published.
    pub fn head_seq(&self, stream: &str, scope: &FirehoseScope) -> u64 {
        self.windows
            .get(&(stream.to_string(), scope.clone()))
            .map(|w| w.last_seq())
            .unwrap_or(0)
    }

    /// The number of frames the `(stream, scope)` retention window currently holds (bounded).
    pub fn window_len(&self, stream: &str, scope: &FirehoseScope) -> usize {
        self.windows
            .get(&(stream.to_string(), scope.clone()))
            .map(|w| w.len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(s: &str) -> FirehoseScope {
        FirehoseScope::parse(s).expect("a bounded scope")
    }

    fn draft(p: &str) -> FrameDraft {
        FrameDraft::new(p)
    }

    // ---- per-(stream, scope) monotonic seq -----------------------------------------------------

    /// **A publish assigns a per-`(stream, scope)` MONOTONIC seq (§4.3).** Frames on one
    /// `(stream, scope)` get `1, 2, 3, …`; a DIFFERENT scope has its OWN independent sequence (the
    /// seq is keyed by `(stream, scope)`, not global) — so two boards never share a cursor.
    #[test]
    fn publish_assigns_per_stream_scope_monotonic_seq() {
        let mut fh = Firehose::new();
        let board_a = scope("board:a");
        let board_b = scope("board:b");

        let f1 = fh.publish("chat-live", &board_a, draft("op-1"));
        let f2 = fh.publish("chat-live", &board_a, draft("op-2"));
        let f3 = fh.publish("chat-live", &board_a, draft("op-3"));
        assert_eq!(
            (f1.seq, f2.seq, f3.seq),
            (1, 2, 3),
            "monotone per (stream,scope)"
        );

        // a DIFFERENT scope on the same stream has its OWN sequence starting at 1.
        let g1 = fh.publish("chat-live", &board_b, draft("op-x"));
        assert_eq!(
            g1.seq, 1,
            "a different scope has an independent monotonic seq"
        );
        // board:a's next frame continues from 3 (unaffected by board:b).
        let f4 = fh.publish("chat-live", &board_a, draft("op-4"));
        assert_eq!(f4.seq, 4, "the original scope's sequence is independent");
    }

    // ---- backfill (last_seq, now] then live, loses 0 ops ---------------------------------------

    /// **D-10 CORE: a reconnect backfills `(last_seq, now]` then live, losing ZERO ops (§4.3).** A
    /// client at `last_seq = 2`; meanwhile 3,4,5 are published; on `resume(2)` it gets EXACTLY 3,4,5
    /// (the gap), then any subsequent live frame (6) — contiguous, no gap, no duplicate.
    #[test]
    fn resume_backfills_the_gap_then_goes_live_losing_zero_ops() {
        let mut fh = Firehose::new();
        let s = scope("doc:design");

        // the client saw up to seq 2, then the connection dropped.
        fh.publish("kn-ops", &s, draft("op-1"));
        fh.publish("kn-ops", &s, draft("op-2"));
        // while disconnected, 3,4,5 are published (the gap).
        fh.publish("kn-ops", &s, draft("op-3"));
        fh.publish("kn-ops", &s, draft("op-4"));
        fh.publish("kn-ops", &s, draft("op-5"));

        // reconnect with last_seq = 2 → backfill (2, now] = {3,4,5}.
        let sub = fh
            .resume("kn-ops", &s, 2)
            .expect("in-window resume backfills");
        let backfilled = sub.drain_ready();
        let seqs: Vec<u64> = backfilled.iter().map(|f| f.seq).collect();
        assert_eq!(
            seqs,
            vec![3, 4, 5],
            "the gap (last_seq, now] is replayed — ZERO ops lost"
        );
        assert_eq!(sub.last_seq(), 5, "the resume cursor advanced to the head");

        // now a LIVE frame is published → it is delivered with NO gap and NO duplicate.
        fh.publish("kn-ops", &s, draft("op-6"));
        let live = sub.drain_ready();
        assert_eq!(
            live.iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![6],
            "live continues gap-free"
        );

        // TOTAL: the client saw 3,4,5,6 across the reconnect — every op exactly once, none lost.
        let mut all = seqs;
        all.extend(live.iter().map(|f| f.seq));
        assert_eq!(
            all,
            vec![3, 4, 5, 6],
            "across the reconnect: 0 lost, 0 duplicate"
        );
    }

    /// A `cursor = None` subscribe starts LIVE from now (no backfill) and receives only frames
    /// published AFTER it opened — the live-from-head case (a fresh viewer joining a hot channel).
    #[test]
    fn subscribe_with_no_cursor_starts_live_from_now() {
        let mut fh = Firehose::new();
        let s = scope("channel:eng");
        fh.publish("chat-live", &s, draft("old-1"));
        fh.publish("chat-live", &s, draft("old-2"));

        // subscribe with no cursor → no backfill, live from head.
        let sub = fh
            .subscribe("chat-live", &s, None)
            .expect("bounded scope subscribes");
        assert!(
            sub.drain_ready().is_empty(),
            "no backfill on a None cursor (live from now)"
        );

        // only frames published after the subscription opened are delivered.
        fh.publish("chat-live", &s, draft("new-3"));
        fh.publish("chat-live", &s, draft("new-4"));
        let live: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
        assert_eq!(
            live,
            vec![3, 4],
            "a None-cursor subscribe receives only post-open live frames"
        );
    }

    /// A resume at the CURRENT head (caught-up client) backfills nothing and just continues live —
    /// the no-op reconnect (a client that never actually fell behind).
    #[test]
    fn resume_at_head_is_a_no_op_backfill() {
        let mut fh = Firehose::new();
        let s = scope("board:7");
        for _ in 0..5 {
            fh.publish("issues", &s, draft("row"));
        }
        // resume at last_seq = 5 (the head) → nothing to replay.
        let sub = fh
            .resume("issues", &s, 5)
            .expect("caught-up resume is fine");
        assert!(
            sub.drain_ready().is_empty(),
            "a caught-up resume backfills nothing"
        );
        assert!(!sub.resync_required(), "a caught-up resume is NOT a resync");
    }

    // ---- an out-of-window last_seq → resync_required -------------------------------------------

    /// **D-10 RESYNC LEG: an out-of-window `last_seq` yields `resync_required` (§4.3, NAMED not
    /// silent).** A SMALL retention window (3 frames); the client's `last_seq` is older than the
    /// window floor → the gap's head was evicted → `resync_required` (fall back to `*.snapshot`,
    /// EB-22). The signal is RAISED, never a silent partial replay.
    #[test]
    fn out_of_window_last_seq_yields_resync_required() {
        // window holds only the most-recent 3 frames.
        let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
        let s = scope("doc:hot");
        // publish 1..6 → the window now holds {4,5,6}; 1,2,3 were evicted.
        for _ in 0..6 {
            fh.publish("kn-ops", &s, draft("op"));
        }
        assert_eq!(
            fh.window_len("kn-ops", &s),
            3,
            "the window is bounded at 3 (1,2,3 evicted)"
        );

        // a client at last_seq = 2 needs op 3 first — but 3 was evicted → resync_required.
        let err = fh
            .resume("kn-ops", &s, 2)
            .expect_err("an out-of-window cursor cannot backfill");
        assert!(
            err.is_resync_required(),
            "the over-window cursor RAISES resync_required (NAMED)"
        );
        if let FirehoseError::ResyncRequired {
            last_seq,
            window_floor,
        } = err
        {
            assert_eq!(last_seq, 2);
            assert_eq!(
                window_floor, 4,
                "the window floor is the oldest held seq (4)"
            );
        } else {
            panic!("expected ResyncRequired");
        }

        // a client at last_seq = 4 (its next-missing op 5 is still held) DOES backfill {5,6} — the
        // boundary case proves the floor check is exact, not over-eager.
        let sub = fh
            .resume("kn-ops", &s, 4)
            .expect("an in-window cursor backfills");
        assert_eq!(
            sub.drain_ready().iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![5, 6]
        );
    }

    /// The exact window-floor boundary: `last_seq` such that the FIRST missing op equals the window
    /// floor is replayable (in-window); one older is `resync_required`. Pins the off-by-one.
    #[test]
    fn window_floor_boundary_is_exact() {
        let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
        let s = scope("board:big");
        for _ in 0..6 {
            fh.publish("issues", &s, draft("row"));
        }
        // window holds {4,5,6}, floor = 4.
        // last_seq = 3 → first missing op = 4 == floor → IN-WINDOW (replays {4,5,6}).
        let sub = fh
            .resume("issues", &s, 3)
            .expect("first-missing == floor is in-window");
        assert_eq!(
            sub.drain_ready().iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![4, 5, 6]
        );
        // last_seq = 2 → first missing op = 3 < floor 4 → resync_required.
        assert!(fh
            .resume("issues", &s, 2)
            .expect_err("first-missing < floor")
            .is_resync_required());
    }

    // ---- an over-broad scope is rejected -------------------------------------------------------

    /// **D-10 SCOPE LEG: the transport REJECTS an over-broad scope (`scope = *`), §4.3.** The
    /// whitelist-not-`*` rule generalised — `*`, `board:*`, an un-prefixed bare id, an unknown
    /// prefix, the empty string are ALL rejected with [`FirehoseError::OverBroadScope`]; only a
    /// bounded `board:`/`doc:`/`channel:<id>` is admitted.
    #[test]
    fn the_transport_rejects_an_over_broad_scope() {
        let mut fh = Firehose::new();

        // the headline fixture: scope = `*` is rejected at the connection-tier subscribe entry.
        let err = fh
            .subscribe_raw("chat-live", "*", None)
            .expect_err("`*` is rejected");
        assert!(
            err.is_over_broad_scope(),
            "scope = * is an over-broad scope (rejected)"
        );

        // every over-broad form is rejected through `subscribe_raw` (the connection-tier entry).
        for raw in [
            "*", "board:*", "doc:a*", "123", "", "   ", "team:eng", "all",
        ] {
            let r = fh.subscribe_raw("chat-live", raw, None);
            assert!(
                r.is_err(),
                "over-broad scope `{raw}` must be rejected at subscribe, got {r:?}"
            );
            assert!(
                r.unwrap_err().is_over_broad_scope(),
                "`{raw}` is an over-broad-scope rejection"
            );
        }

        // a BOUNDED scope through the same entry subscribes fine (the positive control).
        assert!(
            fh.subscribe_raw("chat-live", "channel:eng", None).is_ok(),
            "a bounded scope subscribes"
        );
    }

    /// Re-state the over-broad rejection cleanly (the loop above is awkward with Result) — every
    /// over-broad form is an `Err(OverBroadScope)`, every bounded form parses.
    #[test]
    fn over_broad_scopes_are_all_rejected_bounded_scopes_all_parse() {
        for raw in [
            "*",
            "board:*",
            "doc:a*",
            "channel:*",
            "123",
            "",
            "   ",
            "team:eng",
            "all",
            "board:",
        ] {
            let r = FirehoseScope::parse(raw);
            assert!(
                r.is_err(),
                "over-broad/invalid scope `{raw}` must be rejected, got {r:?}"
            );
            assert!(r.unwrap_err().is_over_broad_scope());
        }
        for raw in ["board:42", "doc:design", "channel:eng", "board:proj-1_x"] {
            let s = FirehoseScope::parse(raw).unwrap_or_else(|_| panic!("`{raw}` must parse"));
            assert_eq!(
                s.selector(),
                raw,
                "a bounded scope round-trips its selector string"
            );
        }
    }

    /// **`run:<run_id>` is admitted as a BOUNDED scope (CI §7.1 / contract 3.5 — the CI live-log
    /// tail).** CI's log lines ride this transport keyed by the run; a viewer subscribes on exactly
    /// one run, never `*` (CI is the heaviest firehose producer). `run:<id>` parses to
    /// [`ScopeKind::Run`] and round-trips, while `run:*` / a bare `run:` are STILL rejected as
    /// over-broad — the `*`-rejection generalises to the new kind exactly as for board/doc/channel.
    #[test]
    fn run_scope_is_a_bounded_kind_and_unbounded_run_is_rejected() {
        let s = FirehoseScope::parse("run:01J0RUN").expect("a bounded run scope parses");
        assert_eq!(s.kind(), ScopeKind::Run, "run: parses to the Run kind");
        assert_eq!(s.id(), "01J0RUN", "the run id is the bounded resource id");
        assert_eq!(
            s.selector(),
            "run:01J0RUN",
            "the run scope round-trips its selector"
        );
        for raw in ["run:*", "run:", "run"] {
            let r = FirehoseScope::parse(raw);
            assert!(
                r.is_err(),
                "unbounded/empty run scope `{raw}` must be rejected, got {r:?}"
            );
            assert!(
                r.unwrap_err().is_over_broad_scope(),
                "`{raw}` is an over-broad-scope rejection"
            );
        }
    }

    /// **`inbox:<principal>` is admitted as a fourth BOUNDED scope (§7 / contract 3.5 C4).** Notif's
    /// `inbox watch` rides the SAME protocol; `inbox:<principal>` parses to [`ScopeKind::Inbox`] and
    /// round-trips, while `inbox:*` / a bare `inbox:` are STILL rejected as over-broad — the
    /// `*`-rejection generalises to the new kind exactly as for board/doc/channel.
    #[test]
    fn inbox_scope_is_a_bounded_kind_and_unbounded_inbox_is_rejected() {
        let s = FirehoseScope::parse("inbox:p-opaque-1").expect("a bounded inbox scope parses");
        assert_eq!(
            s.kind(),
            ScopeKind::Inbox,
            "inbox: parses to the Inbox kind"
        );
        assert_eq!(
            s.id(),
            "p-opaque-1",
            "the principal id is the bounded resource id"
        );
        assert_eq!(
            s.selector(),
            "inbox:p-opaque-1",
            "the inbox scope round-trips its selector"
        );
        // the *-rejection generalises to inbox: an unbounded inbox scope is STILL rejected.
        for raw in ["inbox:*", "inbox:", "inbox"] {
            let r = FirehoseScope::parse(raw);
            assert!(
                r.is_err(),
                "unbounded/empty inbox scope `{raw}` must be rejected, got {r:?}"
            );
            assert!(
                r.unwrap_err().is_over_broad_scope(),
                "`{raw}` is an over-broad-scope rejection"
            );
        }
    }

    // ---- a slow consumer drops to resync_required (no unbounded buffering) ---------------------

    /// **A SLOW consumer is dropped to `resync_required` (§4.3 backpressure) — memory stays bounded,
    /// the gap is NOT buffered.** A subscription with a small in-flight cap; the producer races ahead
    /// while the consumer never pulls → once the in-flight queue hits the cap the subscription is
    /// DROPPED to `resync_required` and its buffer is RELEASED (the slow-consumer drop).
    #[test]
    fn a_slow_consumer_is_dropped_to_resync_required_with_bounded_memory() {
        // in-flight cap 3; window large so ONLY the slow-consumer drop fires.
        let mut fh = Firehose::with_limits(1024, 3);
        let s = scope("channel:firehose");

        let sub = fh.subscribe("chat-live", &s, None).expect("subscribe");
        // the consumer pulls NOTHING; the producer publishes 5 frames.
        for _ in 0..3 {
            fh.publish("chat-live", &s, draft("frame"));
        }
        assert_eq!(sub.ready_len(), 3, "the in-flight queue filled to the cap");
        assert!(
            !sub.resync_required(),
            "not dropped yet (at the cap, not over it)"
        );

        // the 4th frame is OVER the cap → the slow consumer is DROPPED to resync_required.
        fh.publish("chat-live", &s, draft("over-cap"));
        assert!(
            sub.resync_required(),
            "a slow consumer is dropped to resync_required (NAMED)"
        );
        assert_eq!(
            sub.ready_len(),
            0,
            "the buffer is RELEASED — memory bounded, the gap NOT buffered"
        );
        // a pull on a dropped subscription yields nothing (the consumer must resume/*.snapshot).
        assert!(
            sub.pull().is_none(),
            "a dropped subscription delivers nothing until it resumes"
        );
    }

    /// A consumer that KEEPS UP is never dropped: it pulls each frame as it is published, so the
    /// in-flight queue stays near 0 — the happy path (no drop, no resync).
    #[test]
    fn a_keeping_up_consumer_is_never_dropped() {
        let mut fh = Firehose::with_limits(1024, 4);
        let s = scope("channel:eng");
        let sub = fh.subscribe("chat-live", &s, None).expect("subscribe");
        for i in 1..=100u64 {
            fh.publish("chat-live", &s, draft("f"));
            let pulled = sub
                .pull()
                .expect("a keeping-up consumer always has its frame");
            assert_eq!(pulled.seq, i, "delivered in order");
            assert!(
                sub.ready_len() <= 1,
                "the in-flight stays bounded for a keeping-up consumer"
            );
        }
        assert!(
            !sub.resync_required(),
            "a keeping-up consumer is never dropped"
        );
    }

    // ---- tail(range) ---------------------------------------------------------------------------

    /// `tail(stream, scope, [lo, hi])` reads the frames in the seq range the window still holds (the
    /// CI log viewer's `lines N..M`). Out-of-window frames are simply absent (best-effort over the
    /// live window; the durable tail is the T3 log tier).
    #[test]
    fn tail_reads_the_range_the_window_holds() {
        let mut fh = Firehose::new();
        let s = scope("board:logs");
        for _ in 0..10 {
            fh.publish("ci-logs", &s, draft("line"));
        }
        let mid: Vec<u64> = fh.tail("ci-logs", &s, 3, 6).iter().map(|f| f.seq).collect();
        assert_eq!(
            mid,
            vec![3, 4, 5, 6],
            "tail reads the inclusive [lo, hi] range"
        );
        // a range past the head returns only what exists.
        let tail: Vec<u64> = fh
            .tail("ci-logs", &s, 8, 100)
            .iter()
            .map(|f| f.seq)
            .collect();
        assert_eq!(tail, vec![8, 9, 10], "tail clamps to the held frames");
    }

    /// A fan-out publish reaches EVERY open subscription on the `(stream, scope)` (two viewers on the
    /// same hot channel both receive each live frame) — the live delivery property.
    #[test]
    fn publish_fans_out_to_every_open_subscription() {
        let mut fh = Firehose::new();
        let s = scope("channel:town-hall");
        let a = fh.subscribe("chat-live", &s, None).expect("a subscribes");
        let b = fh.subscribe("chat-live", &s, None).expect("b subscribes");
        fh.publish("chat-live", &s, draft("hello"));
        assert_eq!(
            a.drain_ready().iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            b.drain_ready().iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![1],
            "both viewers receive it"
        );
    }

    /// The resume cursor a `SubStream` reports is the last DELIVERED seq — so a client that pulls some
    /// of a backfill, then drops again, resumes from exactly where it left off (the cursor is the
    /// per-frame delivery progress, not the head).
    #[test]
    fn the_resume_cursor_is_the_last_delivered_seq() {
        let mut fh = Firehose::new();
        let s = scope("doc:x");
        for _ in 0..5 {
            fh.publish("kn-ops", &s, draft("op"));
        }
        let sub = fh
            .resume("kn-ops", &s, 0)
            .expect("fresh client replays the window");
        // pull only the first two of the backfill, then "drop".
        assert_eq!(sub.pull().map(|f| f.seq), Some(1));
        assert_eq!(sub.pull().map(|f| f.seq), Some(2));
        assert_eq!(
            sub.last_seq(),
            2,
            "the cursor is the last DELIVERED seq (not the head 5)"
        );
        // resume from 2 → backfill {3,4,5}.
        let sub2 = fh
            .resume("kn-ops", &s, sub.last_seq())
            .expect("resume from the partial cursor");
        assert_eq!(
            sub2.drain_ready().iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
    }
}
