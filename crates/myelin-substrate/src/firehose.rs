//! # The firehose per-connection in-flight frame caps + slow-consumer drop (P-S28 → global P-135)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §7.7 (the firehose resume-cursor seam — the substrate's backpressure role: per-connection
//! in-flight frame caps; a slow consumer dropped to `resync_required`, never buffered unboundedly),
//! §7.1 (bounded-everything generalised to streaming — an unbounded queue is unbounded latency is
//! indistinguishable from down, Little's Law), §10.2 last row (the firehose per-`(stream,scope)`
//! frame-lag + `resync_required` count survival signal), §11 row D-11 (firehose
//! reconnect-loses-zero-ops — the substrate owns the bounded-queue half).
//!
//! **Contract-index:** row 3.5 (the firehose transport + resume-cursor protocol). The PROTOCOL is
//! **Bus-owned** (`subscribe`/`resume`/`scope`, the zero-loss-replay half — landing in P-141/EB-21);
//! THIS module is the substrate's **bounded-and-sheds** half that rides it (§7.7: "the substrate
//! guarantees the bounded-and-sheds half; the Bus guarantees the zero-loss-replay half").
//!
//! ## What this module is (the substrate's stake in 3.5)
//! The firehose carries high-volume, ephemeral-ish frames (CI logs, KN collab op-streams + presence,
//! Chat live delivery + presence + agent partials). A consumer that falls behind must NEVER make the
//! transport grow memory to hold its gap. This module ships the two slices P-S28 names:
//!
//! - **(a) per-connection in-flight frame caps** — [`FrameBuffer`] is a per-subscription bounded
//!   frame queue built on the substrate's one [`crate::shed::BoundedQueue`] primitive (§7.1): a frame
//!   offered over-cap **sheds in the firehose's own bounded queue** ([`PushOutcome::Shed`]) rather
//!   than growing memory. The §7.6 per-surface shed budgets apply to WHICH frames shed first
//!   (presence/speculative before message delivery; agents before humans) — that selector is P-S29's
//!   half; here the cap is the bound and the shed is the over-cap drop.
//! - **(b) slow-consumer drop to `resync_required`** — when a buffer's lag (offered − delivered)
//!   crosses the slow-consumer ceiling, the connection is **dropped to `resync_required`**
//!   ([`PushOutcome::ResyncRequired`] / [`FrameBuffer::resync_required`]): the consumer falls back to
//!   a full `*.snapshot` replay (the cold-rebuild path, **NAMED not silent** — §7.7, contract 3.5's
//!   `resync_required → *.snapshot` fallback) instead of the transport buffering its gap unboundedly.
//!   A dropped buffer holds NO frames (memory is released) and its lag reads `0` — the bounded-memory
//!   guarantee.
//!
//! The per-`(stream,scope)` frame-lag + `resync_required` count are exported into the contract-1.8
//! telemetry signal set ([`FirehoseSignals`]) — the §10.2-last-row survival signals the D-11 drill
//! reads (mapping onto the harness `SignalName::{FirehoseFrameLag, ResyncRequiredCount}`).
//!
//! ## The seam shape (frozen by the architecture, not by the Bus impl)
//! A [`Frame`] is an OPAQUE per-`(stream,scope)`-monotone `seq` plus a [`FrameClass`] (the §7.6 shed
//! priority — presence/speculative shed before message delivery). The substrate's bounded layer is
//! transport-agnostic: it never reads a frame's payload (PII stays out of the backpressure layer) and
//! never needs the Bus's concrete `Frame` type. When the Bus firehose protocol lands (P-141), its
//! `subscribe`/`resume` feeds this bounded buffer; the buffer's `resync_required` verdict is what the
//! Bus turns into a `*.snapshot` replay. The two halves compose at the connection tier (Chat M4).
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - **The Bus-side zero-loss-replay half (`subscribe`/`resume`/`resync_required → *.snapshot`)** is
//!   **P-141 (EB-21)** — the firehose resume-cursor protocol (built FIRST per EI-04 §2.2). This module
//!   is the bounded-and-sheds half; the full D-11 reconnect-loses-zero-ops drill (zero ops lost across
//!   a reconnect) needs BOTH halves and re-proves with the Bus impl. Here the substrate half is proven:
//!   bounded memory + a slow consumer dropped (not buffered) + the survival signals green.
//! - **The scope-bounded selector (reject `*`; board:/doc:/channel: paginated window) + the per-surface
//!   frame shed budgets (which class sheds first)** are **P-S29 (P-136)** — together with this prompt
//!   they complete the substrate's half of D-11. Here [`FirehoseScope`] is the typed selector + the
//!   per-`(stream,scope)` frame-lag key; [`FrameClass`] carries the shed priority the P-S29 selector
//!   reads; the `*`-rejection + paginated-window logic is P-S29's deliverable.
//! - **The real connection tier that opens a [`FrameBuffer`] per subscription + drives delivery** is
//!   Chat M4; the M4 connection-storm re-confirm of this backpressure half is **P-S31 (P-326)**. Here
//!   the buffer is the in-process bounded primitive; the SUB-D11 drill proves it against synthetic load.

use crate::shed::BoundedQueue;

/// **The firehose shed priority class of a frame (§7.6 / §7.7).** A frame's class decides which
/// frames shed FIRST when a buffer is over-cap: presence/speculative frames are ephemeral and shed
/// before durable message delivery; agent-bound frames shed before human-bound frames. The order is
/// the variant order (a LOWER class sheds FIRST), mirroring [`crate::shed::RunClass`].
///
/// The per-surface shed-budget *selector* that consults this class (which fraction of the buffer each
/// class may occupy) is **P-S29's** half; here the class is carried on the [`Frame`] so the buffer can
/// shed the right frames first, and the slow-consumer drop is class-agnostic (a buffer that is *fully*
/// over its slow-consumer ceiling drops the whole connection to `resync_required`, regardless of class).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FrameClass {
    /// Presence / speculative frames (typing indicators, cursor positions, prefetch) — ephemeral,
    /// shed FIRST. A lost presence frame is corrected by the next one; it is never replayed.
    Presence,
    /// Agent-bound delivery (agent partials, agent live frames) — shed before human delivery (§7.6:
    /// agents shed before humans).
    AgentDelivery,
    /// Human-bound message delivery — the frames the product exists to deliver; shed LAST.
    HumanDelivery,
}

impl FrameClass {
    /// Stable lowercase label (for the per-class diagnostics; the survival signals are keyed by
    /// `(stream, scope)`, not class — class governs shed ORDER, P-S29).
    pub fn label(self) -> &'static str {
        match self {
            FrameClass::Presence => "presence",
            FrameClass::AgentDelivery => "agent",
            FrameClass::HumanDelivery => "human",
        }
    }
}

/// **A firehose scope — the bounded selector that BOUNDS which frames arrive (§7.7, contract 3.5).**
///
/// Scope is a bounded selector, **never `*`**: `board:`/`doc:`/`channel:` (a 50k-row board paginates
/// its scope to the visible window + margin). The per-`(stream,scope)` frame-lag survival signal is
/// keyed by this scope. The `*`-rejection + the paginated-window selector logic is **P-S29's** half
/// (P-136); here the type is the seam + the telemetry key, constructed from a bounded selector string.
///
/// Held as the raw selector string (`board:123`, `doc:abc`, `channel:eng`) — a PII-free identifier,
/// so a telemetry label built from it is `control-plane-pii-free` by construction.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FirehoseScope(pub String);

impl FirehoseScope {
    /// The selector string (the `(stream,scope)` telemetry key's scope half).
    pub fn selector(&self) -> &str {
        &self.0
    }
}

/// **One firehose frame, as the substrate's bounded layer sees it (the §7.7 seam).** An OPAQUE
/// per-`(stream,scope)`-monotone `seq` plus its shed [`FrameClass`] — the bounded layer never reads
/// the payload (PII stays out of backpressure; the payload rides the Bus's concrete frame, P-141).
/// `seq` is the resume cursor the Bus replays from; here it is what the buffer reports as last-delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The per-`(stream,scope)` monotonic sequence (the resume cursor; assigned by the Bus, P-141).
    pub seq: u64,
    /// The shed priority class (presence/agent/human) — governs which frames shed first (P-S29).
    pub class: FrameClass,
}

impl Frame {
    /// A frame at `seq` of `class`.
    pub fn new(seq: u64, class: FrameClass) -> Frame {
        Frame { seq, class }
    }
}

/// The outcome of offering a frame to a [`FrameBuffer`] (§7.7 — the bounded-and-sheds verdict).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// The frame was buffered (a slot was free; the consumer will deliver it).
    Buffered,
    /// The buffer was at its per-connection cap — the frame **shed in the firehose's own bounded
    /// queue** (§7.7, §7.1) rather than growing memory. The connection survives (a single over-cap
    /// frame is shed, the consumer keeps consuming); only a SUSTAINED lag drops it (below).
    Shed,
    /// The consumer is too slow — its lag crossed the slow-consumer ceiling, so the connection is
    /// **dropped to `resync_required`** (§7.7, contract 3.5): the buffer is released (memory bounded)
    /// and the consumer must fall back to a full `*.snapshot` replay (the cold-rebuild path, NAMED).
    /// Every subsequent offer also reads this (the connection stays dropped until it re-subscribes).
    ResyncRequired,
}

impl PushOutcome {
    /// `true` iff the frame was buffered (delivered into the bounded queue).
    pub fn is_buffered(self) -> bool {
        matches!(self, PushOutcome::Buffered)
    }

    /// `true` iff the connection was dropped to `resync_required`.
    pub fn is_resync_required(self) -> bool {
        matches!(self, PushOutcome::ResyncRequired)
    }
}

/// **A per-connection in-flight frame buffer (§7.7 (a) + (b); contract 3.5 substrate half).**
///
/// One subscription's bounded frame queue on one firehose `(stream, scope)`. It is the §7.1
/// bounded-everything rule applied to streaming, with the slow-consumer escape hatch §7.7 names:
///
/// - **(a) per-connection cap** — built on the substrate's one [`BoundedQueue`] primitive: at most
///   `capacity` frames are in flight (offered, not yet delivered). A frame offered over-cap
///   [`PushOutcome::Shed`]s in the firehose's own bounded queue — memory never grows past the cap.
/// - **(b) slow-consumer drop** — the buffer tracks per-`(stream,scope)` **frame lag**
///   (`offered_seq − delivered_seq`, the §10.2 survival signal). A SINGLE over-cap shed is survivable;
///   but once the lag crosses `slow_consumer_lag_ceiling` the consumer is structurally too slow, so
///   the buffer **drops the connection to `resync_required`** ([`Self::resync_required`]): it releases
///   every buffered frame (memory → bounded, the lag → `0`) and the consumer falls back to a
///   `*.snapshot` replay. This is the §7.7 "never buffered unboundedly" guarantee made concrete.
///
/// A drill drives a fast producer against a slow consumer and asserts: the frame-lag stays bounded
/// (never grows past the cap before the drop), the connection is dropped to `resync_required` (not
/// buffered), the buffered-frame count after the drop is `0` (memory released), and the
/// `resync_required_count` is accurate.
#[derive(Clone, Debug)]
pub struct FrameBuffer {
    /// The firehose stream this buffer is on (`ci-logs`, `chat-live`, `kn-ops`) — the `(stream,…)`
    /// half of the survival-signal key. A PII-free identifier.
    stream: String,
    /// The bounded scope selector — the `(…,scope)` half of the key (§7.7: never `*`).
    scope: FirehoseScope,
    /// The per-connection in-flight cap (§7.1) — the buffer holds at most this many undelivered
    /// frames; an over-cap offer sheds. Built on the one [`BoundedQueue`] primitive.
    queue: BoundedQueue,
    /// The slow-consumer lag ceiling: once `frame_lag` reaches this, the connection is dropped to
    /// `resync_required` (§7.7). Bounds the gap the transport will ever tolerate before a cold rebuild.
    slow_consumer_lag_ceiling: u64,
    /// The highest frame seq OFFERED to this buffer (the producer side of the lag).
    offered_seq: u64,
    /// The highest frame seq the consumer has DELIVERED (the consumer side of the lag). `frame_lag`
    /// is `offered_seq − delivered_seq` (clamped `>= 0`).
    delivered_seq: u64,
    /// `true` once the connection has been dropped to `resync_required` — it stays dropped (every
    /// subsequent offer reads `ResyncRequired`) until the consumer re-subscribes (a fresh buffer).
    resync_required: bool,
    /// Cumulative count of drops-to-`resync_required` for this buffer (the §10.2 `resync_required`
    /// count survival signal — accurate, NAMED not silent). Increments exactly once per drop.
    resync_required_count: u64,
}

impl FrameBuffer {
    /// Open a per-connection frame buffer on `(stream, scope)` with a per-connection in-flight
    /// `capacity` (§7.1 bound) and a `slow_consumer_lag_ceiling` (§7.7 drop threshold). The ceiling
    /// MUST be `>= capacity` (a connection cannot be "slow" before it has even filled its buffer); a
    /// lower ceiling is raised to `capacity` so the semantics hold.
    pub fn new(
        stream: impl Into<String>,
        scope: FirehoseScope,
        capacity: u32,
        slow_consumer_lag_ceiling: u64,
    ) -> FrameBuffer {
        let capacity = capacity.max(1);
        FrameBuffer {
            stream: stream.into(),
            scope,
            queue: BoundedQueue::new(capacity),
            slow_consumer_lag_ceiling: slow_consumer_lag_ceiling.max(capacity as u64),
            offered_seq: 0,
            delivered_seq: 0,
            resync_required: false,
            resync_required_count: 0,
        }
    }

    /// **Offer a frame to the buffer (§7.7 (a)+(b)).** The producer (the Bus firehose, P-141) calls
    /// this per frame. The verdict:
    /// 1. if already dropped → [`PushOutcome::ResyncRequired`] (the connection stays dropped);
    /// 2. else advance the offered seq + recompute lag; if the lag has reached the slow-consumer
    ///    ceiling → **drop to `resync_required`** (release the buffer, count the drop);
    /// 3. else try to take a per-connection slot; if the bounded queue is full → [`PushOutcome::Shed`]
    ///    (the frame sheds in the firehose's own bounded queue — memory does not grow);
    /// 4. else → [`PushOutcome::Buffered`].
    ///
    /// Frames are assumed offered in seq order (the Bus's per-`(stream,scope)` monotone seq); an
    /// out-of-order or duplicate seq never *lowers* the offered seq (the lag is monotone in the
    /// producer's progress).
    pub fn offer(&mut self, frame: Frame) -> PushOutcome {
        if self.resync_required {
            return PushOutcome::ResyncRequired;
        }
        // advance the producer side of the lag (monotone — a stale/dup seq never rewinds it).
        self.offered_seq = self.offered_seq.max(frame.seq);

        // (b) slow-consumer drop: if the lag has reached the ceiling, the consumer is structurally
        // too slow — drop the WHOLE connection to resync_required rather than buffer its gap.
        if self.frame_lag() >= self.slow_consumer_lag_ceiling {
            self.drop_to_resync();
            return PushOutcome::ResyncRequired;
        }

        // (a) per-connection cap: take a bounded slot, or shed in the firehose's own bounded queue.
        if self.queue.try_acquire() {
            PushOutcome::Buffered
        } else {
            PushOutcome::Shed
        }
    }

    /// **Deliver one buffered frame to the consumer (advance the consumer side of the lag).** The
    /// consumer calls this as it drains a frame: it releases a per-connection slot (so a new frame can
    /// buffer) and advances `delivered_seq` to `frame.seq` (closing the lag). A delivery on a dropped
    /// connection is a no-op (the buffer holds nothing — the consumer is in `*.snapshot` replay).
    pub fn deliver(&mut self, frame: Frame) {
        if self.resync_required {
            return;
        }
        self.queue.release();
        // the consumer's progress is monotone (it never un-delivers a frame).
        self.delivered_seq = self.delivered_seq.max(frame.seq);
    }

    /// **The per-`(stream,scope)` frame lag (§10.2 survival signal):** `offered_seq − delivered_seq`,
    /// clamped `>= 0`. The producer-vs-consumer gap — the bounded-streaming health signal the D-11
    /// drill asserts stays bounded. A drained consumer reads `0`; a dropped connection reads `0` (it
    /// holds no gap — it is in `*.snapshot` replay).
    pub fn frame_lag(&self) -> u64 {
        if self.resync_required {
            return 0;
        }
        self.offered_seq.saturating_sub(self.delivered_seq)
    }

    /// `true` iff the connection has been dropped to `resync_required` (the §7.7 cold-rebuild path).
    /// While `true`, the consumer is doing a full `*.snapshot` replay (Bus side, P-141) — the buffer
    /// holds no frames and grows no memory.
    pub fn resync_required(&self) -> bool {
        self.resync_required
    }

    /// The cumulative `resync_required` drop count for this buffer (the §10.2 survival signal —
    /// accurate, NAMED not silent). One per drop; a re-subscribe opens a fresh buffer at `0`.
    pub fn resync_required_count(&self) -> u64 {
        self.resync_required_count
    }

    /// The current number of buffered (offered-not-yet-delivered) frames — the in-flight memory the
    /// per-connection cap bounds. After a drop-to-`resync_required` this is `0` (memory released).
    pub fn buffered_frames(&self) -> u32 {
        self.queue.in_flight()
    }

    /// The per-connection in-flight cap (§7.1).
    pub fn capacity(&self) -> u32 {
        self.queue.capacity()
    }

    /// The cumulative over-cap shed count (frames shed in the firehose's own bounded queue, §7.1) —
    /// the producer side of the bounded-streaming shed signal.
    pub fn shed_count(&self) -> u64 {
        self.queue.shed_count()
    }

    /// The firehose stream (the `(stream,…)` survival-signal key half).
    pub fn stream(&self) -> &str {
        &self.stream
    }

    /// The bounded scope selector (the `(…,scope)` survival-signal key half).
    pub fn scope(&self) -> &FirehoseScope {
        &self.scope
    }

    /// Drop the connection to `resync_required`: RELEASE every buffered frame (memory → bounded, lag
    /// → 0) and count the drop exactly once. Idempotent — a second call on an already-dropped buffer
    /// does not double-count.
    fn drop_to_resync(&mut self) {
        if self.resync_required {
            return;
        }
        // release the bounded queue's in-flight (memory is freed — the gap is NOT buffered). Release
        // exactly the buffered count (a bounded, deterministic drain — never an open-ended loop).
        for _ in 0..self.queue.in_flight() {
            self.queue.release();
        }
        // a dropped connection holds no gap; the consumer rebuilds from a *.snapshot (Bus, P-141).
        self.delivered_seq = self.offered_seq;
        self.resync_required = true;
        self.resync_required_count += 1;
    }
}

/// **The firehose contract-1.8 survival signals (§10.2 last row; the substrate's producer slice).**
///
/// Aggregates the per-`(stream,scope)` frame-lag + the `resync_required` count across every open
/// [`FrameBuffer`] into the two signals the D-11 drill reads: `firehose_frame_lag` (labelled by
/// `{stream, scope}`) and `resync_required_count` (scalar). The producer side wires this off the real
/// firehose at the connection tier (Chat M4); here a drill snapshots it off the buffers it drove.
///
/// The signal NAMES map onto the harness `SignalName::{FirehoseFrameLag, ResyncRequiredCount}` (the
/// frozen §10.2 set) — the consumer side reads them via the harness telemetry library.
#[derive(Clone, Debug, Default)]
pub struct FirehoseSignals {
    /// Per-`(stream, scope)` frame lag (the §10.2 `firehose_frame_lag`, labelled). One row per buffer.
    pub frame_lag: Vec<FrameLagSample>,
    /// The total `resync_required` drop count across all buffers (the §10.2 `resync_required` count).
    pub resync_required_count: u64,
}

/// One per-`(stream, scope)` frame-lag sample (the labelled §10.2 survival signal).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameLagSample {
    /// The firehose stream (`ci-logs` / `chat-live` / `kn-ops`).
    pub stream: String,
    /// The bounded scope selector (`board:…` / `doc:…` / `channel:…`).
    pub scope: String,
    /// The current frame lag (`offered − delivered`, bounded; `0` on a drained or dropped buffer).
    pub lag: u64,
}

impl FirehoseSignals {
    /// Snapshot the firehose survival signals from a set of open buffers (the producer-side read the
    /// connection tier does per scrape; a drill does it after driving load). Each buffer contributes
    /// one frame-lag row; the `resync_required` counts sum into the scalar.
    pub fn snapshot<'a>(buffers: impl IntoIterator<Item = &'a FrameBuffer>) -> FirehoseSignals {
        let mut frame_lag = Vec::new();
        let mut resync_required_count = 0u64;
        for b in buffers {
            frame_lag.push(FrameLagSample {
                stream: b.stream().to_string(),
                scope: b.scope().selector().to_string(),
                lag: b.frame_lag(),
            });
            resync_required_count += b.resync_required_count();
        }
        FirehoseSignals {
            frame_lag,
            resync_required_count,
        }
    }

    /// The maximum frame lag across all `(stream,scope)` rows — the single number the "frame-lag
    /// stays BOUNDED" drill assertion reads (the bound it must not exceed is the buffers' ceiling).
    pub fn max_frame_lag(&self) -> u64 {
        self.frame_lag.iter().map(|s| s.lag).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(s: &str) -> FirehoseScope {
        FirehoseScope(s.to_string())
    }

    fn human(seq: u64) -> Frame {
        Frame::new(seq, FrameClass::HumanDelivery)
    }

    // ---- (a) per-connection in-flight frame caps: an over-cap offer sheds, never grows memory ------

    /// **An over-cap subscription SHEDS rather than growing memory (§7.7 (a) / §7.1).** A buffer at
    /// its per-connection cap sheds the next frame in the firehose's own bounded queue; the buffered
    /// frame count NEVER exceeds the cap (Little's Law — memory is bounded).
    #[test]
    fn over_cap_subscription_sheds_rather_than_growing_memory() {
        // cap 3, a high slow-consumer ceiling so ONLY the cap (not the slow-consumer drop) fires.
        let mut buf = FrameBuffer::new("chat-live", scope("channel:eng"), 3, 1_000);
        // fill the per-connection cap (the consumer has not delivered any → all stay in flight).
        assert_eq!(buf.offer(human(1)), PushOutcome::Buffered);
        assert_eq!(buf.offer(human(2)), PushOutcome::Buffered);
        assert_eq!(buf.offer(human(3)), PushOutcome::Buffered);
        assert_eq!(buf.buffered_frames(), 3, "the buffer is at its cap");
        // the next frame is OVER cap → it sheds in the firehose's own bounded queue (memory bounded).
        assert_eq!(buf.offer(human(4)), PushOutcome::Shed, "an over-cap frame sheds, never buffers");
        assert_eq!(buf.buffered_frames(), 3, "buffered frames NEVER exceed the cap (Little's Law)");
        assert_eq!(buf.shed_count(), 1, "the shed is counted (the bounded-streaming signal)");
        // delivering a frame frees a slot → a new frame can buffer (the buffer recovers).
        buf.deliver(human(1));
        assert_eq!(buf.buffered_frames(), 2, "delivery freed a slot");
        assert_eq!(buf.offer(human(5)), PushOutcome::Buffered, "a freed slot is reusable");
    }

    // ---- (b) slow-consumer drop to resync_required: bounded memory, NAMED cold rebuild -------------

    /// **A slow consumer is DROPPED to `resync_required` (not buffered unboundedly); memory stays
    /// bounded (§7.7 (b)).** A producer racing ahead of a consumer that never delivers pushes the lag
    /// to the slow-consumer ceiling; the connection is dropped, the buffer is RELEASED (0 frames held),
    /// and the lag reads 0 — the consumer falls back to a `*.snapshot` replay (NAMED, not silent).
    #[test]
    fn slow_consumer_is_dropped_to_resync_required_with_bounded_memory() {
        // cap 4, slow-consumer ceiling 8. The consumer delivers NOTHING (fully stalled).
        let mut buf = FrameBuffer::new("kn-ops", scope("doc:design"), 4, 8);
        // offer frames 1..7: the lag climbs 1..7, all < ceiling 8. The first 4 buffer; 5,6,7 shed
        // (over the per-connection cap) — memory stays at the cap, the lag keeps climbing.
        for seq in 1..=7u64 {
            let out = buf.offer(human(seq));
            assert!(
                matches!(out, PushOutcome::Buffered | PushOutcome::Shed),
                "below the slow-consumer ceiling a frame buffers or sheds, never drops yet: seq={seq} {out:?}"
            );
        }
        assert!(!buf.resync_required(), "not dropped yet (lag 7 < ceiling 8)");
        assert_eq!(buf.frame_lag(), 7, "the frame-lag tracks the producer-vs-consumer gap");
        assert_eq!(buf.buffered_frames(), 4, "memory bounded at the cap even as the lag climbs");

        // frame 8: the lag reaches the ceiling (8) → the SLOW CONSUMER is dropped to resync_required.
        let out = buf.offer(human(8));
        assert_eq!(out, PushOutcome::ResyncRequired, "a slow consumer is dropped to resync_required");
        assert!(buf.resync_required(), "the connection is dropped (the cold-rebuild path, NAMED)");
        // MEMORY IS BOUNDED: the dropped buffer holds NOTHING (it did not buffer the gap).
        assert_eq!(buf.buffered_frames(), 0, "a dropped connection releases its buffer (bounded memory)");
        assert_eq!(buf.frame_lag(), 0, "a dropped connection holds no gap (it is in *.snapshot replay)");
        assert_eq!(buf.resync_required_count(), 1, "the resync_required count is accurate (one drop)");
    }

    /// Once dropped, the connection STAYS dropped (every subsequent offer reads `resync_required`) and
    /// the count never double-increments — the consumer must re-subscribe (a fresh buffer) to recover.
    #[test]
    fn a_dropped_connection_stays_dropped_and_counts_the_drop_once() {
        let mut buf = FrameBuffer::new("ci-logs", scope("board:42"), 2, 3);
        // climb to the ceiling: offer 1,2 (buffer), 3 (lag 3 == ceiling → drop).
        assert_eq!(buf.offer(human(1)), PushOutcome::Buffered);
        assert_eq!(buf.offer(human(2)), PushOutcome::Buffered);
        assert_eq!(buf.offer(human(3)), PushOutcome::ResyncRequired);
        assert_eq!(buf.resync_required_count(), 1);
        // every subsequent offer also reads resync_required (the connection stays dropped) and does
        // NOT re-increment the count (one drop per connection life).
        assert_eq!(buf.offer(human(4)), PushOutcome::ResyncRequired);
        assert_eq!(buf.offer(human(5)), PushOutcome::ResyncRequired);
        assert_eq!(buf.resync_required_count(), 1, "the drop is counted EXACTLY once per connection");
        assert_eq!(buf.buffered_frames(), 0, "memory stays released");
        // a delivery on a dropped connection is a no-op (the consumer is in *.snapshot replay).
        buf.deliver(human(2));
        assert_eq!(buf.buffered_frames(), 0);
    }

    /// **A consumer that keeps up is NEVER dropped: the lag stays bounded and no resync fires.** The
    /// happy path — deliver each frame right after it is offered → lag oscillates near 0, far below the
    /// ceiling, no shed, no drop.
    #[test]
    fn a_keeping_up_consumer_is_never_dropped_and_lag_stays_bounded() {
        let mut buf = FrameBuffer::new("chat-live", scope("channel:eng"), 4, 8);
        for seq in 1..=100u64 {
            assert_eq!(buf.offer(human(seq)), PushOutcome::Buffered, "a keeping-up consumer never sheds");
            buf.deliver(human(seq));
            assert!(buf.frame_lag() <= 1, "lag stays bounded (~0) for a keeping-up consumer");
        }
        assert!(!buf.resync_required(), "a keeping-up consumer is never dropped");
        assert_eq!(buf.resync_required_count(), 0);
        assert_eq!(buf.shed_count(), 0, "no shed on the happy path");
    }

    /// The slow-consumer ceiling is raised to at least the cap (a connection cannot be "slow" before
    /// it has even filled its buffer) — a degenerate ceiling below the cap does not pre-drop a healthy
    /// connection that is just filling its cap.
    #[test]
    fn slow_consumer_ceiling_is_never_below_the_cap() {
        // ask for ceiling 1 with cap 5 → the ceiling is raised to 5 (a connection is never 'slow'
        // before its buffer is full). So frames with lag 1..4 buffer; the drop fires once the lag
        // reaches the raised ceiling (5), never before the cap is reached.
        let mut buf = FrameBuffer::new("kn-ops", scope("doc:x"), 5, 1);
        // lag 1..4 (< raised ceiling 5) → the first four frames buffer, no pre-drop.
        for seq in 1..=4u64 {
            assert_eq!(buf.offer(human(seq)), PushOutcome::Buffered, "seq {seq} must buffer, not pre-drop");
        }
        assert!(!buf.resync_required(), "a healthy connection filling its cap is NOT 'slow'");
        // seq 5: lag reaches the raised ceiling (5 == cap) → the drop fires — never BEFORE the cap.
        assert_eq!(
            buf.offer(human(5)),
            PushOutcome::ResyncRequired,
            "the drop fires once the lag reaches the cap-raised ceiling, never before the cap"
        );
        assert!(buf.resync_required());
    }

    // ---- the §10.2 survival signals: frame-lag bounded + resync_required count accurate -----------

    /// **The firehose survival signals snapshot the per-`(stream,scope)` frame-lag + the
    /// `resync_required` count (§10.2 last row).** Two buffers — one keeping up (lag bounded, no drop),
    /// one slow (dropped to resync) — snapshot into the two signals the D-11 drill reads.
    #[test]
    fn firehose_signals_export_frame_lag_and_resync_required_count() {
        let mut fast = FrameBuffer::new("chat-live", scope("channel:fast"), 4, 8);
        let mut slow = FrameBuffer::new("chat-live", scope("channel:slow"), 4, 8);

        // fast keeps up (lag ~0); slow stalls and is dropped to resync.
        for seq in 1..=3u64 {
            fast.offer(human(seq));
            fast.deliver(human(seq));
        }
        for seq in 1..=8u64 {
            slow.offer(human(seq)); // never delivers → climbs to the ceiling → dropped at seq 8
        }
        assert!(slow.resync_required());

        let sig = FirehoseSignals::snapshot([&fast, &slow]);
        // the frame-lag signal carries one (stream,scope) row per buffer, both BOUNDED.
        assert_eq!(sig.frame_lag.len(), 2);
        assert!(sig.max_frame_lag() <= 8, "every (stream,scope) frame-lag is BOUNDED by the ceiling");
        // the fast row reads ~0; the slow row reads 0 (dropped → no gap held).
        let fast_row = sig.frame_lag.iter().find(|r| r.scope == "channel:fast").unwrap();
        assert!(fast_row.lag <= 1, "the keeping-up scope's lag is ~0");
        // the resync_required count is accurate: exactly one drop across the two buffers.
        assert_eq!(sig.resync_required_count, 1, "the resync_required count is accurate + NAMED");
    }

    /// The accessor surface reads back exactly what the buffer holds — the `(stream, scope)` key, the
    /// per-connection cap, the [`PushOutcome`] predicates, and the max-frame-lag fold. These are the
    /// thin reads the connection tier + the survival-signal scrape rely on, so they are pinned.
    #[test]
    fn accessors_read_back_the_buffer_state_exactly() {
        let mut buf = FrameBuffer::new("ci-logs", scope("board:7"), 5, 9);
        // the (stream, scope) key + the cap read back exactly.
        assert_eq!(buf.stream(), "ci-logs", "the stream key reads back exactly");
        assert_eq!(buf.scope().selector(), "board:7", "the scope selector reads back exactly");
        assert_eq!(buf.capacity(), 5, "the per-connection cap reads back exactly");

        // is_buffered / is_resync_required reflect the outcome precisely.
        let buffered = buf.offer(human(1));
        assert!(buffered.is_buffered(), "a buffered frame reads is_buffered() == true");
        assert!(!buffered.is_resync_required(), "a buffered frame is NOT resync_required");

        // drive a drop and assert the resync predicate flips (and is_buffered does not).
        for seq in 2..=9u64 {
            buf.offer(human(seq));
        }
        assert!(buf.resync_required(), "the buffer dropped to resync");
        let dropped = buf.offer(human(10));
        assert!(dropped.is_resync_required(), "a dropped offer reads is_resync_required() == true");
        assert!(!dropped.is_buffered(), "a dropped offer is NOT buffered");

        // max_frame_lag folds the rows to the largest lag (0 when empty, the max otherwise).
        assert_eq!(FirehoseSignals::default().max_frame_lag(), 0, "an empty signal set has 0 max lag");
        let mut a = FrameBuffer::new("s", scope("doc:a"), 8, 16);
        let mut b = FrameBuffer::new("s", scope("doc:b"), 8, 16);
        for seq in 1..=3u64 {
            a.offer(human(seq)); // a lags 3 (never delivers)
        }
        b.offer(human(1));
        b.deliver(human(1)); // b lags 0
        let sig = FirehoseSignals::snapshot([&a, &b]);
        assert_eq!(sig.max_frame_lag(), 3, "max_frame_lag is the LARGEST (stream,scope) lag, not 0/1");
    }

    /// The frame shed CLASS order is presence → agent → human (a lower class sheds first; the P-S29
    /// selector reads this). Asserts the seam the next prompt builds on is the right shape.
    #[test]
    fn frame_class_shed_order_is_presence_then_agent_then_human() {
        assert!(FrameClass::Presence < FrameClass::AgentDelivery);
        assert!(FrameClass::AgentDelivery < FrameClass::HumanDelivery);
        assert_eq!(FrameClass::Presence.label(), "presence");
        assert_eq!(FrameClass::HumanDelivery.label(), "human");
    }
}
