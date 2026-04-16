use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// Circuit breaker: pauses all strategies if cumulative loss exceeds threshold
/// within a rolling time window.
pub struct CircuitBreaker {
    inner: Mutex<CircuitBreakerInner>,
    window: Duration,
    max_loss_lamports: i64,
}

struct CircuitBreakerInner {
    /// Recent trades: (timestamp, profit/loss in lamports)
    trades: VecDeque<(Instant, i64)>,
    /// Whether the circuit breaker is currently tripped
    tripped: bool,
    /// When it was tripped
    tripped_at: Option<Instant>,
}

impl CircuitBreaker {
    pub fn new(window_secs: u64, max_loss_lamports: i64) -> Self {
        Self {
            inner: Mutex::new(CircuitBreakerInner {
                trades: VecDeque::new(),
                tripped: false,
                tripped_at: None,
            }),
            window: Duration::from_secs(window_secs),
            max_loss_lamports,
        }
    }

    /// Record a trade result. Negative = loss, positive = profit.
    pub fn record_trade(&self, profit_lamports: i64) {
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();

        // Add new trade
        inner.trades.push_back((now, profit_lamports));

        // Evict old trades outside the window
        while let Some(&(ts, _)) = inner.trades.front() {
            if now.duration_since(ts) > self.window {
                inner.trades.pop_front();
            } else {
                break;
            }
        }

        // Check cumulative P&L in window
        let cumulative: i64 = inner.trades.iter().map(|(_, pnl)| pnl).sum();

        if cumulative < -self.max_loss_lamports && !inner.tripped {
            inner.tripped = true;
            inner.tripped_at = Some(now);
            error!(
                cumulative_loss = cumulative,
                window_secs = self.window.as_secs(),
                "CIRCUIT BREAKER TRIPPED — all strategies paused"
            );
        }
    }

    /// Check if trading is allowed.
    pub fn is_allowed(&self) -> bool {
        !self.inner.lock().unwrap().tripped
    }

    /// Check if the breaker is currently tripped.
    pub fn is_tripped(&self) -> bool {
        self.inner.lock().unwrap().tripped
    }

    /// Manually reset the circuit breaker (requires manual intervention).
    pub fn reset(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.tripped = false;
        inner.tripped_at = None;
        inner.trades.clear();
        info!("Circuit breaker manually reset");
    }

    /// Get current rolling P&L within the window.
    pub fn rolling_pnl(&self) -> i64 {
        let inner = self.inner.lock().unwrap();
        let now = Instant::now();
        inner
            .trades
            .iter()
            .filter(|(ts, _)| now.duration_since(*ts) <= self.window)
            .map(|(_, pnl)| pnl)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_not_tripped() {
        let cb = CircuitBreaker::new(300, 500_000);
        cb.record_trade(100_000); // profit
        cb.record_trade(-50_000); // small loss
        assert!(cb.is_allowed());
    }

    #[test]
    fn test_circuit_breaker_trips_on_large_loss() {
        let cb = CircuitBreaker::new(300, 500_000);
        cb.record_trade(-200_000);
        assert!(cb.is_allowed()); // Not yet

        cb.record_trade(-400_000); // cumulative = -600k > -500k threshold
        assert!(!cb.is_allowed());
        assert!(cb.is_tripped());
    }

    #[test]
    fn test_circuit_breaker_reset() {
        let cb = CircuitBreaker::new(300, 500_000);
        cb.record_trade(-600_000);
        assert!(!cb.is_allowed());

        cb.reset();
        assert!(cb.is_allowed());
    }
}
