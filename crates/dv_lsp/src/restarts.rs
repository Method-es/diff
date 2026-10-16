//! When a server that ended starts again (docs/PLAN.md §18.5): after 1, 2 and 5 s, then it
//! shows as failed until the reviewer's **Retry**, like a connection (PLAN §5.2). A server that
//! ran a while before it ended starts the count afresh, so one crash a day doesn't add up to a
//! failure.

use std::time::Duration;

/// The waits before each start again.
pub const RESTART_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
];

/// How long a server must have run for its end to count as a first one again.
pub const HEALTHY_RUN: Duration = Duration::from_secs(60);

/// The restart count of one server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Restarts {
    /// Ends since the last healthy run or retry.
    ends: usize,
}

impl Restarts {
    pub fn new() -> Self {
        Self::default()
    }

    /// The server ended after running for `ran`: how long to wait before starting it again,
    /// or `None` when it has failed until a retry.
    pub fn after_exit(&mut self, ran: Duration) -> Option<Duration> {
        if ran >= HEALTHY_RUN {
            self.ends = 0;
        }
        let delay = RESTART_DELAYS.get(self.ends).copied();
        self.ends += 1;
        delay
    }

    /// The reviewer's **Retry**: the next end waits the first delay again.
    pub fn retry(&mut self) {
        self.ends = 0;
    }

    /// Whether the server has failed until a retry.
    pub fn failed(&self) -> bool {
        self.ends > RESTART_DELAYS.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUICK: Duration = Duration::from_millis(200);

    #[test]
    fn three_restarts_then_failed_until_a_retry() {
        let mut restarts = Restarts::new();
        assert_eq!(restarts.after_exit(QUICK), Some(Duration::from_secs(1)));
        assert_eq!(restarts.after_exit(QUICK), Some(Duration::from_secs(2)));
        assert_eq!(restarts.after_exit(QUICK), Some(Duration::from_secs(5)));
        assert!(!restarts.failed());
        assert_eq!(restarts.after_exit(QUICK), None);
        assert!(restarts.failed());
        restarts.retry();
        assert!(!restarts.failed());
        assert_eq!(restarts.after_exit(QUICK), Some(Duration::from_secs(1)));
    }

    #[test]
    fn a_long_run_starts_the_count_afresh() {
        let mut restarts = Restarts::new();
        restarts.after_exit(QUICK);
        restarts.after_exit(QUICK);
        assert_eq!(
            restarts.after_exit(HEALTHY_RUN),
            Some(Duration::from_secs(1))
        );
    }
}
