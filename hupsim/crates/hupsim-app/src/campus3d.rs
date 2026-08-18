//! The 3D campus scene: extruded per-building floor slabs colored by census
//! aggregate, the HUP Main podium, surveyed skybridge decks, subtle flow
//! ribbons for geometry-less edges, and click/hover picking.

use crate::agg_cache::AggCache;
use crate::dashboard::{DashboardState, DrillFocus, GroupKey};
use crate::mesh::extrude_polygon;
use crate::orbit::OrbitCamera;
use crate::query_panel::QueryPanelState;
use crate::world::{AppWorld, ConnHover, Focus, HoverInfo, SceneRebuild, Selection};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use hupsim_core::aggregate::AggregateMode;
use hupsim_core::color::{Rgb, COLOR_BEDLESS, COLOR_VACANT};
use hupsim_core::ids::NodeId;
use hupsim_core::layout::Footprint;
use hupsim_core::model::{ConnectionKind, FloorLevel};
use std::collections::{HashMap, HashSet};

/// Root of all rebuildable scene content (slabs, plates, decks, labels).
/// The camera and lights live outside it and survive scenario loads.
/// Despawning the root recursively drops every child's `Mesh3d` /
/// `MeshMaterial3d` handle; `Assets<T>` frees an asset once its last strong
/// handle is gone, so repeated loads don't accumulate meshes or materials.
#[derive(Component)]
pub struct CampusSceneRoot;

/// One floor of one building in the 3D massing. `level` is the floor's real
/// label (Ground, 7, 14…) — each slab sits at its label's campus story index
/// (`Building::story_index`: Ground = 0, floor N = story N, 13-skippers
/// compress N > 13 by one), so the same floor number is at the same height
/// in every wing and buildings that skip 13 stack 12 → 14 with no phantom
/// slab. A floor outside its wing's contiguous run floats at its labeled
/// level, visibly detached from the wing's roofline (the contested Rhoads 10
/// rendered this way until EP-23 retired it as a directory artifact).
#[derive(Component, Clone)]
pub struct FloorSlab {
    pub building: NodeId,
    pub level: FloorLevel,
}

/// Index of a connection in `hospital.connections`, for hover tooltips.
#[derive(Component)]
pub struct ConnectionRef(pub usize);

/// Slab entities per building, rebuilt whenever the scene is (EP-9, closing
/// EP-4's skipped item 8): lets [`apply_colors`] re-tint only the buildings
/// whose hover/selection state changed instead of walking every slab entity.
#[derive(Resource, Default)]
pub struct SlabRegistry(pub HashMap<NodeId, Vec<Entity>>);

/// World-space anchor for an egui-drawn text label.
#[derive(Component)]
pub struct LabelAnchor {
    pub text: String,
    pub world_pos: Vec3,
    pub always: bool,
}

pub fn rgb_to_color(c: Rgb) -> Color {
    Color::srgb(c.r, c.g, c.b)
}

pub fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    world: Res<AppWorld>,
    mut registry: ResMut<SlabRegistry>,
) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(200.0, 300.0, 400.0).looking_at(Vec3::ZERO, Vec3::Y),
        OrbitCamera::default(),
        AmbientLight {
            brightness: 400.0,
            ..default()
        },
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 9_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(250.0, 400.0, 150.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    build_scene(&mut commands, &mut meshes, &mut materials, &world, &mut registry);
}

/// Tear down and rebuild the campus scene when a scenario load requests it.
pub fn rebuild_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    world: Res<AppWorld>,
    mut request: ResMut<SceneRebuild>,
    roots: Query<Entity, With<CampusSceneRoot>>,
    mut hover: ResMut<HoverInfo>,
    mut conn_hover: ResMut<ConnHover>,
    mut registry: ResMut<SlabRegistry>,
) {
    if !request.0 {
        return;
    }
    request.0 = false;
    for root in &roots {
        commands.entity(root).despawn();
    }
    // Hover state may point at despawned entities / a different hospital.
    hover.0 = None;
    conn_hover.0 = None;
    build_scene(&mut commands, &mut meshes, &mut materials, &world, &mut registry);
}

fn build_scene(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    world: &AppWorld,
    registry: &mut SlabRegistry,
) {
    registry.0.clear();
    let hospital = world.hospital();
    commands
        .spawn((CampusSceneRoot, Transform::IDENTITY, Visibility::default()))
        .with_children(|parent| {
            // Ground plane.
            parent.spawn((
                Mesh3d(meshes.add(Cuboid::new(1400.0, 0.2, 1200.0))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.24, 0.27, 0.24),
                    perceptual_roughness: 1.0,
                    ..default()
                })),
                Transform::from_xyz(-120.0, -0.1, 80.0),
                Pickable::IGNORE,
            ));

            // Ground plates. The HUP Main plate extrudes to its podium height
            // (`extrude_m`, EP-7: one story, derived from the union of the
            // wing footprints) — it IS the shared street-level Ground story
            // the eight towers rise from (every building's floor list starts
            // at Ground; numbered floors begin above the rim) — in a darker
            // grey so it reads apart from tower slabs. Plates without the
            // field stay a thin plinth.
            for plate in &world.layout.ground_plates {
                let top = plate.extrude_m.unwrap_or(0.35);
                let base_color = if plate.extrude_m.is_some() {
                    Color::srgb(0.26, 0.27, 0.30)
                } else {
                    Color::srgb(0.32, 0.33, 0.36)
                };
                let mesh = extrude_polygon(&plate.footprint.vertices, 0.0, top);
                parent.spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        base_color,
                        perceptual_roughness: 1.0,
                        ..default()
                    })),
                    Transform::IDENTITY,
                    Pickable::IGNORE,
                ));
            }

            // Floor slabs per building.
            for bl in &world.layout.buildings {
                let Some(building) = hospital.buildings.iter().find(|b| b.id == bl.node) else {
                    continue;
                };
                let fh = bl.floor_height_m;
                let gap = (fh * 0.12).min(0.5);
                let centroid = bl.footprint.area_centroid();

                // One slab per compiled floor label, each at its label's
                // campus story index so floor N aligns across buildings.
                // Fallback for floorless buildings (stale scenario loads)
                // mirrors the compiler: `levels` counts total stories
                // including the ground story.
                let storied: Vec<(FloorLevel, i32)> = if building.floors.is_empty() {
                    std::iter::once(FloorLevel::Ground)
                        .chain((1..bl.levels as i32).map(FloorLevel::Numbered))
                        .enumerate()
                        .map(|(i, level)| (level, i as i32))
                        .collect()
                } else {
                    building
                        .floors
                        .iter()
                        .map(|f| (f.level.clone(), building.story_index(&f.level)))
                        .collect()
                };
                let top_story = storied.iter().map(|(_, s)| *s).max().unwrap_or(0) + 1;
                let top_y = bl.ground_elevation_m + top_story as f32 * fh;

                for (level, story) in storied {
                    let y0 = bl.ground_elevation_m + story as f32 * fh;
                    let mesh =
                        extrude_polygon(&bl.footprint.vertices, y0 + gap * 0.5, y0 + fh - gap * 0.5);
                    let slab = parent
                        .spawn((
                            Mesh3d(meshes.add(mesh)),
                            MeshMaterial3d(materials.add(StandardMaterial {
                                base_color: rgb_to_color(COLOR_BEDLESS),
                                perceptual_roughness: 0.9,
                                ..default()
                            })),
                            Transform::IDENTITY,
                            FloorSlab {
                                building: bl.node.clone(),
                                level,
                            },
                        ))
                        .observe(on_slab_click)
                        .observe(on_slab_over)
                        .observe(on_slab_out)
                        .id();
                    registry.0.entry(bl.node.clone()).or_default().push(slab);
                }

                parent.spawn(LabelAnchor {
                    text: building.short_name.clone(),
                    world_pos: Vec3::new(centroid[0], top_y + 6.0, -centroid[1]),
                    always: true,
                });
            }

            // Inter-building connections. Surveyed skybridge decks (EP-2 KML
            // polygons) extrude at their real deck height and are hoverable.
            // Geometry-less flow edges (Dietz & Watson bridge, the Connector
            // corridors) get a faint ground-level ribbon between the nearest
            // points of their footprints. The tunnel edge is unverified and
            // not drawn at all.
            for (i, conn) in hospital.connections.iter().enumerate() {
                if let Some(bridge) = conn
                    .layout
                    .as_deref()
                    .and_then(|id| world.layout.bridges.iter().find(|b| b.id == id))
                {
                    let mut deck = Footprint {
                        vertices: bridge.vertices.clone(),
                    };
                    deck.ensure_ccw();
                    let mesh = extrude_polygon(
                        &deck.vertices,
                        bridge.deck_height_m,
                        bridge.deck_height_m + 3.0,
                    );
                    parent
                        .spawn((
                            Mesh3d(meshes.add(mesh)),
                            MeshMaterial3d(materials.add(StandardMaterial {
                                base_color: Color::srgb(0.75, 0.72, 0.6),
                                perceptual_roughness: 0.8,
                                ..default()
                            })),
                            Transform::IDENTITY,
                            ConnectionRef(i),
                        ))
                        .observe(on_conn_over)
                        .observe(on_conn_out);
                    continue;
                }

                match conn.kind {
                    ConnectionKind::Bridge | ConnectionKind::ConnectorCorridor => {}
                    _ => continue,
                }
                let (Some(a), Some(b)) = (
                    world.layout.building(&conn.from),
                    world.layout.building(&conn.to),
                ) else {
                    continue;
                };
                let (pa, pb) = nearest_footprint_points(&a.footprint, &b.footprint);
                // A grade-level ribbon would be buried inside the extruded
                // podium wherever an endpoint is a HUP Main wing (D&W bridge,
                // the Connector) — lift the ribbon onto the tallest plate
                // either endpoint sits on; off-plate edges stay at grade.
                let plate_top = |p: [f32; 2]| -> f32 {
                    world
                        .layout
                        .ground_plates
                        .iter()
                        .filter(|g| g.footprint.contains_point(p))
                        .filter_map(|g| g.extrude_m)
                        .fold(0.0, f32::max)
                };
                let y = plate_top(pa).max(plate_top(pb)) + 0.6;
                let wa = Vec3::new(pa[0], y, -pa[1]);
                let wb = Vec3::new(pb[0], y, -pb[1]);
                let delta = wb - wa;
                let len = delta.length();
                if len < 1.0 {
                    continue;
                }
                let yaw = f32::atan2(-delta.z, delta.x);
                parent
                    .spawn((
                        Mesh3d(meshes.add(Cuboid::new(len, 0.25, 2.5))),
                        MeshMaterial3d(materials.add(StandardMaterial {
                            base_color: Color::srgba(0.6, 0.62, 0.66, 0.22),
                            alpha_mode: AlphaMode::Blend,
                            perceptual_roughness: 0.9,
                            ..default()
                        })),
                        Transform::from_translation((wa + wb) * 0.5)
                            .with_rotation(Quat::from_rotation_y(yaw)),
                        ConnectionRef(i),
                    ))
                    .observe(on_conn_over)
                    .observe(on_conn_out);
            }
        });
}

/// Closest point to `p` on segment `ab`.
fn seg_closest(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    if len2 < 1e-9 {
        return a;
    }
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0);
    [a[0] + t * ab[0], a[1] + t * ab[1]]
}

/// Nearest pair of boundary points between two non-intersecting footprints.
/// For simple polygons the closest pair always involves a vertex of one and
/// an edge of the other, so checking both directions suffices.
fn nearest_footprint_points(fa: &Footprint, fb: &Footprint) -> ([f32; 2], [f32; 2]) {
    let (va, vb) = (&fa.vertices, &fb.vertices);
    let mut best = (f32::INFINITY, [0.0f32; 2], [0.0f32; 2]);
    let mut consider = |p: [f32; 2], q: [f32; 2]| {
        let d2 = (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2);
        if d2 < best.0 {
            best = (d2, p, q);
        }
    };
    for &p in va {
        for i in 0..vb.len() {
            consider(p, seg_closest(p, vb[i], vb[(i + 1) % vb.len()]));
        }
    }
    for &q in vb {
        for i in 0..va.len() {
            consider(seg_closest(q, va[i], va[(i + 1) % va.len()]), q);
        }
    }
    (best.1, best.2)
}

fn on_slab_click(
    click: On<Pointer<Click>>,
    slabs: Query<&FloorSlab>,
    mut selection: ResMut<Selection>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    if let Ok(slab) = slabs.get(click.entity) {
        selection.0 = Focus::Floor(slab.building.clone(), slab.level.clone());
    }
}

fn on_slab_over(over: On<Pointer<Over>>, slabs: Query<&FloorSlab>, mut hover: ResMut<HoverInfo>) {
    if let Ok(slab) = slabs.get(over.entity) {
        hover.0 = Some((slab.building.clone(), slab.level.clone()));
    }
}

fn on_slab_out(out: On<Pointer<Out>>, slabs: Query<&FloorSlab>, mut hover: ResMut<HoverInfo>) {
    if let Ok(slab) = slabs.get(out.entity) {
        if hover.0.as_ref() == Some(&(slab.building.clone(), slab.level.clone())) {
            hover.0 = None;
        }
    }
}

fn on_conn_over(over: On<Pointer<Over>>, conns: Query<&ConnectionRef>, mut hover: ResMut<ConnHover>) {
    if let Ok(conn) = conns.get(over.entity) {
        hover.0 = Some(conn.0);
    }
}

fn on_conn_out(out: On<Pointer<Out>>, conns: Query<&ConnectionRef>, mut hover: ResMut<ConnHover>) {
    if let Ok(conn) = conns.get(out.entity) {
        if hover.0 == Some(conn.0) {
            hover.0 = None;
        }
    }
}

/// The floor slab a focus lights up: floors light themselves; a focused
/// room (navigator, unit view, or an EP-14 query result) lights the slab
/// its unit sits on.
fn focus_floor(world: &AppWorld, focus: &Focus) -> Option<(NodeId, FloorLevel)> {
    match focus {
        Focus::Floor(b, l) => Some((b.clone(), l.clone())),
        Focus::Room(r) => {
            let h = world.hospital();
            let &ri = world.index.room_pos.get(r)?;
            let &up = world.index.unit_pos.get(&h.rooms[ri].unit)?;
            let u = &h.units[up];
            Some((u.building.clone(), u.level.clone()))
        }
        _ => None,
    }
}

/// Re-tint floor slabs when the world (or hover/selection) changes. Reads
/// the version-keyed [`AggCache`] — no room scans here (EP-9). A world
/// change re-tints every slab (cheap map lookups); a hover/selection-only
/// change touches just the affected buildings via the [`SlabRegistry`].
#[allow(clippy::too_many_arguments)]
pub fn apply_colors(
    world: Res<AppWorld>,
    mut cache: ResMut<AggCache>,
    registry: Res<SlabRegistry>,
    hover: Res<HoverInfo>,
    selection: Res<Selection>,
    dash: Res<DashboardState>,
    query: Res<QueryPanelState>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    slabs: Query<(&FloorSlab, &MeshMaterial3d<StandardMaterial>)>,
    added: Query<(), Added<FloorSlab>>,
    mut last_applied: Local<u64>,
    mut last_hover: Local<Option<(NodeId, FloorLevel)>>,
    mut last_focus: Local<Focus>,
    mut last_drill: Local<Option<GroupKey>>,
    mut last_query_gen: Local<u64>,
) {
    // `added` catches scene rebuilds: fresh slabs spawn in the placeholder
    // color even when `world.version` hasn't moved since the last tint.
    // A drill-through change (EP-13) retints everything too — a group can
    // span many buildings, and drills change only on clicks. Query results
    // (EP-14) behave like drills: a result set spans buildings and changes
    // only on Run/Clear, keyed by the panel's generation counter.
    let drill_key = dash.drill.as_ref().map(|d| d.key);
    let version_dirty = *last_applied != world.version
        || !added.is_empty()
        || *last_drill != drill_key
        || *last_query_gen != query.generation;
    let pointer_dirty = *last_hover != hover.0 || *last_focus != selection.0;
    if !version_dirty && !pointer_dirty {
        return;
    }
    cache.refresh_if_stale(&world);
    let drill = dash.drill.as_ref();
    let query_floors = query.staged.as_ref().map(|s| &s.floors);
    let selected_floor = focus_floor(&world, &selection.0);

    if version_dirty {
        for (slab, mat_handle) in &slabs {
            tint_slab(
                &cache,
                world.agg_mode,
                &hover.0,
                selected_floor.as_ref(),
                drill,
                query_floors,
                &mut materials,
                slab,
                mat_handle,
            );
        }
    } else {
        // Only hover/selection moved: re-tint the buildings leaving and
        // entering that state, via the registry.
        let last_floor = focus_floor(&world, &last_focus);
        let mut touched: Vec<&NodeId> = Vec::new();
        for (b, _) in [&*last_hover, &hover.0].into_iter().flatten() {
            touched.push(b);
        }
        for (b, _) in [&last_floor, &selected_floor].into_iter().flatten() {
            touched.push(b);
        }
        touched.sort();
        touched.dedup();
        for building in touched {
            for &entity in registry.0.get(building).map(Vec::as_slice).unwrap_or(&[]) {
                if let Ok((slab, mat_handle)) = slabs.get(entity) {
                    tint_slab(
                        &cache,
                        world.agg_mode,
                        &hover.0,
                        selected_floor.as_ref(),
                        drill,
                        query_floors,
                        &mut materials,
                        slab,
                        mat_handle,
                    );
                }
            }
        }
    }

    *last_applied = world.version;
    *last_hover = hover.0.clone();
    *last_focus = selection.0.clone();
    *last_drill = drill_key;
    *last_query_gen = query.generation;
}

#[allow(clippy::too_many_arguments)]
fn tint_slab(
    cache: &AggCache,
    agg_mode: AggregateMode,
    hover: &Option<(NodeId, FloorLevel)>,
    selected_floor: Option<&(NodeId, FloorLevel)>,
    drill: Option<&DrillFocus>,
    query_floors: Option<&HashSet<(NodeId, FloorLevel)>>,
    materials: &mut Assets<StandardMaterial>,
    slab: &FloorSlab,
    mat_handle: &MeshMaterial3d<StandardMaterial>,
) {
    let agg = cache.floor(&slab.building, &slab.level);
    let base = if agg.capacity > 0 {
        if agg.census > 0 {
            agg.color(agg_mode)
        } else {
            COLOR_VACANT
        }
    } else {
        COLOR_BEDLESS
    };
    let key = (slab.building.clone(), slab.level.clone());
    let hovered = matches!(
        hover,
        Some((b, l)) if *b == slab.building && *l == slab.level
    );
    let selected = selected_floor == Some(&key);
    // EP-14 placement results: floors holding candidate beds.
    let queried = query_floors.is_some_and(|f| f.contains(&key));
    // EP-13 drill-through: member floor of the focused dashboard group.
    let drilled = drill.is_some_and(|d| d.floors.contains(&key));
    if let Some(mut mat) = materials.get_mut(&mat_handle.0) {
        mat.base_color = rgb_to_color(base);
        mat.emissive = if selected {
            LinearRgba::rgb(0.25, 0.2, 0.05)
        } else if hovered {
            LinearRgba::rgb(0.12, 0.12, 0.16)
        } else if queried {
            // Violet — distinct from the warm selection, the neutral hover,
            // and the drill teal, matching the unit view's result outline.
            LinearRgba::rgb(0.17, 0.06, 0.22)
        } else if drilled {
            // Cool teal so the group read can't be confused with the warm
            // selection highlight.
            LinearRgba::rgb(0.04, 0.16, 0.22)
        } else {
            LinearRgba::BLACK
        };
    }
}
