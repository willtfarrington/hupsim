//! The extension point for simulation models.
//!
//! A future ADT (admission/discharge/transfer) generator, LOS model, or
//! deterioration model implements [`SimProcess`] and registers on the engine.
//! Processes see the world through [`SimCtx`]: mutable hospital, dt, a seeded
//! RNG (determinism!), the event queue for scheduling future events, and the
//! sim log.

use crate::engine::SimLogEntry;
use crate::events::EventQueue;
use crate::transport::TransportBoard;
use hupsim_core::index::HospitalIndex;
use hupsim_core::matrix::RoomMatrix;
use hupsim_core::model::Hospital;
use rand_chacha::ChaCha8Rng;

pub struct SimCtx<'a> {
    pub hospital: &'a mut Hospital,
    pub index: &'a HospitalIndex,
    /// Columnar room view (EP-9), refreshed by the engine at the start of the
    /// tick (after due events, before processes). Row `i` is
    /// `hospital.rooms[i]`. NOTE: status columns are a *tick-start snapshot* —
    /// they do not see mutations made by processes during this tick, so
    /// verify against `hospital` before acting on a row picked from a mask.
    pub matrix: &'a RoomMatrix,
    /// Sim time at the *end* of this tick, minutes.
    pub now_min: f64,
    /// Tick length, minutes.
    pub dt_min: f64,
    pub rng: &'a mut ChaCha8Rng,
    pub queue: &'a mut EventQueue,
    pub log: &'a mut Vec<SimLogEntry>,
    /// Timed-transport state (EP-6), engine-owned. Bed-hunting processes must
    /// skip [`TransportBoard::is_held`] beds; transfer intents detour through
    /// `requests` when [`TransportBoard::timed_active`].
    pub transport: &'a mut TransportBoard,
}

impl SimCtx<'_> {
    pub fn log(&mut self, message: impl Into<String>) {
        self.log.push(SimLogEntry {
            t_min: self.now_min,
            message: message.into(),
        });
    }
}

pub trait SimProcess: Send + Sync {
    /// Stable name shown in the UI process list.
    fn name(&self) -> &str;
    /// One-line description of what the process models.
    fn description(&self) -> &str {
        ""
    }
    /// Advance the model by `ctx.dt_min` minutes.
    fn step(&mut self, ctx: &mut SimCtx);
    /// Clear internal state so a reset engine reproduces a fresh run.
    /// Stateless processes can keep the default no-op.
    fn reset(&mut self) {}
}
