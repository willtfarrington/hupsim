# EP-22 — 3D label overlay papercut

**Size:** S · **Depends on:** nothing (the labels are EP-4-era renderer
furniture; EP-19's taller drawer just made the bleed obvious) ·
**Blocks:** nothing (sequence-flexible papercut; slotted with EP-20 so the
small demo-surface cleanups land together)

## Context

Observed during the EP-19 verification pass (`0b2f3c2` screenshots), filed
verbatim from that session's report:

> 3D building billboard labels (e.g. Clifton, PCAM) paint over the drawer's
> empty region — a pre-existing z-order quirk that a taller drawer makes
> more visible.

The labels are not part of the 3D render: `ui.rs::draw_3d_labels` projects
each `LabelAnchor` (spawned in `campus3d.rs`, one per building + floor)
through `Camera::world_to_viewport` and paints an `egui::Area` at the
resulting screen position. The projection uses the camera's full-window
viewport, so a building sitting low in the frame projects to coordinates
under the dashboard drawer — and the label paints there anyway, floating on
top of the drawer (or of any panel: the same bleed can hit the navigator,
inspector, and bars whenever the camera puts a labeled node behind them).

**Root cause (verified against the vendored egui 0.35 source — do the same
before changing the fix):** `egui::Area` defaults to `Order::Middle`
(`egui-0.35.0/src/containers/area.rs:143`) while panels paint on the
background layer, which is "painted behind all floating windows"
(`layers.rs:10-12`). Layer order beats call order, so the labels always
win over the panels no matter where in `draw_ui` they are drawn.

## In scope

1. **Confine labels to the 3D viewport:** in `draw_3d_labels`, cull or clip
   each label against `ctx.available_rect()` — called after all panels have
   been shown (it is today: the `ViewMode::Campus3D` match arm runs after
   the drawer/left/right/bars), that rect IS the central 3D viewport.
   Either flavor is acceptable; pick one and say why in the commit:
   - *cull*: skip the `Area` when the projected point is outside the rect
     (one line, labels pop out whole);
   - *clip*: `ui.set_clip_rect(...)` inside the `Area` so a label straddling
     a panel edge slides under it smoothly (nicer at the drawer's top edge,
     where the bleed was reported).
2. **Manual re-verification** (painting code — the `44f845c` untested
   class): reuse the EP-19 harness verbatim — forced-open drawer
   (`insert_resource` of an opened `DashboardState`), temp
   `default_size(420.0)`, background `cargo run`, DPI-aware window
   screenshots (`SetProcessDPIAware` — the EP-19 session's `capture.ps1`
   pattern). Confirm (a) no label paints inside the drawer, navigator,
   inspector, or bars; (b) labels still show over the 3D scene and clip or
   cull cleanly at panel edges as the camera orbits; (c) the floor
   tooltips still render (they are pointer-anchored `Area`s and should be
   untouched).

## Out of scope

- Reworking labels as in-scene 3D text or a bevy UI pass; label styling,
  sizing, or density changes; occlusion of labels by buildings (a label
  whose anchor is behind another building still draws — separate, older
  quirk); the pointer-anchored hover/connection tooltips (they follow the
  cursor over the viewport and may legitimately overhang a panel edge);
  any egui/bevy_egui version change.

## Verification / acceptance

- `cargo test --workspace` stays green (no headless surface for this; the
  manual checklist in scope item 2 IS the acceptance).
- The specific reported artifact is gone: drawer open at any height, no
  building/floor label visible inside it.
