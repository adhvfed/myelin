#[cfg(test)]
use std::time::SystemTime;

use myelin_events::clock::system_clock_reading;
#[cfg(test)]
use myelin_events::clock::{clock_reading_at, ClockError};
use myelin_events::Timestamp;

#[cfg(test)]
fn unix_seconds_at(time: SystemTime) -> Result<i64, ClockError> {
    clock_reading_at(time).map(|reading| reading.unix_seconds())
}

pub(crate) fn unix_seconds() -> i64 {
    system_clock_reading()
        .expect("identity security requires a representable system clock")
        .unix_seconds()
}

pub(crate) fn timestamp() -> Timestamp {
    system_clock_reading()
        .expect("identity security requires a representable system clock")
        .timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn security_time_refuses_a_clock_before_the_unix_epoch() {
        let before_epoch = UNIX_EPOCH
            .checked_sub(std::time::Duration::from_secs(1))
            .expect("one second before the Unix epoch is representable");

        assert_eq!(
            unix_seconds_at(before_epoch),
            Err(ClockError::BeforeUnixEpoch)
        );
        assert_eq!(unix_seconds_at(UNIX_EPOCH), Ok(0));
    }
}
