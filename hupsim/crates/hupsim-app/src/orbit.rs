//! Orbit / pan / zoom camera for the 3D campus view.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;

#[derive(Component)]
pub struct OrbitCamera {
    pub focus: Vec3,
    pub radius: f32,
    pub yaw: f32,
    pub pitch: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            focus: Vec3::new(-110.0, 0.0, 20.0),
            radius: 560.0,
            yaw: 0.35,
            pitch: 1.05,
        }
    }
}

pub fn orbit_camera(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    egui_wants: Res<EguiWantsInput>,
    mut q: Query<(&mut OrbitCamera, &mut Transform)>,
) {
    if egui_wants.wants_any_pointer_input() {
        return;
    }
    let Ok((mut orbit, mut tf)) = q.single_mut() else {
        return;
    };

    // Shift+drag aliases middle-drag pan (EP-20) — middle buttons are awkward
    // or absent on trackpads, and both plain buttons orbit, so Shift is free
    // to redirect either button into the same pan path.
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let orbit_buttons = buttons.pressed(MouseButton::Right) || buttons.pressed(MouseButton::Left);
    let panning = buttons.pressed(MouseButton::Middle) || (shift && orbit_buttons);

    // Orbit: right-drag (or left-drag — both feel natural for a viewer app).
    if orbit_buttons && !shift && motion.delta != Vec2::ZERO {
        orbit.yaw -= motion.delta.x * 0.005;
        orbit.pitch = (orbit.pitch + motion.delta.y * 0.005).clamp(0.05, 1.54);
    }
    // Pan: slides `focus` (= the orbit pivot) along the ground plane (EP-20).
    // `right` is already horizontal (no roll); the vertical drag axis uses the
    // camera's up projected onto XZ, so the pivot can't drift off the ground —
    // the old camera-plane pan lifted it by cos(pitch) per unit drag, leaving
    // later orbit/zoom circling a point in the sky. Dividing by sin(pitch)
    // undoes screen-to-ground foreshortening: a ground shift of d reads as
    // d·sin(pitch) on screen, so the scene tracks the cursor exactly as the
    // camera-plane pan did (near top-down, sin ≈ 1 and flat_up ≈ up — feel is
    // unchanged there). The clamp keeps near-horizon pitches merely slower
    // instead of explosive (pitch ≥ 0.05 ⇒ uncompensated factor up to 20×).
    if panning && motion.delta != Vec2::ZERO {
        let pan = orbit.radius * 0.0015;
        let right = tf.right();
        let up = tf.up();
        let flat_up = Vec3::new(up.x, 0.0, up.z).normalize_or_zero();
        let comp = (1.0 / orbit.pitch.sin()).min(4.0);
        orbit.focus =
            orbit.focus - right * motion.delta.x * pan + flat_up * motion.delta.y * pan * comp;
    }
    // Zoom: wheel. Clamp so Line (±1..3) and Pixel (large) units both behave.
    let scroll_amount = scroll.delta.y.clamp(-3.0, 3.0);
    if scroll_amount != 0.0 {
        orbit.radius = (orbit.radius * (1.0 - scroll_amount * 0.12)).clamp(15.0, 1500.0);
    }

    let rot = Quat::from_euler(EulerRot::YXZ, orbit.yaw, -orbit.pitch, 0.0);
    tf.translation = orbit.focus + rot * Vec3::new(0.0, 0.0, orbit.radius);
    tf.rotation = rot;
}
