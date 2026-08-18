//! All egui chrome: toolbar, navigator, inspector/editor, status bar, the 2D
//! unit view host, 3D overlay labels, and hover tooltips.
//!
//! Mutations from widgets are either applied directly (field edits on a
//! focused patient) or queued as [`UiAction`]s and applied after every panel
//! has drawn — keeping closures free of aliasing surprises.

use crate::agg_cache::AggCache;
use crate::campus3d::LabelAnchor;
use crate::dashboard::{self, DashboardState};
use crate::query_panel::{self, QueryPanelState, RightTab};
use crate::report_panel;
use crate::unit_view;
use crate::widgets::{self, rgb32, SparkSpec};
use crate::world::{
    AppWorld, ConnHover, Focus, HoverInfo, OpenUnit, SceneRebuild, Selection, SimDrive, ViewMode,
};
use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Vec2 as EVec2};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use hupsim_core::color::stability_color;
use hupsim_core::ids::{RoomId, UnitId};
use hupsim_core::model::{FloorLevel, Room, RoomKind, RoomStatus};
use hupsim_core::ops;
use hupsim_core::patient::{Acuity, Patient};
use hupsim_core::provenance::{Confidence, Era, Provenance};

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, draw_ui);
    }
}

#[derive(Debug, Clone)]
pub(crate) enum UiAction {
    Select(Focus),
    OpenUnit(UnitId),
    BackToCampus,
    Admit(RoomId),
    /// Stage → act (EP-14): admit a hypothetical staged patient carrying the
    /// query's clinical needs into a result room — same sanctioned
    /// `ops::admit` path as the plain Admit.
    AdmitPlaced {
        room: RoomId,
        telemetry: bool,
        airborne: bool,
    },
    Discharge(RoomId),
    Transfer { from: RoomId, to: RoomId },
    /// Manually reassign an occupied room to RN #(rn+1), pinned until the
    /// next shift turnover (EP-11).
    PinRn { room: RoomId, rn: u16 },
    UnpinRn(RoomId),
    SetOos(RoomId),
    ReturnToService(RoomId),
    AddRoom(UnitId),
    DeleteRoom(RoomId),
    SeedCensus,
    ClearCensus,
    Save,
    Load,
    ResetSim,
}

pub(crate) fn confidence_color(c: Confidence) -> Color32 {
    match c {
        Confidence::Verified => Color32::from_rgb(70, 170, 90),
        Confidence::Reported => Color32::from_rgb(110, 150, 200),
        Confidence::Inferred => Color32::from_rgb(200, 160, 70),
        Confidence::Unverified => Color32::from_rgb(150, 110, 110),
    }
}

fn provenance_badge(ui: &mut egui::Ui, prov: &Provenance) {
    ui.horizontal_wrapped(|ui| {
        let resp = ui.colored_label(
            confidence_color(prov.confidence),
            format!("[{}]", prov.confidence.label()),
        );
        if let Some(src) = &prov.source {
            resp.on_hover_text(src);
        }
        match prov.era {
            Era::Pre2021 => {
                ui.colored_label(Color32::from_rgb(230, 150, 60), "⚠ pre-2021 bed map (stale)");
            }
            Era::PostPavilion => {
                ui.weak("post-Pavilion");
            }
            _ => {}
        }
    });
    if let Some(note) = &prov.note {
        ui.weak(note);
    }
}

/// Text-field scratch, merged into one `Local` so the system stays within
/// Bevy's 16-parameter limit.
#[derive(Default)]
pub struct PanelScratch {
    search: String,
    transfer_target: String,
}

#[allow(clippy::too_many_arguments)]
pub fn draw_ui(
    mut contexts: EguiContexts,
    mut world: ResMut<AppWorld>,
    mut cache: ResMut<AggCache>,
    mut sim: ResMut<SimDrive>,
    mut selection: ResMut<Selection>,
    mut open_unit: ResMut<OpenUnit>,
    mut dash: ResMut<DashboardState>,
    mut qstate: ResMut<QueryPanelState>,
    state: Res<State<ViewMode>>,
    mut next_state: ResMut<NextState<ViewMode>>,
    hover: Res<HoverInfo>,
    conn_hover: Res<ConnHover>,
    mut scene_rebuild: ResMut<SceneRebuild>,
    camera_q: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    labels_q: Query<&LabelAnchor>,
    mut scratch: Local<PanelScratch>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    // One staleness check per frame; edits made while drawing bump `version`
    // and are picked up next frame (a version bump invalidates exactly once).
    cache.refresh_if_stale(&world);
    // EP-17: the census fan re-issues when the world or the sim instant
    // moves — always from the LIVE world (never experiment arms).
    cache.refresh_fan(&world, sim.engine.clock.now_min);
    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    let root = &mut viewport_ui;
    let mut actions: Vec<UiAction> = Vec::new();
    let view = *state.get();

    // EP-13 dashboard keys: D toggles the drawer, Esc clears the group
    // drill-through. Skipped while a text field owns the keyboard.
    if view == ViewMode::Campus3D && !ctx.egui_wants_keyboard_input() {
        let (toggle, clear) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::D),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if toggle {
            dash.toggle();
        }
        if clear {
            dash.drill = None;
        }
    }

    // EP-16: the optional 07:00 auto-open fires on sim-clock crossings only
    // (large jumps — loads, resets — resync silently, no screen stealing).
    if report_panel::crossed_seven_am(&mut dash.report, sim.engine.clock.now_min) {
        report_panel::open_with_snapshot(&mut dash.report, &world, &cache, &sim);
    }

    top_bar(root, &mut world, &mut sim, &mut dash, view, &mut actions);
    bottom_bar(root, &world, &cache, &sim, view);
    // Added after the status bar so the drawer sits directly above it; only
    // painted (and only assembled) while open — closed costs nothing.
    if view == ViewMode::Campus3D && dash.open {
        egui::Panel::bottom("hospital_dashboard")
            .resizable(true)
            .default_size(280.0)
            // Sliver guard: tab strip + column header + a few rows stay
            // visible however far the drawer is dragged down (EP-19).
            .min_size(120.0)
            .show(root, |ui| {
                if let Some(building) =
                    dashboard::draw_dashboard(ui, &world, &cache, &sim, &mut dash)
                {
                    actions.push(UiAction::Select(Focus::Building(building)));
                }
            });
    }
    left_panel(
        root,
        &world,
        &cache,
        &mut dash,
        &selection,
        &mut scratch.search,
        &mut actions,
    );
    right_panel(
        root,
        &mut world,
        &cache,
        &sim,
        &selection,
        &mut qstate,
        &mut scratch.transfer_target,
        &mut actions,
    );

    match view {
        ViewMode::UnitDetail => {
            let unit_id = open_unit.0.clone();
            egui::CentralPanel::default().show(root, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("⬅ Back to campus").clicked() {
                        actions.push(UiAction::BackToCampus);
                    }
                });
                ui.separator();
                if let Some(uid) = &unit_id {
                    egui::ScrollArea::both().show(ui, |ui| {
                        unit_view::draw_unit_view(
                            ui,
                            &world,
                            &cache,
                            &sim.engine,
                            uid,
                            &mut selection,
                            qstate.staged.as_ref().map(|s| &s.rooms),
                        );
                    });
                } else {
                    ui.label("No unit open — pick one from the navigator.");
                }
            });
        }
        ViewMode::Campus3D => {
            // All panels have been shown into `root` by now, so what remains
            // of it is exactly the central 3D viewport (EP-22).
            draw_3d_labels(&ctx, root.available_rect_before_wrap(), &camera_q, &labels_q);
            draw_hover_tooltip(&ctx, &world, &cache, &hover);
            if hover.0.is_none() {
                draw_conn_tooltip(&ctx, &world, &conn_hover);
            }
        }
    }

    // EP-16 morning report: a floating window over either view.
    report_panel::draw_report_window(&ctx, &world, &cache, &sim, &mut dash.report);

    apply_actions(
        actions,
        &mut world,
        &mut sim,
        &mut selection,
        &mut open_unit,
        &mut qstate,
        &mut dash,
        &mut next_state,
        &mut scene_rebuild,
    );
    Ok(())
}

fn top_bar(
    root: &mut egui::Ui,
    world: &mut AppWorld,
    sim: &mut SimDrive,
    dash: &mut DashboardState,
    view: ViewMode,
    actions: &mut Vec<UiAction>,
) {
    egui::Panel::top("toolbar").show(root, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("hupsim");
            ui.weak("· Hospital of the University of Pennsylvania");
            ui.separator();

            match view {
                ViewMode::Campus3D => {
                    ui.label("3D campus");
                    if ui
                        .selectable_label(dash.open, "📊 Dashboard")
                        .on_hover_text("Hospital-wide dashboard (D)")
                        .clicked()
                    {
                        dash.toggle();
                    }
                }
                ViewMode::UnitDetail => {
                    if ui.button("⬅ 3D campus").clicked() {
                        actions.push(UiAction::BackToCampus);
                    }
                }
            }
            // EP-16: the bed-huddle sheet is reachable from either view.
            if ui
                .selectable_label(dash.report.open, "📋 Report")
                .on_hover_text("Morning bed-huddle report — snapshot, exportable as markdown")
                .clicked()
            {
                if dash.report.open {
                    dash.report.open = false;
                } else {
                    // Clear the old snapshot so the window assembles a fresh
                    // one for this instant when it draws.
                    dash.report.report = None;
                    dash.report.open = true;
                }
            }
            ui.separator();

            ui.label("Aggregate:");
            egui::ComboBox::from_id_salt("agg_mode")
                .selected_text(match world.agg_mode {
                    hupsim_core::aggregate::AggregateMode::Mean => "Mean acuity",
                    hupsim_core::aggregate::AggregateMode::Worst => "Worst case",
                })
                .show_ui(ui, |ui| {
                    let before = world.agg_mode;
                    ui.selectable_value(
                        &mut world.agg_mode,
                        hupsim_core::aggregate::AggregateMode::Mean,
                        "Mean acuity",
                    );
                    ui.selectable_value(
                        &mut world.agg_mode,
                        hupsim_core::aggregate::AggregateMode::Worst,
                        "Worst case",
                    );
                    if before != world.agg_mode {
                        world.version += 1;
                    }
                });
            ui.separator();

            // Simulation controls.
            let running = sim.engine.clock.running;
            if ui
                .button(if running { "⏸ Pause" } else { "▶ Run" })
                .clicked()
            {
                sim.engine.clock.running = !running;
            }
            ui.label(sim.engine.clock.label());
            ui.add(
                egui::Slider::new(&mut sim.engine.clock.speed, 1.0..=240.0)
                    .logarithmic(true)
                    .text("sim-min/s"),
            );
            let (di, fi, ni, ei) = (sim.idx_drift, sim.idx_flow, sim.idx_diurnal, sim.idx_ed);
            ui.checkbox(&mut sim.engine.processes[ni].enabled, "diurnal")
                .on_hover_text(
                    "Time-of-day shaped admissions with service-specific LOS and \
                     mid-day discharges (synthetic parameters)",
                );
            ui.checkbox(&mut sim.engine.processes[ei].enabled, "ED")
                .on_hover_text(
                    "ED arrivals; ~30% convert to admissions and board in the ED \
                     when no bed is free (synthetic parameters)",
                );
            ui.checkbox(&mut sim.engine.processes[di].enabled, "drift")
                .on_hover_text("Acuity random walk, volatility tied to level of care");
            ui.checkbox(&mut sim.engine.processes[fi].enabled, "flow")
                .on_hover_text("DEMO: flat-rate arrivals & discharges (superseded by diurnal)");
            ui.checkbox(&mut sim.engine.transport.timed, "transport")
                .on_hover_text(
                    "EP-6: timed bed transports over the physical route graph \
                     (skybridges, the Connector, ground podium, elevators). \
                     Surveyed deck lengths where they exist; inferred walk & \
                     elevator constants otherwise. The unverified HUP \
                     Main↔Clifton tunnel is excluded. Off: placements are \
                     instantaneous; in-flight patients still arrive.",
                );
            if ui.button("Reset sim").clicked() {
                actions.push(UiAction::ResetSim);
            }
            ui.separator();

            if ui
                .button("Seed census")
                .on_hover_text("Fill vacant rooms to realistic occupancy (deterministic)")
                .clicked()
            {
                actions.push(UiAction::SeedCensus);
            }
            if ui.button("Clear census").clicked() {
                actions.push(UiAction::ClearCensus);
            }
            ui.separator();
            if ui.button("💾 Save…").clicked() {
                actions.push(UiAction::Save);
            }
            if ui.button("📂 Load…").clicked() {
                actions.push(UiAction::Load);
            }
        });
    });
}

fn bottom_bar(
    root: &mut egui::Ui,
    world: &AppWorld,
    cache: &AggCache,
    sim: &SimDrive,
    view: ViewMode,
) {
    egui::Panel::bottom("status").show(root, |ui| {
        ui.horizontal_wrapped(|ui| {
            let stats = &cache.stats;
            let staffed = stats.capacity.saturating_sub(stats.out_of_service);
            let occ = if staffed > 0 {
                100.0 * stats.census as f32 / staffed as f32
            } else {
                0.0
            };
            ui.label(format!(
                "Census {}/{} ({occ:.0}%)",
                stats.census, staffed
            ));
            // EP-13: hospital-level deviation vs the trailing-24 h norm.
            let z = dashboard::zbadge_for(
                &sim.engine.rolling.hospital.census,
                stats.census as f32,
            );
            match z.color() {
                Some(c) => ui.colored_label(c, z.text()),
                None => ui.weak(z.text()),
            }
            .on_hover_text(z.hover_text());
            ui.separator();
            ui.colored_label(
                Color32::ORANGE,
                format!("Boarding {}", stats.boarding),
            );
            // EP-6: patients currently off-bed, moving through the campus.
            let in_transit = sim.engine.transport.active.len();
            if in_transit > 0 {
                ui.colored_label(
                    Color32::from_rgb(120, 170, 255),
                    format!("In transit {in_transit}"),
                )
                .on_hover_text(
                    sim.engine
                        .transport
                        .active
                        .iter()
                        .map(|t| {
                            format!(
                                "{} → {} ({}, ETA t={:.0}m)",
                                t.patient.alias, t.dest, t.via, t.eta_min
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            ui.separator();
            ui.colored_label(
                Color32::from_rgb(222, 50, 47),
                format!("Critical {}", stats.critical),
            );
            ui.label(format!("Unstable {}", stats.unstable));
            ui.separator();

            legend(ui);
            ui.separator();
            census_sparkline(ui, cache, sim);

            if let Some(last) = sim.engine.log.last() {
                ui.separator();
                ui.weak(format!("t={:.0}m {}", last.t_min, last.message));
            }
            ui.separator();
            match &world.source_dir {
                Some(dir) => {
                    ui.weak("data: files").on_hover_text(dir.display().to_string());
                }
                None => {
                    ui.weak("data: embedded");
                }
            }
            let warnings = &world.hospital().meta.data_warnings;
            if !warnings.is_empty() {
                ui.colored_label(Color32::ORANGE, format!("⚠ {}", warnings.len()))
                    .on_hover_text(warnings.join("\n"));
            }
            // EP-20: camera bindings hint — campus view only (the 2D unit
            // view has no camera).
            if view == ViewMode::Campus3D {
                ui.separator();
                ui.weak("🖱 orbit drag · pan shift+drag / middle · zoom wheel")
                    .on_hover_text(
                        "Camera: drag with either button orbits · Shift+drag or \
                         middle-drag pans (recenters the orbit pivot) · scroll \
                         wheel zooms",
                    );
            }
        });
    });
}

fn legend(ui: &mut egui::Ui) {
    ui.label("Stability:");
    let (rect, _) = ui.allocate_exact_size(EVec2::new(120.0, 12.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let n = 24;
    let w = rect.width() / n as f32;
    for i in 0..n {
        let t = i as f32 / (n - 1) as f32;
        let seg = Rect::from_min_size(
            Pos2::new(rect.min.x + i as f32 * w, rect.min.y),
            EVec2::new(w + 0.5, rect.height()),
        );
        painter.rect_filled(seg, CornerRadius::ZERO, rgb32(stability_color(t)));
    }
    ui.weak("stable → watcher → unstable");
    ui.label("·");
    let (r2, _) = ui.allocate_exact_size(EVec2::new(12.0, 12.0), Sense::hover());
    ui.painter_at(r2)
        .rect_filled(r2, CornerRadius::same(2), rgb32(hupsim_core::color::COLOR_VACANT));
    ui.weak("vacant");
}

/// Hospital census sparkline with its 24 h norm band — the EP-9 hospital
/// ring buffer through the shared EP-12 widget (EP-13 replaced the old
/// whole-run timeline polyline) — and, past the "now" divider, the EP-17
/// forecast fan (12 h median + 10–90 band in the projection clay). Stays
/// 18 px tall: the bar keeps one line.
fn census_sparkline(ui: &mut egui::Ui, cache: &AggCache, sim: &SimDrive) {
    let series = &sim.engine.rolling.hospital.census;
    let samples: Vec<f32> = series.iter_ordered().collect();
    let band = series.band(widgets::BAND_K);
    let fan = cache.fan.as_ref();
    widgets::sparkline_band(
        ui,
        EVec2::new(210.0, 18.0),
        &SparkSpec {
            samples: &samples,
            band,
            mean: band.map(|(lo, hi)| (lo + hi) * 0.5),
            current: Some(cache.stats.census as f32),
            fan: fan.map(|f| widgets::FanSpec {
                median: &f.median,
                lo: &f.q10,
                hi: &f.q90,
            }),
        },
    )
    .on_hover_text(format!(
        "hospital census, trailing 24 h · {}\n{}",
        match band {
            Some((lo, hi)) => format!("norm band {lo:.1}–{hi:.1} (mean ± {}σ)", widgets::BAND_K),
            None => "norm warming up".to_string(),
        },
        dashboard::fan_hover_text(fan),
    ));
    ui.weak("census · 24 h ⛅ 12 h");
}

fn left_panel(
    root: &mut egui::Ui,
    world: &AppWorld,
    cache: &AggCache,
    dash: &mut DashboardState,
    selection: &Selection,
    search: &mut String,
    actions: &mut Vec<UiAction>,
) {
    egui::Panel::left("navigator")
        .default_size(250.0)
        .show(root, |ui| {
            ui.add_space(4.0);
            // EP-13 drill-through: the dashboard's focused group filters the
            // tree to member units until cleared (✕, Esc, or re-click).
            let mut clear = false;
            if let Some(d) = &dash.drill {
                ui.horizontal(|ui| {
                    ui.strong(format!("🎯 {}", d.name));
                    ui.weak(format!("{} units", d.units.len()));
                    if ui
                        .small_button("✕")
                        .on_hover_text("clear group filter (Esc)")
                        .clicked()
                    {
                        clear = true;
                    }
                });
                ui.separator();
            }
            if clear {
                dash.drill = None;
            }
            ui.horizontal(|ui| {
                ui.label("🔎");
                ui.text_edit_singleline(search);
            });
            let query = search.trim().to_lowercase();
            if !query.is_empty() {
                search_results(ui, world, &query, actions);
                ui.separator();
            }

            let filter = dash.drill.as_ref();
            egui::ScrollArea::vertical().show(ui, |ui| {
                let h = world.hospital();
                for (bedded, title) in [
                    (true, "Inpatient buildings"),
                    (false, "Support & ambulatory"),
                ] {
                    // While a group filter is active, buildings without
                    // member units drop out entirely (title included).
                    let buildings: Vec<_> = h
                        .buildings
                        .iter()
                        .filter(|b| b.has_inpatient_beds == bedded)
                        .filter(|b| {
                            filter.is_none_or(|d| {
                                world
                                    .index
                                    .unit_ids(&b.id)
                                    .iter()
                                    .any(|u| d.units.contains(u))
                            })
                        })
                        .collect();
                    if buildings.is_empty() {
                        continue;
                    }
                    ui.add_space(6.0);
                    ui.strong(title);
                    for b in buildings {
                        let units: Vec<&UnitId> = world
                            .index
                            .unit_ids(&b.id)
                            .iter()
                            .filter(|uid| filter.is_none_or(|d| d.units.contains(*uid)))
                            .collect();
                        let header = if units.is_empty() {
                            format!("{} (no beds)", b.short_name)
                        } else {
                            let agg = cache.building(world, &b.id);
                            format!("{} — {}/{}", b.short_name, agg.census, agg.capacity)
                        };
                        egui::CollapsingHeader::new(header)
                            .id_salt(b.id.as_str())
                            // Forced open while filtering; user-controlled otherwise.
                            .open(filter.map(|_| true))
                            .show(ui, |ui| {
                                if ui.small_button("select building").clicked() {
                                    actions.push(UiAction::Select(Focus::Building(b.id.clone())));
                                }
                                for uid in units {
                                    if let Some(&up) = world.index.unit_pos.get(uid) {
                                        let u = &h.units[up];
                                        let agg = cache.unit_at(up);
                                        let selected = matches!(
                                            &selection.0,
                                            Focus::Unit(s) if s == uid
                                        );
                                        let label = format!(
                                            "{} · {}/{}",
                                            u.name, agg.census, agg.capacity
                                        );
                                        let resp = ui.selectable_label(selected, label);
                                        if resp.clicked() {
                                            actions.push(UiAction::OpenUnit(uid.clone()));
                                        }
                                        resp.on_hover_text(&u.service);
                                    }
                                }
                            });
                    }
                }
            });
        });
}

fn search_results(
    ui: &mut egui::Ui,
    world: &AppWorld,
    query: &str,
    actions: &mut Vec<UiAction>,
) {
    let h = world.hospital();
    let mut shown = 0;
    for u in &h.units {
        if shown >= 6 {
            break;
        }
        if u.name.to_lowercase().contains(query) || u.service.to_lowercase().contains(query) {
            if ui.small_button(format!("unit: {}", u.name)).clicked() {
                actions.push(UiAction::OpenUnit(u.id.clone()));
            }
            shown += 1;
        }
    }
    for r in &h.rooms {
        if shown >= 18 {
            break;
        }
        let occupant = r
            .status
            .patient()
            .map(|p| p.alias.to_lowercase())
            .unwrap_or_default();
        if r.number.to_lowercase().starts_with(query) || (!occupant.is_empty() && occupant.contains(query)) {
            let label = match r.status.patient() {
                Some(p) => format!("room {} — {}", r.number, p.alias),
                None => format!("room {}", r.number),
            };
            if ui.small_button(label).clicked() {
                actions.push(UiAction::OpenUnit(r.unit.clone()));
                actions.push(UiAction::Select(Focus::Room(r.id.clone())));
            }
            shown += 1;
        }
    }
    if shown == 0 {
        ui.weak("no matches");
    }
}

#[allow(clippy::too_many_arguments)]
fn right_panel(
    root: &mut egui::Ui,
    world: &mut AppWorld,
    cache: &AggCache,
    sim: &SimDrive,
    selection: &Selection,
    qstate: &mut QueryPanelState,
    transfer_target: &mut String,
    actions: &mut Vec<UiAction>,
) {
    egui::Panel::right("inspector")
        .default_size(310.0)
        .show(root, |ui| {
            ui.add_space(4.0);
            // EP-14: the inspector and the placement query panel share the
            // right panel as tabs — query and map stay visible together.
            ui.horizontal(|ui| {
                ui.selectable_value(&mut qstate.tab, RightTab::Inspector, "Inspector");
                ui.selectable_value(&mut qstate.tab, RightTab::Placement, "🔍 Placement");
            });
            ui.separator();
            match qstate.tab {
                RightTab::Placement => {
                    query_panel::draw_query_panel(
                        ui, world, cache, sim, selection, qstate, actions,
                    );
                }
                RightTab::Inspector => {
                    egui::ScrollArea::vertical().show(ui, |ui| match selection.0.clone() {
                        Focus::None => {
                            ui.heading("Inspector");
                            ui.separator();
                            ui.label("Click a floor slab in the 3D view, or pick a unit from the navigator.");
                            ui.add_space(8.0);
                            ui.weak("Colors: gray = vacant · green→blue→red = occupied by patient stability. Bedless wings render neutral for topological context.");
                            ui.add_space(8.0);
                            let h = world.hospital();
                            ui.collapsing("Data limitations", |ui| {
                                for l in &h.meta.known_limitations {
                                    ui.weak(format!("• {l}"));
                                }
                            });
                        }
                        Focus::Building(id) => building_inspector(ui, world, cache, &id, actions),
                        Focus::Floor(id, level) => floor_inspector(ui, world, cache, &id, &level, actions),
                        Focus::Unit(id) => unit_inspector(ui, world, cache, &id, actions),
                        Focus::Room(id) => {
                            room_inspector(ui, world, cache, sim, qstate, &id, transfer_target, actions)
                        }
                    });
                }
            }
        });
}

fn building_inspector(
    ui: &mut egui::Ui,
    world: &AppWorld,
    cache: &AggCache,
    id: &hupsim_core::ids::NodeId,
    actions: &mut Vec<UiAction>,
) {
    let h = world.hospital();
    let Some(&bp) = world.index.building_pos.get(id) else {
        ui.label("Building not found.");
        return;
    };
    let b = &h.buildings[bp];
    ui.heading(&b.name);
    if let Some(built) = b.built {
        ui.weak(format!("built {built}"));
    }
    if !b.aliases.is_empty() {
        ui.weak(format!("aka {}", b.aliases.join(", ")));
    }
    provenance_badge(ui, &b.provenance);
    if let Some(n) = &b.note {
        ui.label(n);
    }
    ui.separator();
    let agg = cache.building(world, id);
    if agg.capacity > 0 {
        ui.label(format!(
            "Census {}/{} · boarding {} · OOS {}",
            agg.census, agg.capacity, agg.boarding, agg.out_of_service
        ));
    } else {
        ui.label("No patient rooms (clinics, ORs, labs, offices).");
    }
    ui.separator();
    for uid in world.index.unit_ids(id) {
        if let Some(&up) = world.index.unit_pos.get(uid) {
            let u = &h.units[up];
            if ui.button(&u.name).clicked() {
                actions.push(UiAction::OpenUnit(uid.clone()));
            }
        }
    }
}

fn floor_inspector(
    ui: &mut egui::Ui,
    world: &AppWorld,
    cache: &AggCache,
    id: &hupsim_core::ids::NodeId,
    level: &FloorLevel,
    actions: &mut Vec<UiAction>,
) {
    let h = world.hospital();
    let Some(&bp) = world.index.building_pos.get(id) else {
        return;
    };
    let b = &h.buildings[bp];
    ui.heading(format!("{} — floor {}", b.short_name, level.label()));
    if let Some(spec) = b.floor(level) {
        if let Some(note) = &spec.note {
            ui.colored_label(Color32::from_rgb(230, 150, 60), format!("⚠ {note}"));
        }
    }
    let agg = cache.floor(id, level);
    if agg.capacity > 0 {
        ui.label(format!(
            "Census {}/{} · boarding {} · OOS {}",
            agg.census, agg.capacity, agg.boarding, agg.out_of_service
        ));
        if let Some(mean) = agg.mean_instability {
            ui.label(format!(
                "Mean instability {:.2} · worst {:.2}",
                mean,
                agg.worst_instability.unwrap_or(0.0)
            ));
        }
    } else {
        ui.label("No patient rooms on this floor.");
    }
    ui.separator();
    let units: Vec<_> = world
        .index
        .unit_ids(id)
        .iter()
        .filter(|uid| {
            world
                .index
                .unit_pos
                .get(*uid)
                .map(|&up| &h.units[up].level == level)
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    if units.is_empty() {
        ui.weak("No units mapped here.");
    }
    for uid in units {
        if let Some(&up) = world.index.unit_pos.get(&uid) {
            let u = &h.units[up];
            let agg = cache.unit_at(up);
            ui.horizontal(|ui| {
                if ui.button(format!("Open {}", u.name)).clicked() {
                    actions.push(UiAction::OpenUnit(uid.clone()));
                }
                ui.label(format!("{}/{}", agg.census, agg.capacity));
            });
            ui.weak(&u.service);
        }
    }
}

fn unit_inspector(
    ui: &mut egui::Ui,
    world: &AppWorld,
    cache: &AggCache,
    id: &UnitId,
    actions: &mut Vec<UiAction>,
) {
    let h = world.hospital();
    let Some(&up) = world.index.unit_pos.get(id) else {
        ui.label("Unit not found.");
        return;
    };
    let u = &h.units[up];
    ui.heading(&u.name);
    ui.label(&u.service);
    ui.weak(u.unit_type.label());
    if let Some(core) = &u.elevator_core {
        ui.weak(format!("{core} elevator"));
    }
    provenance_badge(ui, &u.provenance);
    ui.separator();
    let agg = cache.unit_at(up);
    ui.label(format!(
        "Census {}/{} ({:.0}%) · boarding {} · OOS {}",
        agg.census,
        agg.capacity,
        agg.occupancy_frac() * 100.0,
        agg.boarding,
        agg.out_of_service
    ));
    if let Some(mean) = agg.mean_instability {
        ui.horizontal(|ui| {
            ui.label(format!(
                "Mean instability {mean:.2} · worst {:.2}",
                agg.worst_instability.unwrap_or(0.0)
            ));
            let (r, _) = ui.allocate_exact_size(EVec2::new(14.0, 14.0), Sense::hover());
            ui.painter_at(r)
                .rect_filled(r, CornerRadius::same(3), rgb32(stability_color(mean)));
        });
    }
    ui.separator();
    if ui.button("🗺 Open unit view").clicked() {
        actions.push(UiAction::OpenUnit(id.clone()));
    }
    if ui.button("＋ Add room").clicked() {
        actions.push(UiAction::AddRoom(id.clone()));
    }
}

#[allow(clippy::too_many_arguments)]
fn room_inspector(
    ui: &mut egui::Ui,
    world: &mut AppWorld,
    cache: &AggCache,
    sim: &SimDrive,
    qstate: &mut QueryPanelState,
    id: &RoomId,
    transfer_target: &mut String,
    actions: &mut Vec<UiAction>,
) {
    let w = &mut *world;
    let Some(&ri) = w.index.room_pos.get(id) else {
        ui.label("Room not found.");
        return;
    };
    let mut dirty = false;
    // Immutable copies for header display, then a mutable borrow for editing.
    let (number, kind, unit_id, prov) = {
        let r = &w.scenario.hospital.rooms[ri];
        (
            r.number.clone(),
            r.kind,
            r.unit.clone(),
            r.provenance.clone(),
        )
    };
    let unit_name = w
        .index
        .unit_pos
        .get(&unit_id)
        .map(|&up| w.scenario.hospital.units[up].name.clone())
        .unwrap_or_default();

    ui.heading(format!("Room {number}"));
    ui.horizontal(|ui| {
        ui.label(kind.label());
        if ui.small_button(&unit_name).clicked() {
            actions.push(UiAction::Select(Focus::Unit(unit_id.clone())));
        }
    });
    provenance_badge(ui, &prov);
    // EP-14: stage this patient's needs in the placement query panel.
    // Read-only — the query runs immediately and the Placement tab opens.
    if w.scenario.hospital.rooms[ri].status.patient().is_some()
        && ui
            .button("🔍 Find placement")
            .on_hover_text(
                "stage this patient's level of care, telemetry, and isolation \
                 needs as a placement query — results rank candidate beds",
            )
            .clicked()
    {
        qstate.stage_from_room(w, cache, &sim.engine, id);
    }
    ui.separator();

    let room = &mut w.scenario.hospital.rooms[ri];
    match &mut room.status {
        RoomStatus::Vacant => {
            ui.label("Vacant");
            if ui.button("Admit patient").clicked() {
                actions.push(UiAction::Admit(id.clone()));
            }
            if ui.button("Mark out of service").clicked() {
                actions.push(UiAction::SetOos(id.clone()));
            }
            ui.separator();
            if ui.button("🗑 Delete room").clicked() {
                actions.push(UiAction::DeleteRoom(id.clone()));
            }
        }
        RoomStatus::OutOfService { reason } => {
            ui.colored_label(Color32::from_gray(140), format!("Out of service — {reason}"));
            if ui.button("Return to service").clicked() {
                actions.push(UiAction::ReturnToService(id.clone()));
            }
        }
        RoomStatus::Occupied { patient } => {
            let tier = patient.acuity.tier();
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(EVec2::new(16.0, 16.0), Sense::hover());
                ui.painter_at(r).rect_filled(
                    r,
                    CornerRadius::same(3),
                    rgb32(stability_color(patient.acuity.instability)),
                );
                ui.strong(tier.label());
                if patient.boarding {
                    ui.colored_label(Color32::ORANGE, "BOARDING");
                }
            });
            egui::Grid::new("pt_grid").num_columns(2).show(ui, |ui| {
                ui.label("Alias");
                dirty |= ui.text_edit_singleline(&mut patient.alias).changed();
                ui.end_row();

                ui.label("Age");
                let mut age = patient.age.unwrap_or(0) as i32;
                if ui
                    .add(egui::DragValue::new(&mut age).range(0..=120))
                    .changed()
                {
                    patient.age = if age > 0 { Some(age as u8) } else { None };
                    dirty = true;
                }
                ui.end_row();

                ui.label("Service");
                let mut svc = patient.service.clone().unwrap_or_default();
                if ui.text_edit_singleline(&mut svc).changed() {
                    patient.service = if svc.is_empty() { None } else { Some(svc) };
                    dirty = true;
                }
                ui.end_row();

                ui.label("Isolation");
                let mut iso = patient.isolation.clone().unwrap_or_default();
                if ui.text_edit_singleline(&mut iso).changed() {
                    patient.isolation = if iso.is_empty() { None } else { Some(iso) };
                    dirty = true;
                }
                ui.end_row();
            });

            dirty |= ui
                .add(
                    egui::Slider::new(&mut patient.acuity.instability, 0.0..=1.0)
                        .text("instability"),
                )
                .changed();
            dirty |= ui
                .add(
                    egui::Slider::new(&mut patient.acuity.trend_per_hr, -0.2..=0.2)
                        .text("trend/hr"),
                )
                .on_hover_text("Expected deterioration (+) or recovery (−) drift — consumed by the sim scaffolding")
                .changed();
            ui.horizontal(|ui| {
                dirty |= ui.checkbox(&mut patient.telemetry, "telemetry").changed();
                dirty |= ui.checkbox(&mut patient.boarding, "boarding").changed();
            });
            ui.label("Note");
            dirty |= ui
                .add(egui::TextEdit::multiline(&mut patient.note).desired_rows(2))
                .changed();

            // EP-11: RN assignment — ephemeral sim state, read from the
            // roster; reassignment pins the room until the next turnover.
            if let Some(ur) = sim.engine.roster.unit(&unit_id) {
                if let Some(&rn) = ur.assignment.get(id) {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(format!("Nurse: RN #{}", rn + 1));
                        if ur.pins.contains_key(id) {
                            ui.colored_label(Color32::from_rgb(110, 150, 200), "pinned");
                            if ui.small_button("unpin").clicked() {
                                actions.push(UiAction::UnpinRn(id.clone()));
                            }
                        }
                    });
                    if let Some(wl) = ur.workloads.get(rn as usize) {
                        ui.weak(format!(
                            "load {:.2} = census {:.2} + acuity {:.2} + overhead {:.2} + churn {:.2}",
                            wl.scalar(),
                            wl.census_vs_ratio,
                            wl.instability,
                            wl.overhead,
                            wl.churn
                        ));
                    }
                    if ur.rn_count > 1 {
                        egui::ComboBox::from_id_salt("rn_reassign")
                            .selected_text("reassign to…")
                            .show_ui(ui, |ui| {
                                for k in 0..ur.rn_count as u16 {
                                    if k == rn {
                                        continue;
                                    }
                                    let load = ur
                                        .workloads
                                        .get(k as usize)
                                        .map(|w| format!(" (load {:.2})", w.scalar()))
                                        .unwrap_or_default();
                                    if ui
                                        .selectable_label(false, format!("RN #{}{load}", k + 1))
                                        .clicked()
                                    {
                                        actions.push(UiAction::PinRn {
                                            room: id.clone(),
                                            rn: k,
                                        });
                                    }
                                }
                            });
                    }
                }
            }

            ui.separator();
            if ui.button("Discharge").clicked() {
                actions.push(UiAction::Discharge(id.clone()));
            }
            ui.collapsing("Transfer", |ui| {
                ui.horizontal(|ui| {
                    ui.label("To room #:");
                    ui.text_edit_singleline(transfer_target);
                });
                let target = transfer_target.trim().to_string();
                if !target.is_empty() {
                    // Scan the rooms vec (stable order), not the HashMap —
                    // lookup must be deterministic across runs.
                    let found = w
                        .scenario
                        .hospital
                        .rooms
                        .iter()
                        .find(|r| r.number.eq_ignore_ascii_case(&target) && r.id != *id)
                        .map(|r| (r.id.clone(), r.status.is_vacant()));
                    match found {
                        Some((to, true)) => {
                            if ui.button(format!("Move to {target}")).clicked() {
                                actions.push(UiAction::Transfer {
                                    from: id.clone(),
                                    to,
                                });
                            }
                        }
                        Some((_, false)) => {
                            ui.weak("that room is not vacant");
                        }
                        None => {
                            ui.weak("no room with that number");
                        }
                    }
                }
            });
        }
    }
    if dirty {
        w.version += 1;
    }
}

fn draw_3d_labels(
    ctx: &egui::Context,
    viewport: egui::Rect,
    camera_q: &Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    labels_q: &Query<&LabelAnchor>,
) {
    let Ok((camera, cam_tf)) = camera_q.single() else {
        return;
    };
    for anchor in labels_q {
        if let Ok(pos) = camera.world_to_viewport(cam_tf, anchor.world_pos) {
            egui::Area::new(egui::Id::new(("label3d", &anchor.text)))
                .fixed_pos(egui::pos2(pos.x, pos.y))
                .movable(false)
                .interactable(false)
                .show(ctx, |ui| {
                    // The Area floats on Order::Middle, above the background
                    // layer the panels paint on — clip it to the 3D viewport
                    // so labels never bleed over the drawer/panels (EP-22).
                    ui.shrink_clip_rect(viewport);
                    ui.label(
                        egui::RichText::new(&anchor.text)
                            .strong()
                            .size(if anchor.always { 12.0 } else { 10.0 })
                            .color(Color32::WHITE)
                            .background_color(Color32::from_black_alpha(140)),
                    );
                });
        }
    }
}

fn draw_hover_tooltip(
    ctx: &egui::Context,
    world: &AppWorld,
    cache: &AggCache,
    hover: &HoverInfo,
) {
    let Some((building, level)) = &hover.0 else {
        return;
    };
    let Some(pos) = ctx.pointer_latest_pos() else {
        return;
    };
    let h = world.hospital();
    let Some(&bp) = world.index.building_pos.get(building) else {
        return;
    };
    let b = &h.buildings[bp];
    egui::Area::new(egui::Id::new("floor_tooltip"))
        .fixed_pos(pos + EVec2::new(16.0, 16.0))
        .movable(false)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.strong(format!("{} — floor {}", b.short_name, level.label()));
                let agg = cache.floor(building, level);
                if agg.capacity > 0 {
                    ui.label(format!("Census {}/{}", agg.census, agg.capacity));
                    for uid in world.index.unit_ids(building) {
                        if let Some(&up) = world.index.unit_pos.get(uid) {
                            let u = &h.units[up];
                            if &u.level == level {
                                ui.weak(&u.name);
                            }
                        }
                    }
                    ui.weak("click to inspect");
                } else {
                    ui.weak("no patient rooms");
                }
            });
        });
}

/// Tooltip for a hovered connection (skybridge deck or flow ribbon).
fn draw_conn_tooltip(ctx: &egui::Context, world: &AppWorld, hover: &ConnHover) {
    let Some(idx) = hover.0 else {
        return;
    };
    let Some(conn) = world.hospital().connections.get(idx) else {
        return;
    };
    let Some(pos) = ctx.pointer_latest_pos() else {
        return;
    };
    egui::Area::new(egui::Id::new("conn_tooltip"))
        .fixed_pos(pos + EVec2::new(16.0, 16.0))
        .movable(false)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.strong(conn.name.as_deref().unwrap_or("Unnamed connection"));
                ui.horizontal(|ui| {
                    ui.label(conn.kind.label());
                    ui.colored_label(
                        confidence_color(conn.provenance.confidence),
                        format!("[{}]", conn.provenance.confidence.label()),
                    );
                });
            });
        });
}

#[allow(clippy::too_many_arguments)]
fn apply_actions(
    actions: Vec<UiAction>,
    world: &mut AppWorld,
    sim: &mut SimDrive,
    selection: &mut Selection,
    open_unit: &mut OpenUnit,
    qstate: &mut QueryPanelState,
    dash: &mut DashboardState,
    next_state: &mut NextState<ViewMode>,
    scene_rebuild: &mut SceneRebuild,
) {
    for action in actions {
        let w = &mut *world;
        match action {
            UiAction::Select(f) => selection.0 = f,
            UiAction::OpenUnit(uid) => {
                selection.0 = Focus::Unit(uid.clone());
                open_unit.0 = Some(uid);
                next_state.set(ViewMode::UnitDetail);
            }
            UiAction::BackToCampus => next_state.set(ViewMode::Campus3D),
            UiAction::Admit(room) => {
                let n = w.scenario.hospital.rooms.len() as u64;
                let patient = Patient {
                    id: format!("manual.pt.{}.{}", w.version, n).into(),
                    alias: "New Patient".into(),
                    age: Some(65),
                    service: None,
                    admitted_at_min: Some(sim.engine.clock.now_min),
                    acuity: Acuity {
                        instability: 0.15,
                        trend_per_hr: 0.0,
                    },
                    boarding: false,
                    isolation: None,
                    telemetry: false,
                    note: String::new(),
                };
                if ops::admit(&mut w.scenario.hospital, &w.index, &room, patient).is_ok() {
                    w.version += 1;
                }
            }
            UiAction::AdmitPlaced {
                room,
                telemetry,
                airborne,
            } => {
                let n = w.scenario.hospital.rooms.len() as u64;
                let patient = Patient {
                    id: format!("query.pt.{}.{}", w.version, n).into(),
                    alias: "Staged Patient".into(),
                    age: Some(65),
                    service: None,
                    admitted_at_min: Some(sim.engine.clock.now_min),
                    acuity: Acuity {
                        instability: 0.15,
                        trend_per_hr: 0.0,
                    },
                    boarding: false,
                    isolation: airborne.then(|| "Airborne".to_string()),
                    telemetry,
                    note: "admitted via placement query".into(),
                };
                if ops::admit(&mut w.scenario.hospital, &w.index, &room, patient).is_ok() {
                    selection.0 = Focus::Room(room);
                    w.version += 1;
                }
            }
            UiAction::Discharge(room) => {
                if ops::discharge(&mut w.scenario.hospital, &w.index, &room).is_ok() {
                    w.version += 1;
                }
            }
            UiAction::Transfer { from, to } => {
                if ops::transfer(&mut w.scenario.hospital, &w.index, &from, &to).is_ok() {
                    selection.0 = Focus::Room(to);
                    w.version += 1;
                }
            }
            UiAction::PinRn { room, rn } => {
                let engine = &mut sim.engine;
                let now = engine.clock.now_min;
                if engine.roster.pin(
                    &w.scenario.hospital,
                    &w.index,
                    &room,
                    rn,
                    now,
                    &mut engine.log,
                ) {
                    w.version += 1;
                }
            }
            UiAction::UnpinRn(room) => {
                let engine = &mut sim.engine;
                let now = engine.clock.now_min;
                if engine.roster.unpin(
                    &w.scenario.hospital,
                    &w.index,
                    &room,
                    now,
                    &mut engine.log,
                ) {
                    w.version += 1;
                }
            }
            UiAction::SetOos(room) => {
                if ops::set_out_of_service(
                    &mut w.scenario.hospital,
                    &w.index,
                    &room,
                    "manual block",
                )
                .is_ok()
                {
                    w.version += 1;
                }
            }
            UiAction::ReturnToService(room) => {
                if ops::return_to_service(&mut w.scenario.hospital, &w.index, &room).is_ok() {
                    w.version += 1;
                }
            }
            UiAction::AddRoom(uid) => {
                let count = w.index.room_indices(&uid).len();
                let mut n = count + 1;
                let (number, id) = loop {
                    let number = format!("X{n:02}");
                    let id = format!("room.custom.{}.{}", uid.as_str(), n);
                    // Display numbers must stay unique hospital-wide, not just
                    // ids — the transfer-by-number lookup depends on it.
                    let number_taken = w
                        .scenario
                        .hospital
                        .rooms
                        .iter()
                        .any(|r| r.number.eq_ignore_ascii_case(&number));
                    if !number_taken && !w.index.room_pos.contains_key(&RoomId::from(id.as_str()))
                    {
                        break (number, id);
                    }
                    n += 1;
                };
                w.scenario.hospital.rooms.push(Room {
                    id: id.as_str().into(),
                    number,
                    unit: uid,
                    kind: RoomKind::Inpatient,
                    telemetry_capable: false,
                    negative_pressure: false,
                    status: RoomStatus::Vacant,
                    provenance: Provenance::synthetic(),
                });
                w.reindex();
            }
            UiAction::DeleteRoom(room) => {
                let can_delete = w
                    .index
                    .room_pos
                    .get(&room)
                    .map(|&i| w.scenario.hospital.rooms[i].status.is_vacant())
                    .unwrap_or(false);
                if can_delete {
                    w.scenario.hospital.rooms.retain(|r| r.id != room);
                    selection.0 = Focus::None;
                    w.reindex();
                }
            }
            UiAction::SeedCensus => {
                // Stamps backdate relative to the *current* clock (EP-21), so
                // a mid-run re-seed reads as a fresh snapshot, not a house
                // admitted before t0.
                hupsim_data::seed_demo_census(
                    &mut w.scenario.hospital,
                    &w.index,
                    w.scenario.rng_seed,
                    w.scenario.sim_time_min,
                );
                w.version += 1;
            }
            UiAction::ClearCensus => {
                for r in &mut w.scenario.hospital.rooms {
                    r.status = RoomStatus::Vacant;
                }
                w.version += 1;
            }
            UiAction::Save => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("hupsim scenario", &["json"])
                    .set_file_name("hup_scenario.json")
                    .save_file()
                {
                    let secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    w.scenario.meta.modified = format!("unix:{secs}");
                    if let Err(e) = hupsim_data::io::save_scenario(&path, &w.scenario) {
                        error!("save failed: {e}");
                    }
                }
            }
            UiAction::Load => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("hupsim scenario", &["json"])
                    .pick_file()
                {
                    match hupsim_data::io::load_scenario(&path) {
                        Ok(sc) => {
                            sim.engine.reset(sc.rng_seed);
                            sim.engine.clock.now_min = sc.sim_time_min;
                            w.scenario = sc;
                            selection.0 = Focus::None;
                            // Staged query results hold room ids from the
                            // previous hospital — drop them with the world.
                            // Same for staged/ran experiments (unit ids) and
                            // the morning-report snapshot (old patients).
                            qstate.reset_all();
                            dash.compare.reset_all();
                            dash.report
                                .on_world_replaced(w.scenario.sim_time_min);
                            w.reindex();
                            // The route graph indexes the old hospital's
                            // connections/buildings — rebuild it (EP-6);
                            // engine.reset above already dropped any
                            // in-flight transports.
                            sim.engine.transport.graph = Some(SimDrive::route_graph(
                                &w.scenario.hospital,
                                &w.layout,
                            ));
                            // The loaded hospital's building/floor set may
                            // differ from the entities on screen.
                            scene_rebuild.0 = true;
                        }
                        Err(e) => error!("load failed: {e}"),
                    }
                }
            }
            UiAction::ResetSim => {
                let seed = w.scenario.rng_seed;
                sim.engine.reset(seed);
                w.scenario.sim_time_min = 0.0;
                w.version += 1;
            }
        }
    }
}
