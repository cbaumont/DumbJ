use std::time::Duration;

const FIRST: Duration = Duration::from_secs(1);
const MAX_FAILURES: u32 = 5;

/// Restart delays for a server that keeps dying: 1 s doubling, then give up after five failures.
#[derive(Debug, Clone, Default)]
pub(crate) struct Backoff {
    failures: u32,
}

impl Backoff {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn next_delay(&mut self) -> Option<Duration> {
        if self.failures >= MAX_FAILURES {
            return None;
        }
        let delay = FIRST * (1 << self.failures);
        self.failures += 1;
        Some(delay)
    }

    pub(crate) fn reset(&mut self) {
        self.failures = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_gives_up_after_five_failures_and_resets() {
        let mut backoff = Backoff::new();
        let delays: Vec<_> = std::iter::from_fn(|| backoff.next_delay()).collect();
        let secs: Vec<_> = delays.iter().map(Duration::as_secs).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16]);
        assert_eq!(backoff.next_delay(), None);
        backoff.reset();
        assert_eq!(backoff.next_delay(), Some(Duration::from_secs(1)));
    }
}
