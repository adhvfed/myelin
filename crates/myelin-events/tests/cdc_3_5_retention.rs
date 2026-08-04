use myelin_events::{RetentionTuning, StreamClass};
use serde::Deserialize;

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

fn load_thresholds() -> ThresholdsFile {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("thresholds.toml");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read thresholds.toml at {}: {e}", path.display()));
    toml::from_str(&raw).expect("thresholds.toml parses as the expected shape")
}

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
