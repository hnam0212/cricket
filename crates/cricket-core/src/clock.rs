//! Time source for the engine. Injected so tests can control time.

use std::time::{Duration, Instant};

/// Monotonic time since an arbitrary origin.
pub trait Clock {
    fn now(&self) -> Duration;
}

/// Real time, counted from when the clock was created.
#[derive(Debug, Clone)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// A clock that only moves when told to. Clones share the same time.
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub(crate) struct ManualClock(std::rc::Rc<std::cell::Cell<Duration>>);

#[cfg(test)]
impl ManualClock {
    pub(crate) fn advance(&self, by: Duration) {
        self.0.set(self.0.get() + by);
    }
}

#[cfg(test)]
impl Clock for ManualClock {
    fn now(&self) -> Duration {
        self.0.get()
    }
}
