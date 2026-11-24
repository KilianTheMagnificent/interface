use std::time::Duration;

/// Keep the startup burst and hook-triggered burst responsive, then reduce
/// repeated recovery I/O. This remains a bounded safety poll, never proof that
/// a process exited or that a provider effect did not happen.
pub(super) fn recovery_delay(polls_without_wake: usize) -> Duration {
    if polls_without_wake < super::PROVIDER_SESSION_STARTUP_POLLS {
        return super::PROVIDER_SESSION_STARTUP_POLL_DELAY;
    }
    Duration::from_secs(
        (1 + (polls_without_wake - super::PROVIDER_SESSION_STARTUP_POLLS) / 4).min(5) as u64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_backoff_is_bounded_and_a_wake_resets_the_burst() {
        assert_eq!(recovery_delay(0), Duration::from_millis(100));
        assert_eq!(recovery_delay(19), Duration::from_millis(100));
        assert_eq!(recovery_delay(20), Duration::from_secs(1));
        assert_eq!(recovery_delay(36), Duration::from_secs(5));
        assert_eq!(recovery_delay(usize::MAX), Duration::from_secs(5));
        assert_eq!(recovery_delay(0), Duration::from_millis(100));
    }
}
