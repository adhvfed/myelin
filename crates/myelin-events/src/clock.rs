use std::time::{SystemTime, UNIX_EPOCH};

use crate::Timestamp;

pub const MAX_RFC3339_UNIX_SECONDS: i64 = 253_402_300_799;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockReading {
    unix_seconds: i64,
    timestamp: Timestamp,
}

impl ClockReading {
    pub fn unix_seconds(&self) -> i64 {
        self.unix_seconds
    }

    pub fn timestamp(&self) -> Timestamp {
        self.timestamp.clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockError {
    BeforeUnixEpoch,
    OutsideRfc3339,
}

impl core::fmt::Display for ClockError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BeforeUnixEpoch => formatter.write_str("system clock is before the Unix epoch"),
            Self::OutsideRfc3339 => {
                formatter.write_str("system clock is outside the supported RFC 3339 range")
            }
        }
    }
}

impl std::error::Error for ClockError {}

pub fn system_clock_reading() -> Result<ClockReading, ClockError> {
    clock_reading_at(SystemTime::now())
}

pub fn clock_reading_at(time: SystemTime) -> Result<ClockReading, ClockError> {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ClockError::BeforeUnixEpoch)?
        .as_secs();
    let seconds = i64::try_from(seconds).map_err(|_| ClockError::OutsideRfc3339)?;
    clock_reading_from_unix(seconds)
}

pub fn clock_reading_from_unix(unix_seconds: i64) -> Result<ClockReading, ClockError> {
    if !(0..=MAX_RFC3339_UNIX_SECONDS).contains(&unix_seconds) {
        return Err(if unix_seconds < 0 {
            ClockError::BeforeUnixEpoch
        } else {
            ClockError::OutsideRfc3339
        });
    }

    Ok(ClockReading {
        unix_seconds,
        timestamp: Timestamp(format_rfc3339(unix_seconds as u64)),
    })
}

fn format_rfc3339(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let remainder = seconds % 86_400;
    let (hour, minute, second) = (remainder / 3_600, (remainder % 3_600) / 60, remainder % 60);
    let shifted_days = days + 719_468;
    let era = shifted_days.div_euclid(146_097);
    let day_of_era = shifted_days.rem_euclid(146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_reading_has_consistent_unix_and_rfc3339_forms() {
        let reading = clock_reading_from_unix(1_722_470_400).unwrap();
        assert_eq!(reading.unix_seconds(), 1_722_470_400);
        assert_eq!(reading.timestamp().0, "2024-08-01T00:00:00Z");
    }

    #[test]
    fn rollback_and_unrepresentable_time_fail_closed() {
        assert_eq!(
            clock_reading_at(UNIX_EPOCH - Duration::from_secs(1)),
            Err(ClockError::BeforeUnixEpoch)
        );
        assert_eq!(
            clock_reading_from_unix(MAX_RFC3339_UNIX_SECONDS + 1),
            Err(ClockError::OutsideRfc3339)
        );
    }

    #[test]
    fn the_supported_rfc3339_boundaries_are_exact() {
        assert_eq!(
            clock_reading_from_unix(0).unwrap().timestamp().0,
            "1970-01-01T00:00:00Z"
        );
        assert_eq!(
            clock_reading_from_unix(MAX_RFC3339_UNIX_SECONDS)
                .unwrap()
                .timestamp()
                .0,
            "9999-12-31T23:59:59Z"
        );
    }
}
