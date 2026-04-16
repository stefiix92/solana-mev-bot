use std::time::Instant;

/// Simple latency tracker for pipeline stages.
pub struct LatencyTimer {
    start: Instant,
}

impl LatencyTimer {
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Returns elapsed microseconds.
    pub fn elapsed_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }

    /// Returns elapsed milliseconds.
    pub fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}
