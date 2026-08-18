//! EP-16 — bed-huddle morning report: the daily snapshot a capacity-
//! management group runs its morning on. What came in overnight, who should
//! leave today, where the pressure sits — assembled as a pure function over
//! hospital + room matrix + rolling norms + sim log, so the same inputs
//! always produce the same report (and the same exported markdown).
//!
//! Honesty rules carried over from EP-9/EP-13/EP-15:
//! - Discharge readiness is a **labeled heuristic** (stability tier + a
//!   fraction of the EP-10 service-typical median LOS), never clinical truth;
//!   the report says so in its own header.
//! - Patients present before sim start have no admission timestamp; they are
//!   *counted as past the LOS threshold* and reported separately, not hidden.
//! - Norm badges keep the warm-up discipline: no z until the trailing window
//!   fills, no fake zeros.
//! - Log-derived counts are corroboration only; when the bounded sim log no
//!   longer reaches back to the window start, counts are labeled as lower
//!   bounds instead of pretending completeness.

use crate::engine::SimLogEntry;
use crate::params;
use crate::rolling::{RollingSeries, RollingStore};
use hupsim_core::aggregate::UnitAggregate;
use hupsim_core::matrix::{OccState, RoomMatrix, NO_ADMIT_MIN, NO_SERVICE_LINE, NO_UNIT};
use hupsim_core::model::{Hospital, UnitType};
use hupsim_core::patient::StabilityTier;

/// The overnight window: admissions in the trailing 12 h ("since the
/// previous 19:00" when the report is pulled at 07:00).
pub const OVERNIGHT_LOOKBACK_MIN: f64 = 12.0 * 60.0;

/// Discharge-readiness stability bound — the [`StabilityTier::Stable`]
/// boundary: only patients below it are anticipated discharges.
pub const DISCHARGE_READY_MAX_INSTABILITY: f32 = 0.25;

/// Fraction of the service-typical median LOS a patient must have used
/// before the heuristic anticipates a discharge today.
pub const DISCHARGE_LOS_FRACTION: f64 = 0.75;

/// The EP-8 placeholder line for units with no public service identity —
/// its pressure row carries the same [Unverified] marker as the dashboard.
pub const OTHER_LINE_ID: &str = "line.other";

/// "day 3, 07:00" — the same convention as `SimClock::label` (day is
/// 1-based), reproduced here so the report crate-side stays clock-free.
pub fn sim_time_label(t_min: f64) -> String {
    let total = t_min.max(0.0) as u64;
    format!("day {}, {:02}:{:02}", total / 1440 + 1, (total / 60) % 24, total % 60)
}

/// Deviation-vs-norm state for one pressure row. Mirrors the app's ZBadge
/// semantics (EP-9 warm-up honesty) without depending on egui.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NormBadge {
    /// Trailing window not yet full — no norm claim.
    Warming { have: usize, need: usize },
    /// Window full but flat (σ ≈ 0) — no meaningful z.
    Flat,
    Z(f32),
}

impl NormBadge {
    pub fn for_series(series: &RollingSeries, current: f32) -> Self {
        if !series.is_warm() {
            NormBadge::Warming {
                have: series.len(),
                need: series.window(),
            }
        } else {
            match series.z(current) {
                Some(z) => NormBadge::Z(z),
                None => NormBadge::Flat,
            }
        }
    }

    /// Markdown/table cell text.
    pub fn text(&self) -> String {
        match self {
            NormBadge::Warming { have, need } => format!("warming {have}/{need}"),
            NormBadge::Flat => "flat".to_string(),
            NormBadge::Z(z) => format!("{z:+.1}"),
        }
    }

    /// Pressure-sort key: warming rows carry no norm claim and sort below
    /// any real z; a flat norm reads as z 0 (same rule as the dashboard).
    fn sort_z(&self) -> f32 {
        match self {
            NormBadge::Warming { .. } => f32::NEG_INFINITY,
            NormBadge::Flat => 0.0,
            NormBadge::Z(z) => *z,
        }
    }
}

/// One overnight-admits group: a service line (or the no-line bucket) with
/// its admit count and stability-tier mix (Stable, Watcher, Unstable,
/// Critical — [`StabilityTier`] order).
#[derive(Debug, Clone, PartialEq)]
pub struct OvernightRow {
    pub name: String,
    pub count: usize,
    pub tiers: [u32; 4],
}

/// One anticipated discharge: an occupied bed whose patient passes the
/// labeled readiness heuristic.
#[derive(Debug, Clone, PartialEq)]
pub struct DischargeRow {
    pub room_number: String,
    pub unit_name: String,
    pub alias: String,
    pub instability: f32,
    /// Elapsed LOS in hours; `None` = admitted before sim start.
    pub los_hr: Option<f64>,
    /// Service-typical median LOS (EP-10 table) the fraction was taken of.
    pub typical_hr: f64,
}

/// One current boarder: an admitted inpatient physically held in a
/// non-inpatient space (EP-10 ED conversions that found no bed).
#[derive(Debug, Clone, PartialEq)]
pub struct BoarderRow {
    pub alias: String,
    pub room_number: String,
    pub unit_name: String,
    pub instability: f32,
    pub telemetry: bool,
    pub isolation: bool,
    /// Hours since arrival; `None` = present before sim start.
    pub waited_hr: Option<f64>,
}

/// One pressure-table row (per level of care or per service line).
#[derive(Debug, Clone, PartialEq)]
pub struct PressureRow {
    pub name: String,
    pub census: usize,
    /// Licensed minus out-of-service — the occupancy denominator.
    pub staffed: usize,
    pub capacity: usize,
    /// `None` when the group has no staffed beds.
    pub occupancy_pct: Option<f32>,
    /// Occupancy vs the group's trailing-24 h norm.
    pub occ_badge: NormBadge,
    pub boarding: usize,
    /// Anticipated discharges in this group (the heuristic above).
    pub expected_dc: usize,
    /// Overnight-window admissions into this group.
    pub recent_admits: usize,
}

impl PressureRow {
    /// Expected net beds over the morning: anticipated discharges minus the
    /// recent arrival rate (overnight admits as its proxy). Positive =
    /// decompressing, negative = filling.
    pub fn expected_net(&self) -> i64 {
        self.expected_dc as i64 - self.recent_admits as i64
    }
}

/// Everything the assembler reads. The matrix must be fresh for this
/// hospital (the app passes its version-keyed aggregate cache's matrix; the
/// engine's own copy is stale while paused).
pub struct ReportInputs<'a> {
    pub hospital: &'a Hospital,
    pub matrix: &'a RoomMatrix,
    pub rolling: &'a RollingStore,
    pub log: &'a [SimLogEntry],
    pub now_min: f64,
    pub scenario_name: &'a str,
    pub seed: u64,
}

/// The assembled snapshot — everything the report view paints and the
/// markdown export writes.
#[derive(Debug, Clone, PartialEq)]
pub struct MorningReport {
    pub scenario_name: String,
    pub seed: u64,
    pub now_min: f64,
    /// Window start clamped to sim epoch (display; filtering uses the raw
    /// 12 h lookback, which admits at t ≥ 0 satisfy identically).
    pub window_start_min: f64,
    pub overnight: Vec<OvernightRow>,
    pub overnight_total: usize,
    /// Log-corroborated activity in the window: bed admissions (diurnal +
    /// queued events), ED decisions-to-admit (boarding starts — these are
    /// the section's boarder admits), and inpatient discharges.
    pub log_admissions: usize,
    pub log_ed_decisions: usize,
    pub log_discharges: usize,
    /// False when the bounded log no longer reaches the window start — the
    /// counts above are then lower bounds.
    pub log_covers_window: bool,
    pub discharges: Vec<DischargeRow>,
    /// How many of `discharges` were admitted before sim start (unknown LOS,
    /// counted as past threshold — the labeled rule).
    pub discharges_pre_sim: usize,
    pub boarders: Vec<BoarderRow>,
    pub pressure_by_loc: Vec<PressureRow>,
    pub pressure_by_line: Vec<PressureRow>,
}

/// Assemble the morning report. Pure and deterministic: no RNG, no clock
/// reads, no mutation — same inputs, same report, tick for tick.
pub fn assemble(inp: &ReportInputs) -> MorningReport {
    let h = inp.hospital;
    let m = inp.matrix;
    assert_eq!(
        m.len(),
        h.rooms.len(),
        "morning report needs a matrix refreshed for this hospital"
    );

    let n_lines = h.service_lines.len();
    let window_start_raw = inp.now_min - OVERNIGHT_LOOKBACK_MIN;

    // Overnight buckets: one per service line, plus a trailing no-line slot.
    let mut overnight_counts = vec![0usize; n_lines + 1];
    let mut overnight_tiers = vec![[0u32; 4]; n_lines + 1];
    let mut admits_by_loc = vec![0usize; UnitType::ALL.len()];
    let mut dc_by_loc = vec![0usize; UnitType::ALL.len()];
    let mut dc_by_line = vec![0usize; n_lines];
    let mut discharges: Vec<DischargeRow> = Vec::new();
    let mut discharges_pre_sim = 0usize;
    // (sort key, insertion index, row) — longest wait first, unknown last.
    let mut boarders: Vec<(f64, usize, BoarderRow)> = Vec::new();

    for i in 0..m.len() {
        if m.occupancy[i] != OccState::Occupied {
            continue;
        }
        let ut = m.unit_type[i];
        let line = m.service_line[i];
        let admit_min = m.admitted_at_min[i];
        let known_admit = admit_min != NO_ADMIT_MIN;
        let boarding = m.boarding[i];
        let tier = tier_slot(m.instability[i]);

        // Overnight admits: bed admissions (inpatient level of care) plus
        // admitted boarders still held elsewhere, stamped in the window.
        // ED treat-and-observe occupants are arrivals, not admissions.
        if known_admit
            && admit_min as f64 >= window_start_raw
            && (params::is_inpatient_unit(ut) || boarding)
        {
            let bucket = if line == NO_SERVICE_LINE { n_lines } else { line as usize };
            overnight_counts[bucket] += 1;
            overnight_tiers[bucket][tier] += 1;
            admits_by_loc[ut.index()] += 1;
        }

        // Anticipated discharges: the labeled heuristic. Boarders are
        // excluded — they were just admitted, whatever their stability.
        if params::is_inpatient_unit(ut)
            && !boarding
            && m.instability[i] < DISCHARGE_READY_MAX_INSTABILITY
        {
            let line_id =
                (line != NO_SERVICE_LINE).then(|| &h.service_lines[line as usize].id);
            let typical_hr = params::los_for(line_id, ut).median_hr;
            let los_hr = known_admit.then(|| (inp.now_min - admit_min as f64) / 60.0);
            let past_threshold = match los_hr {
                Some(e) => e >= DISCHARGE_LOS_FRACTION * typical_hr,
                // Pre-sim admission: LOS unknown, counted as past threshold
                // (reported separately, stated in the header).
                None => true,
            };
            if past_threshold {
                if los_hr.is_none() {
                    discharges_pre_sim += 1;
                }
                dc_by_loc[ut.index()] += 1;
                if line != NO_SERVICE_LINE {
                    dc_by_line[line as usize] += 1;
                }
                discharges.push(DischargeRow {
                    room_number: h.rooms[i].number.clone(),
                    unit_name: unit_name(h, m, i),
                    alias: alias_of(h, i),
                    instability: m.instability[i],
                    los_hr,
                    typical_hr,
                });
            }
        }

        if boarding {
            let waited_hr = known_admit.then(|| (inp.now_min - admit_min as f64) / 60.0);
            boarders.push((
                waited_hr.unwrap_or(f64::NEG_INFINITY),
                i,
                BoarderRow {
                    alias: alias_of(h, i),
                    room_number: h.rooms[i].number.clone(),
                    unit_name: unit_name(h, m, i),
                    instability: m.instability[i],
                    telemetry: m.telemetry_need[i],
                    isolation: m.isolation[i],
                    waited_hr,
                },
            ));
        }
    }

    boarders.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let boarders: Vec<BoarderRow> = boarders.into_iter().map(|(_, _, r)| r).collect();

    // Overnight rows in taxonomy order (the pressure sort belongs to the
    // pressure tables; this section reads as a fixed-order roster).
    let mut overnight = Vec::new();
    for b in 0..=n_lines {
        if overnight_counts[b] == 0 {
            continue;
        }
        let name = if b == n_lines {
            "(no service line)".to_string()
        } else {
            h.service_lines[b].name.clone()
        };
        overnight.push(OvernightRow {
            name,
            count: overnight_counts[b],
            tiers: overnight_tiers[b],
        });
    }
    let overnight_total = overnight.iter().map(|r| r.count).sum();

    // Log corroboration: engine-format admission/discharge messages inside
    // the window. Formats pinned by tests here and in the engine.
    let mut log_admissions = 0usize;
    let mut log_ed_decisions = 0usize;
    let mut log_discharges = 0usize;
    for e in inp.log {
        if e.t_min < window_start_raw || e.t_min > inp.now_min {
            continue;
        }
        if e.message.starts_with("admission: ") || e.message.starts_with("admission → ") {
            log_admissions += 1;
        } else if e.message.starts_with("ED decision to admit") {
            log_ed_decisions += 1;
        } else if e.message.starts_with("discharge ") && e.message.contains(" ← ") {
            log_discharges += 1;
        }
    }
    let log_covers_window = match inp.log.first() {
        Some(first) => first.t_min <= window_start_raw.max(0.0),
        // Empty log: fine on an untouched world, a gap if sim time passed.
        None => inp.now_min <= 0.0,
    };

    // Pressure tables from the matrix rollups + rolling norms.
    let window = inp.rolling.window();
    let mut pressure_by_loc: Vec<PressureRow> = m
        .rollup_by_unit_type()
        .iter()
        .filter(|(_, agg)| agg.capacity > 0)
        .map(|(t, agg)| {
            pressure_row(
                t.label().to_string(),
                agg,
                Some(&inp.rolling.unit_type(*t).occupancy_frac),
                window,
                dc_by_loc[t.index()],
                admits_by_loc[t.index()],
            )
        })
        .collect();
    sort_pressure(&mut pressure_by_loc);

    // Recent admits per line reuse the overnight buckets (same definition).
    let by_line = m.rollup_by_service_line(n_lines);
    let mut pressure_by_line: Vec<PressureRow> = h
        .service_lines
        .iter()
        .zip(&by_line)
        .enumerate()
        .filter(|(_, (_, agg))| agg.capacity > 0)
        .map(|(i, (line, agg))| {
            let name = if line.id.as_str() == OTHER_LINE_ID {
                format!("{} [Unverified]", line.name)
            } else {
                line.name.clone()
            };
            pressure_row(
                name,
                agg,
                inp.rolling
                    .service_lines
                    .get(&line.id)
                    .map(|w| &w.occupancy_frac),
                window,
                dc_by_line[i],
                overnight_counts[i],
            )
        })
        .collect();
    sort_pressure(&mut pressure_by_line);

    MorningReport {
        scenario_name: inp.scenario_name.to_string(),
        seed: inp.seed,
        now_min: inp.now_min,
        window_start_min: window_start_raw.max(0.0),
        overnight,
        overnight_total,
        log_admissions,
        log_ed_decisions,
        log_discharges,
        log_covers_window,
        discharges,
        discharges_pre_sim,
        boarders,
        pressure_by_loc,
        pressure_by_line,
    }
}

fn tier_slot(instability: f32) -> usize {
    match StabilityTier::from_instability(instability) {
        StabilityTier::Stable => 0,
        StabilityTier::Watcher => 1,
        StabilityTier::Unstable => 2,
        StabilityTier::Critical => 3,
    }
}

fn unit_name(h: &Hospital, m: &RoomMatrix, row: usize) -> String {
    let up = m.unit_pos[row];
    if up == NO_UNIT {
        "(unknown unit)".to_string()
    } else {
        h.units[up as usize].name.clone()
    }
}

fn alias_of(h: &Hospital, row: usize) -> String {
    h.rooms[row]
        .status
        .patient()
        .map(|p| p.alias.clone())
        .unwrap_or_default()
}

fn pressure_row(
    name: String,
    agg: &UnitAggregate,
    occ_series: Option<&RollingSeries>,
    window: usize,
    expected_dc: usize,
    recent_admits: usize,
) -> PressureRow {
    let staffed = agg.capacity.saturating_sub(agg.out_of_service);
    let occ_badge = match occ_series {
        None => NormBadge::Warming { have: 0, need: window },
        Some(s) => NormBadge::for_series(s, agg.occupancy_frac()),
    };
    PressureRow {
        name,
        census: agg.census,
        staffed,
        capacity: agg.capacity,
        occupancy_pct: (staffed > 0).then(|| agg.occupancy_frac() * 100.0),
        occ_badge,
        boarding: agg.boarding,
        expected_dc,
        recent_admits,
    }
}

/// Same pressure ordering as the EP-13 dashboard: occupancy z descending
/// (warming below any real z, flat at 0), boarding breaks ties, stable sort
/// keeps taxonomy order on full ties.
fn sort_pressure(rows: &mut [PressureRow]) {
    rows.sort_by(|a, b| {
        b.occ_badge
            .sort_z()
            .total_cmp(&a.occ_badge.sort_z())
            .then((b.boarding as f32).total_cmp(&(a.boarding as f32)))
    });
}

// ---------------------------------------------------------------------------
// Markdown export
// ---------------------------------------------------------------------------

/// Pipe-table cell hygiene: report strings are synthetic, but a `|` in a
/// name must never break the exported table grid.
fn cell(s: &str) -> String {
    s.replace('|', "/")
}

fn fmt_hours(h: Option<f64>) -> String {
    match h {
        Some(v) => format!("{v:.1}"),
        None => "pre-sim".to_string(),
    }
}

impl MorningReport {
    /// Render the report as a markdown document: header with scenario meta +
    /// seed (the reproducibility line), then the four sections as pipe
    /// tables. Deterministic — byte-identical for identical reports.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        let title_time = sim_time_label(self.now_min);
        md.push_str(&format!("# Morning report — {title_time}\n\n"));
        md.push_str(&format!(
            "- Scenario: {} · seed `{}` · sim minute {:.0}\n",
            cell(&self.scenario_name),
            self.seed,
            self.now_min
        ));
        md.push_str(&format!(
            "- Overnight window: {} → {} (trailing {:.0} h)\n",
            sim_time_label(self.window_start_min),
            title_time,
            OVERNIGHT_LOOKBACK_MIN / 60.0
        ));
        md.push_str(&format!(
            "- Heuristic (labeled): anticipated discharges = stable patients \
             (instability < {DISCHARGE_READY_MAX_INSTABILITY}, not boarding, inpatient level of \
             care) at ≥ {:.0}% of the service-typical median LOS (synthetic EP-10 table); \
             patients present before sim start count as past threshold.\n\n",
            DISCHARGE_LOS_FRACTION * 100.0
        ));

        // 1 — overnight admits.
        md.push_str(&format!("## Overnight admits — {}\n\n", self.overnight_total));
        let bound = if self.log_covers_window { "" } else { "≥ " };
        md.push_str(&format!(
            "Log corroboration: {bound}{} bed admissions · {bound}{} ED admit decisions \
             (boarding starts) · {bound}{} inpatient discharges in the window{}.\n\n",
            self.log_admissions,
            self.log_ed_decisions,
            self.log_discharges,
            if self.log_covers_window {
                ""
            } else {
                " (bounded log no longer reaches the window start — lower bounds)"
            }
        ));
        if self.overnight.is_empty() {
            md.push_str("No admissions in the window.\n\n");
        } else {
            md.push_str("| Service line | Admits | Stable | Watcher | Unstable | Critical |\n");
            md.push_str("|---|---:|---:|---:|---:|---:|\n");
            for r in &self.overnight {
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} |\n",
                    cell(&r.name),
                    r.count,
                    r.tiers[0],
                    r.tiers[1],
                    r.tiers[2],
                    r.tiers[3]
                ));
            }
            md.push('\n');
        }

        // 2 — anticipated discharges.
        md.push_str(&format!(
            "## Anticipated discharges — {}{}\n\n",
            self.discharges.len(),
            if self.discharges_pre_sim > 0 {
                format!(" ({} admitted pre-sim)", self.discharges_pre_sim)
            } else {
                String::new()
            }
        ));
        if self.discharges.is_empty() {
            md.push_str("No patients meet the readiness heuristic.\n\n");
        } else {
            md.push_str("| Room | Unit | Patient | Instability | LOS (h) | Typical (h) |\n");
            md.push_str("|---|---|---|---:|---:|---:|\n");
            for r in &self.discharges {
                md.push_str(&format!(
                    "| {} | {} | {} | {:.2} | {} | {:.0} |\n",
                    cell(&r.room_number),
                    cell(&r.unit_name),
                    cell(&r.alias),
                    r.instability,
                    fmt_hours(r.los_hr),
                    r.typical_hr
                ));
            }
            md.push('\n');
        }

        // 3 — boarding.
        md.push_str(&format!("## Boarding — {}\n\n", self.boarders.len()));
        if self.boarders.is_empty() {
            md.push_str("No admitted patients boarding.\n\n");
        } else {
            md.push_str("| Patient | Location | Instability | Flags | Waiting (h) |\n");
            md.push_str("|---|---|---:|---|---:|\n");
            for b in &self.boarders {
                let mut flags = Vec::new();
                if b.telemetry {
                    flags.push("tele");
                }
                if b.isolation {
                    flags.push("iso");
                }
                md.push_str(&format!(
                    "| {} | {} · {} | {:.2} | {} | {} |\n",
                    cell(&b.alias),
                    cell(&b.room_number),
                    cell(&b.unit_name),
                    b.instability,
                    if flags.is_empty() { "—".to_string() } else { flags.join(" ") },
                    fmt_hours(b.waited_hr)
                ));
            }
            md.push('\n');
        }

        // 4 — pressure tables.
        for (heading, rows) in [
            ("## Pressure — by level of care", &self.pressure_by_loc),
            ("## Pressure — by service line", &self.pressure_by_line),
        ] {
            md.push_str(heading);
            md.push_str("\n\n");
            if rows.is_empty() {
                md.push_str("No bedded groups.\n\n");
                continue;
            }
            md.push_str(
                "| Group | Census | Occupancy | z (24 h) | Boarding | Likely DC | Admits 12 h | Net |\n",
            );
            md.push_str("|---|---:|---:|---|---:|---:|---:|---:|\n");
            for r in rows {
                md.push_str(&format!(
                    "| {} | {}/{} | {} | {} | {} | {} | {} | {:+} |\n",
                    cell(&r.name),
                    r.census,
                    r.staffed,
                    match r.occupancy_pct {
                        Some(p) => format!("{p:.0}%"),
                        None => "—".to_string(),
                    },
                    r.occ_badge.text(),
                    r.boarding,
                    r.expected_dc,
                    r.recent_admits,
                    r.expected_net()
                ));
            }
            md.push('\n');
        }

        md.push_str("---\n");
        md.push_str(&format!(
            "Reproducible snapshot: seed {} at sim minute {:.0} (deterministic engine). \
             Census = staffed beds (licensed − out-of-service). Net = likely discharges − \
             overnight admits. All dynamics are synthetic demo parameters (params.rs), \
             not measured HUP data.\n",
            self.seed, self.now_min
        ));
        md
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hupsim_core::index::HospitalIndex;
    use hupsim_core::model::*;
    use hupsim_core::patient::{Acuity, Patient};
    use hupsim_core::provenance::Provenance;

    /// 07:00 on sim day 4 — a clean "morning report" instant. The raw
    /// 12 h window start is 4020; crafted timestamps sit around it.
    const NOW: f64 = 3.0 * 1440.0 + 7.0 * 60.0; // 4740

    fn patient(alias: &str, instability: f32, admitted: Option<f64>, boarding: bool) -> Patient {
        Patient {
            id: alias.into(),
            alias: alias.into(),
            age: None,
            service: None,
            admitted_at_min: admitted,
            acuity: Acuity { instability, trend_per_hr: 0.0 },
            boarding,
            isolation: None,
            telemetry: false,
            note: String::new(),
        }
    }

    fn unit(id: &str, ut: UnitType, line: Option<&str>) -> Unit {
        Unit {
            id: id.into(),
            name: id.to_uppercase(),
            service: "S".into(),
            service_line: line.map(|l| l.into()),
            unit_type: ut,
            building: "b".into(),
            level: FloorLevel::Numbered(1),
            elevator_core: None,
            target_rn_ratio: None,
            provenance: Provenance::default(),
            note: None,
        }
    }

    fn room(id: &str, unit: &str, status: RoomStatus) -> Room {
        Room {
            id: id.into(),
            number: id.into(),
            unit: unit.into(),
            kind: RoomKind::Inpatient,
            telemetry_capable: false,
            negative_pressure: false,
            status,
            provenance: Provenance::default(),
        }
    }

    fn occupied(p: Patient) -> RoomStatus {
        RoomStatus::Occupied { patient: p }
    }

    /// The crafted EP-16 world: known admits, discharge candidates, and a
    /// boarder, against three units across two service lines.
    fn crafted() -> (Hospital, HospitalIndex) {
        let mut ed_boarder = patient("E. Board", 0.40, Some(NOW - 300.0), true);
        ed_boarder.telemetry = true;
        let h = Hospital {
            meta: HospitalMeta::default(),
            buildings: vec![],
            units: vec![
                unit("med", UnitType::MedSurg, Some("line.medicine")),
                unit("icu", UnitType::Icu, Some("line.critical_care")),
                unit("ed", UnitType::Ed, None),
            ],
            rooms: vec![
                // Overnight admit (in window), Watcher — not discharge-ready.
                room("med.r0", "med", occupied(patient("A. New", 0.30, Some(4200.0), false))),
                // Old admit, stable, 77.3 h into a 96 h median (≥ 75%) — DC.
                room("med.r1", "med", occupied(patient("B. Ready", 0.10, Some(100.0), false))),
                // Pre-sim admit, stable — DC via the labeled pre-sim rule.
                room("med.r2", "med", occupied(patient("C. Legacy", 0.20, None, false))),
                // In-window admit, stable but 10 h in — too early for DC.
                room("med.r3", "med", occupied(patient("D. Early", 0.10, Some(4140.0), false))),
                room("med.r4", "med", RoomStatus::Vacant),
                // Unstable long-stay — never a DC candidate.
                room("icu.r0", "icu", occupied(patient("F. Sick", 0.85, Some(200.0), false))),
                // Overnight ICU admit, Unstable tier.
                room("icu.r1", "icu", occupied(patient("G. Fresh", 0.55, Some(4400.0), false))),
                room("icu.r2", "icu", RoomStatus::OutOfService { reason: "clean".into() }),
                // Admitted boarder held in the ED — overnight admit + boarder.
                room("ed.r0", "ed", occupied(ed_boarder)),
                // ED arrival, not admitted — neither an admit nor a boarder.
                room("ed.r1", "ed", occupied(patient("H. Walkin", 0.30, Some(4600.0), false))),
            ],
            connections: vec![],
            elevator_cores: vec![],
            service_lines: vec![
                ServiceLine { id: "line.medicine".into(), name: "Medicine".into(), description: None },
                ServiceLine {
                    id: "line.critical_care".into(),
                    name: "Critical Care".into(),
                    description: None,
                },
            ],
        };
        let idx = HospitalIndex::build(&h);
        (h, idx)
    }

    fn crafted_log() -> Vec<SimLogEntry> {
        let e = |t_min: f64, message: &str| SimLogEntry { t_min, message: message.into() };
        vec![
            e(3000.0, "admission: Z. Old → med.r9"), // before the window
            e(4100.0, "admission: A. New → med.r0"),
            e(4200.0, "admission → icu.r1"),
            e(4300.0, "discharge B. Gone ← med.r9"),
            e(4400.0, "discharge skipped — patient moved: med.r9"), // not a discharge
            e(4450.0, "ED decision to admit (ICU) — ed.r0 boarding"),
            e(4500.0, "ED release: W. Home ← ed.r7"), // treat-and-release, not inpatient
        ]
    }

    fn crafted_report(rolling: &RollingStore) -> MorningReport {
        let (h, idx) = crafted();
        let m = RoomMatrix::build(&h, &idx);
        assemble(&ReportInputs {
            hospital: &h,
            matrix: &m,
            rolling,
            log: &crafted_log(),
            now_min: NOW,
            scenario_name: "crafted",
            seed: 7,
        })
    }

    /// Warm med/surg norm around 55% occupancy so its current 80% shows a
    /// crisp z; everything else stays warming.
    fn warm_rolling() -> RollingStore {
        let mut rolling = RollingStore::new(4);
        for v in [0.5, 0.5, 0.6, 0.6] {
            rolling.unit_types[UnitType::MedSurg.index()].occupancy_frac.push(v);
        }
        rolling
    }

    #[test]
    fn crafted_world_assembles_exactly() {
        let report = crafted_report(&warm_rolling());

        // Overnight: med.r0 + med.r3 (Medicine), icu.r1 (Critical Care),
        // the ED boarder (no line). The non-admitted ED arrival is excluded.
        assert_eq!(report.overnight_total, 4);
        assert_eq!(
            report.overnight,
            vec![
                OvernightRow { name: "Medicine".into(), count: 2, tiers: [1, 1, 0, 0] },
                OvernightRow { name: "Critical Care".into(), count: 1, tiers: [0, 0, 1, 0] },
                OvernightRow { name: "(no service line)".into(), count: 1, tiers: [0, 1, 0, 0] },
            ]
        );

        // Log corroboration: two bed admissions, one ED admit decision, one
        // real discharge in-window; skipped-discharge and ED-release lines
        // don't count. The first log entry (t=3000) predates the window
        // start, so coverage holds.
        assert_eq!(report.log_admissions, 2);
        assert_eq!(report.log_ed_decisions, 1);
        assert_eq!(report.log_discharges, 1);
        assert!(report.log_covers_window);

        // Anticipated discharges: the ripe stable patient + the pre-sim one.
        assert_eq!(report.discharges.len(), 2);
        assert_eq!(report.discharges_pre_sim, 1);
        let ready = &report.discharges[0];
        assert_eq!(ready.alias, "B. Ready");
        assert_eq!(ready.unit_name, "MED");
        assert!((ready.los_hr.unwrap() - (NOW - 100.0) / 60.0).abs() < 1e-9);
        assert_eq!(ready.typical_hr, 96.0);
        let legacy = &report.discharges[1];
        assert_eq!(legacy.alias, "C. Legacy");
        assert_eq!(legacy.los_hr, None);

        // Boarding: exactly the admitted boarder, 5 h in, telemetry flagged.
        assert_eq!(report.boarders.len(), 1);
        let b = &report.boarders[0];
        assert_eq!(b.alias, "E. Board");
        assert_eq!(b.unit_name, "ED");
        assert!(b.telemetry && !b.isolation);
        assert!((b.waited_hr.unwrap() - 5.0).abs() < 1e-9);

        // Pressure by level of care: med/surg has a real z (0.8 vs the warm
        // 0.55 ± 0.05 norm → +5) and leads; the warming rows follow with the
        // boarding ED ahead of the quiet ICU.
        let names: Vec<&str> =
            report.pressure_by_loc.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["Med/Surg", "Emergency", "ICU"]);
        let med = &report.pressure_by_loc[0];
        assert_eq!((med.census, med.staffed, med.capacity), (4, 5, 5));
        assert!(matches!(med.occ_badge, NormBadge::Z(z) if (z - 5.0).abs() < 0.2));
        assert_eq!((med.expected_dc, med.recent_admits), (2, 2));
        assert_eq!(med.expected_net(), 0);
        let icu = report.pressure_by_loc.iter().find(|r| r.name == "ICU").unwrap();
        assert_eq!((icu.census, icu.staffed, icu.capacity), (2, 2, 3));
        assert_eq!((icu.expected_dc, icu.recent_admits), (0, 1));
        assert_eq!(icu.expected_net(), -1);
        assert!(matches!(icu.occ_badge, NormBadge::Warming { have: 0, need: 4 }));

        // Pressure by service line: both lines bedded, both warming → the
        // stable sort keeps taxonomy order.
        let line_names: Vec<&str> =
            report.pressure_by_line.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(line_names, vec!["Medicine", "Critical Care"]);
    }

    #[test]
    fn log_truncation_reads_as_lower_bound() {
        // Log whose first entry postdates the window start: counts hold but
        // coverage flags them as lower bounds.
        let (h, idx) = crafted();
        let m = RoomMatrix::build(&h, &idx);
        let rolling = RollingStore::new(4);
        let log = vec![SimLogEntry { t_min: 4600.0, message: "admission: L. Late → med.r0".into() }];
        let report = assemble(&ReportInputs {
            hospital: &h,
            matrix: &m,
            rolling: &rolling,
            log: &log,
            now_min: NOW,
            scenario_name: "crafted",
            seed: 7,
        });
        assert_eq!(report.log_admissions, 1);
        assert!(!report.log_covers_window);
        assert!(report.to_markdown().contains("≥ 1 bed admissions"));

        // An empty log on a world where sim time has passed is also a gap…
        let report = assemble(&ReportInputs {
            hospital: &h,
            matrix: &m,
            rolling: &rolling,
            log: &[],
            now_min: NOW,
            scenario_name: "crafted",
            seed: 7,
        });
        assert!(!report.log_covers_window);
        // …but an untouched world at t=0 has nothing to miss.
        let report = assemble(&ReportInputs {
            hospital: &h,
            matrix: &m,
            rolling: &rolling,
            log: &[],
            now_min: 0.0,
            scenario_name: "crafted",
            seed: 7,
        });
        assert!(report.log_covers_window);
    }

    /// Split a markdown document into its pipe tables; each table is the
    /// per-line cell counts + the parsed cells.
    fn tables_of(md: &str) -> Vec<Vec<Vec<String>>> {
        let mut tables = Vec::new();
        let mut current: Vec<Vec<String>> = Vec::new();
        for line in md.lines() {
            let t = line.trim();
            if t.starts_with('|') && t.ends_with('|') && t.len() >= 2 {
                let cells: Vec<String> = t[1..t.len() - 1]
                    .split('|')
                    .map(|c| c.trim().to_string())
                    .collect();
                current.push(cells);
            } else if !current.is_empty() {
                tables.push(std::mem::take(&mut current));
            }
        }
        if !current.is_empty() {
            tables.push(current);
        }
        tables
    }

    #[test]
    fn markdown_export_roundtrips_as_tables() {
        let report = crafted_report(&warm_rolling());
        let md = report.to_markdown();

        // Header carries the reproducibility line.
        assert!(md.starts_with("# Morning report — day 4, 07:00\n"));
        assert!(md.contains("seed `7`"));
        assert!(md.contains("sim minute 4740"));

        // Every pipe table is rectangular: all rows match the header width,
        // and the second row is the alignment separator.
        let tables = tables_of(&md);
        assert_eq!(tables.len(), 5, "overnight, DC, boarding, 2× pressure");
        for table in &tables {
            assert!(table.len() >= 3, "header + separator + at least one row");
            let width = table[0].len();
            assert!(table.iter().all(|row| row.len() == width), "ragged table");
            assert!(table[1].iter().all(|c| c.starts_with("---") || c.ends_with("---:")
                || c == "---"),
                "second row must be the alignment separator, got {:?}", table[1]);
        }

        // Spot-check cells against the model: the med/surg pressure row.
        let loc_table = &tables[3];
        let med_row = loc_table.iter().find(|r| r[0] == "Med/Surg").unwrap();
        assert_eq!(med_row[1], "4/5");
        assert_eq!(med_row[2], "80%");
        assert_eq!(med_row[3], "+5.0");
        assert_eq!(med_row[7], "+0");
        let icu_row = loc_table.iter().find(|r| r[0] == "ICU").unwrap();
        assert_eq!(icu_row[3], "warming 0/4");
        assert_eq!(icu_row[7], "-1");

        // The boarder row keeps its flags and wait.
        let boarding_table = &tables[2];
        let b = boarding_table.iter().find(|r| r[0] == "E. Board").unwrap();
        assert_eq!(b[3], "tele");
        assert_eq!(b[4], "5.0");
        // Pre-sim discharge rows say so instead of faking an LOS.
        let dc_table = &tables[1];
        let legacy = dc_table.iter().find(|r| r[2] == "C. Legacy").unwrap();
        assert_eq!(legacy[4], "pre-sim");
    }

    #[test]
    fn same_world_same_report_text() {
        let rolling = warm_rolling();
        let a = crafted_report(&rolling);
        let b = crafted_report(&rolling);
        assert_eq!(a, b, "assembly must be a pure function of its inputs");
        assert_eq!(a.to_markdown(), b.to_markdown(), "export must be byte-stable");
    }

    #[test]
    fn pipe_cells_cannot_break_the_grid() {
        assert_eq!(cell("Founders | 8"), "Founders / 8");
        // Time label matches the SimClock convention (1-based day).
        assert_eq!(sim_time_label(0.0), "day 1, 00:00");
        assert_eq!(sim_time_label(NOW), "day 4, 07:00");
    }
}
