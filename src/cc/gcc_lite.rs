//! A deliberately small congestion controller: loss-based AIMD bitrate
//! adaptation (a "GCC-lite"). It is not a full Google Congestion Control
//! implementation — see `COVERAGE.md` for the scope boundary.

/// Loss-based additive-increase/multiplicative-decrease bitrate estimator.
#[derive(Debug, Clone)]
pub struct GccLite {
    min_bps: u32,
    max_bps: u32,
    target_bps: u32,
}

impl GccLite {
    /// Create a controller bounded by `min_bps`/`max_bps`, starting at
    /// `start_bps`.
    #[must_use]
    pub fn new(min_bps: u32, max_bps: u32, start_bps: u32) -> Self {
        Self {
            min_bps,
            max_bps,
            target_bps: start_bps.clamp(min_bps, max_bps),
        }
    }

    /// A controller sized for an audio stream (16–64 kbps, start 32 kbps).
    #[must_use]
    pub fn audio() -> Self {
        Self::new(16_000, 64_000, 32_000)
    }

    /// A controller sized for a video stream (128 kbps–2.5 Mbps, start
    /// 1.2 Mbps).
    #[must_use]
    pub fn video() -> Self {
        Self::new(128_000, 2_500_000, 1_200_000)
    }

    /// The current target bitrate in bits per second.
    #[must_use]
    #[allow(dead_code)]
    pub fn target_bps(&self) -> u32 {
        self.target_bps
    }

    /// Feed an observed loss fraction in `0.0..=1.0` for the last interval and
    /// return the updated target bitrate (RFC 8888 / GCC loss controller
    /// thresholds: <2% increase, >10% decrease, otherwise hold).
    pub fn update(&mut self, loss_fraction: f32) -> u32 {
        let loss = loss_fraction.clamp(0.0, 1.0);
        if loss < 0.02 {
            // Additive increase: +8%.
            let inc = (self.target_bps as f64 * 0.08) as u32;
            self.target_bps = self.target_bps.saturating_add(inc.max(1_000));
        } else if loss > 0.10 {
            // Multiplicative decrease proportional to loss.
            let factor = 1.0 - 0.5 * f64::from(loss);
            self.target_bps = (self.target_bps as f64 * factor) as u32;
        }
        self.target_bps = self.target_bps.clamp(self.min_bps, self.max_bps);
        self.target_bps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increases_when_no_loss() {
        let mut cc = GccLite::video();
        let before = cc.target_bps();
        let after = cc.update(0.0);
        assert!(after > before);
    }

    #[test]
    fn decreases_on_heavy_loss() {
        let mut cc = GccLite::video();
        let before = cc.target_bps();
        let after = cc.update(0.30);
        assert!(after < before);
    }

    #[test]
    fn respects_bounds() {
        let mut cc = GccLite::audio();
        for _ in 0..1000 {
            cc.update(0.0);
        }
        assert!(cc.target_bps() <= 64_000);
        for _ in 0..1000 {
            cc.update(1.0);
        }
        assert!(cc.target_bps() >= 16_000);
    }
}
