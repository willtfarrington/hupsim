//! EP-16 morning-report window: the bed-huddle sheet as a floating egui
//! window over either view, plus its markdown export.
//!
//! The report is an explicit **snapshot** (EP-14/EP-15 discipline): it is
//! assembled once when opened/refreshed/auto-fired, carries the world
//! version it saw, and shows a staleness line when the world has moved on —
//! numbers never shimmy mid-read while the sim runs. Assembly itself is the
//! pure `hupsim_sim::report::assemble`, fed the app's version-keyed cache
//! matrix (fresh even while paused; the engine's own matrix is not).

use crate::agg_cache::AggCache;
use crate::widgets::ZBadge;
use crate::world::{AppWorld, SimDrive};
use bevy::log::error;
use bevy_egui::egui::{self, Color32, RichText};
use hupsim_sim::report::{self, MorningReport, NormBadge, ReportInputs};

/// Rows shown in-window per table before honest truncation; the export
/// always carries the full list.
const MAX_ROWS_IN_VIEW: usize = 30;

/// Auto-open fires only on crossings smaller than this many sim minutes —
/// larger jumps are scenario loads/resets, which must not steal the screen.
const AUTO_OPEN_MAX_JUMP_MIN: f64 = 240.0;

#[derive(Default)]
pub struct ReportPanelState {
    pub open: bool,
    /// Auto-open at 07:00 sim time (owner default: off — no screen stealing).
    pub auto_open: bool,
    /// Sim minute last seen by the crossing detector.
    last_seen_min: f64,
    pub report: Option<MorningReport>,
    /// `AppWorld.version` the snapshot saw — staleness marker.
    taken_at_version: u64,
}

impl ReportPanelState {
    /// Drop the snapshot (scenario load — a report about the previous world
    /// must not linger) and resync the 07:00 detector to the new clock.
    pub fn on_world_replaced(&mut self, sim_time_min: f64) {
        self.report = None;
        self.last_seen_min = sim_time_min;
    }
}

/// Assemble a fresh snapshot from the live world.
fn snapshot(world: &AppWorld, cache: &AggCache, sim: &SimDrive) -> MorningReport {
    report::assemble(&ReportInputs {
        hospital: world.hospital(),
        matrix: &cache.matrix,
        rolling: &sim.engine.rolling,
        log: &sim.engine.log,
        now_min: sim.engine.clock.now_min,
        scenario_name: &world.scenario.meta.name,
        seed: sim.engine.seed(),
    })
}

/// 07:00 crossing detector, called once per frame. Returns `true` when the
/// report should auto-fire: the auto-open toggle is on and a 07:00 boundary
/// (420 + 1440·k sim minutes) was crossed since the previous frame at a
/// plausible tick-sized step. Backwards moves and large jumps only resync.
pub fn crossed_seven_am(state: &mut ReportPanelState, now_min: f64) -> bool {
    let prev = state.last_seen_min;
    state.last_seen_min = now_min;
    if !state.auto_open || now_min < prev || now_min - prev > AUTO_OPEN_MAX_JUMP_MIN {
        return false;
    }
    let day_of = |t: f64| ((t - 420.0) / 1440.0).floor();
    day_of(now_min) > day_of(prev)
}

/// Take a snapshot and open the window (toolbar click, auto-fire).
pub fn open_with_snapshot(
    state: &mut ReportPanelState,
    world: &AppWorld,
    cache: &AggCache,
    sim: &SimDrive,
) {
    state.report = Some(snapshot(world, cache, sim));
    state.taken_at_version = world.version;
    state.open = true;
}

fn badge(b: &NormBadge) -> ZBadge {
    match b {
        NormBadge::Warming { have, need } => ZBadge::Warming { have: *have, need: *need },
        NormBadge::Flat => ZBadge::Flat,
        NormBadge::Z(z) => ZBadge::Z(*z),
    }
}

fn z_cell(ui: &mut egui::Ui, b: &NormBadge) {
    let z = badge(b);
    match z.color() {
        Some(c) => ui.colored_label(c, z.text()),
        None => ui.weak(z.text()),
    }
    .on_hover_text(format!(
        "occupancy vs its trailing-24 h norm · {}",
        z.hover_text()
    ));
}

fn hours_cell(h: Option<f64>) -> String {
    match h {
        Some(v) => format!("{v:.1} h"),
        None => "pre-sim".to_string(),
    }
}

/// Honest truncation line under a capped table (the full list is in the
/// export, same rule as the EP-14 top-40 label).
fn truncation_note(ui: &mut egui::Ui, total: usize) {
    if total > MAX_ROWS_IN_VIEW {
        ui.weak(format!(
            "…and {} more — the exported markdown carries the full list.",
            total - MAX_ROWS_IN_VIEW
        ));
    }
}

/// Draw the morning-report window (both views; floating). Opening without a
/// snapshot assembles one on the spot, so a forced-open state can never
/// paint empty.
pub fn draw_report_window(
    ctx: &egui::Context,
    world: &AppWorld,
    cache: &AggCache,
    sim: &SimDrive,
    state: &mut ReportPanelState,
) {
    if !state.open {
        return;
    }
    if state.report.is_none() {
        state.report = Some(snapshot(world, cache, sim));
        state.taken_at_version = world.version;
    }
    let title = state
        .report
        .as_ref()
        .map(|r| format!("Morning report — {}", report::sim_time_label(r.now_min)))
        .unwrap_or_else(|| "Morning report".to_string());

    let mut open = state.open;
    let mut refresh = false;
    egui::Window::new(title)
        .id(egui::Id::new("morning_report"))
        .open(&mut open)
        .default_width(600.0)
        .default_height(560.0)
        .vscroll(true)
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("⟳ Refresh")
                    .on_hover_text("re-snapshot the report at the current sim instant")
                    .clicked()
                {
                    refresh = true;
                }
                if ui
                    .button("💾 Save report…")
                    .on_hover_text("export as markdown (pipe tables, seed line for reproducibility)")
                    .clicked()
                {
                    save_report(state.report.as_ref());
                }
                ui.checkbox(&mut state.auto_open, "auto-open at 07:00")
                    .on_hover_text(
                        "pop this report each sim morning at 07:00 (off by default)",
                    );
            });
            if state.taken_at_version != world.version {
                ui.colored_label(
                    Color32::from_rgb(230, 150, 60),
                    "snapshot — the world has moved on since it was taken · Refresh to update",
                );
            }
            ui.separator();
            if let Some(r) = &state.report {
                draw_report_body(ui, r);
            }
        });
    state.open = open;
    if refresh {
        state.report = Some(snapshot(world, cache, sim));
        state.taken_at_version = world.version;
    }
}

fn save_report(report: Option<&MorningReport>) {
    let Some(r) = report else { return };
    let day = (r.now_min.max(0.0) as u64) / 1440 + 1;
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("markdown", &["md"])
        .set_file_name(format!("hup_morning_report_day{day}.md"))
        .save_file()
    {
        if let Err(e) = std::fs::write(&path, r.to_markdown()) {
            error!("report save failed: {e}");
        }
    }
}

/// The huddle sheet, top to bottom: header, overnight admits, anticipated
/// discharges, boarding, the two pressure tables. Text/table on purpose —
/// it mirrors a printed sheet, not the dashboard.
fn draw_report_body(ui: &mut egui::Ui, r: &MorningReport) {
    ui.weak(format!(
        "{} · seed {} · sim minute {:.0}",
        r.scenario_name, r.seed, r.now_min
    ));
    ui.weak(format!(
        "overnight window: {} → {}",
        report::sim_time_label(r.window_start_min),
        report::sim_time_label(r.now_min)
    ));
    ui.weak(
        "anticipated discharges are a labeled heuristic: stable, not boarding, \
         ≥ 75% of service-typical median LOS (synthetic EP-10 table); pre-sim \
         admissions count as past threshold",
    );
    ui.add_space(4.0);

    // 1 — overnight admits.
    ui.strong(format!("Overnight admits — {}", r.overnight_total));
    let bound = if r.log_covers_window { "" } else { "≥ " };
    ui.weak(format!(
        "log corroboration: {bound}{} bed admissions · {bound}{} ED admit decisions · \
         {bound}{} inpatient discharges{}",
        r.log_admissions,
        r.log_ed_decisions,
        r.log_discharges,
        if r.log_covers_window { "" } else { " (log truncated — lower bounds)" }
    ));
    if r.overnight.is_empty() {
        ui.weak("no admissions in the window");
    } else {
        egui::Grid::new("report_overnight")
            .striped(true)
            .num_columns(6)
            .spacing([14.0, 3.0])
            .show(ui, |ui| {
                for h in ["service line", "admits", "stable", "watcher", "unstable", "critical"] {
                    ui.weak(h);
                }
                ui.end_row();
                for row in &r.overnight {
                    ui.label(&row.name);
                    ui.strong(row.count.to_string());
                    for t in row.tiers {
                        if t > 0 {
                            ui.label(t.to_string());
                        } else {
                            ui.weak("0");
                        }
                    }
                    ui.end_row();
                }
            });
    }
    ui.add_space(6.0);

    // 2 — anticipated discharges.
    ui.strong(format!(
        "Anticipated discharges — {}{}",
        r.discharges.len(),
        if r.discharges_pre_sim > 0 {
            format!(" ({} admitted pre-sim)", r.discharges_pre_sim)
        } else {
            String::new()
        }
    ));
    if r.discharges.is_empty() {
        ui.weak("no patients meet the readiness heuristic");
    } else {
        egui::Grid::new("report_discharges")
            .striped(true)
            .num_columns(5)
            .spacing([14.0, 3.0])
            .show(ui, |ui| {
                for h in ["room", "unit", "patient", "instability", "LOS / typical"] {
                    ui.weak(h);
                }
                ui.end_row();
                for row in r.discharges.iter().take(MAX_ROWS_IN_VIEW) {
                    ui.label(&row.room_number);
                    ui.label(&row.unit_name);
                    ui.label(&row.alias);
                    ui.label(format!("{:.2}", row.instability));
                    ui.label(format!(
                        "{} / {:.0} h median",
                        hours_cell(row.los_hr),
                        row.typical_hr
                    ));
                    ui.end_row();
                }
            });
        truncation_note(ui, r.discharges.len());
    }
    ui.add_space(6.0);

    // 3 — boarding.
    ui.strong(format!("Boarding — {}", r.boarders.len()));
    if r.boarders.is_empty() {
        ui.weak("no admitted patients boarding");
    } else {
        egui::Grid::new("report_boarding")
            .striped(true)
            .num_columns(5)
            .spacing([14.0, 3.0])
            .show(ui, |ui| {
                for h in ["patient", "location", "instability", "flags", "waiting"] {
                    ui.weak(h);
                }
                ui.end_row();
                for b in r.boarders.iter().take(MAX_ROWS_IN_VIEW) {
                    ui.label(&b.alias);
                    ui.label(format!("{} · {}", b.room_number, b.unit_name));
                    ui.label(format!("{:.2}", b.instability));
                    let mut flags = Vec::new();
                    if b.telemetry {
                        flags.push("tele");
                    }
                    if b.isolation {
                        flags.push("iso");
                    }
                    if flags.is_empty() {
                        ui.weak("—");
                    } else {
                        ui.colored_label(Color32::ORANGE, flags.join(" "));
                    }
                    ui.label(hours_cell(b.waited_hr));
                    ui.end_row();
                }
            });
        truncation_note(ui, r.boarders.len());
    }
    ui.add_space(6.0);

    // 4 — pressure tables.
    for (title, rows, grid_id) in [
        ("Pressure — by level of care", &r.pressure_by_loc, "report_ploc"),
        ("Pressure — by service line", &r.pressure_by_line, "report_pline"),
    ] {
        ui.strong(title);
        if rows.is_empty() {
            ui.weak("no bedded groups");
            ui.add_space(6.0);
            continue;
        }
        egui::Grid::new(grid_id)
            .striped(true)
            .num_columns(8)
            .spacing([14.0, 3.0])
            .show(ui, |ui| {
                ui.weak("group").on_hover_text(
                    "rows sort by pressure — occupancy z first, then boarding — \
                     same order as the dashboard drawer",
                );
                for h in ["census", "occ", "z 24 h", "boarding", "likely DC", "admits 12 h", "net"] {
                    ui.weak(h);
                }
                ui.end_row();
                for row in rows {
                    ui.label(&row.name);
                    ui.label(format!("{}/{}", row.census, row.staffed));
                    match row.occupancy_pct {
                        Some(p) => ui.strong(format!("{p:.0}%")),
                        None => ui.weak("—"),
                    };
                    z_cell(ui, &row.occ_badge);
                    if row.boarding > 0 {
                        ui.colored_label(Color32::ORANGE, row.boarding.to_string());
                    } else {
                        ui.weak("0");
                    }
                    ui.label(row.expected_dc.to_string());
                    ui.label(row.recent_admits.to_string());
                    let net = row.expected_net();
                    let text = RichText::new(format!("{net:+}"));
                    if net < 0 {
                        ui.label(text.color(Color32::from_rgb(230, 150, 60)))
                            .on_hover_text("filling faster than it empties");
                    } else {
                        ui.label(text);
                    }
                    ui.end_row();
                }
            });
        ui.add_space(6.0);
    }

    ui.weak(
        "reproducible snapshot — synthetic demo dynamics (params.rs), not measured HUP data",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(auto: bool, seen: f64) -> ReportPanelState {
        ReportPanelState {
            auto_open: auto,
            last_seen_min: seen,
            ..Default::default()
        }
    }

    #[test]
    fn seven_am_crossing_fires_once_per_morning() {
        let mut s = state(true, 410.0);
        assert!(!crossed_seven_am(&mut s, 415.0), "not yet 07:00");
        assert!(crossed_seven_am(&mut s, 421.0), "crossed 07:00");
        assert!(!crossed_seven_am(&mut s, 430.0), "already fired today");
        // Next morning, exactly at the boundary.
        let mut s = state(true, 1859.0);
        assert!(crossed_seven_am(&mut s, 1860.0), "07:00 day 2 inclusive");
    }

    #[test]
    fn auto_open_never_fires_off_or_on_jumps() {
        let mut s = state(false, 410.0);
        assert!(!crossed_seven_am(&mut s, 430.0), "toggle off");
        // Scenario load: a huge forward jump resyncs silently.
        let mut s = state(true, 0.0);
        assert!(!crossed_seven_am(&mut s, 3000.0), "load-sized jump");
        assert_eq!(s.last_seen_min, 3000.0, "detector resynced");
        // Reset: backwards move resyncs silently.
        let mut s = state(true, 3000.0);
        assert!(!crossed_seven_am(&mut s, 0.0), "backwards move");
    }
}
