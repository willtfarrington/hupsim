//! The simulation clock. Sim time is measured in minutes since scenario epoch.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SimClock {
    /// Minutes since scenario epoch.
    pub now_min: f64,
    /// Sim minutes advanced per real second while running.
    pub speed: f64,
    pub running: bool,
}

impl Default for SimClock {
    fn default() -> Self {
        Self {
            now_min: 0.0,
            speed: 10.0,
            running: false,
        }
    }
}

impl SimClock {
    /// Convert a real-time delta into a sim-time delta at current speed.
    /// Returns 0 when paused.
    pub fn dt_min(&self, real_dt_sec: f64) -> f64 {
        if self.running {
            real_dt_sec * self.speed
        } else {
            0.0
        }
    }

    /// Human-readable "day 2, 14:36" style label.
    pub fn label(&self) -> String {
        let total_min = self.now_min.max(0.0) as u64;
        let day = total_min / (24 * 60);
        let hh = (total_min / 60) % 24;
        let mm = total_min % 60;
        format!("day {}, {:02}:{:02}", day + 1, hh, mm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_clock_yields_zero_dt() {
        let c = SimClock::default();
        assert_eq!(c.dt_min(5.0), 0.0);
    }

    #[test]
    fn label_formats() {
        let c = SimClock {
            now_min: 24.0 * 60.0 + 90.0,
            speed: 1.0,
            running: true,
        };
        assert_eq!(c.label(), "day 2, 01:30");
    }
}
