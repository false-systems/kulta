//! Clock abstraction for testable time-dependent logic
//!
//! Production code uses `SystemClock` which delegates to `chrono::Utc::now()`.
//! Tests use `MockClock` to control time deterministically.

use chrono::{DateTime, Utc};

/// Trait for getting the current time
///
/// Injected via `Context` to allow tests to control time.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// Production clock that delegates to `chrono::Utc::now()`
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Mock clock for testing with controllable time
#[cfg(test)]
#[allow(clippy::expect_used)]
pub struct MockClock {
    now: std::sync::Mutex<DateTime<Utc>>,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
impl MockClock {
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            now: std::sync::Mutex::new(now),
        }
    }

    #[allow(dead_code)]
    pub fn set(&self, now: DateTime<Utc>) {
        *self.now.lock().expect("MockClock lock poisoned") = now;
    }

    #[allow(dead_code)]
    pub fn advance(&self, duration: chrono::Duration) {
        let mut now = self.now.lock().expect("MockClock lock poisoned");
        *now += duration;
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
impl Clock for MockClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().expect("MockClock lock poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_clock_returns_time() {
        let clock = SystemClock;
        let now = clock.now();
        // Just verify it returns a reasonable time (after 2020)
        assert!(now.timestamp() > 1_577_836_800);
    }

    #[test]
    fn test_mock_clock_returns_fixed_time() {
        let fixed = Utc::now();
        let clock = MockClock::new(fixed);
        assert_eq!(clock.now(), fixed);
    }

    #[test]
    fn test_mock_clock_advance() {
        let fixed = Utc::now();
        let clock = MockClock::new(fixed);
        clock.advance(chrono::Duration::seconds(60));
        assert_eq!(clock.now(), fixed + chrono::Duration::seconds(60));
    }
}
