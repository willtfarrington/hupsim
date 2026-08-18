//! EP-13 hospital-wide aggregate dashboard: a toggleable bottom drawer over
//! the 3D campus with three group-by tabs — level of care ([`UnitType`]),
//! service line (EP-8 taxonomy), building — each row carrying the same
//! deviation-from-norm anatomy as the EP-12 unit strip (census chip,
//! occupancy z badge, boarding, worst-instability swatch, census sparkline
//! with its 24 h norm band). Rows sort by a pressure key (occupancy z, then
//! boarding) so the surging group is always on top.
//!
//! Assembly is pure ([`assemble_rows`] + the per-tab collectors, unit-tested
//! below) over the EP-9 aggregate-cache rollups and group ring buffers;
//! painting stays thin. The drawer's open flag gates both assembly and
//! painting, and closing clears the drill-through, so a closed dashboard
//! costs nothing per frame.

use crate::agg_cache::AggCache;
use crate::widgets::{self, FanSpec, SparkSpec, ZBadge};
use bevy::prelude::Resource;
use bevy_egui::egui::{self, Color32, CornerRadius, Sense, Vec2};
use hupsim_core::aggregate::UnitAggregate;
use hupsim_core::color::stability_color;
use hupsim_core::ids::{NodeId, UnitId};
use hupsim_core::model::{Building, FloorLevel, Hospital, ServiceLine, UnitType};
use hupsim_core::provenance::Confidence;
use hupsim_sim::rolling::{MetricWindows, RollingSeries, RollingStore};
use std::collections::HashSet;

use widgets::rgb32;

/// The service-line id the EP-8 compiler reserves for units with no public
/// service identity. Its row carries the unverified marker so the grouping
/// never reads as ground truth.
pub const OTHER_LINE_ID: &str = "line.other";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DashTab {
    #[default]
    LevelOfCare,
    ServiceLine,
    Building,
    /// EP-15: stage interventions, run the headless A/B experiment, compare.
    Compare,
}

impl DashTab {
    pub const ALL: [DashTab; 4] = [
        DashTab::LevelOfCare,
        DashTab::ServiceLine,
        DashTab::Building,
        DashTab::Compare,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DashTab::LevelOfCare => "Level of care",
            DashTab::ServiceLine => "Service line",
            DashTab::Building => "Building",
            DashTab::Compare => "⚖ Compare",
        }
    }
}

/// One dashboard grouping. Indices refer into the hospital's own vecs
/// (`service_lines`, `buildings`), exactly like the cache rollups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKey {
    UnitType(UnitType),
    ServiceLine(u16),
    Building(u16),
}

/// An active drill-through: the clicked group plus its precomputed members —
/// unit ids filter the navigator, floors drive the 3D emissive tint.
#[derive(Debug, Clone, PartialEq)]
pub struct DrillFocus {
    pub key: GroupKey,
    pub name: String,
    pub units: HashSet<UnitId>,
    pub floors: HashSet<(NodeId, FloorLevel)>,
}

#[derive(Resource, Default)]
pub struct DashboardState {
    pub open: bool,
    pub tab: DashTab,
    pub drill: Option<DrillFocus>,
    /// EP-15 compare-tab state (staged interventions, last run).
    pub compare: crate::compare_panel::CompareState,
    /// EP-16 morning-report window state (snapshot, 07:00 auto-open).
    pub report: crate::report_panel::ReportPanelState,
}

impl DashboardState {
    /// Toggle the drawer. Closing always clears the drill-through, so a
    /// closed dashboard leaves no navigator filter or slab tint behind.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.drill = None;
        }
    }
}

// ---------------------------------------------------------------------------
// View-model (pure — the headlessly testable part of EP-13).
// ---------------------------------------------------------------------------

/// Everything one dashboard row paints — the EP-12 strip anatomy, per group
/// instead of per unit.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupRow {
    pub key: GroupKey,
    pub name: String,
    pub census: usize,
    pub capacity: usize,
    /// Licensed minus out-of-service (the occupancy denominator).
    pub staffed: usize,
    /// `None` when the group has no staffed beds (fully OOS).
    pub occupancy_pct: Option<f32>,
    /// Occupancy vs the group's trailing-24 h norm (EP-9 warm-up honesty).
    pub occ_z: ZBadge,
    pub boarding: usize,
    pub out_of_service: usize,
    pub worst_instability: Option<f32>,
    /// Trailing census samples, oldest → newest (the sparkline series).
    pub census_series: Vec<f32>,
    /// Norm band (mean ± kσ); `None` until the 24 h window fills.
    pub census_band: Option<(f32, f32)>,
    pub census_mean: Option<f32>,
    /// Unverified-provenance marker (the Other/Unassigned service line).
    pub unverified: bool,
}

/// One group's raw ingredients before assembly.
pub struct GroupInput<'a> {
    pub key: GroupKey,
    pub name: String,
    pub agg: UnitAggregate,
    /// `None` when the rolling store has never tracked this group.
    pub windows: Option<&'a MetricWindows>,
    pub unverified: bool,
}

/// Deviation badge for a rolling series, encoding the EP-9 warm-up rule:
/// no window → warming, partial window → warming with have/need, full-but-
/// flat → no meaningful z, otherwise the standard score.
pub fn zbadge_for(series: &RollingSeries, current: f32) -> ZBadge {
    if !series.is_warm() {
        ZBadge::Warming {
            have: series.len(),
            need: series.window(),
        }
    } else {
        match series.z(current) {
            Some(z) => ZBadge::Z(z),
            None => ZBadge::Flat,
        }
    }
}

/// Pressure sort key: occupancy z leads, boarding count breaks ties. Groups
/// still warming carry no norm claim and sort below any real z (ordering by
/// boarding among themselves); a flat norm (σ ≈ 0) reads as z 0. The caller
/// sorts descending with a *stable* sort, so full ties keep the fixed
/// taxonomy order and the result is deterministic.
fn pressure_key(row: &GroupRow) -> (f32, f32) {
    let z = match row.occ_z {
        ZBadge::Z(z) => z,
        ZBadge::Flat => 0.0,
        ZBadge::Warming { .. } => f32::NEG_INFINITY,
    };
    (z, row.boarding as f32)
}

/// Groups × cache → sorted rows. Zero-capacity groups (a service line with
/// no beds, an unused level of care) drop out; the rest sort surging-first
/// by [`pressure_key`]. `window` sizes the warming badge for groups the
/// rolling store has never tracked.
pub fn assemble_rows(inputs: Vec<GroupInput>, window: usize) -> Vec<GroupRow> {
    let mut rows: Vec<GroupRow> = inputs
        .into_iter()
        .filter(|g| g.agg.capacity > 0)
        .map(|g| {
            let staffed = g.agg.capacity.saturating_sub(g.agg.out_of_service);
            let occ_z = match g.windows {
                None => ZBadge::Warming { have: 0, need: window },
                Some(w) => zbadge_for(&w.occupancy_frac, g.agg.occupancy_frac()),
            };
            let census_series: Vec<f32> = g
                .windows
                .map(|w| w.census.iter_ordered().collect())
                .unwrap_or_default();
            let census_band = g.windows.and_then(|w| w.census.band(widgets::BAND_K));
            GroupRow {
                key: g.key,
                name: g.name,
                census: g.agg.census,
                capacity: g.agg.capacity,
                staffed,
                occupancy_pct: (staffed > 0).then(|| g.agg.occupancy_frac() * 100.0),
                occ_z,
                boarding: g.agg.boarding,
                out_of_service: g.agg.out_of_service,
                worst_instability: g.agg.worst_instability,
                census_series,
                census_band,
                census_mean: census_band.map(|(lo, hi)| (lo + hi) * 0.5),
                unverified: g.unverified,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        let (az, ab) = pressure_key(a);
        let (bz, bb) = pressure_key(b);
        bz.total_cmp(&az).then(bb.total_cmp(&ab))
    });
    rows
}

/// Tab 1 — level of care: one row per bedded [`UnitType`], from the cache's
/// `by_unit_type` rollup ([`UnitType::ALL`] order before pressure sorting).
pub fn level_of_care_rows(
    by_type: &[(UnitType, UnitAggregate)],
    rolling: &RollingStore,
) -> Vec<GroupRow> {
    assemble_rows(
        by_type
            .iter()
            .map(|(t, agg)| GroupInput {
                key: GroupKey::UnitType(*t),
                name: t.label().to_string(),
                agg: agg.clone(),
                windows: Some(rolling.unit_type(*t)),
                unverified: false,
            })
            .collect(),
        rolling.window(),
    )
}

/// Tab 2 — service line: one row per EP-8 line (`by_line` indexed like
/// `lines`); the Other/Unassigned line carries the unverified marker.
pub fn service_line_rows(
    lines: &[ServiceLine],
    by_line: &[UnitAggregate],
    rolling: &RollingStore,
) -> Vec<GroupRow> {
    assemble_rows(
        lines
            .iter()
            .zip(by_line)
            .enumerate()
            .map(|(i, (line, agg))| GroupInput {
                key: GroupKey::ServiceLine(i as u16),
                name: line.name.clone(),
                agg: agg.clone(),
                windows: rolling.service_lines.get(&line.id),
                unverified: line.id.as_str() == OTHER_LINE_ID,
            })
            .collect(),
        rolling.window(),
    )
}

/// Tab 3 — building: one row per bedded building (the navigator's grouping),
/// `by_building` indexed like `buildings`.
pub fn building_rows(
    buildings: &[Building],
    by_building: &[UnitAggregate],
    rolling: &RollingStore,
) -> Vec<GroupRow> {
    assemble_rows(
        buildings
            .iter()
            .zip(by_building)
            .enumerate()
            .filter(|(_, (b, _))| b.has_inpatient_beds)
            .map(|(i, (b, agg))| GroupInput {
                key: GroupKey::Building(i as u16),
                name: b.short_name.clone(),
                agg: agg.clone(),
                windows: rolling.buildings.get(&b.id),
                unverified: false,
            })
            .collect(),
        rolling.window(),
    )
}

/// Member units of a group, plus the (building, floor) slabs they occupy —
/// the drill-through payload (navigator filter + 3D emissive tint).
pub fn group_members(
    h: &Hospital,
    key: GroupKey,
) -> (HashSet<UnitId>, HashSet<(NodeId, FloorLevel)>) {
    let mut units = HashSet::new();
    let mut floors = HashSet::new();
    for u in &h.units {
        let member = match key {
            GroupKey::UnitType(t) => u.unit_type == t,
            GroupKey::ServiceLine(i) => h
                .service_lines
                .get(i as usize)
                .is_some_and(|l| u.service_line.as_ref() == Some(&l.id)),
            GroupKey::Building(i) => {
                h.buildings.get(i as usize).is_some_and(|b| u.building == b.id)
            }
        };
        if member {
            units.insert(u.id.clone());
            floors.insert((u.building.clone(), u.level.clone()));
        }
    }
    (units, floors)
}

// ---------------------------------------------------------------------------
// Painting (thin).
// ---------------------------------------------------------------------------

/// Draw the drawer content. Returns a building to focus when a building row
/// was clicked (the caller owns `Focus`; a building row behaves like the
/// navigator's building click). `sim` is read-only: the group tabs read its
/// rolling norms; the EP-15 compare tab additionally reads its seed, clock,
/// and process toggles to parameterize experiment runs (on clones — the
/// live engine is never mutated here).
pub fn draw_dashboard(
    ui: &mut egui::Ui,
    world: &crate::world::AppWorld,
    cache: &AggCache,
    sim: &crate::world::SimDrive,
    dash: &mut DashboardState,
) -> Option<NodeId> {
    let h = world.hospital();
    let rolling: &RollingStore = &sim.engine.rolling;
    let mut focus_building = None;
    let mut clear_drill = false;
    let mut close = false;

    ui.horizontal(|ui| {
        ui.strong("Hospital dashboard");
        ui.separator();
        for tab in DashTab::ALL {
            if ui.selectable_label(dash.tab == tab, tab.label()).clicked() {
                dash.tab = tab;
            }
        }
        if let Some(d) = &dash.drill {
            ui.separator();
            ui.strong(format!("🎯 {}", d.name));
            ui.weak(format!("{} units", d.units.len()));
            if ui
                .small_button("✕ clear")
                .on_hover_text("clear group focus (Esc)")
                .clicked()
            {
                clear_drill = true;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("✕").on_hover_text("close dashboard (D)").clicked() {
                close = true;
            }
        });
    });
    if clear_drill {
        dash.drill = None;
    }
    if close {
        dash.toggle();
        return None;
    }
    ui.separator();

    // EP-17: the hospital census fan rides above every group tab (never the
    // compare tab — no forecasts of hypotheticals, scope fence).
    if dash.tab != DashTab::Compare {
        forecast_strip(ui, cache, sim);
    }

    let rows = match dash.tab {
        DashTab::LevelOfCare => level_of_care_rows(&cache.by_unit_type, rolling),
        DashTab::ServiceLine => {
            service_line_rows(&h.service_lines, &cache.by_service_line, rolling)
        }
        DashTab::Building => building_rows(&h.buildings, &cache.buildings, rolling),
        DashTab::Compare => {
            // auto_shrink off (here and on the rows ScrollArea below): the
            // panel persists its height from the frame's content rect, so a
            // shrink-to-rows scroll area snaps the drawer back to content
            // height and drops the dragged size (EP-19).
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                crate::compare_panel::draw_compare(ui, world, sim, &mut dash.compare);
            });
            return None;
        }
    };
    if rows.is_empty() {
        ui.weak("No bedded groups to show.");
        // Same persistence trap as the scroll areas: fill the allotted
        // height so the one-line message doesn't shrink the stored size.
        ui.allocate_space(ui.available_size());
        return None;
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new(("dash_rows", dash.tab))
            .striped(true)
            .num_columns(6)
            .spacing(Vec2::new(16.0, 4.0))
            .show(ui, |ui| {
                ui.weak("group").on_hover_text(
                    "rows sort by pressure — occupancy z first, then boarding — \
                     so the surging group is on top",
                );
                ui.weak("census");
                ui.weak("occupancy");
                ui.weak("boarding");
                ui.weak("worst");
                ui.weak("census · 24 h");
                ui.end_row();

                for row in &rows {
                    draw_row(ui, h, dash, row, &mut focus_building);
                    ui.end_row();
                }
            });
    });
    focus_building
}

/// Shared hover legend for the census fan — the honesty line the EP-17
/// brief requires wherever the fan is shown (flow decomposition + the
/// measured band coverage).
pub fn fan_hover_text(fan: Option<&hupsim_sim::forecast::CensusFan>) -> String {
    match fan {
        None => "⛅ forecast pending".to_string(),
        Some(f) => format!(
            "⛅ {:.0} h fan (dashed median · shaded 10–90): model-consistent projection \
             from the sim's own λ/LOS/discharge tables applied to the standing census — \
             not a reading of the event queue. Expected +{:.0} in · −{:.0} out. \
             The 10–90 band covered {:.0}% of forecasts in a 7-day replay (CALIBRATION.md).",
            f.horizon_min() / 60.0,
            f.expected_in,
            f.expected_out,
            hupsim_sim::forecast::FAN_COVERAGE_10_90_PCT,
        ),
    }
}

/// EP-17 hospital forecast strip: the census fan at a readable size with the
/// one-line honesty caption. Hospital-level only — per-group fans were
/// deliberately skipped (see the row sparkline note in `draw_row`).
fn forecast_strip(ui: &mut egui::Ui, cache: &AggCache, sim: &crate::world::SimDrive) {
    let Some(fan) = cache.fan.as_ref() else {
        return;
    };
    let series = &sim.engine.rolling.hospital.census;
    let samples: Vec<f32> = series.iter_ordered().collect();
    let band = series.band(widgets::BAND_K);
    ui.horizontal(|ui| {
        ui.strong("Hospital census");
        widgets::kpi_chip(
            ui,
            "",
            &format!("{}/{}", fan.census_now, fan.staffed_now),
            None,
        )
        .on_hover_text("census / staffed beds, whole hospital");
        widgets::sparkline_band(
            ui,
            Vec2::new(430.0, 34.0),
            &SparkSpec {
                samples: &samples,
                band,
                mean: band.map(|(lo, hi)| (lo + hi) * 0.5),
                current: Some(fan.census_now as f32),
                fan: Some(FanSpec {
                    median: &fan.median,
                    lo: &fan.q10,
                    hi: &fan.q90,
                }),
            },
        )
        .on_hover_text(fan_hover_text(Some(fan)));
        // The honesty caption the brief requires under/beside the fan.
        ui.weak(format!(
            "⛅ 12 h model-consistent projection · 10–90 band covered {:.0}% in replay · CALIBRATION.md",
            hupsim_sim::forecast::FAN_COVERAGE_10_90_PCT
        ))
        .on_hover_text(fan_hover_text(Some(fan)));
    });
    ui.separator();
}

/// The six cells of one group row.
fn draw_row(
    ui: &mut egui::Ui,
    h: &Hospital,
    dash: &mut DashboardState,
    row: &GroupRow,
    focus_building: &mut Option<NodeId>,
) {
    let selected = dash.drill.as_ref().is_some_and(|d| d.key == row.key);

    // Group name — the click target for drill-through.
    ui.horizontal(|ui| {
        let resp = ui
            .selectable_label(selected, &row.name)
            .on_hover_text(match row.key {
                GroupKey::Building(_) => "click to inspect this building",
                _ => "click to focus: filters the navigator and tints member floors in 3D",
            });
        if row.unverified {
            // Same ink as the inspector's [Unverified] provenance badge.
            ui.colored_label(
                crate::ui::confidence_color(Confidence::Unverified),
                "[Unverified]",
            )
            .on_hover_text(
                "units with no public service identity — \
                 this grouping is a placeholder, not ground truth",
            );
        }
        if resp.clicked() {
            match row.key {
                GroupKey::Building(i) => {
                    if let Some(b) = h.buildings.get(i as usize) {
                        *focus_building = Some(b.id.clone());
                    }
                }
                key => {
                    if selected {
                        dash.drill = None; // re-click toggles the focus off
                    } else {
                        let (units, floors) = group_members(h, key);
                        dash.drill = Some(DrillFocus {
                            key,
                            name: row.name.clone(),
                            units,
                            floors,
                        });
                    }
                }
            }
        }
    });

    widgets::kpi_chip(ui, "", &format!("{}/{}", row.census, row.staffed), None).on_hover_text(
        format!(
            "{} licensed − {} OOS = {} staffed beds",
            row.capacity, row.out_of_service, row.staffed
        ),
    );

    ui.horizontal(|ui| {
        match row.occupancy_pct {
            Some(p) => {
                ui.strong(format!("{p:.0}%"));
            }
            None => {
                ui.weak("—");
            }
        }
        let z = &row.occ_z;
        let r = match z.color() {
            Some(c) => ui.colored_label(c, z.text()),
            None => ui.weak(z.text()),
        };
        r.on_hover_text(format!("occupancy vs its trailing-24 h norm · {}", z.hover_text()));
    });

    if row.boarding > 0 {
        ui.colored_label(Color32::ORANGE, row.boarding.to_string());
    } else {
        ui.weak("0");
    }

    match row.worst_instability {
        Some(w) => {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let (r, _) = ui.allocate_exact_size(Vec2::new(11.0, 11.0), Sense::hover());
                ui.painter_at(r)
                    .rect_filled(r, CornerRadius::same(2), rgb32(stability_color(w)));
                ui.label(format!("{w:.2}"));
            });
        }
        None => {
            ui.weak("—");
        }
    }

    widgets::sparkline_band(
        ui,
        Vec2::new(150.0, 22.0),
        &SparkSpec {
            samples: &row.census_series,
            band: row.census_band,
            mean: row.census_mean,
            current: Some(row.census as f32),
            // No per-group fans (EP-17 judgment call): the 22 px row spark
            // already carries three encodings, and the hospital-level model's
            // flow split across groups (vacancy-share placement, ED transfer
            // coupling) is exactly what a simple model cannot honestly
            // claim. The hospital fan lives in the strip above.
            fan: None,
        },
    )
    .on_hover_text(match row.census_band {
        Some((lo, hi)) => format!(
            "group census, trailing 24 h · norm band {lo:.1}–{hi:.1} (mean ± {}σ)",
            widgets::BAND_K
        ),
        None => "group census, trailing 24 h — norm warming up".to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use hupsim_core::model::{BuildingKind, HospitalMeta, Unit};
    use hupsim_core::provenance::Provenance;

    fn agg(
        capacity: usize,
        census: usize,
        oos: usize,
        boarding: usize,
        worst: Option<f32>,
    ) -> UnitAggregate {
        UnitAggregate {
            capacity,
            census,
            out_of_service: oos,
            boarding,
            mean_instability: worst.map(|w| w * 0.6),
            worst_instability: worst,
        }
    }

    fn fill(w: &mut MetricWindows, occ: &[f32], census: &[f32]) {
        for &v in occ {
            w.occupancy_frac.push(v);
        }
        for &v in census {
            w.census.push(v);
        }
    }

    fn empty_by_type() -> Vec<(UnitType, UnitAggregate)> {
        UnitType::ALL
            .iter()
            .map(|t| (*t, UnitAggregate::default()))
            .collect()
    }

    #[test]
    fn zbadge_encodes_warmup_flat_and_z() {
        let mut s = RollingSeries::new(3);
        s.push(1.0);
        assert_eq!(zbadge_for(&s, 5.0), ZBadge::Warming { have: 1, need: 3 });
        s.push(1.0);
        s.push(1.0);
        assert_eq!(zbadge_for(&s, 1.0), ZBadge::Flat, "σ≈0 yields no z");
        let mut v = RollingSeries::new(3);
        for x in [1.0, 2.0, 3.0] {
            v.push(x);
        }
        match zbadge_for(&v, 4.0) {
            ZBadge::Z(z) => assert!((z - v.z(4.0).unwrap()).abs() < 1e-6),
            other => panic!("expected a z, got {other:?}"),
        }
    }

    #[test]
    fn level_of_care_drops_bedless_types_and_sorts_surging_first() {
        let mut rolling = RollingStore::new(4);
        // ICU: warm norm around 50% occupancy; currently 95% → large +z.
        fill(
            &mut rolling.unit_types[UnitType::Icu.index()],
            &[0.5, 0.45, 0.55, 0.5],
            &[10.0, 9.0, 11.0, 10.0],
        );
        // Med-surg: warm but flat (σ ≈ 0) → no meaningful z, sorts at z 0.
        fill(&mut rolling.unit_types[UnitType::MedSurg.index()], &[0.5; 4], &[15.0; 4]);
        let mut by_type = empty_by_type();
        by_type[UnitType::Icu.index()].1 = agg(20, 19, 0, 0, Some(0.9));
        by_type[UnitType::MedSurg.index()].1 = agg(30, 15, 0, 0, Some(0.4));
        // The ED never sampled → warming, sorts below any real z despite
        // carrying boarding.
        by_type[UnitType::Ed.index()].1 = agg(40, 30, 0, 6, None);

        let rows = level_of_care_rows(&by_type, &rolling);
        assert_eq!(rows.len(), 3, "bedless unit types must be dropped");
        assert_eq!(rows[0].name, "ICU");
        assert!(matches!(rows[0].occ_z, ZBadge::Z(z) if z > widgets::Z_HOT));
        assert_eq!(rows[1].name, "Med/Surg");
        assert_eq!(rows[1].occ_z, ZBadge::Flat);
        assert_eq!(rows[2].name, "Emergency");
        assert_eq!(rows[2].occ_z, ZBadge::Warming { have: 0, need: 4 });
        // Row data passes straight through from cache + ring buffers.
        assert_eq!(rows[0].census, 19);
        assert_eq!(rows[0].staffed, 20);
        assert_eq!(rows[0].worst_instability, Some(0.9));
        assert_eq!(rows[0].census_series, vec![10.0, 9.0, 11.0, 10.0]);
        let expect = rolling.unit_type(UnitType::Icu).census.band(widgets::BAND_K);
        assert_eq!(rows[0].census_band, expect);
        assert!(rows[0].census_band.is_some());
    }

    #[test]
    fn boarding_breaks_ties_and_full_ties_keep_taxonomy_order() {
        // Nothing recorded: every group is warming, so ordering falls to
        // boarding, then the stable UnitType::ALL order.
        let rolling = RollingStore::new(4);
        let mut by_type = empty_by_type();
        by_type[UnitType::MedSurg.index()].1 = agg(10, 8, 0, 0, None);
        by_type[UnitType::Icu.index()].1 = agg(10, 8, 0, 2, None);
        by_type[UnitType::Stepdown.index()].1 = agg(10, 8, 0, 0, None);
        let rows = level_of_care_rows(&by_type, &rolling);
        assert_eq!(rows[0].name, "ICU", "boarding outranks warming peers");
        assert_eq!(rows[1].name, "Med/Surg");
        assert_eq!(rows[2].name, "Stepdown");
        // Deterministic: same inputs, same order.
        assert_eq!(rows, level_of_care_rows(&by_type, &rolling));
    }

    #[test]
    fn service_lines_mark_other_unverified_and_drop_zero_bed_lines() {
        let line = |id: &str, name: &str| ServiceLine {
            id: id.into(),
            name: name.into(),
            description: None,
        };
        let lines = vec![
            line("line.medicine", "Medicine"),
            line("line.transplant", "Transplant"),
            line(OTHER_LINE_ID, "Other / Unassigned"),
        ];
        let by_line = vec![
            agg(30, 20, 0, 0, Some(0.5)),
            agg(0, 0, 0, 0, None), // a line with zero beds
            agg(12, 6, 0, 0, Some(0.3)),
        ];
        let mut rolling = RollingStore::new(4);
        let mut mw = MetricWindows::new(4);
        fill(&mut mw, &[0.6, 0.6, 0.62, 0.61], &[18.0, 18.0, 19.0, 18.0]);
        rolling.service_lines.insert("line.medicine".into(), mw);

        let rows = service_line_rows(&lines, &by_line, &rolling);
        assert_eq!(rows.len(), 2, "the zero-bed line must be dropped");
        let med = rows.iter().find(|r| r.name == "Medicine").unwrap();
        assert!(!med.unverified);
        assert!(matches!(med.occ_z, ZBadge::Z(_)));
        assert!(matches!(med.key, GroupKey::ServiceLine(0)));
        let other = rows.iter().find(|r| r.name == "Other / Unassigned").unwrap();
        assert!(other.unverified, "line.other must carry the marker");
        // Never tracked by the store → warming 0/window, empty sparkline.
        assert_eq!(other.occ_z, ZBadge::Warming { have: 0, need: 4 });
        assert!(other.census_series.is_empty());
        assert!(other.census_band.is_none());
    }

    fn building(id: &str, short: &str, bedded: bool) -> Building {
        Building {
            id: id.into(),
            name: short.into(),
            short_name: short.into(),
            kind: BuildingKind::Building,
            built: None,
            aliases: vec![],
            floors: vec![],
            has_inpatient_beds: bedded,
            provenance: Provenance::default(),
            note: None,
        }
    }

    #[test]
    fn building_rows_are_bedded_buildings_only() {
        let buildings = vec![
            building("b.pav", "Pavilion", true),
            building("b.office", "Offices", false),
            building("b.founders", "Founders", true),
        ];
        let by_building = vec![
            agg(500, 400, 10, 4, Some(0.7)),
            agg(0, 0, 0, 0, None),
            agg(120, 90, 0, 0, Some(0.5)),
        ];
        let rolling = RollingStore::new(4);
        let rows = building_rows(&buildings, &by_building, &rolling);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.name != "Offices"));
        // Both warming → boarding puts the Pavilion first.
        assert_eq!(rows[0].name, "Pavilion");
        assert!(matches!(rows[0].key, GroupKey::Building(0)));
        // OOS moves the staffed denominator, as in the unit strip.
        assert_eq!(rows[0].staffed, 490);
        assert!((rows[0].occupancy_pct.unwrap() - 100.0 * 400.0 / 490.0).abs() < 1e-3);
    }

    fn members_hospital() -> Hospital {
        let unit = |id: &str, building: &str, level: i32, ut: UnitType, line: &str| Unit {
            id: id.into(),
            name: id.to_uppercase(),
            service: "S".into(),
            service_line: Some(line.into()),
            unit_type: ut,
            building: building.into(),
            level: FloorLevel::Numbered(level),
            elevator_core: None,
            target_rn_ratio: None,
            provenance: Provenance::default(),
            note: None,
        };
        Hospital {
            meta: HospitalMeta::default(),
            buildings: vec![building("b1", "B1", true), building("b2", "B2", true)],
            units: vec![
                unit("u1", "b1", 1, UnitType::MedSurg, "line.medicine"),
                unit("u2", "b1", 2, UnitType::Icu, "line.critical"),
                unit("u3", "b2", 1, UnitType::MedSurg, "line.medicine"),
            ],
            rooms: vec![],
            connections: vec![],
            elevator_cores: vec![],
            service_lines: vec![
                ServiceLine {
                    id: "line.medicine".into(),
                    name: "Medicine".into(),
                    description: None,
                },
                ServiceLine {
                    id: "line.critical".into(),
                    name: "Critical Care".into(),
                    description: None,
                },
            ],
        }
    }

    /// The EP-13 acceptance scenario, headless: the real compiled hospital
    /// with the EP-10 processes on, a 24 h warm-up to fill the norms, then
    /// every vacant ICU bed forced out of service. The ICU / Critical Care
    /// rows must surge to the top of their tabs with a positive z, and
    /// continued ED pressure against the constrained house shows boarding.
    #[test]
    fn constrained_icu_surges_to_the_top() {
        use hupsim_core::index::HospitalIndex;
        use hupsim_core::matrix::RoomMatrix;
        use hupsim_core::model::RoomStatus;
        use hupsim_sim::prelude::{AcuityDrift, DiurnalFlow, EdPressure, SimEngine};

        // Empty start: the seeded demo census saturates the house within a
        // day (occupancy pegged at 100% everywhere), which would flatten the
        // contrast this test needs. From empty, the diurnal day gives every
        // group a sub-saturation norm the ICU constraint can then break.
        let loaded = hupsim_data::load_default().expect("embedded data compiles");
        let mut h = loaded.hospital;
        let idx = HospitalIndex::build(&h);

        let mut eng = SimEngine::new(42);
        eng.register(Box::new(AcuityDrift::default()), true);
        eng.register(Box::new(DiurnalFlow::default()), true);
        eng.register(Box::new(EdPressure::default()), true);
        for _ in 0..96 {
            eng.tick(&mut h, &idx, 15.0); // 24 h at the sampling cadence
        }

        // Constrain: every vacant ICU bed goes out of service, so current
        // occupancy (census / staffed) jumps well above its trailing norm.
        let icu_rooms: Vec<usize> = h
            .units
            .iter()
            .filter(|u| u.unit_type == UnitType::Icu)
            .flat_map(|u| idx.room_indices(&u.id).to_vec())
            .collect();
        assert!(!icu_rooms.is_empty(), "the compiled hospital has ICU beds");
        for ri in icu_rooms {
            if h.rooms[ri].status.is_vacant() {
                h.rooms[ri].status = RoomStatus::OutOfService {
                    reason: "surge drill".into(),
                };
            }
        }
        // A few more hours of ED pressure against the constrained house —
        // short enough that the norm window stays dominated by the
        // unconstrained day.
        for _ in 0..16 {
            eng.tick(&mut h, &idx, 15.0);
        }

        let m = RoomMatrix::build(&h, &idx);
        let rows = level_of_care_rows(&m.rollup_by_unit_type(), &eng.rolling);
        assert_eq!(rows[0].name, "ICU", "constrained ICU must lead its tab");
        assert!(
            matches!(rows[0].occ_z, ZBadge::Z(z) if z > 0.0),
            "ICU occupancy must sit above its norm, got {:?}",
            rows[0].occ_z
        );

        let line_rows = service_line_rows(
            &h.service_lines,
            &m.rollup_by_service_line(h.service_lines.len()),
            &eng.rolling,
        );
        // The general ICUs (MICU/SICU) sit in Critical Care; with their
        // vacant beds gone the line leads the service-line tab.
        assert_eq!(line_rows[0].name, "Critical Care");
        assert!(matches!(line_rows[0].occ_z, ZBadge::Z(z) if z > 0.0));

        // Keep the constraint through the morning surge: ICU-bound ED
        // conversions can't place and must board at some point.
        let mut saw_boarding = false;
        for _ in 0..48 {
            eng.tick(&mut h, &idx, 15.0);
            saw_boarding |= hupsim_core::aggregate::hospital_stats(&h).boarding > 0;
        }
        assert!(
            saw_boarding,
            "the constrained house must show boarding under ED pressure"
        );
    }

    #[test]
    fn group_members_collects_units_and_floors() {
        let h = members_hospital();
        fn ids(set: &HashSet<UnitId>) -> Vec<&str> {
            let mut v: Vec<&str> = set.iter().map(|u| u.as_str()).collect();
            v.sort();
            v
        }

        let (units, floors) = group_members(&h, GroupKey::UnitType(UnitType::MedSurg));
        assert_eq!(ids(&units), vec!["u1", "u3"]);
        assert!(floors.contains(&("b1".into(), FloorLevel::Numbered(1))));
        assert!(floors.contains(&("b2".into(), FloorLevel::Numbered(1))));
        assert_eq!(floors.len(), 2);

        let (units, floors) = group_members(&h, GroupKey::ServiceLine(1));
        assert_eq!(ids(&units), vec!["u2"]);
        assert_eq!(floors.len(), 1);
        assert!(floors.contains(&("b1".into(), FloorLevel::Numbered(2))));

        let (units, _) = group_members(&h, GroupKey::Building(1));
        assert_eq!(ids(&units), vec!["u3"]);

        // Dangling indices (stale drill after a scenario swap) match nothing.
        let (units, floors) = group_members(&h, GroupKey::ServiceLine(9));
        assert!(units.is_empty() && floors.is_empty());
    }
}
