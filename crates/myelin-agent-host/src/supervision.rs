use std::time::Duration;

/// A tiny bounded backoff for durable worker polling.
///
/// Successful contact restores normal responsiveness. Consecutive infrastructure
/// failures back off without ever making recovery feel abandoned.
#[derive(Clone, Debug)]
pub struct PollBackoff {
    baseline: Duration,
    maximum: Duration,
    next: Duration,
}

impl PollBackoff {
    pub fn new(baseline: Duration, maximum: Duration) -> Self {
        assert!(!baseline.is_zero(), "poll baseline must be positive");
        assert!(maximum >= baseline, "poll maximum must cover its baseline");
        Self {
            baseline,
            maximum,
            next: baseline,
        }
    }

    pub fn next_delay(&self) -> Duration {
        self.next
    }

    pub fn succeeded(&mut self) {
        self.next = self.baseline;
    }

    pub fn failed(&mut self) {
        self.next = self.next.saturating_mul(2).min(self.maximum);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_polling_is_prompt_and_repeated_failures_are_bounded() {
        let mut backoff = PollBackoff::new(Duration::from_millis(100), Duration::from_secs(5));
        assert_eq!(backoff.next_delay(), Duration::from_millis(100));

        for expected in [200, 400, 800, 1_600, 3_200, 5_000, 5_000] {
            backoff.failed();
            assert_eq!(backoff.next_delay(), Duration::from_millis(expected));
        }

        backoff.succeeded();
        assert_eq!(backoff.next_delay(), Duration::from_millis(100));
    }
}
