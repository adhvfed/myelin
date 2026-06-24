//! # CDC — contract 3.5 firehose retention-window tuning (EB-30 / P-439, M5)
//!
//! **Contract-index:** row 3.5 (the firehose transport + resume-cursor protocol). EB-30 HARDENS this
//! contract by closing the named M2 retention-window floor with the MEASURED per-stream-class numbers.
//!
//! This is the consumer/provider conformance for the retention-window TUNING: the workspace
//! `thresholds.toml` (`[firehose_retention]`) is the versioned SOURCE OF TRUTH for the measured
//! windows; `retention::StreamClass::tuning()` is the in-code expression a `Firehose::for_stream_class`
//! opens a window from. This CDC asserts the two are byte-for-byte in lock-step — so a future
//! hand-edit of the file that drifts from the code (or vice versa) fails HERE, never silently at
//! runtime. It ALSO re-asserts the §4.3 headline invariant (window > p99 reconnect gap, with
//! headroom) from the FILE's numbers, so the measured-data assertion holds on the recorded source of
//! truth, not just the in-code copy.

use myelin_events::{RetentionTuning, StreamClass};
use serde::Deserialize;

/// One `[[firehose_retention]]` row in `thresholds.toml` (the versioned source of truth).
#[derive(Debug, Deserialize)]
struct RetentionRow {
    class: String,
    window_frames: usize,
    p99_reconnect_gap_frames: usize,
}

#[derive(Debug, Deserialize)]
struct ThresholdsFile {
    #[serde(default)]
    firehose_retention: Vec<RetentionRow>,
}

/// Load + parse the workspace-root `thresholds.toml` (the single versioned source of truth, P-S22).
fn load_thresholds() -> ThresholdsFile {
    // tests run with CWD = the crate dir; the workspace root is two levels up (crates/<crate>/).
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("thresholds.toml");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read thresholds.toml at {}: {e}", path.display()));
    toml::from_str(&raw).expect("thresholds.toml parses as the expected shape")
}

/// **CDC: every `retention::StreamClass` has a `[[firehose_retention]]` row in `thresholds.toml`, and
/// the row's numbers EQUAL the in-code `tuning()` measured numbers (the file is the source of truth,
/// kept in lock-step with the code).** A drift in either direction fails here.
#[test]
fn thresholds_file_firehose_retention_matches_the_code_tuning() {
    let file = load_thresholds();
    assert_eq!(
        file.firehose_retention.len(),
        StreamClass::ALL.len(),
        "thresholds.toml must carry exactly one [[firehose_retention]] row per StreamClass"
    );

    for class in StreamClass::ALL {
        let t = class.tuning();
        let row = file
            .firehose_retention
            .iter()
            .find(|r| r.class == class.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "thresholds.toml is missing the [[firehose_retention]] row for `{}`",
                    class.as_str()
                )
            });
        assert_eq!(
            row.window_frames,
            t.window_frames,
            "`{}` window_frames: thresholds.toml ({}) must equal the code tuning ({})",
            class.as_str(),
            row.window_frames,
            t.window_frames,
        );
        assert_eq!(
            row.p99_reconnect_gap_frames,
            t.p99_reconnect_gap_frames,
            "`{}` p99_reconnect_gap_frames: thresholds.toml ({}) must equal the code tuning ({})",
            class.as_str(),
            row.p99_reconnect_gap_frames,
            t.p99_reconnect_gap_frames,
        );
    }
}

/// **CDC: the §4.3 invariant (window EXCEEDS the measured p99 reconnect gap, with headroom) holds on
/// the FILE's recorded numbers — not just the in-code copy.** The recorded source of truth is itself
/// valid; a hand-edit shrinking a recorded window below its recorded p99 gap fails here.
#[test]
fn thresholds_file_windows_exceed_the_recorded_p99_gap_with_headroom() {
    let file = load_thresholds();
    for row in &file.firehose_retention {
        assert!(
            row.window_frames > row.p99_reconnect_gap_frames,
            "`{}`: recorded window {} must EXCEED the recorded p99 reconnect gap {} (§4.3)",
            row.class,
            row.window_frames,
            row.p99_reconnect_gap_frames,
        );
        assert!(
            row.window_frames
                >= row
                    .p99_reconnect_gap_frames
                    .saturating_mul(RetentionTuning::MIN_HEADROOM_X),
            "`{}`: recorded window {} must hold >= {}x the recorded p99 gap {} (the §4.3 \
             comfortably-exceeds posture)",
            row.class,
            row.window_frames,
            RetentionTuning::MIN_HEADROOM_X,
            row.p99_reconnect_gap_frames,
        );
    }
}
