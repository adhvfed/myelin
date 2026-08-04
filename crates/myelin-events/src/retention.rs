#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StreamClass {
    CiLog,
    CollabOp,
    ChatLive,
}

impl StreamClass {
    pub const ALL: [StreamClass; 3] = [
        StreamClass::CiLog,
        StreamClass::CollabOp,
        StreamClass::ChatLive,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            StreamClass::CiLog => "ci_log",
            StreamClass::CollabOp => "collab_op",
            StreamClass::ChatLive => "chat_live",
        }
    }

    pub fn tuning(self) -> RetentionTuning {
        match self {
            StreamClass::CiLog => RetentionTuning {
                class: self,
                window_frames: 49_152,
                p99_reconnect_gap_frames: 12_288,
            },
            StreamClass::CollabOp => RetentionTuning {
                class: self,
                window_frames: 8_192,
                p99_reconnect_gap_frames: 2_048,
            },
            StreamClass::ChatLive => RetentionTuning {
                class: self,
                window_frames: 2_048,
                p99_reconnect_gap_frames: 512,
            },
        }
    }

    pub fn window_frames(self) -> usize {
        self.tuning().window_frames
    }

    pub fn p99_reconnect_gap_frames(self) -> usize {
        self.tuning().p99_reconnect_gap_frames
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionTuning {
    pub class: StreamClass,
    pub window_frames: usize,
    pub p99_reconnect_gap_frames: usize,
}

impl RetentionTuning {
    pub const MIN_HEADROOM_X: usize = 4;

    pub fn window_exceeds_p99_gap(&self) -> bool {
        self.window_frames > self.p99_reconnect_gap_frames
    }

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

    #[test]
    fn every_class_window_exceeds_its_measured_p99_reconnect_gap_with_headroom() {
        for class in StreamClass::ALL {
            let t = class.tuning();
            assert_eq!(t.class, class, "the tuning is for its own class");
            assert!(
                t.window_exceeds_p99_gap(),
                "{}: window {} must EXCEED the measured p99 reconnect gap {} (§4.3 - else a routine \
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
