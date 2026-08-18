//! 2D unit detail view: a synthetic double-loaded-corridor floor plan drawn
//! with the egui painter. Rooms are colored by patient stability, clickable,
//! and hover-tooltipped. Units with many bays (the ED) fall back to a grid.
//!
//! EP-12: a header strip above the room grid carries the unit's operational
//! read — census/occupancy with a z-vs-24h badge, stability mix, a census
//! sparkline with its norm band, and per-RN workload bars — while the floor
//! plan stays the visual center. The strip's data assembly is a pure
//! function over the EP-9 cache / ring buffers and the EP-11 roster
//! ([`assemble_strip`], unit-tested below); painting stays thin. No
//! per-frame `Vec<Room>` scans: everything cached reads from [`AggCache`],
//! series from the engine's rolling store, staffing from the roster.

use crate::agg_cache::AggCache;
use crate::widgets::{self, SparkSpec, ZBadge};
use crate::world::{AppWorld, Focus, Selection};
use bevy_egui::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};
use hupsim_core::aggregate::UnitAggregate;
use hupsim_core::color::{stability_color, COLOR_OOS, COLOR_VACANT};
use hupsim_core::ids::UnitId;
use hupsim_core::model::{RoomStatus, Unit};
use hupsim_core::patient::StabilityTier;
use hupsim_core::workload::{self, WorkloadBreakdown, WorkloadWeights};
use hupsim_sim::engine::SimEngine;
use hupsim_sim::rolling::MetricWindows;
use hupsim_sim::roster::UnitRoster;

use widgets::rgb32;

pub fn room_fill(status: &RoomStatus) -> Color32 {
    match status {
        RoomStatus::Vacant => rgb32(COLOR_VACANT),
        RoomStatus::OutOfService { .. } => rgb32(COLOR_OOS),
        RoomStatus::Occupied { patient } => rgb32(stability_color(patient.acuity.instability)),
    }
}

fn room_tooltip(ui: &mut egui::Ui, room: &hupsim_core::model::Room, rn: Option<u16>) {
    ui.strong(format!("Room {}", room.number));
    ui.label(room.kind.label());
    match &room.status {
        RoomStatus::Vacant => {
            ui.label("Vacant");
        }
        RoomStatus::OutOfService { reason } => {
            ui.label(format!("Out of service — {reason}"));
        }
        RoomStatus::Occupied { patient } => {
            ui.label(format!(
                "{} ({})",
                patient.alias,
                patient.age.map_or("?".into(), |a| a.to_string())
            ));
            let tier = patient.acuity.tier();
            ui.label(format!(
                "Instability {:.2} — {}",
                patient.acuity.instability,
                tier.label()
            ));
            if let Some(rn) = rn {
                ui.label(format!("Nurse: RN #{}", rn + 1));
            }
            if patient.boarding {
                ui.colored_label(Color32::ORANGE, "BOARDING (admitted, not in licensed bed)");
            }
            if let Some(iso) = &patient.isolation {
                ui.label(format!("Isolation: {iso}"));
            }
            if patient.telemetry {
                ui.label("On telemetry");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Header-strip view-model (pure — the headlessly testable part of EP-12).
// ---------------------------------------------------------------------------

/// Everything the header strip paints, precomputed from cache + roster.
#[derive(Debug, Clone, PartialEq)]
pub struct StripVm {
    pub census: usize,
    pub capacity: usize,
    /// Licensed beds minus out-of-service — the denominator of occupancy.
    pub staffed_capacity: usize,
    /// `None` when there are no staffed beds (bedless/fully-OOS units).
    pub occupancy_pct: Option<f32>,
    /// Census vs its trailing-24 h norm (EP-9 warm-up honesty encoded).
    pub census_z: ZBadge,
    /// Census above staffed beds — unreachable in the room model but kept as
    /// a defensive state so bad data shows loudly instead of silently.
    pub over_capacity: bool,
    /// Boarders held here (the ED), or this unit full while patients board
    /// hospital-wide — the blockage this unit is part of.
    pub boarding_pressure: bool,
    pub boarding: usize,
    pub out_of_service: usize,
    /// Occupied-room counts per stability tier (Stable…Critical).
    pub tiers: [u32; 4],
    pub mean_instability: Option<f32>,
    pub worst_instability: Option<f32>,
    /// Trailing census samples, oldest → newest.
    pub census_series: Vec<f32>,
    /// Norm band (mean ± kσ); `None` until the 24 h window fills.
    pub census_band: Option<(f32, f32)>,
    /// Norm midline; only shown once the band exists (no half-warm norms).
    pub census_mean: Option<f32>,
    /// `None` for unit types outside the RN model (research, other) — the
    /// strip collapses its staffing row for those.
    pub staffing: Option<StaffingVm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StaffingVm {
    /// Effective patients-per-RN target for this unit.
    pub ratio: f32,
    /// Bar position of an exactly-at-ratio census with nothing on top.
    pub at_ratio_load: f32,
    /// Common bar scale across the unit's RNs.
    pub scale_max: f32,
    /// Empty when the unit is staffed-type but currently has no census.
    pub rns: Vec<RnVm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RnVm {
    /// 0-based; display as "RN #{rn+1}" / tag "N{rn+1}".
    pub rn: u16,
    pub patients: usize,
    pub load: WorkloadBreakdown,
    /// Carrying more patients than the unit's target ratio.
    pub over_ratio: bool,
}

impl RnVm {
    /// Components ordered like [`widgets::WORKLOAD_SEGMENTS`].
    pub fn components(&self) -> [f32; 4] {
        [
            self.load.census_vs_ratio,
            self.load.instability,
            self.load.overhead,
            self.load.churn,
        ]
    }
}

/// Assemble the strip view-model. Pure over its inputs: the cached unit
/// aggregate + tier counts (EP-9 app cache), the unit's rolling windows
/// (EP-9 ring buffers, `None` if never sampled), the RN roster (EP-11 —
/// which owns the workload weights), and the hospital-wide boarding count.
pub fn assemble_strip(
    unit: &Unit,
    agg: &UnitAggregate,
    tiers: [u32; 4],
    windows: Option<&MetricWindows>,
    window: usize,
    roster: &hupsim_sim::roster::RnRoster,
    hospital_boarding: usize,
) -> StripVm {
    let weights: &WorkloadWeights = &roster.weights;
    let unit_roster: Option<&UnitRoster> = roster.unit(&unit.id);
    let staffed = agg.capacity.saturating_sub(agg.out_of_service);

    let census_z = match windows {
        None => ZBadge::Warming { have: 0, need: window },
        Some(w) if !w.census.is_warm() => ZBadge::Warming {
            have: w.census.len(),
            need: w.census.window(),
        },
        Some(w) => match w.census.z(agg.census as f32) {
            Some(z) => ZBadge::Z(z),
            None => ZBadge::Flat,
        },
    };

    let census_series: Vec<f32> = windows
        .map(|w| w.census.iter_ordered().collect())
        .unwrap_or_default();
    let census_band = windows.and_then(|w| w.census.band(widgets::BAND_K));
    let census_mean = census_band.map(|(lo, hi)| (lo + hi) * 0.5);

    let staffing = if workload::is_rn_staffed(unit.unit_type) {
        let ratio = workload::effective_rn_ratio(unit);
        let at_ratio_load = weights.census; // census/ratio = 1.0, weighted
        let rns: Vec<RnVm> = unit_roster
            .map(|r| {
                (0..r.rn_count)
                    .map(|k| {
                        let patients = r.rn_rooms.get(k).map_or(0, |v| v.len());
                        RnVm {
                            rn: k as u16,
                            patients,
                            load: r.workloads.get(k).copied().unwrap_or_default(),
                            over_ratio: patients as f32 > ratio,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let scale_max = rns
            .iter()
            .map(|r| r.load.scalar())
            .fold(at_ratio_load * 1.6, f32::max)
            * 1.05;
        Some(StaffingVm { ratio, at_ratio_load, scale_max, rns })
    } else {
        None
    };

    StripVm {
        census: agg.census,
        capacity: agg.capacity,
        staffed_capacity: staffed,
        occupancy_pct: (staffed > 0).then(|| agg.occupancy_frac() * 100.0),
        census_z,
        over_capacity: staffed > 0 && agg.census > staffed,
        boarding_pressure: agg.boarding > 0
            || (staffed > 0 && agg.census >= staffed && hospital_boarding > 0),
        boarding: agg.boarding,
        out_of_service: agg.out_of_service,
        tiers,
        mean_instability: agg.mean_instability,
        worst_instability: agg.worst_instability,
        census_series,
        census_band,
        census_mean,
        staffing,
    }
}

// ---------------------------------------------------------------------------
// Strip painting (thin).
// ---------------------------------------------------------------------------

/// Small stability swatch + labeled value ("mean 0.32").
fn stability_chip(ui: &mut egui::Ui, label: &str, value: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (r, _) = ui.allocate_exact_size(Vec2::new(11.0, 11.0), Sense::hover());
        ui.painter_at(r)
            .rect_filled(r, CornerRadius::same(2), rgb32(stability_color(value)));
        ui.weak(label);
        ui.strong(format!("{value:.2}"));
    });
}

/// Paint the three strip rows; returns the RN row under the pointer, which
/// the room grid uses to highlight that RN's rooms this same frame.
fn draw_strip(ui: &mut egui::Ui, vm: &StripVm) -> Option<u16> {
    // Row 1 — census/capacity, deviation badge, pressure states.
    ui.horizontal_wrapped(|ui| {
        let occ = vm
            .occupancy_pct
            .map_or(String::new(), |p| format!(" · {p:.0}%"));
        widgets::kpi_chip(
            ui,
            "Census",
            &format!("{}/{}{occ}", vm.census, vm.staffed_capacity),
            Some(&vm.census_z),
        )
        .on_hover_text(format!(
            "{} licensed − {} OOS = {} staffed beds",
            vm.capacity, vm.out_of_service, vm.staffed_capacity
        ));
        if vm.over_capacity {
            ui.colored_label(widgets::COLOR_HOT_HIGH, "⚠ over capacity")
                .on_hover_text("census exceeds staffed beds");
        } else if vm.boarding_pressure {
            ui.colored_label(widgets::COLOR_HOT_HIGH, "⚠ boarding pressure")
                .on_hover_text(if vm.boarding > 0 {
                    "admitted patients are boarding in this unit's bays"
                } else {
                    "unit is full while admitted patients board hospital-wide"
                });
        }
        if vm.boarding > 0 {
            ui.colored_label(Color32::ORANGE, format!("Boarding {}", vm.boarding));
        }
        if vm.out_of_service > 0 {
            ui.weak(format!("OOS {}", vm.out_of_service));
        }
    });

    // Row 2 — stability mix + census-vs-norm sparkline. Skipped entirely for
    // bedless units (nothing to plot).
    if vm.capacity > 0 {
        ui.horizontal(|ui| {
            ui.weak("Stability");
            widgets::tier_histogram(ui, Vec2::new(92.0, 30.0), &vm.tiers);
            match (vm.mean_instability, vm.worst_instability) {
                (Some(mean), worst) => {
                    stability_chip(ui, "mean", mean);
                    if let Some(w) = worst {
                        stability_chip(ui, "worst", w);
                    }
                }
                _ => {
                    ui.weak("empty");
                }
            }
            ui.separator();
            widgets::sparkline_band(
                ui,
                Vec2::new(210.0, 32.0),
                &SparkSpec {
                    samples: &vm.census_series,
                    band: vm.census_band,
                    mean: vm.census_mean,
                    current: Some(vm.census as f32),
                    // Per-unit fans are out of EP-17 scope (hospital-level
                    // only — see the dashboard strip).
                    fan: None,
                },
            )
            .on_hover_text(match vm.census_band {
                Some((lo, hi)) => format!(
                    "unit census, trailing 24 h · norm band {lo:.1}–{hi:.1} (mean ± {}σ)",
                    widgets::BAND_K
                ),
                None => vm.census_z.hover_text(),
            });
            ui.weak("census · 24 h");
        });
    }

    // Row 3 — RN workload bars. Collapsed for unstaffed unit types.
    let mut hovered_rn = None;
    if let Some(staffing) = &vm.staffing {
        if staffing.rns.is_empty() {
            ui.weak("No RNs on — unit empty.");
        } else {
            for rn in &staffing.rns {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (tag_rect, tag_resp) =
                        ui.allocate_exact_size(Vec2::new(24.0, 13.0), Sense::hover());
                    let bar = widgets::workload_bar(
                        ui,
                        Vec2::new(180.0, 13.0),
                        rn.components(),
                        staffing.scale_max,
                        staffing.at_ratio_load,
                    );
                    let hovered = tag_resp.hovered() || bar.hovered();
                    widgets::rn_tag(&ui.painter_at(tag_rect), tag_rect, rn.rn, hovered);
                    if hovered {
                        hovered_rn = Some(rn.rn);
                    }
                    let comps = rn.components();
                    bar.on_hover_ui(move |ui| widgets::workload_tooltip(ui, comps));
                    ui.weak(format!("{} pts", rn.patients));
                    if rn.over_ratio {
                        ui.colored_label(widgets::COLOR_HOT_HIGH, "▲ over ratio")
                            .on_hover_text(format!(
                                "{} patients against a {:.0}:1 target",
                                rn.patients, staffing.ratio
                            ));
                    }
                });
            }
        }
    }
    hovered_rn
}

/// Draw the unit header strip + floor plan; returns without drawing if the
/// unit is unknown. `query_rooms` is the staged placement-result set
/// (EP-14): member cells get the violet outline while results are staged.
pub fn draw_unit_view(
    ui: &mut egui::Ui,
    world: &AppWorld,
    cache: &AggCache,
    engine: &SimEngine,
    unit_id: &UnitId,
    selection: &mut Selection,
    query_rooms: Option<&std::collections::HashSet<hupsim_core::ids::RoomId>>,
) {
    let Some(&unit_pos) = world.index.unit_pos.get(unit_id) else {
        ui.label("Unit not found.");
        return;
    };
    let room_indices: Vec<usize> = world.index.room_indices(unit_id).to_vec();
    let unit = &world.scenario.hospital.units[unit_pos];

    ui.horizontal(|ui| {
        ui.heading(&unit.name);
        ui.label(egui::RichText::new(format!("· {}", unit.service)).weak());
        ui.label(egui::RichText::new(unit.unit_type.label()).weak());
    });

    // EP-12 header strip: assembled from the version-keyed cache, the EP-9
    // ring buffers, and the EP-11 roster — nothing here rescans Vec<Room>.
    let roster = engine.roster.unit(unit_id);
    let vm = assemble_strip(
        unit,
        &cache.unit_at(unit_pos),
        cache.unit_tiers_at(unit_pos),
        engine.rolling.units.get(unit_id),
        engine.rolling.window(),
        &engine.roster,
        cache.stats.boarding,
    );
    let hovered_rn = draw_strip(ui, &vm);
    ui.add_space(4.0);

    let n = room_indices.len();
    if n == 0 {
        ui.label("No rooms in this unit.");
        return;
    }

    let avail = ui.available_size();
    let corridor_mode = n <= 44;

    // Geometry of the synthetic plan.
    let (cols, rows) = if corridor_mode {
        (n.div_ceil(2), 2)
    } else {
        let cols = ((n as f32).sqrt() * 1.6).ceil() as usize;
        (cols.max(1), n.div_ceil(cols.max(1)))
    };
    let corridor_h = if corridor_mode { 26.0 } else { 8.0 };
    let gap = 4.0;
    let room_w = ((avail.x - 16.0 - gap * cols as f32) / cols as f32).clamp(26.0, 88.0);
    let room_h = if corridor_mode {
        ((avail.y - corridor_h - 40.0 - gap * 2.0) / 2.0).clamp(40.0, 110.0)
    } else {
        ((avail.y - 40.0 - gap * rows as f32) / rows as f32).clamp(26.0, 70.0)
    };

    let plan_w = cols as f32 * (room_w + gap) + gap;
    let plan_h = rows as f32 * (room_h + gap) + corridor_h + gap;
    let (plan_rect, _) = ui.allocate_exact_size(Vec2::new(plan_w, plan_h), Sense::hover());
    let painter = ui.painter_at(plan_rect);

    // Corridor band.
    if corridor_mode {
        let c_rect = Rect::from_min_size(
            Pos2::new(plan_rect.min.x, plan_rect.min.y + room_h + gap),
            Vec2::new(plan_w, corridor_h),
        );
        painter.rect_filled(c_rect, CornerRadius::same(3), Color32::from_gray(52));
        painter.text(
            c_rect.center(),
            egui::Align2::CENTER_CENTER,
            "corridor",
            FontId::proportional(11.0),
            Color32::from_gray(120),
        );
    }

    let selected_room = match &selection.0 {
        Focus::Room(r) => Some(r.clone()),
        _ => None,
    };

    for (i, &ri) in room_indices.iter().enumerate() {
        let (col, row) = (i % cols, i / cols);
        let y_off = if corridor_mode && row == 1 {
            room_h + gap + corridor_h + gap
        } else {
            row as f32 * (room_h + gap)
        };
        let rect = Rect::from_min_size(
            Pos2::new(
                plan_rect.min.x + gap + col as f32 * (room_w + gap),
                plan_rect.min.y + y_off,
            ),
            Vec2::new(room_w, room_h),
        );

        let room = &world.scenario.hospital.rooms[ri];
        let fill = room_fill(&room.status);
        let is_selected = selected_room.as_ref() == Some(&room.id);
        // EP-14: cell is in the staged placement results (violet outline,
        // matching the 3D slab tint's hue family).
        let in_query = query_rooms.is_some_and(|s| s.contains(&room.id));
        let rn = roster.and_then(|r| r.assignment.get(&room.id).copied());

        let id = ui.id().with(("room", i));
        let resp = ui.interact(rect, id, Sense::click());
        painter.rect_filled(rect, CornerRadius::same(4), fill);
        let stroke = if is_selected {
            Stroke::new(2.5, Color32::from_rgb(255, 210, 60))
        } else if in_query {
            Stroke::new(2.0, widgets::COLOR_QUERY)
        } else if resp.hovered() {
            Stroke::new(1.5, Color32::WHITE)
        } else {
            Stroke::new(1.0, Color32::from_gray(30))
        };
        painter.rect_stroke(rect, CornerRadius::same(4), stroke, StrokeKind::Inside);

        // Room number + occupant initials.
        painter.text(
            Pos2::new(rect.center().x, rect.min.y + 10.0),
            egui::Align2::CENTER_CENTER,
            &room.number,
            FontId::monospace((room_w * 0.22).clamp(9.0, 13.0)),
            Color32::BLACK,
        );
        if let RoomStatus::Occupied { patient } = &room.status {
            painter.text(
                Pos2::new(rect.center().x, rect.center().y + 8.0),
                egui::Align2::CENTER_CENTER,
                &patient.alias,
                FontId::proportional((room_w * 0.18).clamp(8.0, 12.0)),
                Color32::from_black_alpha(200),
            );
            if patient.boarding {
                painter.circle_filled(
                    Pos2::new(rect.max.x - 7.0, rect.min.y + 7.0),
                    3.5,
                    Color32::ORANGE,
                );
            }
            if patient.acuity.tier() == StabilityTier::Critical {
                painter.rect_stroke(
                    rect.shrink(1.0),
                    CornerRadius::same(4),
                    Stroke::new(2.0, Color32::from_rgb(255, 40, 40)),
                    StrokeKind::Inside,
                );
            }
            // EP-11/12: nurse identity tag, color-keyed to the workload bars
            // (skipped in very short cells where it would collide with the
            // occupant line).
            if let Some(rn) = rn {
                if rect.height() >= 34.0 {
                    let tag = Rect::from_min_size(
                        Pos2::new(rect.min.x + 3.0, rect.max.y - 13.0),
                        Vec2::new(21.0, 10.0),
                    );
                    widgets::rn_tag(&painter, tag, rn, hovered_rn == Some(rn));
                }
            }
        }
        // Hovering an RN's workload bar in the strip highlights their rooms.
        if rn.is_some() && rn == hovered_rn {
            painter.rect_stroke(
                rect.shrink(0.75),
                CornerRadius::same(4),
                Stroke::new(2.0, Color32::WHITE),
                StrokeKind::Inside,
            );
        }

        if resp.clicked() {
            selection.0 = Focus::Room(room.id.clone());
        }
        let room_clone = room.clone();
        resp.on_hover_ui(|ui| room_tooltip(ui, &room_clone, rn));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hupsim_core::model::{FloorLevel, UnitType};
    use hupsim_core::provenance::Provenance;
    use hupsim_sim::rolling::RollingSeries;
    use hupsim_sim::roster::RnRoster;

    /// Wrap a hand-built `UnitRoster` under the test unit's id ("u").
    fn in_roster(ur: UnitRoster) -> RnRoster {
        let mut r = RnRoster::default();
        r.units.insert("u".into(), ur);
        r
    }

    fn unit(unit_type: UnitType, ratio: Option<f32>) -> Unit {
        Unit {
            id: "u".into(),
            name: "U".into(),
            service: "S".into(),
            service_line: None,
            unit_type,
            building: "b".into(),
            level: FloorLevel::Numbered(1),
            elevator_core: None,
            target_rn_ratio: ratio,
            provenance: Provenance::default(),
            note: None,
        }
    }

    fn agg(capacity: usize, census: usize, oos: usize, boarding: usize) -> UnitAggregate {
        UnitAggregate {
            capacity,
            census,
            out_of_service: oos,
            boarding,
            mean_instability: (census > 0).then_some(0.3),
            worst_instability: (census > 0).then_some(0.7),
        }
    }

    /// A roster with the given per-RN room counts (room ids are synthetic —
    /// the VM only counts them).
    fn roster(per_rn: &[usize], weights: &WorkloadWeights, ratio: f32) -> UnitRoster {
        let mut r = UnitRoster {
            rn_count: per_rn.len(),
            ..Default::default()
        };
        for (k, &count) in per_rn.iter().enumerate() {
            let rooms: Vec<hupsim_core::ids::RoomId> = (0..count)
                .map(|i| format!("r{k}.{i}").as_str().into())
                .collect();
            for room in &rooms {
                r.assignment.insert(room.clone(), k as u16);
            }
            r.rn_rooms.push(rooms);
            r.churn.push(0.0);
            let patients =
                vec![hupsim_core::workload::RnLoadInput { instability: 0.2, ..Default::default() }; count];
            r.workloads.push(workload::compose(weights, ratio, &patients, 0.0));
        }
        r
    }

    fn windows_with_census(samples: &[f32], window: usize) -> MetricWindows {
        let mut w = MetricWindows::new(window);
        for &v in samples {
            w.census.push(v);
        }
        w
    }

    #[test]
    fn occupied_unit_reads_cache_and_roster() {
        let w = WorkloadWeights::default();
        let u = unit(UnitType::MedSurg, Some(5.0));
        let r = roster(&[5, 4], &w, 5.0);
        let vm = assemble_strip(&u, &agg(20, 9, 2, 0), [4, 3, 1, 1], None, 96, &in_roster(r), 0);
        assert_eq!(vm.census, 9);
        assert_eq!(vm.staffed_capacity, 18);
        assert!((vm.occupancy_pct.unwrap() - 50.0).abs() < 1e-4);
        assert_eq!(vm.tiers, [4, 3, 1, 1]);
        assert_eq!(vm.mean_instability, Some(0.3));
        let staffing = vm.staffing.expect("med-surg is RN-staffed");
        assert_eq!(staffing.rns.len(), 2);
        assert_eq!(staffing.rns[0].patients, 5);
        assert_eq!(staffing.rns[1].patients, 4);
        assert!(staffing.rns.iter().all(|r| !r.over_ratio));
        // Components mirror the composed breakdown and sum to its scalar.
        let c = staffing.rns[0].components();
        assert!((c.iter().sum::<f32>() - staffing.rns[0].load.scalar()).abs() < 1e-5);
        // Common scale covers every bar and the at-ratio tick.
        for rn in &staffing.rns {
            assert!(staffing.scale_max >= rn.load.scalar());
        }
        assert!(staffing.scale_max >= staffing.at_ratio_load);
    }

    #[test]
    fn warm_up_never_fakes_a_z() {
        let u = unit(UnitType::MedSurg, None);
        // No windows at all (sim never sampled): 0/window.
        let vm = assemble_strip(&u, &agg(10, 5, 0, 0), [0; 4], None, 96, &RnRoster::default(), 0);
        assert_eq!(vm.census_z, ZBadge::Warming { have: 0, need: 96 });
        assert!(vm.census_band.is_none());
        assert!(vm.census_series.is_empty());
        // Partially-filled window: have/need reported, still no z, no band.
        let mw = windows_with_census(&[3.0, 4.0, 5.0], 96);
        let vm = assemble_strip(&u, &agg(10, 5, 0, 0), [0; 4], Some(&mw), 96, &RnRoster::default(), 0);
        assert_eq!(vm.census_z, ZBadge::Warming { have: 3, need: 96 });
        assert!(vm.census_band.is_none());
        assert!(vm.census_mean.is_none());
        assert_eq!(vm.census_series, vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn warm_window_yields_z_and_band() {
        let u = unit(UnitType::MedSurg, None);
        let mw = windows_with_census(&[1.0, 2.0, 3.0, 4.0], 4);
        let vm = assemble_strip(&u, &agg(10, 5, 0, 0), [0; 4], Some(&mw), 4, &RnRoster::default(), 0);
        // Same z the ring buffer itself reports (mean 2.5, σ ≈ 1.118).
        let mut expect = RollingSeries::new(4);
        for v in [1.0, 2.0, 3.0, 4.0] {
            expect.push(v);
        }
        match vm.census_z {
            ZBadge::Z(z) => assert!((z - expect.z(5.0).unwrap()).abs() < 1e-6),
            other => panic!("expected a z, got {other:?}"),
        }
        let (lo, hi) = vm.census_band.expect("warm window has a band");
        let (elo, ehi) = expect.band(widgets::BAND_K).unwrap();
        assert!((lo - elo).abs() < 1e-6 && (hi - ehi).abs() < 1e-6);
        assert!((vm.census_mean.unwrap() - 2.5).abs() < 1e-5);
    }

    #[test]
    fn flat_warm_window_is_flat_not_zero() {
        let u = unit(UnitType::MedSurg, None);
        let mw = windows_with_census(&[7.0; 4], 4);
        let vm = assemble_strip(&u, &agg(10, 7, 0, 0), [0; 4], Some(&mw), 4, &RnRoster::default(), 0);
        assert_eq!(vm.census_z, ZBadge::Flat);
    }

    #[test]
    fn over_ratio_flags_the_heavy_rn_only() {
        let w = WorkloadWeights::default();
        let u = unit(UnitType::MedSurg, Some(5.0));
        // Balance shifted a boundary: RN #1 carries ratio+1.
        let r = roster(&[6, 5], &w, 5.0);
        let vm = assemble_strip(&u, &agg(12, 11, 0, 0), [11, 0, 0, 0], None, 96, &in_roster(r), 0);
        let staffing = vm.staffing.unwrap();
        assert!(staffing.rns[0].over_ratio, "6 pts at 5:1 is over ratio");
        assert!(!staffing.rns[1].over_ratio, "5 pts at 5:1 is at ratio");
    }

    #[test]
    fn research_unit_collapses_staffing_but_keeps_census() {
        let vm = assemble_strip(
            &unit(UnitType::Research, None),
            &agg(6, 2, 0, 0),
            [2, 0, 0, 0],
            None,
            96,
            &RnRoster::default(),
            0,
        );
        assert!(vm.staffing.is_none(), "research units carry no RN roster");
        assert_eq!(vm.census, 2);
        assert!(vm.occupancy_pct.is_some());
    }

    #[test]
    fn empty_staffed_unit_keeps_the_row_with_no_rns() {
        let r = UnitRoster::default(); // derived roster of an empty unit
        let vm = assemble_strip(
            &unit(UnitType::Icu, Some(2.0)),
            &agg(10, 0, 0, 0),
            [0; 4],
            None,
            96,
            &in_roster(r),
            0,
        );
        let staffing = vm.staffing.expect("ICU is a staffed type");
        assert!(staffing.rns.is_empty());
        assert_eq!(vm.occupancy_pct, Some(0.0));
        assert_eq!(vm.mean_instability, None);
    }

    #[test]
    fn oos_moves_staffed_capacity_and_occupancy() {
        let u = unit(UnitType::MedSurg, None);
        let vm = assemble_strip(&u, &agg(10, 5, 4, 0), [5, 0, 0, 0], None, 96, &RnRoster::default(), 0);
        assert_eq!(vm.staffed_capacity, 6);
        assert!((vm.occupancy_pct.unwrap() - 100.0 * 5.0 / 6.0).abs() < 1e-3);
        // Fully-OOS unit: no denominator, no percentage.
        let vm = assemble_strip(&u, &agg(10, 0, 10, 0), [0; 4], None, 96, &RnRoster::default(), 0);
        assert_eq!(vm.occupancy_pct, None);
    }

    #[test]
    fn pressure_states() {
        let u = unit(UnitType::MedSurg, None);
        // Full unit + hospital-wide boarders → pressure.
        let vm = assemble_strip(&u, &agg(10, 10, 0, 0), [10, 0, 0, 0], None, 96, &RnRoster::default(), 3);
        assert!(vm.boarding_pressure);
        assert!(!vm.over_capacity);
        // Full unit, nobody boarding anywhere → no pressure.
        let vm = assemble_strip(&u, &agg(10, 10, 0, 0), [10, 0, 0, 0], None, 96, &RnRoster::default(), 0);
        assert!(!vm.boarding_pressure);
        // One free bed → no pressure even with boarders elsewhere.
        let vm = assemble_strip(&u, &agg(10, 9, 0, 0), [9, 0, 0, 0], None, 96, &RnRoster::default(), 3);
        assert!(!vm.boarding_pressure);
        // The ED: local boarders are pressure regardless of vacancy.
        let ed = unit(UnitType::Ed, Some(4.0));
        let vm = assemble_strip(&ed, &agg(20, 8, 0, 3), [8, 0, 0, 0], None, 96, &RnRoster::default(), 3);
        assert!(vm.boarding_pressure);
        assert_eq!(vm.boarding, 3);
    }
}
