//! Bridges Bevy's fixed timestep into the deterministic sim engine.

use crate::world::{AppWorld, SimDrive};
use bevy::prelude::*;

/// Runs at the FixedUpdate rate (10 Hz). Converts real seconds into sim
/// minutes at the clock's speed, ticks the engine, and marks the world dirty.
pub fn sim_tick(mut world: ResMut<AppWorld>, mut sim: ResMut<SimDrive>, time: Res<Time>) {
    let dt_min = sim.engine.clock.dt_min(time.delta_secs() as f64);
    if dt_min <= 0.0 {
        return;
    }
    let w = &mut *world;
    sim.engine
        .tick(&mut w.scenario.hospital, &w.index, dt_min);
    w.scenario.sim_time_min = sim.engine.clock.now_min;
    w.version += 1;
}

/// Keeps the EP-11 RN roster in step with *editor* mutations — admissions,
/// discharges, census seeding — made while the clock is paused (ticking
/// worlds are maintained inside `SimEngine::tick`). Version-keyed like the
/// aggregate cache; `maintain` is idempotent when nothing changed.
pub fn roster_sync(world: Res<AppWorld>, mut sim: ResMut<SimDrive>, mut synced: Local<u64>) {
    if *synced == world.version {
        return;
    }
    *synced = world.version;
    let engine = &mut sim.engine;
    let now = engine.clock.now_min;
    engine
        .roster
        .maintain(&world.scenario.hospital, &world.index, now, &mut engine.log);
}
