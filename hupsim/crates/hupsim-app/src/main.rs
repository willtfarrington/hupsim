//! hupsim — HUP hospital-system desktop simulator.
//!
//! 3D campus massing (toggleable to per-unit 2D floor plans), room-level
//! occupancy/stability editing, and scaffolding for time-series simulation.

mod agg_cache;
mod campus3d;
mod compare_panel;
mod dashboard;
mod mesh;
mod orbit;
mod query_panel;
mod report_panel;
mod sim_drive;
mod ui;
mod unit_view;
mod widgets;
mod world;

use bevy::picking::mesh_picking::MeshPickingPlugin;
use bevy::prelude::*;
use bevy_egui::EguiPlugin;
use world::{AppWorld, ConnHover, HoverInfo, OpenUnit, SceneRebuild, Selection, SimDrive, ViewMode};

fn main() {
    let loaded = match hupsim_data::load_default() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("hupsim: failed to load hospital data: {e}");
            std::process::exit(1);
        }
    };
    let mut app_world = AppWorld::new(loaded);

    // First launch shows a plausible Tuesday, not an empty shell — seeded
    // mid-stream (EP-21): backdated admit stamps at the t0 snapshot.
    let seed = app_world.scenario.rng_seed;
    {
        let w = &mut app_world;
        hupsim_data::seed_demo_census(&mut w.scenario.hospital, &w.index, seed, 0.0);
        w.version += 1;
    }
    let sim = SimDrive::new(seed, &app_world.scenario.hospital, &app_world.layout);

    App::new()
        .add_plugins(
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "hupsim — Hospital of the University of Pennsylvania".to_string(),
                    resolution: (1600u32, 950u32).into(),
                    ..default()
                }),
                ..default()
            }),
        )
        .add_plugins(MeshPickingPlugin)
        .add_plugins(EguiPlugin::default())
        .add_plugins(ui::UiPlugin)
        .insert_resource(app_world)
        .insert_resource(sim)
        .init_resource::<agg_cache::AggCache>()
        .init_resource::<campus3d::SlabRegistry>()
        .init_resource::<dashboard::DashboardState>()
        .init_resource::<query_panel::QueryPanelState>()
        .init_resource::<Selection>()
        .init_resource::<HoverInfo>()
        .init_resource::<ConnHover>()
        .init_resource::<SceneRebuild>()
        .init_resource::<OpenUnit>()
        .init_state::<ViewMode>()
        .insert_resource(Time::<Fixed>::from_hz(10.0))
        .add_systems(Startup, campus3d::setup_scene)
        .add_systems(
            Update,
            (
                orbit::orbit_camera.run_if(in_state(ViewMode::Campus3D)),
                campus3d::rebuild_scene,
                campus3d::apply_colors,
            ),
        )
        .add_systems(FixedUpdate, sim_drive::sim_tick)
        .add_systems(Update, sim_drive::roster_sync)
        .run();
}
