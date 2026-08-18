# EP-12 — Unit dashboard (header strip)

**Size:** M · **Depends on:** EP-9, EP-10, EP-11 · **Blocks:** EP-13 (shares widget kit)

## Context

The unit-detail view (`crates/hupsim-app/src/unit_view.rs`) renders the room grid with
almost no unit-level context (name + service only). Owner-approved design: a **header
strip above the room grid** — the floor plan stays dominant — carrying
informatically/clinically minded indicators: capacity/over-capacity, stability,
statistical deviation from the trailing-24h norm (EP-9), and RN workload (EP-11).
Charts are **hand-painted egui** (owner decision — no egui_plot; the app already
hand-paints its sparkline and legend in `ui.rs`). Load the `dataviz` skill before
building the widgets — color/encoding discipline matters here, and the app already has
an Oklab stability spectrum (`hupsim-core/src/color.rs`) the dashboard must not fight.

## In scope

1. **Widget kit** (new `crates/hupsim-app/src/widgets.rs`, reused by EP-13): KPI chip
   (label + value + optional Δ/z badge), sparkline-with-band (line over the ring buffer,
   mean±kσ band shaded, current point emphasized), mini histogram (stability tiers),
   workload bar (segmented by the EP-11 component breakdown, over/under-ratio marker).
   All read from the EP-9 cache/ring buffers — **no per-frame room scans**.
2. **Header strip** in `draw_unit_view`: row 1 — unit name/service line, census/capacity
   with occupancy % and z-vs-24h badge, over-capacity state (census > staffed capacity,
   or boarding pressure feeding this unit), boarding count, OOS count. Row 2 — stability:
   tier histogram + mean/worst chips (colors from `stability_color`), census sparkline
   with norm band (per-unit series, not the hospital one). Row 3 — RN workload bars
   (RN #1…#n), each with breakdown on hover, over-ratio flagged. Strip collapses
   gracefully for bedless/research units (hide staffing row, keep census).
3. **Per-room RN tags**: small "N1"/"N2" tag in each occupied room cell (the grid loop
   already draws number + alias + boarding dot + critical outline); tag color-keyed per
   RN with a muted categorical set that does not collide with the stability fill
   (dataviz skill: categorical-on-sequential discipline). Hovering an RN's workload bar
   highlights that RN's rooms in the grid.
4. **Warm-up honesty**: before the 24h window fills, z badges render as "—" with a
   tooltip ("norm warming up: 9/96 samples"), never fake zeros (EP-9 returns `None`).
5. **Tests**: what's testable headlessly — the strip's data assembly (a pure function
   from cache+roster → strip view-model) gets unit tests; painting stays thin.

## Out of scope

- Hospital-wide rollups (EP-13). Query/staging (EP-14). New simulation behavior.
- egui_plot or any charting dependency.

## Verification / acceptance

- `cargo test --workspace` green; view-model tests cover occupied/empty/warm-up states.
- Run with EP-10 sim on: open an ICU and a med-surg unit — the strip reads correctly at
  a glance (census, z badge coloring only when |z| ≥ threshold, workload bars balanced
  per EP-11), hover-highlighting works, and the room grid remains the visual center.
- A unit at ratio +1 patient shows the over-ratio marker; forcing rooms OOS moves
  staffed capacity and occupancy % correctly.
