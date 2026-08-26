use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::CliError;

pub(crate) fn unix_seconds() -> Result<i64, CliError> {
    unix_seconds_at(SystemTime::now())
}

fn unix_seconds_at(time: SystemTime) -> Result<i64, CliError> {
    let elapsed = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CliError::Config("the local system clock is before the Unix epoch".into()))?;
    i64::try_from(elapsed.as_secs()).map_err(|_| {
        CliError::Config("the local system clock is outside the supported timestamp range".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_clock_before_the_unix_epoch_is_unavailable_not_1970() {
        let before_epoch = UNIX_EPOCH
            .checked_sub(Duration::from_secs(1))
            .expect("the platform represents the rollback fixture");

        let error = unix_seconds_at(before_epoch).unwrap_err();
        assert!(
            matches!(error, CliError::Config(message) if message.contains("before the Unix epoch"))
        );
        assert_eq!(unix_seconds_at(UNIX_EPOCH), Ok(0));
    }
}
