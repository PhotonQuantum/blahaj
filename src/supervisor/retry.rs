use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct RetryGuard {
    retries: VecDeque<Instant>,
    tolerance_interval: Duration,
    tolerance_count: usize,
}

impl RetryGuard {
    pub fn new(
        retries: VecDeque<Instant>,
        tolerance_interval: Duration,
        tolerance_count: usize,
    ) -> Self {
        Self {
            retries,
            tolerance_interval,
            tolerance_count,
        }
    }
}

impl RetryGuard {
    /// Evict all events before a given time.
    fn evict(&mut self, before: Instant) {
        let evict_range = 0..self.retries.iter().filter(|item| *item < &before).count();
        drop(self.retries.drain(evict_range));
    }

    /// Mark current event.
    /// Returns `false` if the upperbound is reached.
    pub fn mark(&mut self) -> bool {
        let now = Instant::now();
        self.retries.push_back(now);
        self.evict(now - self.tolerance_interval);
        self.retries.len() < self.tolerance_count
    }
}
