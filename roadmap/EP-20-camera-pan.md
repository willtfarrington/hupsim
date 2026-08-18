# EP-20 — Camera pan discoverability & bindings

**Size:** S · **Depends on:** — (touches only `orbit.rs` + one UI hint) ·
**Blocks:** nothing (cosmetic/UX; sequence-flexible)

## Context

During EP-15 QA the owner asked for a linear side-to-side shift as a
counterpart to orbit, with the pan recentering the orbit pivot so rotation
keeps working. **The mechanic already exists with exactly those semantics**
(`orbit.rs:47-53`): middle-mouse drag pans `OrbitCamera.focus` in the camera
plane, and `focus` *is* the orbit pivot, so post-pan rotation circles the new
center. The finding is therefore not a missing feature but two papercuts:

1. **Discoverability:** the owner — the person who commissioned the app —
   could not find it. No surface anywhere lists the camera controls
   (orbit: left/right-drag · pan: middle-drag · zoom: wheel).
2. **Binding accessibility:** middle-drag is awkward or unavailable on
   trackpads and buttonless mice, so for some setups the feature may as well
   not exist.

## In scope

1. **Alias binding:** Shift + left-drag (and Shift + right-drag, since both
   buttons orbit today) pans, reusing the middle-drag math verbatim — one
   shared code path, not a copy. Modifier check must not fight egui: skip
   when `egui_wants.wants_any_pointer_input()`, same as the existing guard.
2. **Controls hint:** a one-line, always-available reminder of the camera
   scheme — e.g. weak text or a hoverable "🖱" chip in the status bar or
   toolbar ("orbit drag · pan ⇧drag / middle · zoom wheel"). Placement is a
   judgment call; keep it a single quiet line, no modal tutorial, and show it
   only in the Campus3D view (the 2D unit view has no camera).
3. **Judgment note while in the file:** the current pan moves `focus` along
   camera right/up, so near-horizon pitches let vertical drags lift the pivot
   off the ground plane. Consider projecting the pan onto the ground (XZ)
   for map-like feel — but verify near-top-down behavior first (`tf.up()` is
   nearly horizontal there, which is why the current scheme mostly feels
   fine). If it isn't a clear improvement, leave the math alone and note why.

## Out of scope

- Input remapping/config, touch gestures, camera inertia or animation,
  persisting camera state in scenarios, keyboard-driven camera movement
  (revisit only if the owner asks).

## Verification / acceptance

- Manual (interactive camera — no headless surface): on a trackpad-only
  setup, Shift+drag pans; orbiting afterwards circles the panned-to center;
  the hint is visible in Campus3D, absent in the unit view, and its bindings
  match reality. `cargo test --workspace` stays green.
