//! Acuity drift — a mean-reverting random walk over every occupied
//! patient's instability, with volatility and reversion anchor tied to the
//! room's level of care (EP-10 realism pass). ICU patients wander more;
//! observation-level patients barely move. Still not a clinical model — the
//! hook where a NEWS2/eCART-style deterioration model would plug in.

use crate::params;
use crate::process::{SimCtx, SimProcess};
use hupsim_core::model::{RoomStatus, UnitType};
use rand::Rng;

/// `d(instability) = trend·dt + revert·(baseline − x)·dt + vol(type)·√dt·noise`,
/// clamped to [0,1]. One RNG draw per occupied room per tick, in
/// `Hospital.rooms` order — deterministic given the seed.
pub struct AcuityDrift {
    /// Base noise scale per √hour, before the per-unit-type factor.
    /// 0.05 is gentle; 0.2 is chaotic.
    pub volatility_per_sqrt_hr: f32,
}

impl Default for AcuityDrift {
    fn default() -> Self {
        Self {
            volatility_per_sqrt_hr: 0.06,
        }
    }
}

impl SimProcess for AcuityDrift {
    fn name(&self) -> &str {
        "Acuity drift"
    }

    fn description(&self) -> &str {
        "Mean-reverting random-walk instability, volatility tied to level of care (ICU wanders, obs barely moves). Synthetic — replace with a real deterioration model."
    }

    fn step(&mut self, ctx: &mut SimCtx) {
        let dt_hr = (ctx.dt_min / 60.0) as f32;
        if dt_hr <= 0.0 {
            return;
        }
        let sqrt_dt = dt_hr.sqrt();
        for (i, room) in ctx.hospital.rooms.iter_mut().enumerate() {
            if let RoomStatus::Occupied { patient } = &mut room.status {
                // Unit type from the matrix's structural column (row i ==
                // rooms[i]); structural columns are never stale within a tick.
                let ut = ctx
                    .matrix
                    .unit_type
                    .get(i)
                    .copied()
                    .unwrap_or(UnitType::Other);
                let vol = self.volatility_per_sqrt_hr * params::drift_volatility_factor(ut);
                let noise: f32 = ctx.rng.random_range(-1.0..1.0);
                let x = patient.acuity.instability;
                let revert =
                    (params::acuity_baseline(ut) - x) * params::ACUITY_REVERT_PER_HR * dt_hr;
                let delta = patient.acuity.trend_per_hr * dt_hr + revert + vol * sqrt_dt * noise;
                patient.acuity.instability = (x + delta).clamp(0.0, 1.0);
                // Mean-revert the trend itself a little so runs don't rail.
                patient.acuity.trend_per_hr *= 1.0 - (0.05 * dt_hr).min(1.0);
            }
        }
    }
}
