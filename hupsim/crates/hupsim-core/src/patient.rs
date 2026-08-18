//! Synthetic patients and the abstract acuity model.
//!
//! `instability` is a deliberately model-agnostic 0..1 scalar: it is the hook
//! where a NEWS2/MEWS-style score, an eCART-like model, or a hand-set clinical
//! judgment plugs in. The UI and colormap only ever consume this scalar.

use crate::ids::PatientId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Patient {
    pub id: PatientId,
    /// Synthetic display name — never real PHI.
    pub alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age: Option<u8>,
    /// Admitting/primary service, e.g. "General Medicine", "Cardiac Surgery".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Sim-clock admission timestamp in minutes (None = present before t0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admitted_at_min: Option<f64>,
    pub acuity: Acuity,
    /// Admitted inpatient physically boarding in a non-inpatient space (ED bay, obs).
    #[serde(default)]
    pub boarding: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,
    #[serde(default)]
    pub telemetry: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Acuity {
    /// 0.0 = rock stable … 1.0 = peri-arrest. Drives the green→blue→red spectrum.
    pub instability: f32,
    /// Expected drift in instability per hour — scaffolding for the
    /// time-series simulation (deterioration/recovery trajectories).
    #[serde(default)]
    pub trend_per_hr: f32,
}

impl Default for Acuity {
    fn default() -> Self {
        Self {
            instability: 0.1,
            trend_per_hr: 0.0,
        }
    }
}

impl Acuity {
    pub fn clamped(mut self) -> Self {
        self.instability = self.instability.clamp(0.0, 1.0);
        self
    }

    pub fn tier(&self) -> StabilityTier {
        StabilityTier::from_instability(self.instability)
    }
}

/// Categorical read of the instability scalar, for filters and labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StabilityTier {
    Stable,
    Watcher,
    Unstable,
    Critical,
}

impl StabilityTier {
    pub fn from_instability(x: f32) -> Self {
        match x {
            x if x < 0.25 => StabilityTier::Stable,
            x if x < 0.5 => StabilityTier::Watcher,
            x if x < 0.8 => StabilityTier::Unstable,
            _ => StabilityTier::Critical,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StabilityTier::Stable => "Stable",
            StabilityTier::Watcher => "Watcher",
            StabilityTier::Unstable => "Unstable",
            StabilityTier::Critical => "Critical",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_thresholds() {
        assert_eq!(StabilityTier::from_instability(0.0), StabilityTier::Stable);
        assert_eq!(StabilityTier::from_instability(0.25), StabilityTier::Watcher);
        assert_eq!(StabilityTier::from_instability(0.5), StabilityTier::Unstable);
        assert_eq!(StabilityTier::from_instability(0.8), StabilityTier::Critical);
        assert_eq!(StabilityTier::from_instability(1.0), StabilityTier::Critical);
    }

    #[test]
    fn acuity_clamps() {
        let a = Acuity {
            instability: 1.7,
            trend_per_hr: 0.0,
        }
        .clamped();
        assert_eq!(a.instability, 1.0);
    }
}
