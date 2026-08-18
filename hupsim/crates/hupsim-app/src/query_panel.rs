//! EP-14 query / staging panel: the Placement tab of the right panel. Stage
//! a patient's needs (copied from a real patient via "Find placement", or
//! set hypothetically through the criteria chips), Run, and get a ranked
//! list of candidate beds — while the 3D campus stays visible, result floors
//! tinted live, and the open unit view outlining result cells.
//!
//! The panel is pure decision support: running a query is read-only over the
//! world (matrix + EP-9 norms + EP-11 roster — never the RNG), and acting on
//! a result hands off to the existing sanctioned ops (`ops::transfer` /
//! `ops::admit` via `UiAction`). Results are a snapshot of the world version
//! they ran against; the panel says so when the world has moved on, and
//! re-running is one click — it never re-runs behind the user's back.

use crate::agg_cache::AggCache;
use crate::widgets;
use crate::world::{AppWorld, Focus, Selection, SimDrive};
use bevy::prelude::Resource;
use bevy_egui::egui::{self, Vec2};
use hupsim_core::ids::{NodeId, RoomId};
use hupsim_core::model::{FloorLevel, UnitType};
use hupsim_core::patient::StabilityTier;
use hupsim_core::query::{
    self, LocTier, PlacementQuery, PlacementResult, PlacementWeights, TeleFit, PLACEMENT_TARGETS,
};
use hupsim_sim::engine::SimEngine;
use std::collections::HashSet;

use crate::ui::UiAction;

/// Which tab the right panel shows. The inspector and the query panel
/// coexist as tabs (owner brief left tab-vs-second-panel open; tabs keep the
/// 3D viewport wide, which is the point of a dockable panel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightTab {
    #[default]
    Inspector,
    Placement,
}

/// Covering-RN qualitative labels for the reasons line ("RN #2 light").
/// Thresholds sit against the workload composite: at-ratio census alone
/// scores ~1.0, so under [`RN_LIGHT_MAX`] reads light; past [`RN_HEAVY_MIN`]
/// the unit strip is visibly stacked.
const RN_LIGHT_MAX: f32 = 1.2;
const RN_HEAVY_MIN: f32 = 2.0;

/// Displayed-result cap. The engine ranks every candidate (a wide-open
/// med/surg query can pass hundreds of beds); the list shows the top slice
/// and says how many were cut — never a silent truncation.
const RESULT_DISPLAY_CAP: usize = 40;

/// A ran query's snapshot: the ranked results, the highlight sets derived
/// from them (room ids for unit-view outlines, floors for 3D slab tint),
/// the criteria they answered, and the world version they ran against.
pub struct StagedResults {
    pub results: Vec<PlacementResult>,
    pub query: PlacementQuery,
    pub rooms: HashSet<RoomId>,
    pub floors: HashSet<(NodeId, FloorLevel)>,
    pub world_version: u64,
}

#[derive(Resource, Default)]
pub struct QueryPanelState {
    pub tab: RightTab,
    /// The criteria being edited (chips bind straight to this).
    pub query: PlacementQuery,
    /// Last Run's snapshot; `None` = nothing staged, no highlights.
    pub staged: Option<StagedResults>,
    /// Bumped on every result-set change — the 3D tint system keys on it,
    /// like the dashboard's drill state.
    pub generation: u64,
}

impl QueryPanelState {
    /// Run the staged criteria against the current world (read-only; no RNG).
    pub fn run(&mut self, world: &AppWorld, cache: &AggCache, engine: &SimEngine) {
        self.staged = Some(run_query(world, cache, engine, &self.query));
        self.generation += 1;
    }

    /// Drop the staged results (and with them every highlight).
    pub fn clear_staged(&mut self) {
        if self.staged.take().is_some() {
            self.generation += 1;
        }
    }

    /// Reset to a blank panel — scenario loads must not carry stale room ids.
    pub fn reset_all(&mut self) {
        self.query = PlacementQuery::default();
        self.tab = RightTab::Inspector;
        self.clear_staged();
    }

    /// "Find placement" on an occupied room: copy the patient's needs into
    /// the criteria, run immediately, and bring the Placement tab forward.
    pub fn stage_from_room(
        &mut self,
        world: &AppWorld,
        cache: &AggCache,
        engine: &SimEngine,
        room: &RoomId,
    ) {
        if let Some(q) = query_from_room(world, room) {
            self.query = q;
            self.run(world, cache, engine);
            self.tab = RightTab::Placement;
        }
    }
}

/// Build a query from a real patient: telemetry and airborne isolation from
/// the patient; level of care and service line from their current unit —
/// except ED boarders, whose target maps from acuity tier (Critical → ICU,
/// Unstable → stepdown, else med/surg) with no line requested (an ED stay
/// has no inpatient service identity yet). All editable before Run.
pub fn query_from_room(world: &AppWorld, room_id: &RoomId) -> Option<PlacementQuery> {
    let h = world.hospital();
    let &ri = world.index.room_pos.get(room_id)?;
    let room = &h.rooms[ri];
    let p = room.status.patient()?;
    let &up = world.index.unit_pos.get(&room.unit)?;
    let u = &h.units[up];

    let (target, service_line) = if u.unit_type == UnitType::Ed {
        let target = match p.acuity.tier() {
            StabilityTier::Critical => UnitType::Icu,
            StabilityTier::Unstable => UnitType::Stepdown,
            _ => UnitType::MedSurg,
        };
        (target, None)
    } else {
        let line = u
            .service_line
            .as_ref()
            .and_then(|id| h.service_lines.iter().position(|l| &l.id == id))
            .map(|i| i as u16);
        (u.unit_type, line)
    };

    Some(PlacementQuery {
        target,
        telemetry: p.telemetry,
        airborne: p
            .isolation
            .as_deref()
            .is_some_and(|s| s.to_ascii_lowercase().contains("airborne")),
        service_line,
        from_room: Some(room_id.clone()),
        overflow: false,
    })
}

/// Assemble the scoring context and run the engine: matrix + per-unit
/// aggregates from the version-keyed cache, occupancy z from the EP-9
/// rolling norms, covering-RN loads from the EP-11 roster. Everything read
/// is derived state — this never touches the sim RNG or mutates the world.
pub fn run_query(
    world: &AppWorld,
    cache: &AggCache,
    engine: &SimEngine,
    q: &PlacementQuery,
) -> StagedResults {
    let h = world.hospital();
    let unit_occ_z: Vec<Option<f32>> = h
        .units
        .iter()
        .enumerate()
        .map(|(i, u)| {
            let occ = cache.units.get(i).map_or(0.0, |a| a.occupancy_frac());
            engine
                .rolling
                .units
                .get(&u.id)
                .and_then(|w| w.occupancy_frac.z(occ))
        })
        .collect();
    let covering = engine.roster.covering_rn_loads(h, &world.index);
    let ctx = query::PlacementContext {
        matrix: &cache.matrix,
        unit_aggs: &cache.units,
        unit_occ_z: &unit_occ_z,
        covering_rn: &covering,
    };
    let results = query::run(&ctx, q, &PlacementWeights::default());

    let mut rooms = HashSet::new();
    let mut floors = HashSet::new();
    for r in &results {
        rooms.insert(r.room.clone());
        if let Some(u) = h.units.get(r.unit_pos as usize) {
            floors.insert((u.building.clone(), u.level.clone()));
        }
    }
    StagedResults {
        results,
        query: q.clone(),
        rooms,
        floors,
        world_version: world.version,
    }
}

/// The reasons line, from facts the engine already computed — e.g.
/// "exact LOC · service match · unit at 78% (z −0.4) · RN #2 light · tele
/// capable". Criteria the query didn't engage are simply absent.
fn reasons_line(r: &PlacementResult) -> String {
    let mut parts: Vec<String> = vec![r.tier.label().to_string()];
    match r.service_match {
        Some(true) => parts.push("service match".into()),
        Some(false) => parts.push("other service".into()),
        None => {}
    }
    let z = match r.occ_z {
        Some(z) => format!("z {z:+.1}"),
        None => "z —".into(),
    };
    parts.push(format!("unit at {:.0}% ({z})", r.occupancy_frac * 100.0));
    if let Some((rn, load)) = r.covering_rn {
        let qual = if load < RN_LIGHT_MAX {
            "light"
        } else if load < RN_HEAVY_MIN {
            "steady"
        } else {
            "heavy"
        };
        parts.push(format!("RN #{} {qual}", rn + 1));
    }
    match r.tele {
        TeleFit::Capable => parts.push("tele capable".into()),
        TeleFit::PortableMonitor => parts.push("tele via portable monitor".into()),
        TeleFit::NotNeeded => {}
    }
    parts.join(" · ")
}

/// Draw the Placement tab. Selection focus, unit opening, and every world
/// mutation go through `actions`; only the panel's own state (criteria,
/// staged results) is touched directly.
pub fn draw_query_panel(
    ui: &mut egui::Ui,
    world: &AppWorld,
    cache: &AggCache,
    sim: &SimDrive,
    selection: &Selection,
    qs: &mut QueryPanelState,
    actions: &mut Vec<UiAction>,
) {
    let h = world.hospital();
    ui.heading("Placement query");
    ui.weak("Stage patient needs, Run, act on a result. Results highlight in the 3D campus and unit views.");
    ui.add_space(4.0);

    // Staged-from chip (a real patient's query becomes a transfer handoff).
    if let Some(from) = qs.query.from_room.clone() {
        let desc = world.index.room_pos.get(&from).map(|&ri| {
            let room = &h.rooms[ri];
            match room.status.patient() {
                Some(p) => format!("{} in room {}", p.alias, room.number),
                None => format!("room {} (now vacant)", room.number),
            }
        });
        ui.horizontal(|ui| {
            ui.label(format!("for {}", desc.unwrap_or_else(|| "an unknown room".into())));
            if ui
                .small_button("✕")
                .on_hover_text("detach from the patient — acting becomes admit, not transfer")
                .clicked()
            {
                qs.query.from_room = None;
            }
        });
    }

    // Criteria chips.
    ui.horizontal_wrapped(|ui| {
        ui.label("Needs:");
        egui::ComboBox::from_id_salt("query_loc")
            .selected_text(qs.query.target.label())
            .show_ui(ui, |ui| {
                for t in PLACEMENT_TARGETS {
                    ui.selectable_value(&mut qs.query.target, t, t.label());
                }
            });
        ui.checkbox(&mut qs.query.telemetry, "telemetry")
            .on_hover_text("scored, not filtered: non-capable rooms stay listed as labeled portable-monitor placements");
        ui.checkbox(&mut qs.query.airborne, "airborne isolation")
            .on_hover_text("hard filter: requires a negative-pressure room");
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Service:");
        let selected = qs
            .query
            .service_line
            .and_then(|i| h.service_lines.get(i as usize))
            .map_or("any", |l| l.name.as_str());
        egui::ComboBox::from_id_salt("query_line")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut qs.query.service_line, None, "any");
                for (i, line) in h.service_lines.iter().enumerate() {
                    ui.selectable_value(&mut qs.query.service_line, Some(i as u16), &line.name);
                }
            });
        let overflow_band = query::overflow_types(qs.query.target);
        let label = ui.checkbox(&mut qs.query.overflow, "adjacent-LOC overflow");
        if overflow_band.is_empty() {
            label.on_hover_text(format!(
                "{} has no adjacent-LOC band — the toggle changes nothing here",
                qs.query.target.label()
            ));
        } else {
            label.on_hover_text(format!(
                "also list {} beds, clearly labeled OVERFLOW — they always rank below every exact-LOC candidate",
                overflow_band
                    .iter()
                    .map(|t| t.label())
                    .collect::<Vec<_>>()
                    .join(" / ")
            ));
        }
    });

    ui.horizontal(|ui| {
        if ui.button("▶ Run query").clicked() {
            qs.run(world, cache, &sim.engine);
        }
        if qs.staged.is_some() && ui.button("✕ Clear").on_hover_text("drop results and highlights").clicked() {
            qs.clear_staged();
        }
    });
    ui.separator();

    let Some(staged) = &qs.staged else {
        ui.weak("No results staged. Set criteria and Run — or click an occupied room and “Find placement”.");
        return;
    };

    if staged.world_version != world.version {
        ui.colored_label(
            widgets::COLOR_HOT_HIGH,
            "⚠ world changed since this run — re-run for live results",
        );
    }
    ui.horizontal(|ui| {
        ui.strong(format!("{} candidate beds", staged.results.len()));
        if staged.results.len() > RESULT_DISPLAY_CAP {
            ui.weak(format!("showing top {RESULT_DISPLAY_CAP} — refine the query"));
        }
    })
    .response
    .on_hover_text(
        "ranked: exact level of care always above overflow, then service match, \
         occupancy headroom, RN headroom, telemetry fit (weights in core::query)",
    );
    if staged.results.is_empty() {
        ui.weak("Nothing qualifies. Loosen a hard criterion — or try the overflow toggle.");
        return;
    }

    let scale = PlacementWeights::default().max_for(&staged.query).max(1e-3);
    let selected_room = match &selection.0 {
        Focus::Room(r) => Some(r.clone()),
        _ => None,
    };

    egui::ScrollArea::vertical()
        .id_salt("query_results")
        .show(ui, |ui| {
            for (rank, r) in staged.results.iter().take(RESULT_DISPLAY_CAP).enumerate() {
                // Resolve display names through the id, not the row — ids
                // survive world edits made after the run.
                let Some(&ri) = world.index.room_pos.get(&r.room) else {
                    continue;
                };
                let room = &h.rooms[ri];
                let unit_name = world
                    .index
                    .unit_pos
                    .get(&room.unit)
                    .map_or("?", |&up| h.units[up].name.as_str());

                ui.horizontal(|ui| {
                    let is_selected = selected_room.as_ref() == Some(&r.room);
                    let resp = ui
                        .selectable_label(
                            is_selected,
                            format!("{}. {} — {}", rank + 1, room.number, unit_name),
                        )
                        .on_hover_text("focus this room (highlights its floor in 3D)");
                    if resp.clicked() {
                        actions.push(UiAction::Select(Focus::Room(r.room.clone())));
                    }
                    if r.tier == LocTier::Overflow {
                        ui.colored_label(
                            widgets::COLOR_HOT_HIGH,
                            format!("OVERFLOW · {}", r.unit_type.label()),
                        )
                        .on_hover_text(
                            "adjacent level of care — listed only because the overflow \
                             toggle is on; ranks below every exact-LOC candidate",
                        );
                    }
                });
                ui.horizontal(|ui| {
                    let comps = r.breakdown.components();
                    widgets::score_bar(ui, Vec2::new(130.0, 12.0), comps, scale)
                        .on_hover_ui(move |ui| widgets::score_tooltip(ui, comps));
                    ui.strong(format!("{:.2}", r.breakdown.total()));
                    if ui.small_button("🗺").on_hover_text("open unit view").clicked() {
                        actions.push(UiAction::OpenUnit(room.unit.clone()));
                        actions.push(UiAction::Select(Focus::Room(r.room.clone())));
                    }
                    match &staged.query.from_room {
                        Some(from) => {
                            if ui
                                .small_button("→ transfer")
                                .on_hover_text("move the staged patient here (ops::transfer)")
                                .clicked()
                            {
                                actions.push(UiAction::Transfer {
                                    from: from.clone(),
                                    to: r.room.clone(),
                                });
                            }
                        }
                        None => {
                            if ui
                                .small_button("→ admit")
                                .on_hover_text(
                                    "admit a staged patient with these needs (ops::admit)",
                                )
                                .clicked()
                            {
                                actions.push(UiAction::AdmitPlaced {
                                    room: r.room.clone(),
                                    telemetry: staged.query.telemetry,
                                    airborne: staged.query.airborne,
                                });
                            }
                        }
                    }
                });
                ui.weak(reasons_line(r));
                ui.add_space(6.0);
            }
        });
}
