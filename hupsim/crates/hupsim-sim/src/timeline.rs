//! Bounded time-series recorder: per-unit census/acuity samples at a fixed
//! sim-time interval. This is the substrate the future time-series charts and
//! analyses read from.

use hupsim_core::aggregate::aggregate_unit;
use hupsim_core::ids::UnitId;
use hupsim_core::index::HospitalIndex;
use hupsim_core::model::Hospital;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitSample {
    pub unit: UnitId,
    pub capacity: u32,
    pub census: u32,
    pub boarding: u32,
    pub mean_instability: Option<f32>,
    pub worst_instability: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineSample {
    pub t_min: f64,
    pub hospital_census: u32,
    pub units: Vec<UnitSample>,
}

#[derive(Debug)]
pub struct Timeline {
    /// Sampling interval in sim minutes.
    pub sample_every_min: f64,
    pub max_samples: usize,
    samples: VecDeque<TimelineSample>,
    last_sample_min: f64,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            sample_every_min: 15.0,
            max_samples: 4 * 24 * 14, // two weeks of 15-min samples
            samples: VecDeque::new(),
            last_sample_min: f64::NEG_INFINITY,
        }
    }
}

impl Timeline {
    /// Record a sample if the interval has elapsed. Returns true when sampled.
    ///
    /// The cadence anchor advances on the interval grid (not to `now_min`), so
    /// coarse ticks don't stretch the effective sampling period. A tick that
    /// jumps several intervals still records one sample — timeline consumers
    /// must read `t_min`, not assume uniform spacing.
    pub fn maybe_sample(&mut self, now_min: f64, h: &Hospital, idx: &HospitalIndex) -> bool {
        let elapsed = now_min - self.last_sample_min;
        if elapsed < self.sample_every_min {
            return false;
        }
        if self.last_sample_min == f64::NEG_INFINITY {
            self.last_sample_min = now_min;
        } else {
            let intervals = (elapsed / self.sample_every_min).floor();
            self.last_sample_min += intervals * self.sample_every_min;
        }
        let mut hospital_census = 0u32;
        let units = h
            .units
            .iter()
            .map(|u| {
                let a = aggregate_unit(h, idx, &u.id);
                hospital_census += a.census as u32;
                UnitSample {
                    unit: u.id.clone(),
                    capacity: a.capacity as u32,
                    census: a.census as u32,
                    boarding: a.boarding as u32,
                    mean_instability: a.mean_instability,
                    worst_instability: a.worst_instability,
                }
            })
            .collect();
        self.samples.push_back(TimelineSample {
            t_min: now_min,
            hospital_census,
            units,
        });
        while self.samples.len() > self.max_samples {
            self.samples.pop_front();
        }
        true
    }

    pub fn samples(&self) -> impl Iterator<Item = &TimelineSample> {
        self.samples.iter()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn clear(&mut self) {
        self.samples.clear();
        self.last_sample_min = f64::NEG_INFINITY;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hupsim_core::model::HospitalMeta;

    fn empty() -> (Hospital, HospitalIndex) {
        let h = Hospital {
            meta: HospitalMeta::default(),
            buildings: vec![],
            units: vec![],
            rooms: vec![],
            connections: vec![],
            elevator_cores: vec![],
            service_lines: vec![],
        };
        let idx = HospitalIndex::build(&h);
        (h, idx)
    }

    #[test]
    fn samples_at_interval_and_bounds() {
        let (h, idx) = empty();
        let mut tl = Timeline {
            sample_every_min: 10.0,
            max_samples: 3,
            ..Default::default()
        };
        assert!(tl.maybe_sample(0.0, &h, &idx));
        assert!(!tl.maybe_sample(5.0, &h, &idx));
        assert!(tl.maybe_sample(10.0, &h, &idx));
        assert!(tl.maybe_sample(25.0, &h, &idx));
        assert!(tl.maybe_sample(40.0, &h, &idx));
        assert_eq!(tl.len(), 3); // bounded, oldest dropped
        let first = tl.samples().next().unwrap();
        assert_eq!(first.t_min, 10.0);
    }
}
