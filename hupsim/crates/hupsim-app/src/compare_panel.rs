//! EP-15 compare tab of the hospital dashboard drawer: stage interventions,
//! run the headless A/B experiment, and read the overlaid baseline vs
//! intervention series per selected group.
//!
//! Session flow (owner brief): staging and running never mutate the live
//! scenario — `run_experiment` clones the world, and the live engine is only
//! read (its seed, clock, process toggles, and rolling norms). Results are a
//! snapshot: they carry the world version they ran against and the panel
//! says so when the world has moved on, like the EP-14 query panel.

use crate::widgets::{self, AbSpec, COLOR_AB_BASELINE, COLOR_AB_VARIANT};
use crate::world::{AppWorld, SimDrive};
use bevy_egui::egui::{self, Color32, CornerRadius, Sense, Vec2};
use hupsim_core::ids::UnitId;
use hupsim_core::model::{Hospital, UnitType};
use hupsim_core::workload;
use hupsim_sim::clock::SimClock;
use hupsim_sim::experiment::{
    run_experiment, summarize, ExperimentResult, ExperimentSpec, Intervention, MetricSeries,
    ProcessToggles, SeriesKey,
};

/// Which intervention the draft row is building.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DraftKind {
    #[default]
    CloseUnit,
    ArrivalWave,
    FlexRatio,
}

impl DraftKind {
    const ALL: [DraftKind; 3] = [DraftKind::CloseUnit, DraftKind::ArrivalWave, DraftKind::FlexRatio];

    fn label(self) -> &'static str {
        match self {
            DraftKind::CloseUnit => "Close unit",
            DraftKind::ArrivalWave => "Arrival wave",
            DraftKind::FlexRatio => "Flex RN ratio",
        }
    }
}

/// The draft intervention being edited (chips bind straight to this).
pub struct Draft {
    pub kind: DraftKind,
    pub unit: Option<UnitId>,
    pub ratio: f32,
    pub wave_count: u32,
    pub wave_start_hr: f64,
    pub wave_duration_hr: f64,
    pub wave_need: UnitType,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            kind: DraftKind::CloseUnit,
            unit: None,
            ratio: 4.0,
            wave_count: 20,
            wave_start_hr: 0.0,
            wave_duration_hr: 6.0,
            wave_need: UnitType::MedSurg,
        }
    }
}

/// A completed run plus the metadata the panel needs to stay honest about
/// what it is showing.
pub struct RanExperiment {
    pub result: ExperimentResult,
    /// Sim minute the experiment started from (= the clock when Run was hit).
    pub ran_at_min: f64,
    /// `AppWorld.version` the run cloned — staleness marker.
    pub world_version: u64,
    /// Which group's series the charts show.
    pub group: SeriesKey,
}

/// All compare-tab state, owned by the dashboard drawer state.
#[derive(Default)]
pub struct CompareState {
    pub staged: Vec<Intervention>,
    pub horizon_hr: f64,
    pub draft: Draft,
    pub ran: Option<RanExperiment>,
}

impl CompareState {
    fn horizon(&mut self) -> &mut f64 {
        if self.horizon_hr <= 0.0 {
            self.horizon_hr = 48.0;
        }
        &mut self.horizon_hr
    }

    /// Drop results (scenario load — stale unit ids must not linger).
    pub fn reset_all(&mut self) {
        *self = Self::default();
    }
}

/// Run the staged experiment against the current world. Read-only over the
/// live state; the result lands in `state.ran`.
fn run(state: &mut CompareState, world: &AppWorld, sim: &SimDrive) {
    let engine = &sim.engine;
    let start_min = engine.clock.now_min;
    // Decorrelated from the live stream but reproducible: same world moment,
    // same seed, same result.
    let seed = engine.seed() ^ (start_min as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut spec = ExperimentSpec::new(seed, start_min, state.horizon_hr.max(1.0));
    spec.toggles = ProcessToggles {
        diurnal: engine.processes[sim.idx_diurnal].enabled,
        ed: engine.processes[sim.idx_ed].enabled,
        drift: engine.processes[sim.idx_drift].enabled,
    };
    spec.interventions = state.staged.clone();
    let result = run_experiment(world.hospital(), Some(&engine.rolling), &spec);
    state.ran = Some(RanExperiment {
        result,
        ran_at_min: start_min,
        world_version: world.version,
        group: SeriesKey::Hospital,
    });
}

pub fn draw_compare(
    ui: &mut egui::Ui,
    world: &AppWorld,
    sim: &SimDrive,
    state: &mut CompareState,
) {
    let h = world.hospital();
    staging_row(ui, h, state);
    ui.horizontal(|ui| {
        ui.label("Horizon:");
        ui.add(
            egui::Slider::new(state.horizon(), 24.0..=96.0)
                .step_by(12.0)
                .suffix(" h"),
        );
        let run_label = if state.staged.is_empty() {
            "▶ Run A/A"
        } else {
            "▶ Run A/B"
        };
        if ui
            .button(run_label)
            .on_hover_text(
                "Clone the current world twice, replay both with one seed \
                 (baseline vs staged interventions), headlessly. The live sim \
                 is never touched.",
            )
            .clicked()
        {
            run(state, world, sim);
        }
        ui.weak("one deterministic realization — not an expectation");
    });
    ui.separator();

    let Some(ran) = &mut state.ran else {
        ui.weak(
            "Stage an intervention (or none, for an A/A null check) and Run. \
             Variants run on clones for the chosen horizon; every difference \
             between the lines is attributable to the intervention.",
        );
        return;
    };

    // Header: legend + provenance of the run.
    let desc = ran.result.spec.describe(h);
    ui.horizontal_wrapped(|ui| {
        legend_swatch(ui, COLOR_AB_BASELINE, "baseline");
        legend_swatch(ui, COLOR_AB_VARIANT, &format!("intervention — {desc}"));
        ui.separator();
        let started = SimClock {
            now_min: ran.ran_at_min,
            ..SimClock::default()
        };
        ui.weak(format!(
            "from {} · {:.0} h horizon · seed {:x}",
            started.label(),
            ran.result.spec.horizon_min / 60.0,
            ran.result.spec.seed
        ));
        if ran.world_version != world.version {
            ui.colored_label(
                Color32::from_rgb(200, 160, 70),
                "⚠ world has moved on — re-run to compare against now",
            );
        }
    });
    if ran.result.variant.wave_backlog > 0 {
        ui.colored_label(
            Color32::ORANGE,
            format!(
                "wave demand not fully placed: {} of {} admissions never found a bed",
                ran.result.variant.wave_backlog,
                ran.result.variant.wave_backlog + ran.result.variant.wave_injected
            ),
        );
    }

    // Group picker.
    let options = group_options(h, ran);
    let current = options
        .iter()
        .find(|(k, _)| *k == ran.group)
        .map(|(_, l)| l.clone())
        .unwrap_or_else(|| "Hospital".into());
    ui.horizontal(|ui| {
        ui.label("Group:");
        egui::ComboBox::from_id_salt("ab_group")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (key, label) in &options {
                    if ui
                        .selectable_label(ran.group == *key, label)
                        .clicked()
                    {
                        ran.group = key.clone();
                    }
                }
            });
    });

    let (Some(a), Some(b)) = (
        ran.result.baseline.series.series(&ran.group),
        ran.result.variant.series.series(&ran.group),
    ) else {
        ui.weak("no series for this group");
        return;
    };
    charts_row(ui, a, b, ran.result.spec.sample_every_min);
}

/// Staged-intervention chips plus the draft row that adds one.
fn staging_row(ui: &mut egui::Ui, h: &Hospital, state: &mut CompareState) {
    let mut remove: Option<usize> = None;
    ui.horizontal_wrapped(|ui| {
        ui.strong("Interventions:");
        if state.staged.is_empty() {
            ui.weak("none staged (A/A)");
        }
        for (i, iv) in state.staged.iter().enumerate() {
            egui::Frame::new()
                .fill(Color32::from_gray(36))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(iv.describe(h));
                        if ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                });
        }
    });
    if let Some(i) = remove {
        state.staged.remove(i);
    }

    let draft = &mut state.draft;
    let mut add: Option<Intervention> = None;
    ui.horizontal_wrapped(|ui| {
        ui.label("Add:");
        egui::ComboBox::from_id_salt("ab_draft_kind")
            .selected_text(draft.kind.label())
            .show_ui(ui, |ui| {
                for k in DraftKind::ALL {
                    ui.selectable_value(&mut draft.kind, k, k.label());
                }
            });
        match draft.kind {
            DraftKind::CloseUnit => {
                unit_combo(ui, h, &mut draft.unit, "ab_close_unit", |_| true);
                if let Some(unit) = draft.unit.clone() {
                    if ui.button("＋ stage").clicked() {
                        add = Some(Intervention::CloseUnit {
                            unit,
                            reason: "what-if closure".into(),
                        });
                    }
                }
            }
            DraftKind::ArrivalWave => {
                ui.add(
                    egui::DragValue::new(&mut draft.wave_count)
                        .range(1..=200)
                        .prefix("+"),
                )
                .on_hover_text("extra admissions");
                egui::ComboBox::from_id_salt("ab_wave_need")
                    .selected_text(draft.wave_need.label())
                    .show_ui(ui, |ui| {
                        for t in [UnitType::MedSurg, UnitType::Stepdown, UnitType::Icu] {
                            ui.selectable_value(&mut draft.wave_need, t, t.label());
                        }
                    });
                ui.label("over");
                ui.add(
                    egui::DragValue::new(&mut draft.wave_duration_hr)
                        .range(0.5..=48.0)
                        .suffix(" h"),
                );
                ui.label("from t+");
                ui.add(
                    egui::DragValue::new(&mut draft.wave_start_hr)
                        .range(0.0..=72.0)
                        .suffix(" h"),
                );
                if ui.button("＋ stage").clicked() {
                    add = Some(Intervention::ArrivalWave {
                        start_hr: draft.wave_start_hr,
                        duration_hr: draft.wave_duration_hr,
                        count: draft.wave_count,
                        need: draft.wave_need,
                    });
                }
            }
            DraftKind::FlexRatio => {
                unit_combo(ui, h, &mut draft.unit, "ab_flex_unit", |u| {
                    workload::is_rn_staffed(u.unit_type)
                });
                ui.label("to");
                ui.add(
                    egui::DragValue::new(&mut draft.ratio)
                        .range(1.0..=8.0)
                        .speed(0.1)
                        .suffix(" pts/RN"),
                );
                if let Some(unit) = draft.unit.clone() {
                    if ui.button("＋ stage").clicked() {
                        add = Some(Intervention::FlexRatio {
                            unit,
                            ratio: draft.ratio,
                        });
                    }
                }
            }
        }
    });
    if let Some(iv) = add {
        state.staged.push(iv);
    }
}

/// Unit picker for the draft row; `keep` filters (e.g. RN-staffed only).
fn unit_combo(
    ui: &mut egui::Ui,
    h: &Hospital,
    slot: &mut Option<UnitId>,
    salt: &str,
    keep: impl Fn(&hupsim_core::model::Unit) -> bool,
) {
    let units: Vec<&hupsim_core::model::Unit> = h.units.iter().filter(|u| keep(u)).collect();
    if slot.as_ref().is_none_or(|id| !units.iter().any(|u| &u.id == id)) {
        *slot = units.first().map(|u| u.id.clone());
    }
    let current = slot
        .as_ref()
        .and_then(|id| units.iter().find(|u| &u.id == id))
        .map(|u| u.name.clone())
        .unwrap_or_else(|| "—".into());
    egui::ComboBox::from_id_salt(salt)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for u in units {
                if ui
                    .selectable_label(slot.as_ref() == Some(&u.id), &u.name)
                    .clicked()
                {
                    *slot = Some(u.id.clone());
                }
            }
        });
}

fn legend_swatch(ui: &mut egui::Ui, color: Color32, label: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(14.0, 3.0), Sense::hover());
    ui.painter_at(r.expand2(Vec2::new(0.0, 4.0)))
        .line_segment([r.left_center(), r.right_center()], (2.0, color));
    ui.label(label);
}

/// The three overlaid charts (census, boarding, occupancy z) plus the
/// summary deltas the brief calls for (peak boarding, hours over 95%
/// occupancy, workload peaks).
fn charts_row(ui: &mut egui::Ui, a: &MetricSeries, b: &MetricSeries, cadence_min: f64) {
    let sa = summarize(a, cadence_min);
    let sb = summarize(b, cadence_min);
    let size = Vec2::new(220.0, 56.0);
    let dense = |v: &[f32]| -> Vec<Option<f32>> { v.iter().copied().map(Some).collect() };

    egui::ScrollArea::horizontal().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.weak("census");
                widgets::ab_sparkline(
                    ui,
                    size,
                    &AbSpec {
                        a: &dense(&a.census),
                        b: &dense(&b.census),
                        guide: None,
                        band: None,
                    },
                )
                .on_hover_text("group census over the run, baseline vs intervention");
                delta_line(ui, "peak", sa.peak_census, sb.peak_census, "");
            });
            ui.vertical(|ui| {
                ui.weak("boarding");
                widgets::ab_sparkline(
                    ui,
                    size,
                    &AbSpec {
                        a: &dense(&a.boarding),
                        b: &dense(&b.boarding),
                        guide: None,
                        band: None,
                    },
                )
                .on_hover_text("patients boarding (waiting for an appropriate bed)");
                delta_line(ui, "peak", sa.peak_boarding, sb.peak_boarding, "");
            });
            ui.vertical(|ui| {
                ui.weak("occupancy z");
                widgets::ab_sparkline(
                    ui,
                    size,
                    &AbSpec {
                        a: &a.occ_z,
                        b: &b.occ_z,
                        guide: Some(0.0),
                        band: Some((-widgets::Z_HOT, widgets::Z_HOT)),
                    },
                )
                .on_hover_text(
                    "occupancy vs the pre-experiment 24 h norm; shaded = |z| < 2. \
                     Gaps mean the norm window hadn't warmed at that sample.",
                );
                delta_line(ui, ">95% occ", sa.hours_over_95, sb.hours_over_95, " h");
            });
            ui.vertical(|ui| {
                ui.weak("workload");
                match (sa.peak_workload, sb.peak_workload) {
                    (Some(wa), Some(wb)) => {
                        delta_line(ui, "peak RN load", wa, wb, "");
                        ui.weak("per-RN composite peak\nacross the run");
                    }
                    _ => {
                        ui.weak("no staffed RNs in group");
                    }
                }
            });
        });
    });
}

/// "label: A vs B (Δ±x)" — the printed summary delta under each chart.
fn delta_line(ui: &mut egui::Ui, label: &str, a: f32, b: f32, unit: &str) {
    let d = b - a;
    ui.horizontal(|ui| {
        ui.weak(format!("{label}:"));
        ui.label(format!("{a:.1}{unit} vs {b:.1}{unit}"));
        let text = format!("Δ{d:+.1}{unit}");
        if d.abs() < 0.05 {
            ui.weak(text);
        } else {
            // Delta ink mirrors the z-badge convention: amber = the
            // intervention runs hotter, blue-gray = relief.
            let c = if d > 0.0 {
                widgets::COLOR_HOT_HIGH
            } else {
                widgets::COLOR_HOT_LOW
            };
            ui.colored_label(c, text);
        }
    });
}

/// Every group the run tracked, labeled for the picker: hospital, bedded
/// levels of care, service lines, buildings, then units.
fn group_options(h: &Hospital, ran: &RanExperiment) -> Vec<(SeriesKey, String)> {
    let rs_a = &ran.result.baseline.series;
    let rs_b = &ran.result.variant.series;
    let mut v = vec![(SeriesKey::Hospital, "Hospital".to_string())];
    for t in UnitType::ALL {
        let any = |s: &MetricSeries| s.census.iter().any(|&c| c > 0.0);
        let key = SeriesKey::UnitType(t);
        let has = rs_a.series(&key).is_some_and(any) || rs_b.series(&key).is_some_and(any);
        if has {
            v.push((key, format!("care: {}", t.label())));
        }
    }
    for id in rs_a.service_lines.keys() {
        let name = h
            .service_lines
            .iter()
            .find(|l| &l.id == id)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| id.to_string());
        v.push((SeriesKey::ServiceLine(id.clone()), format!("line: {name}")));
    }
    for id in rs_a.buildings.keys() {
        let name = h
            .buildings
            .iter()
            .find(|b| &b.id == id)
            .map(|b| b.short_name.clone())
            .unwrap_or_else(|| id.to_string());
        v.push((SeriesKey::Building(id.clone()), format!("bldg: {name}")));
    }
    for id in rs_a.units.keys() {
        let name = h
            .units
            .iter()
            .find(|u| &u.id == id)
            .map(|u| u.name.clone())
            .unwrap_or_else(|| id.to_string());
        v.push((SeriesKey::Unit(id.clone()), format!("unit: {name}")));
    }
    v
}
