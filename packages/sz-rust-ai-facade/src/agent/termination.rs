use crate::agent::trace::TerminateReason;
use std::time::Duration;

pub struct TerminationPolicy {
    max_steps: u32,
    max_tokens: Option<u32>,
    timeout: Option<Duration>,
}

impl TerminationPolicy {
    pub fn new(max_steps: u32) -> Self {
        Self {
            max_steps,
            max_tokens: None,
            timeout: None,
        }
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn check(
        &self,
        current_steps: u32,
        current_tokens: u32,
        elapsed: Duration,
    ) -> Option<TerminateReason> {
        if current_steps >= self.max_steps {
            return Some(TerminateReason::MaxSteps);
        }
        if let Some(max_tokens) = self.max_tokens {
            if current_tokens >= max_tokens {
                return Some(TerminateReason::MaxTokens);
            }
        }
        if let Some(timeout) = self.timeout {
            if elapsed >= timeout {
                return Some(TerminateReason::Timeout);
            }
        }
        None
    }

    pub fn max_steps(&self) -> u32 {
        self.max_steps
    }
}

impl Default for TerminationPolicy {
    fn default() -> Self {
        Self::new(25)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::trace::TerminateReason;

    #[test]
    fn no_termination_when_under_limits() {
        let policy = TerminationPolicy::new(10);
        assert!(policy.check(5, 100, Duration::from_secs(1)).is_none());
    }

    #[test]
    fn max_steps_termination() {
        let policy = TerminationPolicy::new(3);
        assert_eq!(
            policy.check(3, 0, Duration::ZERO),
            Some(TerminateReason::MaxSteps)
        );
        assert!(policy.check(2, 0, Duration::ZERO).is_none());
    }

    #[test]
    fn max_tokens_termination() {
        let policy = TerminationPolicy::new(100).with_max_tokens(500);
        assert_eq!(
            policy.check(1, 500, Duration::ZERO),
            Some(TerminateReason::MaxTokens)
        );
        assert!(policy.check(1, 499, Duration::ZERO).is_none());
    }

    #[test]
    fn timeout_termination() {
        let policy = TerminationPolicy::new(100).with_timeout(Duration::from_secs(5));
        assert_eq!(
            policy.check(0, 0, Duration::from_secs(5)),
            Some(TerminateReason::Timeout)
        );
        assert!(policy.check(0, 0, Duration::from_secs(4)).is_none());
    }

    #[test]
    fn max_steps_takes_priority() {
        let policy = TerminationPolicy::new(3)
            .with_max_tokens(100)
            .with_timeout(Duration::from_secs(1));
        assert_eq!(
            policy.check(3, 200, Duration::from_secs(2)),
            Some(TerminateReason::MaxSteps)
        );
    }

    #[test]
    fn default_is_25_steps() {
        let policy = TerminationPolicy::default();
        assert_eq!(policy.max_steps(), 25);
    }
}
