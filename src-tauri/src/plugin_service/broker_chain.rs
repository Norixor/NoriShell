//! Bounded accounting for one plugin API broker chain.

use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

pub(super) const MAX_STEPS: usize = 16;
pub(super) const MAX_CALLBACK_BYTES: usize = 512 * 1024;
pub(super) const MAX_ELAPSED: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChainError {
    Limit,
    TimedOut,
}

pub(super) struct BrokerChain {
    deadline: Instant,
    call_ids: BTreeSet<String>,
    steps: usize,
    callback_bytes: usize,
}

impl BrokerChain {
    pub(super) fn new() -> Self {
        Self {
            deadline: Instant::now() + MAX_ELAPSED,
            call_ids: BTreeSet::new(),
            steps: 0,
            callback_bytes: 0,
        }
    }
    pub(super) fn remaining(&self) -> Result<Duration, ChainError> {
        self.deadline
            .checked_duration_since(Instant::now())
            .ok_or(ChainError::TimedOut)
    }
    pub(super) fn accept_call(&mut self, call_id: &str) -> Result<(), ChainError> {
        if self.steps >= MAX_STEPS || !self.call_ids.insert(call_id.to_owned()) {
            return Err(ChainError::Limit);
        }
        self.steps += 1;
        Ok(())
    }
    pub(super) fn account_callback(&mut self, bytes: usize) -> Result<(), ChainError> {
        self.callback_bytes = self
            .callback_bytes
            .checked_add(bytes)
            .ok_or(ChainError::Limit)?;
        (self.callback_bytes <= MAX_CALLBACK_BYTES)
            .then_some(())
            .ok_or(ChainError::Limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_steps_duplicate_ids_and_total_callback_bytes() {
        let mut chain = BrokerChain::new();
        for index in 0..MAX_STEPS {
            assert!(chain.accept_call(&format!("call-{index}")).is_ok());
        }
        assert_eq!(chain.accept_call("call-over-limit"), Err(ChainError::Limit));

        let mut duplicate = BrokerChain::new();
        assert!(duplicate.accept_call("one").is_ok());
        assert_eq!(duplicate.accept_call("one"), Err(ChainError::Limit));

        assert!(chain.account_callback(MAX_CALLBACK_BYTES).is_ok());
        assert_eq!(chain.account_callback(1), Err(ChainError::Limit));
    }

    #[test]
    fn reports_when_the_wall_clock_budget_has_expired() {
        let mut chain = BrokerChain::new();
        chain.deadline = Instant::now() - Duration::from_nanos(1);
        assert_eq!(chain.remaining(), Err(ChainError::TimedOut));
    }
}
