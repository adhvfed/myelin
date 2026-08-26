use std::time::{SystemTime, UNIX_EPOCH};

use myelin_events::Timestamp;

fn unix_seconds_at(time: SystemTime) -> Result<i64, &'static str> {
    let elapsed = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch")?;
    i64::try_from(elapsed.as_secs()).map_err(|_| "system clock exceeds the signed timestamp range")
}

pub(crate) fn unix_seconds() -> i64 {
    unix_seconds_at(SystemTime::now()).expect(
        "identity security requires a system clock at or after the Unix epoch and within i64",
    )
}

pub(crate) fn timestamp() -> Timestamp {
    let now = chrono::DateTime::from_timestamp(unix_seconds(), 0)
        .expect("identity security requires a system clock representable by chrono");
    Timestamp(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_time_refuses_a_clock_before_the_unix_epoch() {
        let before_epoch = UNIX_EPOCH
            .checked_sub(std::time::Duration::from_secs(1))
            .expect("one second before the Unix epoch is representable");

        assert_eq!(
            unix_seconds_at(before_epoch),
            Err("system clock is before the Unix epoch")
        );
        assert_eq!(unix_seconds_at(UNIX_EPOCH), Ok(0));
    }
}
