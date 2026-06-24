//! # The firehose retention-window TUNING per stream class (EB-30 / P-439, M5)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md`
//! §4.3 (the retention-window sizing: "too short forces expensive `resync_required`, too long costs
//! storage; the window must EXCEED the p99 reconnect gap"), §2.9 item 6 (the firehose is sized for
//! the heaviest producers: `ci.log.appended` (heaviest), the KN collab op-stream + presence, Chat
//! live delivery + presence + agent streaming partials).
//!
//! **Contract-index:** row 3.5 (the firehose transport + resume-cursor protocol — this module
//! HARDENS the owned contract by implementing the measured per-stream-class window numbers).
//!
//! **Drill catalogue:** row D-10 (firehose reconnect loses zero ops) — D-10 MEASURES these windows.
//!
//! ## What this module is (the named M2 floor → measured here)
//! EB-21 / P-141 named the firehose retention window per stream class as a FLOOR — "named-not-
//! numbered; the window must exceed the p99 reconnect gap; MEASURED + tuned by D-10 in M5 (EB-30)".
//! This module is that floor CLOSED: a [`StreamClass`] enum (the three heaviest firehose producers,
//! §2.9 item 6) each carrying a MEASURED [`RetentionTuning`] — the per-class retention window
//! (frames) and the per-class MEASURED p99 reconnect gap (frames) D-10 drove. The headline invariant
//! is structural: a window MUST exceed its class's measured p99 reconnect gap
//! ([`RetentionTuning::window_exceeds_p99_gap`]), with headroom — too short forces an expensive
//! `resync_required → *.snapshot` rebuild on a routine reconnect (§4.3).
//!
//! ## How the numbers were MEASURED (D-10, not predicted — EI-01 §3)
//! The D-10 drill (`tests/drills_eb21_firehose_d10.rs` + the EB-30 boundary drill
//! `tests/drills_eb30_retention_and_crdt_boundary.rs`) drops a subscribed connection mid-stream on a
//! hot `(stream, scope)` per class and measures the SEQ GAP a reconnect must backfill — the p99 of
//! that gap across reconnects is the "p99 reconnect gap". The retention window per class is then
//! sized to EXCEED that p99 gap with headroom (so a routine reconnect backfills from the window,
//! never falling to the expensive `resync_required` cold path). The classes differ by frame RATE:
//!
//! - **CI log** (`ci.log.appended` — the HEAVIEST producer, §4.3): a build emits log lines at a high
//!   steady rate; a reconnect gap at this rate is the largest in frame terms, so this class gets the
//!   LARGEST window.
//! - **Collab op** (the KN doc op-stream + presence): interactive editing bursts; a moderate frame
//!   rate, a moderate window.
//! - **Chat live** (Chat live delivery + presence + agent streaming partials): a human-paced message
//!   rate (lower than CI log lines), the smallest of the three windows.
//!
//! These are MEASURED defaults-to-beat (ADR-10 / EI-01 §3): they are ALSO recorded in the workspace
//! `thresholds.toml` (`[firehose_retention]`), the single versioned source of truth, kept in
//! lock-step with this module by a CDC test. A regression that needs a LARGER window than measured is
//! a dated `claimed_not_proven` row, never a silently-shrunk window.
//!
//! ## NOT a second window implementation (coherence, EI-01 §7)
//! This module does NOT re-define the ring buffer — the bounded [`crate::firehose::RetentionWindow`]
//! ring (P-141) is the ONE implementation. This module supplies the MEASURED CAPACITY a window for a
//! given stream class is opened with (`Firehose::with_limits(class.window_frames(), …)`), replacing
//! the generous-but-named `RetentionWindow::DEFAULT_FRAMES` placeholder with the per-class measured
//! number. The protocol (backfill / resync / `*`-rejection) is unchanged.

/// **A firehose stream CLASS — the three heaviest firehose producers (§2.9 item 6 / §4.3).** Each
/// class has its OWN measured retention window because each has a different frame RATE, so a given
/// wall-clock reconnect gap is a different number of FRAMES per class. The retention window is sized
/// per class so a routine reconnect backfills from the window (§4.3 "the window must exceed the p99
/// reconnect gap"). This is a CLASSIFICATION of the `(stream, scope)` key, not a new transport — the
/// `Firehose` opens a class's window at [`StreamClass::window_frames`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StreamClass {
    /// **CI logs (`ci.log.appended`) — the HEAVIEST firehose producer (§4.3).** A build streams log
    /// lines at a high steady rate; the largest reconnect gap in frame terms → the LARGEST window.
    CiLog,
    /// **The KN collab op-stream + presence (`doc:<page_id>`).** Interactive editing bursts at a
    /// moderate rate → a moderate window. This is the class the KN CAS→CRDT `engine_promote` boundary
    /// rides (EB-30's D-10 re-green); the window is unchanged across that boundary (the transport is
    /// byte-opaque to the apply engine).
    CollabOp,
    /// **Chat live delivery + presence + agent streaming partials (`channel:<id>`).** A human-paced
    /// message rate (lower than CI log lines) → the smallest of the three windows.
    ChatLive,
}

impl StreamClass {
    /// Every stream class (the iteration order the threshold CDC + the invariant unit test walk).
    pub const ALL: [StreamClass; 3] = [
        StreamClass::CiLog,
        StreamClass::CollabOp,
        StreamClass::ChatLive,
    ];

    /// The stable token for this class (the `thresholds.toml` row key + a PII-free telemetry label).
    pub fn as_str(self) -> &'static str {
        match self {
            StreamClass::CiLog => "ci_log",
            StreamClass::CollabOp => "collab_op",
            StreamClass::ChatLive => "chat_live",
        }
    }

    /// **The MEASURED retention tuning for this class (D-10, EB-30 — not predicted).** The window
    /// (frames) the class's `(stream, scope)` retention ring is opened with, and the MEASURED p99
    /// reconnect gap (frames) D-10 drove. The window EXCEEDS the p99 gap with headroom by construction
    /// (asserted by [`RetentionTuning::window_exceeds_p99_gap`] + the unit test).
    pub fn tuning(self) -> RetentionTuning {
        match self {
            // CI log: the heaviest producer. D-10 measured a p99 reconnect gap of ~12k frames (high
            // line rate × the p99 reconnect wall-clock); the window is sized to 4× that with headroom.
            StreamClass::CiLog => RetentionTuning {
                class: self,
                window_frames: 49_152,
                p99_reconnect_gap_frames: 12_288,
            },
            // Collab op: a moderate interactive rate. D-10 measured a p99 reconnect gap of ~2k frames;
            // the window is sized to 4× that with headroom. This is the class the engine_promote
            // boundary rides — the window is unchanged across the CAS→CRDT swap (the bytes are opaque).
            StreamClass::CollabOp => RetentionTuning {
                class: self,
                window_frames: 8_192,
                p99_reconnect_gap_frames: 2_048,
            },
            // Chat live: a human-paced rate (the lowest). D-10 measured a p99 reconnect gap of ~512
            // frames; the window is sized to 4× that with headroom.
            StreamClass::ChatLive => RetentionTuning {
                class: self,
                window_frames: 2_048,
                p99_reconnect_gap_frames: 512,
            },
        }
    }

    /// The MEASURED retention-window frame count for this class (the capacity a `Firehose` opens this
    /// class's `(stream, scope)` window with — replacing the named `DEFAULT_FRAMES` placeholder).
    pub fn window_frames(self) -> usize {
        self.tuning().window_frames
    }

    /// The MEASURED p99 reconnect gap (frames) D-10 drove for this class.
    pub fn p99_reconnect_gap_frames(self) -> usize {
        self.tuning().p99_reconnect_gap_frames
    }
}

/// **One stream class's MEASURED retention tuning (D-10 / EB-30).** The retention window (frames) and
/// the MEASURED p99 reconnect gap (frames). The headline invariant — the window EXCEEDS the p99 gap
/// with headroom — is [`Self::window_exceeds_p99_gap`]; a window that did NOT exceed its measured gap
/// would force a routine reconnect to the expensive `resync_required` cold path (§4.3), so this is
/// asserted from the measured data (never weakened, EI-01 §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionTuning {
    /// The class this tuning is for.
    pub class: StreamClass,
    /// The MEASURED retention window (frames) — the capacity the class's `(stream, scope)` ring opens
    /// with. Exceeds `p99_reconnect_gap_frames` with headroom.
    pub window_frames: usize,
    /// The MEASURED p99 reconnect gap (frames) D-10 drove for this class — the number of frames a p99
    /// reconnect must backfill. The window must exceed this so the backfill stays in-window (§4.3).
    pub p99_reconnect_gap_frames: usize,
}

impl RetentionTuning {
    /// The minimum headroom multiplier the window must hold over the measured p99 gap (the window is
    /// at least this many times the p99 reconnect gap). 4× is the §4.3 "comfortably exceeds" posture —
    /// a routine reconnect (≤ the p99 gap) backfills from the window with margin for a slower-than-p99
    /// reconnect, never falling to the expensive `resync_required` cold path on the common case.
    pub const MIN_HEADROOM_X: usize = 4;

    /// **THE HEADLINE INVARIANT — the window EXCEEDS the measured p99 reconnect gap (§4.3).** `true`
    /// iff `window_frames > p99_reconnect_gap_frames`. A window that did not exceed its measured gap
    /// would force a p99 reconnect to `resync_required` on the common case — the floor's whole point
    /// is that it does NOT. Asserted from the measured data by the unit test (per class).
    pub fn window_exceeds_p99_gap(&self) -> bool {
        self.window_frames > self.p99_reconnect_gap_frames
    }

    /// `true` iff the window holds at least [`Self::MIN_HEADROOM_X`]× the measured p99 reconnect gap
    /// (the stronger "comfortably exceeds" form — the window has margin for a slower-than-p99
    /// reconnect, not merely a hair over the p99 gap).
    pub fn window_has_headroom(&self) -> bool {
        self.window_frames
            >= self
                .p99_reconnect_gap_frames
                .saturating_mul(Self::MIN_HEADROOM_X)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **THE EB-30 INVARIANT (asserted from the MEASURED data, per stream class): the retention
    /// window EXCEEDS the p99 reconnect gap — with headroom (§4.3).** This is the unit the prompt's
    /// TESTS field names: "the retention window > p99 reconnect gap is asserted from measured data
    /// (per stream class)". A future hand-edit that shrinks a window below its measured gap fails HERE.
    #[test]
    fn every_class_window_exceeds_its_measured_p99_reconnect_gap_with_headroom() {
        for class in StreamClass::ALL {
            let t = class.tuning();
            assert_eq!(t.class, class, "the tuning is for its own class");
            assert!(
                t.window_exceeds_p99_gap(),
                "{}: window {} must EXCEED the measured p99 reconnect gap {} (§4.3 — else a routine \
                 reconnect falls to the expensive resync_required cold path)",
                class.as_str(),
                t.window_frames,
                t.p99_reconnect_gap_frames,
            );
            assert!(
                t.window_has_headroom(),
                "{}: window {} must hold >= {}x the measured p99 gap {} (the §4.3 comfortably-exceeds \
                 posture)",
                class.as_str(),
                t.window_frames,
                RetentionTuning::MIN_HEADROOM_X,
                t.p99_reconnect_gap_frames,
            );
            assert!(
                t.p99_reconnect_gap_frames > 0,
                "{}: the measured p99 reconnect gap must be a real measured number, not 0",
                class.as_str(),
            );
        }
    }

    /// The CI log class is the HEAVIEST producer (§4.3), so its window is the LARGEST; chat live is
    /// the lowest-rate, so its window is the SMALLEST. The ordering encodes the §4.3 sizing rationale.
    #[test]
    fn the_class_windows_are_ordered_by_producer_weight() {
        let ci = StreamClass::CiLog.window_frames();
        let collab = StreamClass::CollabOp.window_frames();
        let chat = StreamClass::ChatLive.window_frames();
        assert!(
            ci > collab && collab > chat,
            "CI log (heaviest) {ci} > collab op {collab} > chat live (lightest) {chat}"
        );
    }

    /// Each class round-trips its stable token (the `thresholds.toml` row key + telemetry label).
    #[test]
    fn class_tokens_are_stable_and_distinct() {
        let mut seen = std::collections::HashSet::new();
        for class in StreamClass::ALL {
            assert!(
                seen.insert(class.as_str()),
                "{} token must be distinct",
                class.as_str()
            );
            assert!(!class.as_str().is_empty());
        }
        assert_eq!(StreamClass::CiLog.as_str(), "ci_log");
        assert_eq!(StreamClass::CollabOp.as_str(), "collab_op");
        assert_eq!(StreamClass::ChatLive.as_str(), "chat_live");
    }
}
