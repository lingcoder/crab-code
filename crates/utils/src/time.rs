use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch. Returns 0 if the system clock is set
/// before the epoch.
#[must_use]
pub fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Milliseconds since the Unix epoch. Returns 0 if the system clock is set
/// before the epoch; saturates at `u64::MAX`.
#[must_use]
pub fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_secs_is_after_2020() {
        // 2020-01-01T00:00:00Z
        assert!(epoch_secs() > 1_577_836_800);
    }

    #[test]
    fn epoch_millis_consistent_with_secs() {
        let secs = epoch_secs();
        let millis = epoch_millis();
        // Allow a couple of seconds of skew between the two calls.
        assert!(millis / 1000 >= secs);
        assert!(millis / 1000 - secs < 5);
    }
}
