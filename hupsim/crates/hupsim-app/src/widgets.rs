//! EP-12 widget kit: hand-painted egui dashboard primitives (no egui_plot —
//! owner decision), shared by the unit header strip and the EP-13 hospital
//! dashboard. Widgets paint exactly what they are given; all data assembly
//! (and the warm-up/None discipline that comes with it) happens in the
//! callers' view-models, so painting stays thin and the assembly testable.

use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind,
    Vec2,
};
use hupsim_core::color::stability_color;

/// sRGB `Rgb` (0..1) → egui color. Shared by every panel that paints the
/// stability spectrum.
pub fn rgb32(c: hupsim_core::color::Rgb) -> Color32 {
    Color32::from_rgb(
        (c.r * 255.0) as u8,
        (c.g * 255.0) as u8,
        (c.b * 255.0) as u8,
    )
}

/// |z| at/above which a deviation badge colors; below it stays neutral ink
/// ("z badge coloring only when |z| ≥ threshold").
pub const Z_HOT: f32 = 2.0;

/// Sparkline norm bands shade mean ± `BAND_K`·σ — matched to [`Z_HOT`] so a
/// point escaping the shaded band is exactly a hot badge.
pub const BAND_K: f32 = 2.0;

/// Amber for high-side deviation and pressure states (the app's existing ⚠
/// color), blue-gray for low-side deviation (the app's "pinned"/Reported
/// blue). Status colors stay reserved: orange = boarding, red = critical.
pub const COLOR_HOT_HIGH: Color32 = Color32::from_rgb(230, 150, 60);
pub const COLOR_HOT_LOW: Color32 = Color32::from_rgb(110, 150, 200);

/// Muted categorical palette for RN identity tags — categorical marks sitting
/// ON the sequential stability fill, so deliberately lower-chroma than the
/// spectrum anchors and paired with an "N#" text label everywhere (identity
/// is never color-alone). Fixed slot order, never cycled; RN #9+ folds to
/// [`RN_OVERFLOW`], where the label alone carries identity.
///
/// Validated with the dataviz six-checks validator against the egui dark
/// surface (#1b1b1b), adjacent pairlist — RN room runs are contiguous along
/// the corridor and workload rows stack in index order, so real adjacency is
/// RN k vs k±1. All checks pass: lightness band L 0.48–0.67, chroma ≥ 0.10,
/// worst adjacent CVD ΔE 13.0, worst adjacent normal-vision ΔE 17.3,
/// contrast vs surface ≥ 3:1.
pub const RN_COLORS: [Color32; 8] = [
    Color32::from_rgb(0x00, 0x91, 0x7f), // teal
    Color32::from_rgb(0x8b, 0x78, 0xd8), // violet
    Color32::from_rgb(0x7d, 0x7c, 0x26), // olive
    Color32::from_rgb(0xc7, 0x75, 0xa2), // pink
    Color32::from_rgb(0x4a, 0x5f, 0xc0), // steel
    Color32::from_rgb(0xb8, 0x88, 0x3f), // tan
    Color32::from_rgb(0xa3, 0x4b, 0x88), // plum
    Color32::from_rgb(0xc9, 0x7a, 0x58), // clay
];

/// Neutral tag fill for RN #9 and beyond (label-only identity).
pub const RN_OVERFLOW: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x8a);

/// Tag color for a 0-based RN index.
pub fn rn_color(rn: u16) -> Color32 {
    RN_COLORS
        .get(rn as usize)
        .copied()
        .unwrap_or(RN_OVERFLOW)
}

/// Black or white ink for text sitting on `fill` (tags span L 0.48–0.67, so
/// neither works everywhere).
pub fn ink_on(fill: Color32) -> Color32 {
    let luma = 0.2126 * fill.r() as f32 + 0.7152 * fill.g() as f32 + 0.0722 * fill.b() as f32;
    if luma > 140.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// Representative color of each stability tier: the spectrum sampled at the
/// tier's instability midpoint, so the histogram cannot fight the room fills.
pub fn tier_color(tier: usize) -> Color32 {
    const MIDPOINTS: [f32; 4] = [0.12, 0.37, 0.65, 0.9];
    rgb32(stability_color(MIDPOINTS[tier.min(3)]))
}

pub const TIER_LABELS: [&str; 4] = ["Stable", "Watcher", "Unstable", "Critical"];

/// Deviation-from-norm badge state. Encodes the EP-9 warm-up honesty rule:
/// before the trailing window fills there is no z — never a fake zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZBadge {
    /// Window not yet full; `have`/`need` feed the warm-up tooltip.
    Warming { have: usize, need: usize },
    /// Window full but flat (σ ≈ 0): "how many sigmas" is meaningless.
    Flat,
    Z(f32),
}

impl ZBadge {
    pub fn text(&self) -> String {
        match self {
            ZBadge::Warming { .. } | ZBadge::Flat => "z —".to_string(),
            ZBadge::Z(z) => format!("z {z:+.1}"),
        }
    }

    /// Badge ink: colored only when |z| ≥ [`Z_HOT`], `None` = neutral.
    pub fn color(&self) -> Option<Color32> {
        match self {
            ZBadge::Z(z) if *z >= Z_HOT => Some(COLOR_HOT_HIGH),
            ZBadge::Z(z) if *z <= -Z_HOT => Some(COLOR_HOT_LOW),
            _ => None,
        }
    }

    pub fn hover_text(&self) -> String {
        match self {
            ZBadge::Warming { have, need } => {
                format!("norm warming up: {have}/{need} samples")
            }
            ZBadge::Flat => "trailing 24 h is flat (σ ≈ 0) — no meaningful z".to_string(),
            ZBadge::Z(_) => "standard score vs the trailing-24 h norm".to_string(),
        }
    }
}

/// KPI chip: weak label + strong value + optional badge, in a subtle rounded
/// frame. The badge renders in its color when given one, neutral weak ink
/// otherwise. Returns the chip's response for caller hover text.
pub fn kpi_chip(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    badge: Option<&ZBadge>,
) -> Response {
    let resp = egui::Frame::new()
        .fill(Color32::from_gray(36))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                if !label.is_empty() {
                    ui.weak(label);
                }
                ui.strong(value);
                if let Some(b) = badge {
                    let text = b.text();
                    let r = match b.color() {
                        Some(c) => ui.colored_label(c, text),
                        None => ui.weak(text),
                    };
                    r.on_hover_text(b.hover_text());
                }
            });
        })
        .response;
    // No trailing add_space: the chip must stay layout-agnostic — egui
    // asserts on add_space inside a Grid, and the EP-13 drawer rows are one.
    resp
}

/// Forecast-fan ink (EP-17): deliberately the SAME clay as
/// [`COLOR_AB_VARIANT`] — the kit's established "modeled, not observed" hue,
/// already validated as a pair against the chart blue (deutan ΔE 17.9, both
/// ≥ 3:1 on the gray-28 surface). The two uses can never co-occur on one
/// chart: the fan projects the live world only and the compare tab never
/// shows a fan (EP-17 scope fence), so clay unambiguously means "projection"
/// here and "intervention arm" there. Identity never rides on color alone —
/// the fan sits right of a "now" divider, median dashed, and every fan chart
/// carries a caption or hover legend.
pub const COLOR_FORECAST: Color32 = Color32::from_rgb(0xc9, 0x7a, 0x58);

/// Fan band fill: the forecast clay at low alpha, hue-distinct from the blue
/// norm band (which is clipped to the history side of the divider).
pub const COLOR_FORECAST_BAND: Color32 =
    Color32::from_rgba_unmultiplied_const(0xc9, 0x7a, 0x58, 56);

/// Data for the projection fan appended to a sparkline (EP-17): quantile
/// curves on the same y scale, index 0 = "now" (shared with the live point),
/// step k = one forecast step to the right of the history.
pub struct FanSpec<'a> {
    pub median: &'a [f32],
    pub lo: &'a [f32],
    pub hi: &'a [f32],
}

/// Data for [`sparkline_band`]: an ordered series plus its trailing norm.
pub struct SparkSpec<'a> {
    /// Oldest → newest samples (a ring buffer's `iter_ordered`).
    pub samples: &'a [f32],
    /// Shaded norm band (mean ± kσ); `None` while the window warms up.
    pub band: Option<(f32, f32)>,
    /// Norm mean, drawn as a dashed midline when present.
    pub mean: Option<f32>,
    /// Live value, emphasized as the final point (it may sit past the last
    /// recorded sample).
    pub current: Option<f32>,
    /// Forecast fan appended past the live point (EP-17). Requires
    /// `current`; drawn in [`COLOR_FORECAST`] beyond a vertical "now"
    /// divider, with the norm band clipped to the history side.
    pub fan: Option<FanSpec<'a>>,
}

/// Sparkline with a shaded mean±kσ band and an optional projection fan.
/// Draws a quiet placeholder until it has two samples to connect.
pub fn sparkline_band(ui: &mut egui::Ui, size: Vec2, spec: &SparkSpec) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(3), Color32::from_gray(28));

    // The fan shares its first point with the live point; only its steps
    // beyond "now" occupy new x slots.
    let fan_steps = match (&spec.fan, spec.current) {
        (Some(f), Some(_)) => f.median.len().saturating_sub(1),
        _ => 0,
    };

    // With a fan present the chart is drawable from the very first frame
    // (a paused fresh start has projection but no history yet); without
    // one, two samples are needed to connect a line.
    if spec.samples.len() < 2 && fan_steps == 0 {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "collecting samples…",
            FontId::proportional(10.0),
            Color32::from_gray(110),
        );
        return resp;
    }

    // Vertical scale spans samples ∪ band ∪ current ∪ fan, padded so the
    // line never kisses the frame.
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for &v in spec.samples {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if let Some((b_lo, b_hi)) = spec.band {
        lo = lo.min(b_lo);
        hi = hi.max(b_hi);
    }
    if let Some(c) = spec.current {
        lo = lo.min(c);
        hi = hi.max(c);
    }
    if fan_steps > 0 {
        let f = spec.fan.as_ref().unwrap();
        for &v in f.lo.iter().chain(f.hi.iter()) {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    let pad = ((hi - lo) * 0.08).max(0.5);
    let (lo, hi) = (lo - pad, hi + pad);
    let inner = rect.shrink(2.0);
    let y_of = |v: f32| inner.max.y - inner.height() * (v - lo) / (hi - lo);

    // The current point extends the series by one slot on the x axis; the
    // fan extends past it.
    let n_hist = spec.samples.len() + usize::from(spec.current.is_some());
    let n = n_hist + fan_steps;
    let x_of = |i: usize| inner.min.x + inner.width() * i as f32 / (n - 1).max(1) as f32;
    let x_now = x_of(n_hist - 1);

    if let Some((b_lo, b_hi)) = spec.band {
        // Norm band and mean stop at "now": the trailing norm makes no claim
        // about the projection region.
        let band_rect = Rect::from_min_max(
            Pos2::new(inner.min.x, y_of(b_hi)),
            Pos2::new(x_now, y_of(b_lo)),
        );
        painter.rect_filled(
            band_rect.intersect(inner),
            CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(110, 170, 220, 26),
        );
    }
    if let Some(mean) = spec.mean {
        let y = y_of(mean);
        painter.add(egui::Shape::dashed_line(
            &[Pos2::new(inner.min.x, y), Pos2::new(x_now, y)],
            Stroke::new(1.0, Color32::from_gray(105)),
            3.0,
            3.0,
        ));
    }

    let pts: Vec<Pos2> = spec
        .samples
        .iter()
        .enumerate()
        .map(|(i, &v)| Pos2::new(x_of(i), y_of(v)))
        .collect();
    painter.add(egui::Shape::line(
        pts,
        Stroke::new(1.5, Color32::from_rgb(110, 170, 220)),
    ));

    if fan_steps > 0 {
        let f = spec.fan.as_ref().unwrap();
        // "Now" divider: the seam between observed and projected.
        painter.line_segment(
            [
                Pos2::new(x_now, inner.min.y),
                Pos2::new(x_now, inner.max.y),
            ],
            Stroke::new(1.0, Color32::from_gray(90)),
        );
        // Quantile wedge as a triangle strip (a lo/hi path is generally
        // non-convex, so a mesh, not a polygon).
        let mut mesh = egui::Mesh::default();
        for k in 0..f.median.len() {
            let x = x_of(n_hist - 1 + k);
            mesh.colored_vertex(Pos2::new(x, y_of(f.hi[k])), COLOR_FORECAST_BAND);
            mesh.colored_vertex(Pos2::new(x, y_of(f.lo[k])), COLOR_FORECAST_BAND);
        }
        for k in 0..f.median.len().saturating_sub(1) {
            let i = (2 * k) as u32;
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i + 1, i + 3, i + 2);
        }
        painter.add(egui::Shape::mesh(mesh));
        // Dashed median — projected, never confusable with the solid
        // observed line.
        let median_pts: Vec<Pos2> = f
            .median
            .iter()
            .enumerate()
            .map(|(k, &v)| Pos2::new(x_of(n_hist - 1 + k), y_of(v)))
            .collect();
        painter.add(egui::Shape::dashed_line(
            &median_pts,
            Stroke::new(1.5, COLOR_FORECAST),
            3.0,
            2.0,
        ));
    }

    if let Some(c) = spec.current {
        let p = Pos2::new(x_now, y_of(c));
        if let Some(&last) = spec.samples.last() {
            painter.line_segment(
                [Pos2::new(x_of(n_hist - 2), y_of(last)), p],
                Stroke::new(1.5, Color32::from_rgb(110, 170, 220)),
            );
        }
        painter.circle_filled(p, 2.5, Color32::from_rgb(210, 235, 255));
    }
    resp
}

/// A/B compare line colors (EP-15). Baseline keeps the kit's chart blue —
/// it is the same census line every other tab draws — and the intervention
/// line is a clay orange. Two-line categorical overlay validated with the
/// dataviz six-checks validator against the egui dark surface (#1b1b1b):
/// deutan ΔE 17.9, tritan 25.1, normal-vision ΔE 21.1, both ≥ 3:1 contrast.
/// The clay sits in the RN-tag lightness band; identity never rides on color
/// alone — every compare chart is paired with a printed legend. Deliberately
/// NOT the status orange (boarding), hot amber (z), or query violet.
pub const COLOR_AB_BASELINE: Color32 = Color32::from_rgb(110, 170, 220);
pub const COLOR_AB_VARIANT: Color32 = Color32::from_rgb(0xc9, 0x7a, 0x58);

/// Data for [`ab_sparkline`]: two index-aligned series (baseline A,
/// intervention B), oldest → newest. `None` samples break the line — the
/// EP-9 honesty rule for z-lanes that haven't warmed — never plot as zero.
pub struct AbSpec<'a> {
    pub a: &'a [Option<f32>],
    pub b: &'a [Option<f32>],
    /// Dashed horizontal reference (e.g. z = 0).
    pub guide: Option<f32>,
    /// Shaded horizontal band (e.g. the |z| < 2 quiet zone).
    pub band: Option<(f32, f32)>,
}

/// Overlaid A/B sparkline (EP-15 compare view): baseline in
/// [`COLOR_AB_BASELINE`], intervention on top in [`COLOR_AB_VARIANT`].
/// Identity is carried by the caller's legend, never color alone.
pub fn ab_sparkline(ui: &mut egui::Ui, size: Vec2, spec: &AbSpec) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(3), Color32::from_gray(28));

    let points = |s: &[Option<f32>]| s.iter().flatten().count();
    if points(spec.a) < 2 && points(spec.b) < 2 {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "no data",
            FontId::proportional(10.0),
            Color32::from_gray(110),
        );
        return resp;
    }

    // Vertical scale spans both series ∪ band ∪ guide, padded.
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for &v in spec.a.iter().chain(spec.b).flatten() {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if let Some((b_lo, b_hi)) = spec.band {
        lo = lo.min(b_lo);
        hi = hi.max(b_hi);
    }
    if let Some(g) = spec.guide {
        lo = lo.min(g);
        hi = hi.max(g);
    }
    let pad = ((hi - lo) * 0.08).max(0.5);
    let (lo, hi) = (lo - pad, hi + pad);
    let inner = rect.shrink(2.0);
    let y_of = |v: f32| inner.max.y - inner.height() * (v - lo) / (hi - lo);
    let n = spec.a.len().max(spec.b.len());
    let x_of = |i: usize| inner.min.x + inner.width() * i as f32 / (n - 1).max(1) as f32;

    if let Some((b_lo, b_hi)) = spec.band {
        let band_rect = Rect::from_min_max(
            Pos2::new(inner.min.x, y_of(b_hi)),
            Pos2::new(inner.max.x, y_of(b_lo)),
        );
        painter.rect_filled(
            band_rect.intersect(inner),
            CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(110, 170, 220, 18),
        );
    }
    if let Some(g) = spec.guide {
        let y = y_of(g);
        painter.add(egui::Shape::dashed_line(
            &[Pos2::new(inner.min.x, y), Pos2::new(inner.max.x, y)],
            Stroke::new(1.0, Color32::from_gray(105)),
            3.0,
            3.0,
        ));
    }

    // Polyline per series, broken at None samples (a gap, never a fake zero).
    let draw_series = |s: &[Option<f32>], color: Color32| {
        let mut run: Vec<Pos2> = Vec::new();
        for (i, v) in s.iter().enumerate() {
            match v {
                Some(v) => run.push(Pos2::new(x_of(i), y_of(*v))),
                None => {
                    if run.len() > 1 {
                        painter.add(egui::Shape::line(
                            std::mem::take(&mut run),
                            Stroke::new(1.5, color),
                        ));
                    } else {
                        run.clear();
                    }
                }
            }
        }
        if run.len() > 1 {
            painter.add(egui::Shape::line(run, Stroke::new(1.5, color)));
        }
    };
    draw_series(spec.a, COLOR_AB_BASELINE);
    draw_series(spec.b, COLOR_AB_VARIANT);
    resp
}

/// Mini histogram of the four stability tiers, colored from the spectrum's
/// own tier midpoints. Hover lists exact counts (colored swatch + label per
/// tier — identity never rides on the bar color alone).
pub fn tier_histogram(ui: &mut egui::Ui, size: Vec2, counts: &[u32; 4]) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(3), Color32::from_gray(28));

    let max = counts.iter().copied().max().unwrap_or(0).max(1) as f32;
    let inner = rect.shrink(3.0);
    let gap = 2.0;
    let bar_w = (inner.width() - gap * 3.0) / 4.0;
    for (t, &c) in counts.iter().enumerate() {
        let x = inner.min.x + t as f32 * (bar_w + gap);
        // Zero-count tiers keep a 1.5 px baseline stub so the slot reads as
        // present-but-empty rather than missing.
        let h = if c == 0 {
            1.5
        } else {
            (inner.height() * c as f32 / max).max(2.5)
        };
        let bar = Rect::from_min_size(
            Pos2::new(x, inner.max.y - h),
            Vec2::new(bar_w, h),
        );
        let color = if c == 0 {
            Color32::from_gray(70)
        } else {
            tier_color(t)
        };
        painter.rect_filled(bar, CornerRadius::same(1), color);
    }

    resp.on_hover_ui(|ui| {
        ui.strong("Stability mix");
        for (t, &c) in counts.iter().enumerate() {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::new(10.0, 10.0), Sense::hover());
                ui.painter_at(r)
                    .rect_filled(r, CornerRadius::same(2), tier_color(t));
                ui.label(format!("{} {c}", TIER_LABELS[t]));
            });
        }
    })
}

/// Fixed component order and fills of the workload bar: four lightness steps
/// of the app's chart blue (ordinal steps of one hue — component identity is
/// carried by order + the hover legend, not by competing hues).
pub const WORKLOAD_SEGMENTS: [(&str, Color32); 4] = [
    ("census vs ratio", Color32::from_rgb(52, 88, 120)),
    ("acuity", Color32::from_rgb(82, 130, 172)),
    ("iso/tele overhead", Color32::from_rgb(116, 168, 212)),
    ("churn", Color32::from_rgb(158, 202, 238)),
];

/// Fixed component order and fills of the placement score bar (EP-14):
/// four lightness steps of one teal ramp — deliberately a different hue from
/// the workload blue so a score bar can never be misread as a load bar when
/// both are on screen. Same EP-12 discipline: an ordinal ramp of one hue,
/// component identity carried by order + the hover legend, never color
/// alone. Ramp is lightness-monotone with adjacent-step OKLab ΔE ≥ 12
/// (better than the kit blue's 10.6); the darkest step sits on the gray-28
/// track and every bar is paired with a printed total + hover legend (the
/// contrast relief the six-checks validator requires of the deep step).
pub const SCORE_SEGMENTS: [(&str, Color32); 4] = [
    ("service match", Color32::from_rgb(0x26, 0x63, 0x56)),
    ("occupancy headroom", Color32::from_rgb(0x3d, 0x8a, 0x77)),
    ("RN headroom", Color32::from_rgb(0x5c, 0xb2, 0x9a)),
    ("telemetry fit", Color32::from_rgb(0x8e, 0xd9, 0xc0)),
];

/// Highlight ink for placement-query results (unit-view cell outlines, the
/// panel's accents) — violet, reserved so it can't collide with the warm
/// selection yellow, the drill teal, or the status colors.
pub const COLOR_QUERY: Color32 = Color32::from_rgb(196, 134, 244);

/// Shared painter for a segmented composition bar: ordered components on a
/// common scale, an optional vertical reference tick. Both composition bars
/// (workload, placement score) route through here so their anatomy stays
/// identical.
fn segment_bar(
    ui: &mut egui::Ui,
    size: Vec2,
    components: &[f32],
    segments: &[(&str, Color32)],
    scale_max: f32,
    tick: Option<f32>,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(3), Color32::from_gray(28));

    let inner = rect.shrink2(Vec2::new(2.0, 2.0));
    let scale = scale_max.max(1e-3);
    let gap = 1.5;
    let mut x = inner.min.x;
    for (i, &v) in components.iter().enumerate() {
        let w = (inner.width() * v / scale).max(0.0);
        if w > 0.5 {
            let seg = Rect::from_min_max(
                Pos2::new(x, inner.min.y),
                Pos2::new((x + w - gap).min(inner.max.x), inner.max.y),
            );
            if seg.width() > 0.0 {
                painter.rect_filled(seg, CornerRadius::same(1), segments[i].1);
            }
        }
        x += w;
    }

    if let Some(mark) = tick {
        let tick_x = inner.min.x + inner.width() * (mark / scale).clamp(0.0, 1.0);
        painter.line_segment(
            [
                Pos2::new(tick_x, rect.min.y),
                Pos2::new(tick_x, rect.max.y),
            ],
            Stroke::new(1.0, Color32::from_gray(130)),
        );
    }
    resp
}

/// One RN's workload bar: the four components as ordered segments on a
/// common scale, with a reference tick at the at-ratio census load. The
/// caller supplies the components already ordered like
/// [`WORKLOAD_SEGMENTS`] (i.e. a `WorkloadBreakdown` flattened).
pub fn workload_bar(
    ui: &mut egui::Ui,
    size: Vec2,
    components: [f32; 4],
    scale_max: f32,
    at_ratio_mark: f32,
) -> Response {
    segment_bar(
        ui,
        size,
        &components,
        &WORKLOAD_SEGMENTS,
        scale_max,
        Some(at_ratio_mark),
    )
}

/// One placement result's score bar (EP-14): the four criteria as ordered
/// segments, on a common scale across the result list (the query's best
/// attainable score), so bar length ranks candidates at a glance. Components
/// ordered like [`SCORE_SEGMENTS`] (a `ScoreBreakdown` flattened).
pub fn score_bar(ui: &mut egui::Ui, size: Vec2, components: [f32; 4], scale_max: f32) -> Response {
    segment_bar(ui, size, &components, &SCORE_SEGMENTS, scale_max, None)
}

/// Hover legend shared by the composition bars: headline total, then a
/// swatch + label + value per component (identity never rides on color
/// alone).
fn composition_tooltip(
    ui: &mut egui::Ui,
    headline: &str,
    segments: &[(&str, Color32)],
    components: &[f32],
) {
    ui.strong(format!("{headline} {:.2}", components.iter().sum::<f32>()));
    for ((label, color), v) in segments.iter().zip(components) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::new(10.0, 10.0), Sense::hover());
            ui.painter_at(r).rect_filled(r, CornerRadius::same(2), *color);
            ui.label(format!("{label} {v:.2}"));
        });
    }
}

/// Hover legend for a workload bar. Colors mirror [`WORKLOAD_SEGMENTS`].
pub fn workload_tooltip(ui: &mut egui::Ui, components: [f32; 4]) {
    composition_tooltip(ui, "load", &WORKLOAD_SEGMENTS, &components);
}

/// Hover legend for a placement score bar. Colors mirror [`SCORE_SEGMENTS`].
pub fn score_tooltip(ui: &mut egui::Ui, components: [f32; 4]) {
    composition_tooltip(ui, "score", &SCORE_SEGMENTS, &components);
}

/// Small identity tag ("N1") painted into a room cell or beside a workload
/// bar. `emphasized` brightens the outline (the hover-highlight pairing).
pub fn rn_tag(painter: &egui::Painter, rect: Rect, rn: u16, emphasized: bool) {
    let fill = rn_color(rn);
    painter.rect_filled(rect, CornerRadius::same(2), fill);
    let stroke = if emphasized {
        Stroke::new(1.5, Color32::WHITE)
    } else {
        Stroke::new(1.0, Color32::from_black_alpha(160))
    };
    painter.rect_stroke(rect, CornerRadius::same(2), stroke, StrokeKind::Inside);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        format!("N{}", rn + 1),
        FontId::monospace((rect.height() * 0.82).clamp(7.0, 10.0)),
        ink_on(fill),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z_badge_colors_only_at_threshold() {
        assert_eq!(ZBadge::Z(1.9).color(), None);
        assert_eq!(ZBadge::Z(2.0).color(), Some(COLOR_HOT_HIGH));
        assert_eq!(ZBadge::Z(-2.4).color(), Some(COLOR_HOT_LOW));
        assert_eq!(ZBadge::Flat.color(), None);
        assert_eq!(ZBadge::Warming { have: 3, need: 96 }.color(), None);
    }

    #[test]
    fn z_badge_never_fakes_a_number_while_warming() {
        let b = ZBadge::Warming { have: 9, need: 96 };
        assert_eq!(b.text(), "z —");
        assert!(b.hover_text().contains("9/96"));
        assert_eq!(ZBadge::Flat.text(), "z —");
        assert_eq!(ZBadge::Z(2.34).text(), "z +2.3");
    }

    #[test]
    fn rn_palette_is_fixed_order_with_neutral_overflow() {
        assert_eq!(rn_color(0), RN_COLORS[0]);
        assert_eq!(rn_color(7), RN_COLORS[7]);
        // Never cycled: #9 and beyond fold to the neutral tag.
        assert_eq!(rn_color(8), RN_OVERFLOW);
        assert_eq!(rn_color(200), RN_OVERFLOW);
    }

    #[test]
    fn tag_ink_flips_with_fill_lightness() {
        assert_eq!(ink_on(Color32::from_rgb(0x4a, 0x5f, 0xc0)), Color32::WHITE);
        assert_eq!(ink_on(Color32::from_rgb(0xc9, 0xa8, 0x76)), Color32::BLACK);
    }
}
