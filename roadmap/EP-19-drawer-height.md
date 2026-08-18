# EP-19 — Dashboard drawer height papercut

**Size:** S · **Depends on:** EP-13 (drawer), EP-15 (compare tab shares the fix) ·
**Blocks:** nothing (sequence-flexible; slotted before EP-16 so the demo surface
is clean before more features land on it)

## Context

Found by the owner during the EP-15 manual eyeball pass (the same pass that
caught the `kpi_chip` grid panic, `44f845c`) — a latent EP-13 defect in the
hospital dashboard drawer:

1. The drawer cannot be dragged taller than the current tab's rows — a drag
   sticks for the drag itself, then snaps back to content height on release.
2. Switching tabs keeps the previous tab's (row-count-pinned) height, so a
   taller tab opens clipped: at worst rows are silently hidden behind a subtle
   scrollbar, at best the user must re-drag on every tab switch.

**Root cause (verified against the vendored egui 0.35 source — do the same
before changing the fix):** `Panel::show_inside_dyn`
(`egui-0.35.0/src/containers/panel.rs`, ~line 784 and the `PanelState` store
just after) persists the panel's height from the **frame's content rect**
(`shifted_outer_rect = inner_response.response.rect`), *not* from the dragged
size. The drawer's rows live in `egui::ScrollArea::vertical()` whose default
`auto_shrink` is `TRUE` on both axes (`scroll_area.rs:397`), so the content —
and therefore the stored panel height — is exactly as tall as the rows of
whichever tab drew last. The dragged height never survives a frame in which
content is shorter.

## In scope

1. **Fill the drawer, don't shrink to rows:** the rows `ScrollArea` in
   `dashboard.rs::draw_dashboard` and the compare tab's wrapping
   `ScrollArea::vertical()` (`DashTab::Compare` arm) get
   `.auto_shrink([false, false])`, so the drawer content always fills the
   panel's allotted height. The frame then reports the dragged size, the
   panel persists it, drag-to-grow sticks, and the height is stable across
   all four tabs.
2. **Sliver guard (judgment call):** consider a sensible `min_height` on the
   `hospital_dashboard` panel in `ui.rs` so the drawer can't be dragged to an
   unusably thin strip. Keep `default_size(280.0)`.
3. **Manual re-verification** (this is painting code — the untested class the
   `44f845c` lesson is about): open the drawer at a short height on
   Level of care, switch to Building and to ⚖ Compare, and confirm (a) no
   re-drag is needed, (b) the height holds across switches, (c) when rows do
   overflow the chosen height, a scrollbar appears and the pressure-sorted
   top rows stay visible, (d) drag-to-grow and drag-to-shrink both persist.
   The forced-open trick from the EP-15 session (`insert_resource` of an
   opened `DashboardState` + `RUST_BACKTRACE=1 cargo run` in background)
   reproduces drawer states without interaction.

## Out of scope

- Per-tab remembered heights; auto-sizing the drawer to its tallest tab
  (content-driven growth is exactly what caused this); any egui/bevy_egui
  version change; touching the EP-14 right-panel tabs (side panels size on
  the cross axis — not this bug).

## Verification / acceptance

- `cargo test --workspace` stays green (no headless surface exists for this;
  the manual checklist in scope item 3 IS the acceptance).
- The specific reported failure is gone: open drawer on a low-row tab,
  switch to a higher-row tab — nothing hidden, no re-drag required.
