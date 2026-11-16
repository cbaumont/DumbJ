use std::time::Duration;

const FIRST: Duration = Duration::from_secs(1);
const CAP: Duration = Duration::from_secs(60);
const MAX_FAILURES: u32 = 5;

/// Restart delays for a server that keeps dying: 1 s doubling to 60 s, then give up.
#[derive(Debug, Clone, Default)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a failure and returns how long to wait before restarting, or `None` to give up.
    pub fn next_delay(&mut self) -> Option<Duration> {
        if self.failures >= MAX_FAILURES {
            return None;
        }
        let delay = FIRST.saturating_mul(1 << self.failures.min(16)).min(CAP);
        self.failures += 1;
        Some(delay)
    }

    /// Call once a server has stayed up long enough to count as healthy.
    pub fn reset(&mut self) {
        self.failures = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_then_gives_up_after_five_failures() {
        let mut backoff = Backoff::new();
        let delays: Vec<_> = std::iter::from_fn(|| backoff.next_delay()).collect();
        let secs: Vec<_> = delays.iter().map(Duration::as_secs).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16]);
        assert_eq!(backoff.next_delay(), None);
    }

    #[test]
    fn reset_starts_over() {
        let mut backoff = Backoff::new();
        for _ in 0..5 {
            backoff.next_delay();
        }
        backoff.reset();
        assert_eq!(backoff.next_delay(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn delay_is_capped() {
        let mut backoff = Backoff { failures: 4 };
        assert!(backoff.next_delay().unwrap() <= CAP);
    }
}
