//! Smoothed transfer speed and time left, shared by downloads and uploads.

use std::time::{Duration, Instant};

/// Smoothed transfer speed and ETA. The first sample is reported right away, later ones at
/// most every `min_interval` (unless the transfer has finished).
pub(crate) struct SpeedMeter {
    min_interval: Duration,
    last_time: Instant,
    last_bytes: u64,
    speed: f64,
    reported: bool,
}

impl SpeedMeter {
    pub(crate) fn new(start_bytes: u64, min_interval: Duration) -> Self {
        Self {
            min_interval,
            last_time: Instant::now(),
            last_bytes: start_bytes,
            speed: 0.0,
            reported: false,
        }
    }

    /// Record that `done` of `total` bytes have been transferred. Returns the smoothed speed in
    /// bytes per second and the seconds left, or `None` when it is too early for a new sample.
    pub(crate) fn sample(&mut self, done: u64, total: u64) -> Option<(u64, Option<u64>)> {
        let elapsed = self.last_time.elapsed();
        if self.reported && elapsed < self.min_interval && done < total {
            return None;
        }
        if elapsed < Duration::from_millis(1) {
            return None; // too short to measure a speed
        }
        let instant = done.saturating_sub(self.last_bytes) as f64 / elapsed.as_secs_f64();
        self.speed = if self.speed == 0.0 {
            instant
        } else {
            0.7 * self.speed + 0.3 * instant
        };
        // Below one byte per second the speed shows as zero; let it reach zero for real.
        if self.speed < 1.0 {
            self.speed = 0.0;
        }
        self.reported = true;
        self.last_time = Instant::now();
        self.last_bytes = done;
        let eta = (self.speed >= 1.0)
            .then(|| (total.saturating_sub(done) as f64 / self.speed).ceil() as u64);
        Some((self.speed as u64, eta))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_reports_infinite_speed() {
        let mut meter = SpeedMeter::new(0, Duration::from_millis(250));
        // Completion right away: elapsed is (nearly) zero.
        if let Some((speed, _)) = meter.sample(100, 100) {
            assert!(speed < u64::MAX);
        }
    }

    #[test]
    fn stalled_transfer_has_no_eta() {
        let mut meter = SpeedMeter::new(0, Duration::ZERO);
        std::thread::sleep(Duration::from_millis(5));
        let (speed, eta) = meter.sample(0, 100).unwrap();
        assert_eq!(speed, 0);
        assert_eq!(eta, None);
    }
}
