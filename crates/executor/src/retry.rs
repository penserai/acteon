use std::time::Duration;

/// Strategy for computing delay between retry attempts.
///
/// Each variant defines a different backoff curve. All variants clamp the
/// computed delay so it never exceeds the configured maximum.
#[derive(Debug, Clone)]
pub enum RetryStrategy {
    /// Exponential backoff: `base * multiplier^attempt`, optionally with
    /// deterministic jitter.
    Exponential {
        /// Initial delay before the first retry.
        base: Duration,
        /// Upper bound on the computed delay.
        max: Duration,
        /// Factor applied on each successive attempt.
        multiplier: f64,
        /// When `true`, a deterministic jitter factor is applied so that
        /// concurrent callers do not all retry at the same instant.
        jitter: bool,
    },
    /// Linear backoff: `delay * (attempt + 1)`, clamped to `max`.
    Linear {
        /// Per-attempt increment.
        delay: Duration,
        /// Upper bound on the computed delay.
        max: Duration,
    },
    /// Constant delay between every retry attempt.
    Constant {
        /// Fixed delay duration.
        delay: Duration,
    },
}

impl RetryStrategy {
    /// Compute the delay duration for the given zero-based `attempt` number.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use acteon_executor::RetryStrategy;
    ///
    /// let strategy = RetryStrategy::Constant { delay: Duration::from_secs(1) };
    /// assert_eq!(strategy.delay_for(0), Duration::from_secs(1));
    /// assert_eq!(strategy.delay_for(5), Duration::from_secs(1));
    /// ```
    pub fn delay_for(&self, attempt: u32) -> Duration {
        match self {
            Self::Exponential {
                base,
                max,
                multiplier,
                jitter,
            } => {
                let base_secs = base.as_secs_f64();
                // In practice `attempt` is a small retry count (< 100), so
                // wrapping from u32 to i32 cannot occur.
                #[allow(clippy::cast_possible_wrap)]
                let raw = base_secs * multiplier.powi(attempt as i32);

                let adjusted = if *jitter {
                    // Deterministic jitter: vary by +0% to +40% based on the
                    // attempt number.  This spreads retries across a window
                    // without requiring a random number generator.
                    let jitter_factor = 1.0 + 0.1 * f64::from(attempt % 5);
                    raw * jitter_factor
                } else {
                    raw
                };

                let clamped = adjusted.min(max.as_secs_f64());
                Duration::from_secs_f64(clamped)
            }
            Self::Linear { delay, max } => {
                let raw = delay.as_secs_f64() * f64::from(attempt + 1);
                let clamped = raw.min(max.as_secs_f64());
                Duration::from_secs_f64(clamped)
            }
            Self::Constant { delay } => *delay,
        }
    }
}

impl Default for RetryStrategy {
    fn default() -> Self {
        Self::Exponential {
            base: Duration::from_millis(100),
            max: Duration::from_secs(30),
            multiplier: 2.0,
            jitter: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_no_jitter_basic() {
        let strategy = RetryStrategy::Exponential {
            base: Duration::from_millis(100),
            max: Duration::from_secs(60),
            multiplier: 2.0,
            jitter: false,
        };
        // attempt 0: 100ms * 2^0 = 100ms
        assert_eq!(strategy.delay_for(0), Duration::from_millis(100));
        // attempt 1: 100ms * 2^1 = 200ms
        assert_eq!(strategy.delay_for(1), Duration::from_millis(200));
        // attempt 2: 100ms * 2^2 = 400ms
        assert_eq!(strategy.delay_for(2), Duration::from_millis(400));
        // attempt 3: 100ms * 2^3 = 800ms
        assert_eq!(strategy.delay_for(3), Duration::from_millis(800));
    }

    #[test]
    fn exponential_no_jitter_clamped() {
        let strategy = RetryStrategy::Exponential {
            base: Duration::from_secs(1),
            max: Duration::from_secs(5),
            multiplier: 3.0,
            jitter: false,
        };
        // attempt 0: 1s
        assert_eq!(strategy.delay_for(0), Duration::from_secs(1));
        // attempt 1: 3s
        assert_eq!(strategy.delay_for(1), Duration::from_secs(3));
        // attempt 2: 9s -> clamped to 5s
        assert_eq!(strategy.delay_for(2), Duration::from_secs(5));
        // attempt 10: clamped to 5s
        assert_eq!(strategy.delay_for(10), Duration::from_secs(5));
    }

    #[test]
    fn exponential_with_jitter() {
        let strategy = RetryStrategy::Exponential {
            base: Duration::from_millis(100),
            max: Duration::from_secs(60),
            multiplier: 2.0,
            jitter: true,
        };
        // attempt 0: 100ms * 1.0 * (1.0 + 0.1*0) = 100ms
        assert_eq!(strategy.delay_for(0), Duration::from_millis(100));
        // attempt 1: 200ms * (1.0 + 0.1*1) = 200 * 1.1 = 220ms
        assert_eq!(strategy.delay_for(1), Duration::from_millis(220));
        // attempt 5: same jitter factor as attempt 0 (5 % 5 == 0)
        let d5 = strategy.delay_for(5);
        // 100ms * 2^5 * 1.0 = 3200ms
        assert_eq!(d5, Duration::from_millis(3200));
    }

    #[test]
    fn linear_basic() {
        let strategy = RetryStrategy::Linear {
            delay: Duration::from_millis(500),
            max: Duration::from_secs(5),
        };
        // attempt 0: 500ms * 1 = 500ms
        assert_eq!(strategy.delay_for(0), Duration::from_millis(500));
        // attempt 1: 500ms * 2 = 1000ms
        assert_eq!(strategy.delay_for(1), Duration::from_secs(1));
        // attempt 4: 500ms * 5 = 2500ms
        assert_eq!(strategy.delay_for(4), Duration::from_millis(2500));
    }

    #[test]
    fn linear_clamped() {
        let strategy = RetryStrategy::Linear {
            delay: Duration::from_secs(2),
            max: Duration::from_secs(5),
        };
        // attempt 2: 2s * 3 = 6s -> clamped to 5s
        assert_eq!(strategy.delay_for(2), Duration::from_secs(5));
    }

    #[test]
    fn constant_always_same() {
        let strategy = RetryStrategy::Constant {
            delay: Duration::from_millis(250),
        };
        for attempt in 0..10 {
            assert_eq!(strategy.delay_for(attempt), Duration::from_millis(250));
        }
    }

    #[test]
    fn default_is_exponential_with_jitter() {
        let strategy = RetryStrategy::default();
        match strategy {
            RetryStrategy::Exponential {
                base,
                max,
                multiplier,
                jitter,
            } => {
                assert_eq!(base, Duration::from_millis(100));
                assert_eq!(max, Duration::from_secs(30));
                assert!((multiplier - 2.0).abs() < f64::EPSILON);
                assert!(jitter);
            }
            _ => panic!("default should be Exponential"),
        }
    }
}
