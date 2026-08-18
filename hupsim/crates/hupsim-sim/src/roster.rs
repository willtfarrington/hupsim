//! `RnRoster` — the EP-11 RN staffing model: per staffed unit, a set of
//! anonymized RNs ("RN #1"…) sized from the unit's patients-per-RN ratio and
//! current census, with a deterministic acuity-balanced room assignment and
//! per-RN workload burden.
//!
//! **Ephemeral sim state by design** (owner decision): rosters are NOT part
//! of `Scenario`/schema v1. They regenerate deterministically from the world
//! (seed + current census) on load/reset — assignments are shift-local
//! operational state, not part of the saved world. Derivation uses no RNG:
//! same census + same tick sequence ⇒ identical rosters.
//!
//! Assignment: a unit's occupied rooms in unit-geography order (the
//! compiler's corridor order) are partitioned into one contiguous run per
//! RN, then a bounded greedy pass moves boundary rooms to equalize the
//! workload composite ([`hupsim_core::workload`]) across RNs. Runs stay
//! contiguous; each boundary may shift at most [`MAX_BOUNDARY_SHIFT`] rooms
//! from its equal-split position. Ties break by room index (the sweep is
//! strictly left-to-right and only strictly-improving moves are taken).
//!
//! Churn (the composite's fourth term) is a per-RN decaying event counter
//! ([`CHURN_HALFLIFE_MIN`]) fed by occupancy diffing: every admit /
//! discharge / transfer touching a room credits the room's RN, regardless
//! of whether the mutation came from a queue event, a flow process, or the
//! editor. Boarding patients count like any other occupant.
//!
//! Shift turnover at 07:00 and 19:00 sim time re-derives every roster with
//! fresh RNs (renumbered, churn zeroed, manual pins cleared — that's the
//! point of a handoff) and logs a handoff line per staffed unit.

use crate::engine::SimLogEntry;
use hupsim_core::ids::{PatientId, RoomId, UnitId};
use hupsim_core::index::HospitalIndex;
use hupsim_core::model::{Hospital, Unit};
use hupsim_core::workload::{self, RnLoadInput, WorkloadBreakdown, WorkloadWeights};
use std::collections::{BTreeMap, BTreeSet};

/// Day shift starts 07:00 = minute 420 of the sim day; shifts are 12 h, so
/// boundaries fall at 420 + 720·n sim minutes (07:00 and 19:00 daily).
pub const FIRST_SHIFT_MIN: f64 = 420.0;
pub const SHIFT_LEN_MIN: f64 = 720.0;

/// Half-life of the churn counters: an admit/discharge/transfer "costs" its
/// RN for a few trailing hours, then fades. Synthetic, like the weights.
pub const CHURN_HALFLIFE_MIN: f64 = 240.0;

/// How far the balance pass may move a run boundary from its equal-split
/// position (the brief's "bounded swap distance" — keeps each RN's rooms
/// geographically clustered even on a badly skewed unit).
pub const MAX_BOUNDARY_SHIFT: usize = 4;

/// Number of shift boundaries at or before `t_min`.
fn shift_count(t_min: f64) -> i64 {
    if t_min < FIRST_SHIFT_MIN {
        0
    } else {
        ((t_min - FIRST_SHIFT_MIN) / SHIFT_LEN_MIN).floor() as i64 + 1
    }
}

/// One unit's roster for the current shift.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UnitRoster {
    /// RNs on shift: ⌈census / ratio⌉, min 1 while census > 0, 0 when empty.
    pub rn_count: usize,
    /// Occupied room → RN index (0-based; display as "RN #{i+1}").
    pub assignment: BTreeMap<RoomId, u16>,
    /// Manual overrides (subset of `assignment`): pinned until turnover or a
    /// census-triggered re-derive that can't honor them (logged when dropped).
    pub pins: BTreeMap<RoomId, u16>,
    /// Per-RN room lists in unit corridor order (parallel to `workloads`).
    pub rn_rooms: Vec<Vec<RoomId>>,
    /// Per-RN decayed churn counters (trailing-window event counts).
    pub churn: Vec<f32>,
    /// Per-RN workload, recomputed from live acuity every maintenance pass.
    pub workloads: Vec<WorkloadBreakdown>,
}

/// All rosters plus the room-occupancy snapshot the churn diff runs against.
/// Owned by the engine (ticked worlds) and maintainable from the app while
/// the clock is paused (editor mutations) — `maintain` is idempotent when
/// nothing changed.
#[derive(Debug, Clone, PartialEq)]
pub struct RnRoster {
    pub weights: WorkloadWeights,
    pub churn_halflife_min: f64,
    /// Per staffed unit. `BTreeMap` for deterministic iteration.
    pub units: BTreeMap<UnitId, UnitRoster>,
    /// Occupant snapshot, row-parallel to `Hospital.rooms`.
    occupants: Vec<Option<PatientId>>,
    /// Room-id snapshot guarding against structural change.
    room_ids: Vec<RoomId>,
    last_now_min: f64,
    /// False until the first maintenance pass (or after `clear`): the next
    /// pass rebuilds silently — no churn credit, no turnover logs.
    primed: bool,
}

impl Default for RnRoster {
    fn default() -> Self {
        Self {
            weights: WorkloadWeights::default(),
            churn_halflife_min: CHURN_HALFLIFE_MIN,
            units: BTreeMap::new(),
            occupants: Vec::new(),
            room_ids: Vec::new(),
            last_now_min: 0.0,
            primed: false,
        }
    }
}

impl RnRoster {
    /// Roster for one unit, if it is staffed and has been derived.
    pub fn unit(&self, id: &UnitId) -> Option<&UnitRoster> {
        self.units.get(id)
    }

    /// RN index (0-based) assigned to an occupied room.
    pub fn rn_of(&self, unit: &UnitId, room: &RoomId) -> Option<u16> {
        self.units.get(unit)?.assignment.get(room).copied()
    }

    /// Drop everything (sim reset / scenario load): the next `maintain`
    /// regenerates rosters from the current census, silently.
    pub fn clear(&mut self) {
        *self = Self {
            weights: self.weights,
            churn_halflife_min: self.churn_halflife_min,
            ..Self::default()
        };
    }

    /// The one entry point: diff occupancy for churn, apply shift turnover,
    /// re-derive assignments where census changed, recompute workloads.
    /// Called by the engine every tick and by the app after editor
    /// mutations; cheap and idempotent when nothing changed.
    pub fn maintain(
        &mut self,
        h: &Hospital,
        idx: &HospitalIndex,
        now_min: f64,
        log: &mut Vec<SimLogEntry>,
    ) {
        // First pass, or structural change (rooms added/removed/reordered):
        // rebuild everything silently — this is the load/reset regeneration
        // path, and pins/churn cannot meaningfully survive a room-set edit.
        let structural = !self.primed
            || self.room_ids.len() != h.rooms.len()
            || h.rooms.iter().zip(&self.room_ids).any(|(r, id)| r.id != *id);
        if structural {
            self.rebuild(h, idx, now_min);
            return;
        }

        // Decay churn by elapsed sim time.
        let dt = (now_min - self.last_now_min).max(0.0);
        if dt > 0.0 {
            let decay = 0.5f32.powf((dt / self.churn_halflife_min) as f32);
            for ur in self.units.values_mut() {
                for c in &mut ur.churn {
                    *c *= decay;
                }
            }
        }

        // Diff occupancy against the snapshot. A room losing its patient
        // credits its current RN immediately (the event belongs to whoever
        // held the room); a room gaining a patient credits its assignee
        // after re-derivation. An occupant swap is both.
        let mut touched: BTreeSet<UnitId> = BTreeSet::new();
        let mut late_credit: Vec<(UnitId, RoomId)> = Vec::new();
        for (i, room) in h.rooms.iter().enumerate() {
            let cur = room.status.patient().map(|p| &p.id);
            if self.occupants[i].as_ref() == cur {
                continue;
            }
            let staffed = idx
                .unit_pos
                .get(&room.unit)
                .is_some_and(|&up| workload::is_rn_staffed(h.units[up].unit_type));
            if staffed {
                touched.insert(room.unit.clone());
                if self.occupants[i].is_some() {
                    if let Some(ur) = self.units.get_mut(&room.unit) {
                        if let Some(&rn) = ur.assignment.get(&room.id) {
                            if let Some(c) = ur.churn.get_mut(rn as usize) {
                                *c += 1.0;
                            }
                        }
                    }
                }
                if cur.is_some() {
                    late_credit.push((room.unit.clone(), room.id.clone()));
                }
            }
            self.occupants[i] = cur.cloned();
        }

        // Shift turnover: fresh RNs everywhere — renumbered, churn zeroed,
        // pins cleared — plus a handoff line per staffed unit. Otherwise,
        // incrementally re-derive only the units whose census changed,
        // carrying churn and honoring pins where possible.
        if shift_count(now_min) > shift_count(self.last_now_min) {
            let mut fresh = BTreeMap::new();
            for u in h.units.iter().filter(|u| workload::is_rn_staffed(u.unit_type)) {
                let ur = Self::derive(self.weights, h, idx, u, None, now_min, log);
                if ur.rn_count > 0 {
                    log.push(SimLogEntry {
                        t_min: now_min,
                        message: format!("{}: shift change, {} RNs on", u.name, ur.rn_count),
                    });
                }
                fresh.insert(u.id.clone(), ur);
            }
            self.units = fresh;
        } else {
            for uid in &touched {
                let Some(&up) = idx.unit_pos.get(uid) else { continue };
                let prev = self.units.remove(uid);
                let ur =
                    Self::derive(self.weights, h, idx, &h.units[up], prev.as_ref(), now_min, log);
                self.units.insert(uid.clone(), ur);
            }
        }

        // Churn for newly occupied rooms goes to whoever now holds them.
        for (uid, rid) in late_credit {
            if let Some(ur) = self.units.get_mut(&uid) {
                if let Some(&rn) = ur.assignment.get(&rid) {
                    if let Some(c) = ur.churn.get_mut(rn as usize) {
                        *c += 1.0;
                    }
                }
            }
        }

        self.recompute_workloads(h, idx);
        self.last_now_min = now_min;
    }

    /// Manually reassign an occupied room to RN `rn` (0-based) and pin it
    /// there until the next turnover or an un-honorable re-derive. Returns
    /// `false` (and does nothing) if the room/unit/RN doesn't resolve.
    pub fn pin(
        &mut self,
        h: &Hospital,
        idx: &HospitalIndex,
        room: &RoomId,
        rn: u16,
        now_min: f64,
        log: &mut Vec<SimLogEntry>,
    ) -> bool {
        let Some(&ri) = idx.room_pos.get(room) else { return false };
        let r = &h.rooms[ri];
        if r.status.patient().is_none() {
            return false;
        }
        let Some(ur) = self.units.get_mut(&r.unit) else { return false };
        if (rn as usize) >= ur.rn_count {
            return false;
        }
        ur.pins.insert(room.clone(), rn);
        ur.assignment.insert(room.clone(), rn);
        Self::rebuild_rn_rooms(ur, h, idx, &r.unit);
        self.recompute_workloads(h, idx);
        let unit_name = idx
            .unit_pos
            .get(&r.unit)
            .map(|&up| h.units[up].name.as_str())
            .unwrap_or("?");
        log.push(SimLogEntry {
            t_min: now_min,
            message: format!("{}: room {} reassigned to RN #{}", unit_name, r.number, rn + 1),
        });
        true
    }

    /// Remove a manual pin; the unit re-derives its natural assignment.
    pub fn unpin(
        &mut self,
        h: &Hospital,
        idx: &HospitalIndex,
        room: &RoomId,
        now_min: f64,
        log: &mut Vec<SimLogEntry>,
    ) -> bool {
        let Some(&ri) = idx.room_pos.get(room) else { return false };
        let unit_id = h.rooms[ri].unit.clone();
        let Some(&up) = idx.unit_pos.get(&unit_id) else { return false };
        let Some(ur) = self.units.get_mut(&unit_id) else { return false };
        if ur.pins.remove(room).is_none() {
            return false;
        }
        let prev = self.units.remove(&unit_id);
        let fresh = Self::derive(self.weights, h, idx, &h.units[up], prev.as_ref(), now_min, log);
        self.units.insert(unit_id, fresh);
        self.recompute_workloads(h, idx);
        log.push(SimLogEntry {
            t_min: now_min,
            message: format!(
                "{}: room {} unpinned",
                h.units[up].name, h.rooms[ri].number
            ),
        });
        true
    }

    /// Silent full regeneration: snapshot + every staffed unit from scratch.
    fn rebuild(&mut self, h: &Hospital, idx: &HospitalIndex, now_min: f64) {
        self.room_ids = h.rooms.iter().map(|r| r.id.clone()).collect();
        self.occupants = h
            .rooms
            .iter()
            .map(|r| r.status.patient().map(|p| p.id.clone()))
            .collect();
        self.units.clear();
        let mut sink = Vec::new(); // fresh derives can't drop pins; discard
        for u in h.units.iter().filter(|u| workload::is_rn_staffed(u.unit_type)) {
            let ur = Self::derive(self.weights, h, idx, u, None, now_min, &mut sink);
            self.units.insert(u.id.clone(), ur);
        }
        self.recompute_workloads(h, idx);
        self.last_now_min = now_min;
        self.primed = true;
    }

    /// Derive one unit's roster. With `prev`, churn counters carry over
    /// (index-aligned) and pins are honored where possible — a pin dies,
    /// with a log line, when its room vacated or its RN index fell off the
    /// shrunk roster. Without `prev` (turnover, rebuild) everything is fresh.
    fn derive(
        weights: WorkloadWeights,
        h: &Hospital,
        idx: &HospitalIndex,
        unit: &Unit,
        prev: Option<&UnitRoster>,
        now_min: f64,
        log: &mut Vec<SimLogEntry>,
    ) -> UnitRoster {
        let ratio = workload::effective_rn_ratio(unit);
        let rows: Vec<usize> = idx
            .room_indices(&unit.id)
            .iter()
            .copied()
            .filter(|&i| h.rooms[i].status.patient().is_some())
            .collect();
        let census = rows.len();

        let mut roster = UnitRoster::default();
        if census == 0 {
            if let Some(prev) = prev {
                for room in prev.pins.keys() {
                    log_pin_drop(log, now_min, unit, h, idx, room, "room vacated");
                }
            }
            return roster;
        }
        let rn_count = ((census as f32 / ratio).ceil() as usize).max(1);
        roster.rn_count = rn_count;

        // Carry honorable pins; log the rest as dropped.
        if let Some(prev) = prev {
            let occupied: BTreeSet<&RoomId> = rows.iter().map(|&i| &h.rooms[i].id).collect();
            for (room, &rn) in &prev.pins {
                if !occupied.contains(room) {
                    log_pin_drop(log, now_min, unit, h, idx, room, "room vacated");
                } else if (rn as usize) >= rn_count {
                    log_pin_drop(log, now_min, unit, h, idx, room, "RN off roster");
                } else {
                    roster.pins.insert(room.clone(), rn);
                }
            }
            roster.churn = prev.churn.clone();
        }
        roster.churn.resize(rn_count, 0.0);

        // Marginal load of one occupied room (see workload::room_load).
        let load_of = |i: usize| -> f32 {
            let p = h.rooms[i].status.patient().expect("rows are occupied");
            workload::room_load(
                &weights,
                ratio,
                &RnLoadInput {
                    instability: p.acuity.instability,
                    isolation: p.isolation.is_some(),
                    telemetry: p.telemetry,
                },
            )
        };

        // Free rooms (corridor order, pins excluded) split into one
        // contiguous run per RN; pinned rooms and churn pre-load their RN so
        // the balance pass routes fewer rooms toward already-burdened RNs.
        let free: Vec<usize> = rows
            .iter()
            .copied()
            .filter(|&i| !roster.pins.contains_key(&h.rooms[i].id))
            .collect();
        let mut base = vec![0.0f32; rn_count];
        for (rn, c) in roster.churn.iter().enumerate() {
            base[rn] += weights.churn * c;
        }
        for (room, &rn) in &roster.pins {
            if let Some(&i) = idx.room_pos.get(room) {
                base[rn as usize] += load_of(i);
            }
        }
        let loads: Vec<f32> = free.iter().map(|&i| load_of(i)).collect();
        let cuts = balance_cuts(&loads, &base);

        // Materialize the assignment: run j → RN j, pins overlaid on top.
        let run_bounds = |j: usize| -> (usize, usize) {
            let s = if j == 0 { 0 } else { cuts[j - 1] };
            let e = if j + 1 == rn_count { free.len() } else { cuts[j] };
            (s, e)
        };
        for j in 0..rn_count {
            let (s, e) = run_bounds(j);
            for &i in &free[s..e] {
                roster.assignment.insert(h.rooms[i].id.clone(), j as u16);
            }
        }
        for (room, &rn) in &roster.pins {
            roster.assignment.insert(room.clone(), rn);
        }
        Self::rebuild_rn_rooms(&mut roster, h, idx, &unit.id);
        roster
    }

    /// Per-RN room lists in unit corridor order, from the assignment map.
    fn rebuild_rn_rooms(ur: &mut UnitRoster, h: &Hospital, idx: &HospitalIndex, unit: &UnitId) {
        ur.rn_rooms = vec![Vec::new(); ur.rn_count];
        for &i in idx.room_indices(unit) {
            if let Some(&rn) = ur.assignment.get(&h.rooms[i].id) {
                ur.rn_rooms[rn as usize].push(h.rooms[i].id.clone());
            }
        }
    }

    /// Covering RN per room — (0-based index, workload composite scalar),
    /// row-parallel to `Hospital.rooms` — the placement engine's RN-headroom
    /// input (EP-14). An occupied room's cover is its assigned RN; a vacant
    /// room's is the RN of the nearest occupied room in unit corridor order
    /// (ties toward the corridor start): the runs are contiguous, so that is
    /// the RN whose territory the empty bed sits in or beside — who would
    /// most naturally absorb it. `None` where no roster makes a claim
    /// (unstaffed unit type, empty unit, roster not yet derived). Read-only:
    /// touches no sim state and no RNG.
    pub fn covering_rn_loads(
        &self,
        h: &Hospital,
        idx: &HospitalIndex,
    ) -> Vec<Option<(u16, f32)>> {
        let mut out = vec![None; h.rooms.len()];
        for (uid, ur) in &self.units {
            if ur.rn_count == 0 {
                continue;
            }
            let rows = idx.room_indices(uid);
            let assigned: Vec<Option<u16>> = rows
                .iter()
                .map(|&i| ur.assignment.get(&h.rooms[i].id).copied())
                .collect();
            for (p, &i) in rows.iter().enumerate() {
                let rn = assigned[p].or_else(|| {
                    (1..rows.len()).find_map(|d| {
                        p.checked_sub(d)
                            .and_then(|q| assigned[q])
                            .or_else(|| assigned.get(p + d).copied().flatten())
                    })
                });
                if let Some(rn) = rn {
                    let load = ur.workloads.get(rn as usize).map_or(0.0, |w| w.scalar());
                    out[i] = Some((rn, load));
                }
            }
        }
        out
    }

    /// Recompute every RN's workload breakdown from live patient state +
    /// churn. Runs every maintenance pass — acuity drifts continuously, and
    /// the composite must track it without re-deriving assignments.
    fn recompute_workloads(&mut self, h: &Hospital, idx: &HospitalIndex) {
        let weights = self.weights;
        for (uid, ur) in &mut self.units {
            let Some(&up) = idx.unit_pos.get(uid) else { continue };
            let ratio = workload::effective_rn_ratio(&h.units[up]);
            ur.workloads = (0..ur.rn_count)
                .map(|rn| {
                    let patients: Vec<RnLoadInput> = ur.rn_rooms[rn]
                        .iter()
                        .filter_map(|rid| idx.room_pos.get(rid))
                        .filter_map(|&i| h.rooms[i].status.patient())
                        .map(|p| RnLoadInput {
                            instability: p.acuity.instability,
                            isolation: p.isolation.is_some(),
                            telemetry: p.telemetry,
                        })
                        .collect();
                    workload::compose(&weights, ratio, &patients, ur.churn[rn])
                })
                .collect();
        }
    }
}

fn log_pin_drop(
    log: &mut Vec<SimLogEntry>,
    now_min: f64,
    unit: &Unit,
    h: &Hospital,
    idx: &HospitalIndex,
    room: &RoomId,
    reason: &str,
) {
    let number = idx
        .room_pos
        .get(room)
        .map(|&i| h.rooms[i].number.as_str())
        .unwrap_or(room.as_str());
    log.push(SimLogEntry {
        t_min: now_min,
        message: format!("{}: pin dropped for room {number} ({reason})", unit.name),
    });
}

/// Cut positions splitting `loads` (per-room marginal loads, corridor
/// order) into `base.len()` contiguous runs: equal-count split, then a
/// bounded greedy pass that moves boundary rooms from the heavier to the
/// lighter neighbor while it strictly reduces the pairwise imbalance.
/// Deterministic: strict left-to-right sweeps, strictly-improving moves
/// only, and every candidate is examined in room-index order.
fn balance_cuts(loads: &[f32], base: &[f32]) -> Vec<usize> {
    const EPS: f32 = 1e-4;
    const MAX_PASSES: usize = 32;

    let k = loads.len();
    let n = base.len();
    if n <= 1 {
        return Vec::new();
    }

    // Equal-count split; the first (k mod n) runs take the extra room.
    let (q, r) = (k / n, k % n);
    let mut cuts = Vec::with_capacity(n - 1);
    let mut acc = 0;
    for j in 0..n - 1 {
        acc += q + usize::from(j < r);
        cuts.push(acc);
    }
    let init = cuts.clone();

    let mut prefix = vec![0.0f32; k + 1];
    for (i, l) in loads.iter().enumerate() {
        prefix[i + 1] = prefix[i] + l;
    }
    let run_load = |cuts: &[usize], j: usize| -> f32 {
        let s = if j == 0 { 0 } else { cuts[j - 1] };
        let e = if j + 1 == n { k } else { cuts[j] };
        prefix[e] - prefix[s] + base[j]
    };

    // Keep every run non-empty when there are enough rooms to go around.
    let min_run = usize::from(k >= n);
    for _ in 0..MAX_PASSES {
        let mut improved = false;
        for j in 0..n - 1 {
            let lo = (if j == 0 { 0 } else { cuts[j - 1] } + min_run)
                .max(init[j].saturating_sub(MAX_BOUNDARY_SHIFT));
            let hi = (if j + 2 == n { k } else { cuts[j + 1] })
                .saturating_sub(min_run)
                .min(init[j] + MAX_BOUNDARY_SHIFT);
            if lo > hi {
                continue;
            }
            let d = run_load(&cuts, j) - run_load(&cuts, j + 1);
            if d > EPS && cuts[j] > lo {
                // Give run j's last room to run j+1.
                let l = loads[cuts[j] - 1];
                if (d - 2.0 * l).abs() < d.abs() - EPS {
                    cuts[j] -= 1;
                    improved = true;
                }
            } else if d < -EPS && cuts[j] < hi {
                // Take run j+1's first room into run j.
                let l = loads[cuts[j]];
                if (d + 2.0 * l).abs() < d.abs() - EPS {
                    cuts[j] += 1;
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
    cuts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acuity::AcuityDrift;
    use crate::diurnal::DiurnalFlow;
    use crate::edflow::EdPressure;
    use crate::engine::SimEngine;
    use crate::testworld::world;
    use hupsim_core::model::UnitType;
    use hupsim_core::ops;
    use hupsim_core::patient::{Acuity, Patient};

    fn pt(name: &str, instability: f32, isolation: bool, telemetry: bool) -> Patient {
        Patient {
            id: name.into(),
            alias: name.into(),
            age: Some(60),
            service: None,
            admitted_at_min: Some(0.0),
            acuity: Acuity { instability, trend_per_hr: 0.0 },
            boarding: false,
            isolation: isolation.then(|| "contact".to_string()),
            telemetry,
            note: String::new(),
        }
    }

    /// Occupy the first `n` rooms of `unit` with stable patients.
    fn occupy(
        h: &mut hupsim_core::model::Hospital,
        idx: &hupsim_core::index::HospitalIndex,
        unit: &str,
        n: usize,
    ) {
        let rooms: Vec<RoomId> = idx.room_indices(&unit.into())[..n]
            .iter()
            .map(|&i| h.rooms[i].id.clone())
            .collect();
        for (k, room) in rooms.iter().enumerate() {
            ops::admit(h, idx, room, pt(&format!("{unit}.p{k}"), 0.1, false, false)).unwrap();
        }
    }

    #[test]
    fn roster_sizes_from_ratio_and_census() {
        let (mut h, idx) = world(&[("ms", UnitType::MedSurg, 30)]);
        h.units[0].target_rn_ratio = Some(6.0);
        let mut roster = RnRoster::default();
        let mut log = Vec::new();

        // Empty unit: no RNs on.
        roster.maintain(&h, &idx, 0.0, &mut log);
        assert_eq!(roster.unit(&"ms".into()).unwrap().rn_count, 0);

        // 12 patients at 6:1 → 2 RNs; every occupied room assigned.
        occupy(&mut h, &idx, "ms", 12);
        roster.maintain(&h, &idx, 10.0, &mut log);
        let ur = roster.unit(&"ms".into()).unwrap();
        assert_eq!(ur.rn_count, 2);
        assert_eq!(ur.assignment.len(), 12);
        assert_eq!(ur.workloads.len(), 2);

        // Single patient: min 1 RN even far below ratio.
        let (mut h1, idx1) = world(&[("ms", UnitType::MedSurg, 30)]);
        occupy(&mut h1, &idx1, "ms", 1);
        let mut r1 = RnRoster::default();
        r1.maintain(&h1, &idx1, 0.0, &mut log);
        assert_eq!(r1.unit(&"ms".into()).unwrap().rn_count, 1);
    }

    #[test]
    fn assignment_is_contiguous_in_corridor_order() {
        let (mut h, idx) = world(&[("ms", UnitType::MedSurg, 30)]);
        h.units[0].target_rn_ratio = Some(4.0);
        occupy(&mut h, &idx, "ms", 11); // → 3 RNs
        let mut roster = RnRoster::default();
        roster.maintain(&h, &idx, 0.0, &mut Vec::new());
        let ur = roster.unit(&"ms".into()).unwrap();
        assert_eq!(ur.rn_count, 3);
        // Walking the corridor, RN indices never decrease (contiguous runs),
        // and every RN holds at least one room.
        let rn_seq: Vec<u16> = idx.room_indices(&"ms".into())
            .iter()
            .filter_map(|&i| ur.assignment.get(&h.rooms[i].id).copied())
            .collect();
        assert_eq!(rn_seq.len(), 11);
        assert!(rn_seq.windows(2).all(|w| w[0] <= w[1]), "runs must be contiguous: {rn_seq:?}");
        for rn in 0..3u16 {
            assert!(rn_seq.contains(&rn), "RN #{} has no rooms", rn + 1);
        }
    }

    #[test]
    fn balance_evens_out_a_skewed_acuity_unit() {
        // Heavy end of the corridor: 4 unstable isolation+telemetry
        // patients, then 8 stable walkie-talkies. Naive equal-count split
        // would give RN #1 all four heavy rooms.
        let (mut h, idx) = world(&[("ms", UnitType::MedSurg, 12)]);
        h.units[0].target_rn_ratio = Some(4.0);
        let rooms: Vec<RoomId> = idx.room_indices(&"ms".into())
            .iter()
            .map(|&i| h.rooms[i].id.clone())
            .collect();
        for (k, room) in rooms.iter().enumerate() {
            let p = if k < 4 {
                pt(&format!("heavy{k}"), 0.9, true, true)
            } else {
                pt(&format!("light{k}"), 0.05, false, false)
            };
            ops::admit(&mut h, &idx, room, p).unwrap();
        }
        let mut roster = RnRoster::default();
        roster.maintain(&h, &idx, 0.0, &mut Vec::new());
        let ur = roster.unit(&"ms".into()).unwrap();
        assert_eq!(ur.rn_count, 3);

        let scalars: Vec<f32> = ur.workloads.iter().map(|w| w.scalar()).collect();
        let (min, max) = (
            scalars.iter().cloned().fold(f32::INFINITY, f32::min),
            scalars.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        );
        // Naive split imbalance is ≈ 4.3; balanced must land within one
        // heavy-room granule (the best any contiguous split can do).
        let heavy_room = hupsim_core::workload::room_load(
            &roster.weights,
            4.0,
            &hupsim_core::workload::RnLoadInput { instability: 0.9, isolation: true, telemetry: true },
        );
        assert!(
            max - min <= heavy_room + 1e-3,
            "max-min {} should be within one heavy room load {heavy_room} (scalars {scalars:?})",
            max - min
        );
    }

    #[test]
    fn turnover_renumbers_resets_churn_and_logs_handoff() {
        let (mut h, idx) = world(&[("ms", UnitType::MedSurg, 30)]);
        h.units[0].target_rn_ratio = Some(5.0);
        occupy(&mut h, &idx, "ms", 10);
        let mut roster = RnRoster::default();
        let mut log = Vec::new();
        roster.maintain(&h, &idx, 100.0, &mut log);

        // Accrue churn: discharge one room, admit another.
        let uid: UnitId = "ms".into();
        let first = h.rooms[idx.room_indices(&uid)[0]].id.clone();
        ops::discharge(&mut h, &idx, &first).unwrap();
        roster.maintain(&h, &idx, 200.0, &mut log);
        let churn_before: f32 = roster.unit(&uid).unwrap().churn.iter().sum();
        assert!(churn_before > 0.9, "discharge must credit churn, got {churn_before}");
        // Pin something so we can watch turnover clear it.
        let pinned = h.rooms[idx.room_indices(&uid)[3]].id.clone();
        assert!(roster.pin(&h, &idx, &pinned, 1, 200.0, &mut log));
        assert!(log.iter().any(|l| l.message.contains("reassigned to RN #2")));

        // Cross 07:00 (minute 420): handoff.
        roster.maintain(&h, &idx, 430.0, &mut log);
        let ur = roster.unit(&uid).unwrap();
        assert_eq!(ur.rn_count, 2, "9 patients at 5:1 → 2 RNs on the fresh shift");
        assert!(ur.churn.iter().all(|&c| c == 0.0), "turnover resets churn");
        assert!(ur.pins.is_empty(), "pins do not survive a handoff");
        assert!(
            log.iter().any(|l| l.message == "MS: shift change, 2 RNs on"),
            "handoff line missing: {log:?}"
        );
    }

    #[test]
    fn pin_survives_unrelated_admission_and_drop_is_logged_when_roster_shrinks() {
        let (mut h, idx) = world(&[("ms", UnitType::MedSurg, 30)]);
        h.units[0].target_rn_ratio = Some(4.0);
        occupy(&mut h, &idx, "ms", 9); // → 3 RNs
        let mut roster = RnRoster::default();
        let mut log = Vec::new();
        roster.maintain(&h, &idx, 10.0, &mut log);
        let uid: UnitId = "ms".into();

        // Pin room 0 (naturally RN #1 territory) to RN #3.
        let pinned = h.rooms[idx.room_indices(&uid)[0]].id.clone();
        assert!(roster.pin(&h, &idx, &pinned, 2, 10.0, &mut log));
        assert_eq!(roster.rn_of(&uid, &pinned), Some(2));

        // Unrelated admission re-derives the unit; the pin must hold.
        let vacant = h.rooms[idx.room_indices(&uid)[20]].id.clone();
        ops::admit(&mut h, &idx, &vacant, pt("newcomer", 0.2, false, false)).unwrap();
        roster.maintain(&h, &idx, 20.0, &mut log);
        assert_eq!(roster.rn_of(&uid, &pinned), Some(2), "pin must survive unrelated census change");
        assert!(roster.unit(&uid).unwrap().pins.contains_key(&pinned));

        // Discharge down to 2 patients (incl. the pinned room) → 1 RN;
        // RN #3 no longer exists, so the pin drops with a log line.
        let keep: Vec<RoomId> = [0usize, 20]
            .iter()
            .map(|&k| h.rooms[idx.room_indices(&uid)[k]].id.clone())
            .collect();
        let occupied: Vec<RoomId> = idx.room_indices(&uid)
            .iter()
            .filter(|&&i| h.rooms[i].status.patient().is_some())
            .map(|&i| h.rooms[i].id.clone())
            .collect();
        for room in occupied.iter().filter(|r| !keep.contains(r)) {
            ops::discharge(&mut h, &idx, room).unwrap();
        }
        roster.maintain(&h, &idx, 30.0, &mut log);
        let ur = roster.unit(&uid).unwrap();
        assert_eq!(ur.rn_count, 1);
        assert!(ur.pins.is_empty());
        assert_eq!(roster.rn_of(&uid, &pinned), Some(0));
        assert!(
            log.iter().any(|l| l.message.contains("pin dropped") && l.message.contains("RN off roster")),
            "dropped pin must be logged: {log:?}"
        );
    }

    #[test]
    fn churn_credits_touched_rns_and_decays() {
        let (mut h, idx) = world(&[("ms", UnitType::MedSurg, 30)]);
        h.units[0].target_rn_ratio = Some(5.0);
        occupy(&mut h, &idx, "ms", 5); // 1 RN — all credit lands on RN #1
        let mut roster = RnRoster::default();
        let mut log = Vec::new();
        roster.maintain(&h, &idx, 0.0, &mut log);
        let uid: UnitId = "ms".into();

        // One discharge + one admission = 2 churn events.
        let gone = h.rooms[idx.room_indices(&uid)[0]].id.clone();
        let fresh = h.rooms[idx.room_indices(&uid)[10]].id.clone();
        ops::discharge(&mut h, &idx, &gone).unwrap();
        ops::admit(&mut h, &idx, &fresh, pt("in", 0.2, false, false)).unwrap();
        roster.maintain(&h, &idx, 0.0, &mut log);
        let ur = roster.unit(&uid).unwrap();
        assert!((ur.churn[0] - 2.0).abs() < 1e-6, "churn {:?}", ur.churn);
        // The churn component shows up in the workload breakdown.
        assert!(ur.workloads[0].churn > 0.0);

        // One half-life later it has decayed by half.
        roster.maintain(&h, &idx, CHURN_HALFLIFE_MIN, &mut log);
        let ur = roster.unit(&uid).unwrap();
        assert!((ur.churn[0] - 1.0).abs() < 1e-3, "churn should halve, got {:?}", ur.churn);
    }

    #[test]
    fn rosters_are_deterministic_under_the_full_process_stack() {
        let run = |seed: u64| -> RnRoster {
            let (mut h, idx) = world(&[
                ("ms1", UnitType::MedSurg, 30),
                ("ms2", UnitType::MedSurg, 30),
                ("icu", UnitType::Icu, 4),
                ("sd", UnitType::Stepdown, 6),
                ("ed", UnitType::Ed, 12),
            ]);
            let mut eng = SimEngine::new(seed);
            eng.register(Box::new(DiurnalFlow::default()), true);
            eng.register(Box::new(EdPressure::default()), true);
            eng.register(Box::new(AcuityDrift::default()), true);
            for _ in 0..200 {
                eng.tick(&mut h, &idx, 30.0); // crosses several turnovers
            }
            eng.roster
        };
        let a = run(7);
        assert_eq!(a, run(7), "same seed + same tick sequence ⇒ identical rosters");
        assert!(
            a.units.values().any(|u| u.rn_count > 0),
            "the stack should have produced staffed units"
        );
    }

    #[test]
    fn covering_rn_loads_assigns_vacant_rooms_to_the_nearest_run() {
        // Rooms r0..r5; occupy r0,r1,r4,r5 at 2:1 → 2 RNs with contiguous
        // runs [r0,r1] and [r4,r5], a vacant gap r2,r3 between them.
        let (mut h, idx) = world(&[("ms", UnitType::MedSurg, 6), ("lab", UnitType::Research, 2)]);
        h.units[0].target_rn_ratio = Some(2.0);
        let uid: UnitId = "ms".into();
        let rooms: Vec<RoomId> = idx.room_indices(&uid)
            .iter()
            .map(|&i| h.rooms[i].id.clone())
            .collect();
        for k in [0usize, 1, 4, 5] {
            ops::admit(&mut h, &idx, &rooms[k], pt(&format!("p{k}"), 0.2, false, false)).unwrap();
        }
        let mut roster = RnRoster::default();
        roster.maintain(&h, &idx, 0.0, &mut Vec::new());
        let ur = roster.unit(&uid).unwrap();
        assert_eq!(ur.rn_count, 2);

        let cover = roster.covering_rn_loads(&h, &idx);
        let of = |k: usize| cover[idx.room_indices(&uid)[k]];
        // Occupied rooms carry their own assignment + that RN's live load.
        assert_eq!(of(0).unwrap().0, ur.assignment[&rooms[0]]);
        assert!((of(0).unwrap().1 - ur.workloads[of(0).unwrap().0 as usize].scalar()).abs() < 1e-6);
        // Vacant rooms fall to the nearest run: r2 borders r1 (RN #1's run),
        // r3 borders r4 (RN #2's run).
        assert_eq!(of(2).unwrap().0, ur.assignment[&rooms[1]]);
        assert_eq!(of(3).unwrap().0, ur.assignment[&rooms[4]]);
        // Research beds are outside the RN model: no claim.
        for &i in idx.room_indices(&"lab".into()) {
            assert_eq!(cover[i], None);
        }
        // An empty staffed unit makes no claim either.
        for room in rooms.iter().take(2).chain(rooms.iter().skip(4)) {
            ops::discharge(&mut h, &idx, room).unwrap();
        }
        roster.maintain(&h, &idx, 1.0, &mut Vec::new());
        let cover = roster.covering_rn_loads(&h, &idx);
        assert!(idx.room_indices(&uid).iter().all(|&i| cover[i].is_none()));
    }

    #[test]
    fn regenerates_from_census_after_clear_without_turnover_spam() {
        let (mut h, idx) = world(&[("ms", UnitType::MedSurg, 30)]);
        occupy(&mut h, &idx, "ms", 8);
        let mut roster = RnRoster::default();
        let mut log = Vec::new();
        roster.maintain(&h, &idx, 100.0, &mut log);
        let before = roster.unit(&"ms".into()).unwrap().clone();

        // Clear + re-maintain at a late sim time (a loaded scenario):
        // identical roster, and no phantom "shift change" logs.
        roster.clear();
        roster.maintain(&h, &idx, 2000.0, &mut log);
        assert_eq!(roster.unit(&"ms".into()).unwrap(), &before);
        assert!(
            !log.iter().any(|l| l.message.contains("shift change")),
            "regeneration must be silent: {log:?}"
        );
    }
}
