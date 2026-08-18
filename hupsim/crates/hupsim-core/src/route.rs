//! Campus route graph (EP-6) — the "navigation-and-patient-flow-conscious
//! topology" payoff. Derives a traversal graph from `hospital.connections` +
//! `hospital.elevator_cores` + the campus layout: nodes are (building, level)
//! pairs, edges are bridge / connector / tunnel / adjacency / floor-continuity
//! hops plus elevator rides and the shared HUP Main ground podium. Weights are
//! traversal minutes (walk speed × distance where geometry exists; wait+ride
//! constants for elevators). Plain Dijkstra, no new dependencies.
//!
//! Model granularity is deliberately coarse: horizontal movement *within* a
//! (building, level) node is free — distances between buildings absorb the
//! interior walk (centroid-to-centroid × a circuity factor). Surveyed skybridge
//! decks use their real KML lengths; every other weight is tagged inferred.
//!
//! Confidence traversal policy: `verified`/`reported` edges are traversable;
//! `inferred` edges are traversable and flag the route; `unverified` edges
//! (the HUP Main↔Clifton tunnel) are excluded unless
//! `TraversalPolicy::include_unverified` is set. (The contested Rhoads L10
//! continuity rode this policy until EP-23 archived it with its unit.)
//!
//! The authored connection set alone leaves wings like Rhoads, Maloney, and
//! White disconnected — physical HUP Main circulation is implied by the shared
//! ground podium, not by edges (see roadmap decision 3). Buildings whose
//! footprints sit on a ground plate are therefore pairwise connected at Ground,
//! tagged inferred. Buildings with no authored elevator core (PCAM) get a
//! synthetic inferred "circulation" core so routes can change level there.

use std::collections::HashMap;

use crate::ids::{NodeId, RoomId};
use crate::index::HospitalIndex;
use crate::layout::{CampusLayout, Footprint};
use crate::model::{Building, Connection, ConnectionKind, FloorLevel, Hospital};
use crate::provenance::Confidence;
use thiserror::Error;

/// Weight constants for the route graph. PROVENANCE: synthetic/inferred
/// professional norms, not sourced to HUP — one visible table, like the sim's
/// `params.rs`. Surveyed deck lengths are the only measured inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteParams {
    /// Occupied-bed push speed, meters per minute (~1 m/s with corners/doors).
    pub walk_m_per_min: f32,
    /// Corridor circuity multiplier applied to straight-line (inferred)
    /// distances. Surveyed deck lengths are used as-is.
    pub circuity: f32,
    /// Elevator wait for a bed-capable car, minutes (flat per ride).
    pub elevator_wait_min: f32,
    /// Elevator ride minutes per story traversed.
    pub elevator_min_per_story: f32,
    /// Minutes charged for a transfer within one (building, level) node.
    pub same_floor_min: f32,
    /// Floor for any derived horizontal distance, meters (adjacent wings'
    /// centroids can be close; a zero-weight edge would be dishonest).
    pub min_edge_m: f32,
}

impl Default for RouteParams {
    fn default() -> Self {
        Self {
            walk_m_per_min: 60.0,
            circuity: 1.4,
            elevator_wait_min: 2.5,
            elevator_min_per_story: 0.3,
            same_floor_min: 2.0,
            min_edge_m: 5.0,
        }
    }
}

/// Which confidence tiers are traversable. Verified/reported/inferred always
/// are; unverified edges are excluded by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TraversalPolicy {
    pub include_unverified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteNode {
    pub building: NodeId,
    pub level: FloorLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteEdgeKind {
    Bridge,
    Tunnel,
    Connector,
    Adjacency,
    FloorContinuity,
    Elevator,
    Podium,
}

impl RouteEdgeKind {
    pub fn label(&self) -> &'static str {
        match self {
            RouteEdgeKind::Bridge => "skybridge",
            RouteEdgeKind::Tunnel => "tunnel",
            RouteEdgeKind::Connector => "connector corridor",
            RouteEdgeKind::Adjacency => "structural adjacency",
            RouteEdgeKind::FloorContinuity => "floor continuity",
            RouteEdgeKind::Elevator => "elevator",
            RouteEdgeKind::Podium => "ground podium",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteEdge {
    pub a: usize,
    pub b: usize,
    pub kind: RouteEdgeKind,
    /// Traversal weight, minutes. Always positive.
    pub minutes: f32,
    /// Horizontal distance, meters (0 for elevator hops).
    pub meters: f32,
    /// Display name: the connection's name or the elevator core's name.
    pub name: Option<String>,
    /// Confidence of the underlying *link* (from the connection's provenance;
    /// derived podium/elevator edges are inferred).
    pub confidence: Confidence,
    /// True unless the distance comes from surveyed deck geometry.
    pub weight_inferred: bool,
    /// Index into `hospital.connections` when the edge mirrors one (for UI
    /// highlighting); None for derived podium/elevator edges.
    pub conn: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteHop {
    pub from_building: NodeId,
    pub from_level: FloorLevel,
    pub to_building: NodeId,
    pub to_level: FloorLevel,
    pub kind: RouteEdgeKind,
    pub name: Option<String>,
    pub minutes: f32,
    pub conn: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub hops: Vec<RouteHop>,
    pub minutes: f32,
    pub meters: f32,
    /// Worst (highest) confidence tier among traversed links.
    pub worst_confidence: Confidence,
    /// True if any hop's weight is inferred rather than surveyed.
    pub inferred_weights: bool,
}

impl Route {
    /// Landmark summary for log lines: named bridge/connector/tunnel hops in
    /// order, deduplicated — `"via Silverstein↔Clifton skybridge"` — or
    /// `"direct"` when the route uses none.
    pub fn via_summary(&self) -> String {
        let mut names: Vec<String> = Vec::new();
        for hop in &self.hops {
            let landmark = matches!(
                hop.kind,
                RouteEdgeKind::Bridge | RouteEdgeKind::Connector | RouteEdgeKind::Tunnel
            );
            if !landmark {
                continue;
            }
            let name = hop
                .name
                .clone()
                .unwrap_or_else(|| hop.kind.label().to_string());
            if names.last() != Some(&name) {
                names.push(name);
            }
        }
        if names.is_empty() {
            "direct".to_string()
        } else {
            format!("via {}", names.join(" + "))
        }
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum RouteError {
    #[error("room not found: {0}")]
    UnknownRoom(RoomId),
    #[error("no route node for {building} {}", level.label())]
    NoNode { building: NodeId, level: FloorLevel },
    #[error("no traversable route: {from} → {to}")]
    Unreachable { from: String, to: String },
}

#[derive(Debug, Clone)]
pub struct RouteGraph {
    pub nodes: Vec<RouteNode>,
    pub edges: Vec<RouteEdge>,
    /// node index → indices into `edges` (undirected: each edge appears in
    /// both endpoints' lists).
    adj: Vec<Vec<usize>>,
    node_ix: HashMap<(NodeId, FloorLevel), usize>,
    pub params: RouteParams,
    pub policy: TraversalPolicy,
}

/// Topology levels are bare ints; 0 means the shared Ground story.
fn lvl(n: i32) -> FloorLevel {
    if n == 0 {
        FloorLevel::Ground
    } else {
        FloorLevel::Numbered(n)
    }
}

/// Levels a connection spans: authored `levels` when present, else a
/// kind-specific default (documented inference — see module doc).
fn conn_levels(conn: &Connection, from: Option<&Building>, to: Option<&Building>) -> Vec<FloorLevel> {
    if !conn.levels.is_empty() {
        return conn.levels.iter().map(|&n| lvl(n)).collect();
    }
    match conn.kind {
        // Dietz & Watson: no published level; 2 keeps it distinct from the
        // level-1 Connector below it.
        ConnectionKind::Bridge => vec![FloorLevel::Numbered(2)],
        // "The Connector is the FIRST floor" (ground-story decision record).
        ConnectionKind::ConnectorCorridor => vec![FloorLevel::Numbered(1)],
        // Below-grade alignment unpublished; modeled as a grade-level hop.
        ConnectionKind::Tunnel => vec![FloorLevel::Ground],
        // Contiguous wings: every level both buildings actually model.
        ConnectionKind::StructuralAdjacency | ConnectionKind::FloorContinuity => {
            let (Some(a), Some(b)) = (from, to) else {
                return vec![FloorLevel::Ground];
            };
            let mut shared: Vec<FloorLevel> = a
                .floors
                .iter()
                .filter(|f| b.floor(&f.level).is_some())
                .map(|f| f.level.clone())
                .collect();
            if !shared.contains(&FloorLevel::Ground) {
                shared.push(FloorLevel::Ground);
            }
            shared
        }
        ConnectionKind::NoPhysicalConnection => vec![],
    }
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

/// Long axis of a (4-vertex) deck polygon = the walkable span.
fn deck_length(vertices: &[[f32; 2]]) -> f32 {
    let mut best = 0.0f32;
    for (i, &a) in vertices.iter().enumerate() {
        for &b in &vertices[i + 1..] {
            best = best.max(dist(a, b));
        }
    }
    best
}

fn centroid(f: &Footprint) -> [f32; 2] {
    f.area_centroid()
}

impl RouteGraph {
    pub fn build(
        h: &Hospital,
        layout: &CampusLayout,
        params: RouteParams,
        policy: TraversalPolicy,
    ) -> Self {
        let bpos: HashMap<&NodeId, usize> =
            h.buildings.iter().enumerate().map(|(i, b)| (&b.id, i)).collect();
        let building = |id: &NodeId| bpos.get(id).map(|&i| &h.buildings[i]);
        let traversable =
            |c: Confidence| policy.include_unverified || c != Confidence::Unverified;

        // --- Collect node levels per building (deterministic order). -------
        // Sources: Ground, the building's floor list, unit locations, and the
        // levels of traversable connections touching the building.
        let mut levels: Vec<Vec<FloorLevel>> = h
            .buildings
            .iter()
            .map(|b| {
                let mut v = vec![FloorLevel::Ground];
                v.extend(b.floors.iter().map(|f| f.level.clone()));
                v
            })
            .collect();
        for u in &h.units {
            if let Some(&i) = bpos.get(&u.building) {
                levels[i].push(u.level.clone());
            }
        }
        for conn in &h.connections {
            if !traversable(conn.provenance.confidence) {
                continue;
            }
            let spans = conn_levels(conn, building(&conn.from), building(&conn.to));
            for end in [&conn.from, &conn.to] {
                if let Some(&i) = bpos.get(end) {
                    levels[i].extend(spans.iter().cloned());
                }
            }
        }
        let mut nodes: Vec<RouteNode> = Vec::new();
        let mut node_ix: HashMap<(NodeId, FloorLevel), usize> = HashMap::new();
        for (i, b) in h.buildings.iter().enumerate() {
            let mut ls = std::mem::take(&mut levels[i]);
            ls.sort_by(|a, b| a.order_key().total_cmp(&b.order_key()));
            ls.dedup();
            for l in ls {
                let key = (b.id.clone(), l.clone());
                if !node_ix.contains_key(&key) {
                    node_ix.insert(key, nodes.len());
                    nodes.push(RouteNode {
                        building: b.id.clone(),
                        level: l,
                    });
                }
            }
        }

        let mut edges: Vec<RouteEdge> = Vec::new();
        let node_of = |b: &NodeId, l: &FloorLevel| node_ix.get(&(b.clone(), l.clone())).copied();

        // --- Connection edges. ---------------------------------------------
        for (ci, conn) in h.connections.iter().enumerate() {
            if conn.kind == ConnectionKind::NoPhysicalConnection
                || !traversable(conn.provenance.confidence)
            {
                continue;
            }
            let kind = match conn.kind {
                ConnectionKind::Bridge => RouteEdgeKind::Bridge,
                ConnectionKind::Tunnel => RouteEdgeKind::Tunnel,
                ConnectionKind::ConnectorCorridor => RouteEdgeKind::Connector,
                ConnectionKind::StructuralAdjacency => RouteEdgeKind::Adjacency,
                ConnectionKind::FloorContinuity => RouteEdgeKind::FloorContinuity,
                ConnectionKind::NoPhysicalConnection => continue,
            };
            // Surveyed deck length where the layout ref resolves; otherwise
            // centroid-to-centroid straight line × circuity (inferred).
            let deck = conn
                .layout
                .as_deref()
                .and_then(|id| layout.bridges.iter().find(|br| br.id == id));
            let (meters, weight_inferred) = match deck {
                Some(br) => (deck_length(&br.vertices).max(params.min_edge_m), false),
                None => {
                    let fa = layout.building(&conn.from).map(|bl| &bl.footprint);
                    let fb = layout.building(&conn.to).map(|bl| &bl.footprint);
                    let m = match (fa, fb) {
                        (Some(fa), Some(fb)) => {
                            dist(centroid(fa), centroid(fb)) * params.circuity
                        }
                        // No geometry at all: a nominal indoor block.
                        _ => 60.0,
                    };
                    (m.max(params.min_edge_m), true)
                }
            };
            let minutes = meters / params.walk_m_per_min;
            for l in conn_levels(conn, building(&conn.from), building(&conn.to)) {
                let (Some(a), Some(b)) = (node_of(&conn.from, &l), node_of(&conn.to, &l)) else {
                    continue;
                };
                edges.push(RouteEdge {
                    a,
                    b,
                    kind,
                    minutes,
                    meters,
                    name: conn.name.clone(),
                    confidence: conn.provenance.confidence,
                    weight_inferred,
                    conn: Some(ci),
                });
            }
        }

        // --- Podium ground edges. ------------------------------------------
        // Buildings whose footprint centroid sits on a shared ground plate are
        // pairwise walkable at Ground (roadmap decision 3: the podium IS the
        // shared street-level circulation layer). Inferred.
        for plate in &layout.ground_plates {
            let members: Vec<&Building> = h
                .buildings
                .iter()
                .filter(|b| {
                    layout
                        .building(&b.id)
                        .map(|bl| plate.footprint.contains_point(centroid(&bl.footprint)))
                        .unwrap_or(false)
                })
                .collect();
            for (i, a) in members.iter().enumerate() {
                for b in &members[i + 1..] {
                    let (Some(na), Some(nb)) = (
                        node_of(&a.id, &FloorLevel::Ground),
                        node_of(&b.id, &FloorLevel::Ground),
                    ) else {
                        continue;
                    };
                    let ca = centroid(&layout.building(&a.id).unwrap().footprint);
                    let cb = centroid(&layout.building(&b.id).unwrap().footprint);
                    let meters = (dist(ca, cb) * params.circuity).max(params.min_edge_m);
                    edges.push(RouteEdge {
                        a: na,
                        b: nb,
                        kind: RouteEdgeKind::Podium,
                        minutes: meters / params.walk_m_per_min,
                        meters,
                        name: None,
                        confidence: Confidence::Inferred,
                        weight_inferred: true,
                        conn: None,
                    });
                }
            }
        }

        // --- Elevator edges. -----------------------------------------------
        // A core serves its reported patient floors plus (inferred) Ground and
        // every circulation level already modeled as a node for its building —
        // elevators physically reach the lobby and the bridge/connector
        // levels even though the wayfinding data lists only patient floors.
        let node_levels_of = |bid: &NodeId| -> Vec<FloorLevel> {
            nodes
                .iter()
                .filter(|n| &n.building == bid)
                .map(|n| n.level.clone())
                .collect()
        };
        let mut cored: Vec<&NodeId> = Vec::new();
        for core in &h.elevator_cores {
            let Some(bid) = core.building.as_ref() else {
                continue;
            };
            let Some(b) = building(bid) else { continue };
            cored.push(bid);
            let reported: Vec<FloorLevel> =
                core.serves_patient_floors.iter().map(|&n| lvl(n)).collect();
            // A core serves every level its building models (elevators reach
            // the lobby and bridge levels, not just the wayfinding-listed
            // patient floors). Hops between two reported floors keep the
            // wayfinding confidence; anything else is inferred.
            let mut served: Vec<FloorLevel> = node_levels_of(bid);
            served.sort_by(|a, b| a.order_key().total_cmp(&b.order_key()));
            served.dedup();
            for (i, la) in served.iter().enumerate() {
                for lb in &served[i + 1..] {
                    let (Some(na), Some(nb)) = (node_of(bid, la), node_of(bid, lb)) else {
                        continue;
                    };
                    let stories = (b.story_index(la) - b.story_index(lb)).abs();
                    let both_reported = reported.contains(la) && reported.contains(lb);
                    edges.push(RouteEdge {
                        a: na,
                        b: nb,
                        kind: RouteEdgeKind::Elevator,
                        minutes: params.elevator_wait_min
                            + stories as f32 * params.elevator_min_per_story,
                        meters: 0.0,
                        name: Some(format!("{} elevator", core.name)),
                        confidence: if both_reported {
                            Confidence::Reported
                        } else {
                            Confidence::Inferred
                        },
                        weight_inferred: true,
                        conn: None,
                    });
                }
            }
        }

        // Synthetic circulation core for buildings with no authored one
        // (PCAM): routes must be able to change level there. Inferred.
        for b in &h.buildings {
            if cored.contains(&&b.id) {
                continue;
            }
            let mut served = node_levels_of(&b.id);
            served.sort_by(|a, b| a.order_key().total_cmp(&b.order_key()));
            served.dedup();
            if served.len() < 2 {
                continue;
            }
            for (i, la) in served.iter().enumerate() {
                for lb in &served[i + 1..] {
                    let (Some(na), Some(nb)) = (node_of(&b.id, la), node_of(&b.id, lb)) else {
                        continue;
                    };
                    let stories = (b.story_index(la) - b.story_index(lb)).abs();
                    edges.push(RouteEdge {
                        a: na,
                        b: nb,
                        kind: RouteEdgeKind::Elevator,
                        minutes: params.elevator_wait_min
                            + stories as f32 * params.elevator_min_per_story,
                        meters: 0.0,
                        name: Some(format!("{} circulation", b.short_name)),
                        confidence: Confidence::Inferred,
                        weight_inferred: true,
                        conn: None,
                    });
                }
            }
        }

        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
        for (ei, e) in edges.iter().enumerate() {
            adj[e.a].push(ei);
            adj[e.b].push(ei);
        }
        Self {
            nodes,
            edges,
            adj,
            node_ix,
            params,
            policy,
        }
    }

    pub fn node_index(&self, building: &NodeId, level: &FloorLevel) -> Option<usize> {
        self.node_ix.get(&(building.clone(), level.clone())).copied()
    }

    /// Dijkstra shortest path by minutes. Deterministic: ties break on node
    /// index. Returns None when unreachable.
    pub fn route(&self, from: usize, to: usize) -> Option<Route> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        #[derive(PartialEq)]
        struct Key(f32, usize);
        impl Eq for Key {}
        impl PartialOrd for Key {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }
        impl Ord for Key {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                self.0.total_cmp(&other.0).then(self.1.cmp(&other.1))
            }
        }

        if from >= self.nodes.len() || to >= self.nodes.len() {
            return None;
        }
        let mut dist_min = vec![f32::INFINITY; self.nodes.len()];
        let mut prev: Vec<Option<(usize, usize)>> = vec![None; self.nodes.len()]; // (node, edge)
        let mut heap = BinaryHeap::new();
        dist_min[from] = 0.0;
        heap.push(Reverse(Key(0.0, from)));
        while let Some(Reverse(Key(d, n))) = heap.pop() {
            if d > dist_min[n] {
                continue;
            }
            if n == to {
                break;
            }
            for &ei in &self.adj[n] {
                let e = &self.edges[ei];
                let m = if e.a == n { e.b } else { e.a };
                let nd = d + e.minutes;
                if nd < dist_min[m] {
                    dist_min[m] = nd;
                    prev[m] = Some((n, ei));
                    heap.push(Reverse(Key(nd, m)));
                }
            }
        }
        if !dist_min[to].is_finite() {
            return None;
        }
        let mut hops_rev: Vec<RouteHop> = Vec::new();
        let mut meters = 0.0;
        let mut worst = Confidence::Verified;
        let mut inferred = false;
        let mut cur = to;
        while cur != from {
            let (p, ei) = prev[cur].expect("finite distance implies a predecessor chain");
            let e = &self.edges[ei];
            meters += e.meters;
            worst = worst.max(e.confidence);
            inferred |= e.weight_inferred;
            hops_rev.push(RouteHop {
                from_building: self.nodes[p].building.clone(),
                from_level: self.nodes[p].level.clone(),
                to_building: self.nodes[cur].building.clone(),
                to_level: self.nodes[cur].level.clone(),
                kind: e.kind,
                name: e.name.clone(),
                minutes: e.minutes,
                conn: e.conn,
            });
            cur = p;
        }
        hops_rev.reverse();
        Some(Route {
            hops: hops_rev,
            minutes: dist_min[to],
            meters,
            worst_confidence: worst,
            inferred_weights: inferred,
        })
    }

    /// Room-to-room route via each room's unit location. Same-node transfers
    /// cost `same_floor_min`. Unreachable pairs are reported, never panicked.
    pub fn route_rooms(
        &self,
        h: &Hospital,
        idx: &HospitalIndex,
        from: &RoomId,
        to: &RoomId,
    ) -> Result<Route, RouteError> {
        let loc = |room: &RoomId| -> Result<(NodeId, FloorLevel), RouteError> {
            let ri = idx
                .room_pos
                .get(room)
                .copied()
                .ok_or_else(|| RouteError::UnknownRoom(room.clone()))?;
            let ui = idx
                .unit_pos
                .get(&h.rooms[ri].unit)
                .copied()
                .ok_or_else(|| RouteError::UnknownRoom(room.clone()))?;
            let u = &h.units[ui];
            Ok((u.building.clone(), u.level.clone()))
        };
        let (fb, fl) = loc(from)?;
        let (tb, tl) = loc(to)?;
        let fnode = self
            .node_index(&fb, &fl)
            .ok_or_else(|| RouteError::NoNode {
                building: fb.clone(),
                level: fl.clone(),
            })?;
        let tnode = self
            .node_index(&tb, &tl)
            .ok_or_else(|| RouteError::NoNode {
                building: tb.clone(),
                level: tl.clone(),
            })?;
        if fnode == tnode {
            return Ok(Route {
                hops: vec![],
                minutes: self.params.same_floor_min,
                meters: 0.0,
                worst_confidence: Confidence::Verified,
                inferred_weights: true,
            });
        }
        self.route(fnode, tnode).ok_or_else(|| RouteError::Unreachable {
            from: format!("{} {}", fb, fl.label()),
            to: format!("{} {}", tb, tl.label()),
        })
    }

    /// Human label for a node, e.g. `"Founders 5"`.
    pub fn node_label(&self, h: &Hospital, node: usize) -> String {
        let n = &self.nodes[node];
        let short = h
            .buildings
            .iter()
            .find(|b| b.id == n.building)
            .map(|b| b.short_name.as_str())
            .unwrap_or_else(|| n.building.as_str());
        format!("{} {}", short, n.level.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Bridge, BuildingLayout, GroundPlate};
    use crate::model::*;
    use crate::provenance::Provenance;

    fn building(id: &str, floors: &[i32]) -> Building {
        Building {
            id: id.into(),
            name: id.to_string(),
            short_name: id.to_string(),
            kind: BuildingKind::Wing,
            built: None,
            aliases: vec![],
            floors: floors
                .iter()
                .map(|&n| FloorSpec {
                    level: lvl(n),
                    has_patient_rooms: true,
                    contents: vec![],
                    note: None,
                })
                .collect(),
            has_inpatient_beds: true,
            provenance: Provenance::default(),
            note: None,
        }
    }

    fn footprint(cx: f32, cy: f32) -> Footprint {
        Footprint {
            vertices: vec![
                [cx - 10.0, cy - 10.0],
                [cx + 10.0, cy - 10.0],
                [cx + 10.0, cy + 10.0],
                [cx - 10.0, cy + 10.0],
            ],
        }
    }

    /// Two towers A and B, 120 m apart, bridge at level 2 (surveyed deck),
    /// unverified tunnel at ground, elevator core in each; a third tower C on
    /// no plate, with no connections at all (unreachable).
    fn world() -> (Hospital, CampusLayout) {
        let h = Hospital {
            meta: HospitalMeta {
                name: "T".into(),
                ..Default::default()
            },
            buildings: vec![
                building("A", &[0, 1, 2, 3]),
                building("B", &[0, 1, 2, 3, 4]),
                building("C", &[0, 1]),
            ],
            units: vec![
                Unit {
                    id: "uA3".into(),
                    name: "A3".into(),
                    service: "med".into(),
                    service_line: None,
                    unit_type: UnitType::MedSurg,
                    building: "A".into(),
                    level: FloorLevel::Numbered(3),
                    elevator_core: None,
                    target_rn_ratio: None,
                    provenance: Provenance::default(),
                    note: None,
                },
                Unit {
                    id: "uB4".into(),
                    name: "B4".into(),
                    service: "med".into(),
                    service_line: None,
                    unit_type: UnitType::MedSurg,
                    building: "B".into(),
                    level: FloorLevel::Numbered(4),
                    elevator_core: None,
                    target_rn_ratio: None,
                    provenance: Provenance::default(),
                    note: None,
                },
            ],
            rooms: vec![
                Room {
                    id: "rA".into(),
                    number: "301".into(),
                    unit: "uA3".into(),
                    kind: RoomKind::Inpatient,
                    telemetry_capable: false,
                    negative_pressure: false,
                    status: RoomStatus::Vacant,
                    provenance: Provenance::default(),
                },
                Room {
                    id: "rB".into(),
                    number: "401".into(),
                    unit: "uB4".into(),
                    kind: RoomKind::Inpatient,
                    telemetry_capable: false,
                    negative_pressure: false,
                    status: RoomStatus::Vacant,
                    provenance: Provenance::default(),
                },
            ],
            connections: vec![
                Connection {
                    from: "A".into(),
                    to: "B".into(),
                    kind: ConnectionKind::Bridge,
                    name: Some("A↔B skybridge".into()),
                    layout: Some("bridge.ab".into()),
                    levels: vec![2],
                    provenance: Provenance {
                        confidence: Confidence::Verified,
                        ..Default::default()
                    },
                    note: None,
                },
                Connection {
                    from: "A".into(),
                    to: "B".into(),
                    kind: ConnectionKind::Tunnel,
                    name: None,
                    layout: None,
                    levels: vec![],
                    provenance: Provenance {
                        confidence: Confidence::Unverified,
                        ..Default::default()
                    },
                    note: None,
                },
            ],
            elevator_cores: vec![
                ElevatorCore {
                    name: "A core".into(),
                    building: Some("A".into()),
                    serves_patient_floors: vec![2, 3],
                    note: None,
                },
                ElevatorCore {
                    name: "B core".into(),
                    building: Some("B".into()),
                    serves_patient_floors: vec![2, 3, 4],
                    note: None,
                },
            ],
            service_lines: vec![],
        };
        let layout = CampusLayout {
            origin_lat: 0.0,
            origin_lon: 0.0,
            buildings: vec![
                BuildingLayout {
                    node: "A".into(),
                    footprint: footprint(0.0, 0.0),
                    levels: 4,
                    floor_height_m: 4.0,
                    ground_elevation_m: 0.0,
                    provenance: Provenance::default(),
                },
                BuildingLayout {
                    node: "B".into(),
                    footprint: footprint(120.0, 0.0),
                    levels: 5,
                    floor_height_m: 4.0,
                    ground_elevation_m: 0.0,
                    provenance: Provenance::default(),
                },
                BuildingLayout {
                    node: "C".into(),
                    footprint: footprint(500.0, 500.0),
                    levels: 2,
                    floor_height_m: 4.0,
                    ground_elevation_m: 0.0,
                    provenance: Provenance::default(),
                },
            ],
            ground_plates: vec![],
            bridges: vec![Bridge {
                id: "bridge.ab".into(),
                from: "A".into(),
                to: "B".into(),
                level: 2,
                deck_height_m: 8.0,
                vertices: vec![[10.0, -2.0], [110.0, -2.0], [110.0, 2.0], [10.0, 2.0]],
                provenance: Provenance::default(),
            }],
            insets: vec![],
            notes: vec![],
        };
        (h, layout)
    }

    fn graph(policy: TraversalPolicy) -> (Hospital, CampusLayout, RouteGraph) {
        let (h, layout) = world();
        let g = RouteGraph::build(&h, &layout, RouteParams::default(), policy);
        (h, layout, g)
    }

    #[test]
    fn known_pair_takes_elevator_bridge_elevator() {
        let (h, _l, g) = graph(TraversalPolicy::default());
        let idx = HospitalIndex::build(&h);
        let r = g
            .route_rooms(&h, &idx, &"rA".into(), &"rB".into())
            .expect("route A3 → B4");
        let kinds: Vec<RouteEdgeKind> = r.hops.iter().map(|hop| hop.kind).collect();
        assert_eq!(
            kinds,
            vec![
                RouteEdgeKind::Elevator,
                RouteEdgeKind::Bridge,
                RouteEdgeKind::Elevator
            ],
            "A3 →(elev)→ A2 →(bridge)→ B2 →(elev)→ B4, got {kinds:?}"
        );
        assert!(r.minutes > 0.0);
        // Deck is 100 m; elevator legs are wait+ride constants.
        assert!((r.meters - 100.0).abs() < 1.0, "meters = {}", r.meters);
        assert_eq!(r.via_summary(), "via A↔B skybridge");
    }

    #[test]
    fn all_edge_weights_positive() {
        let (_h, _l, g) = graph(TraversalPolicy {
            include_unverified: true,
        });
        assert!(!g.edges.is_empty());
        for e in &g.edges {
            assert!(e.minutes > 0.0, "non-positive weight on {:?}", e.kind);
        }
    }

    #[test]
    fn unverified_tunnel_excluded_by_default_included_by_policy() {
        let (_h, _l, g) = graph(TraversalPolicy::default());
        assert!(
            !g.edges.iter().any(|e| e.kind == RouteEdgeKind::Tunnel),
            "tunnel must be excluded by default"
        );
        let (_h, _l, g) = graph(TraversalPolicy {
            include_unverified: true,
        });
        let tunnel = g
            .edges
            .iter()
            .find(|e| e.kind == RouteEdgeKind::Tunnel)
            .expect("tunnel present under include_unverified");
        assert_eq!(tunnel.confidence, Confidence::Unverified);
    }

    #[test]
    fn unreachable_pair_reported_not_panicked() {
        let (h, _l, g) = graph(TraversalPolicy::default());
        let a = g.node_index(&"A".into(), &FloorLevel::Numbered(3)).unwrap();
        let c = g.node_index(&"C".into(), &FloorLevel::Numbered(1)).unwrap();
        assert!(g.route(a, c).is_none());
        let _ = h;
    }

    #[test]
    fn same_node_transfer_costs_same_floor_min() {
        let (mut h, layout) = world();
        // Second room in the same unit.
        h.rooms.push(Room {
            id: "rA2".into(),
            number: "302".into(),
            unit: "uA3".into(),
            kind: RoomKind::Inpatient,
            telemetry_capable: false,
            negative_pressure: false,
            status: RoomStatus::Vacant,
            provenance: Provenance::default(),
        });
        let g = RouteGraph::build(
            &h,
            &layout,
            RouteParams::default(),
            TraversalPolicy::default(),
        );
        let idx = HospitalIndex::build(&h);
        let r = g.route_rooms(&h, &idx, &"rA".into(), &"rA2".into()).unwrap();
        assert!(r.hops.is_empty());
        assert_eq!(r.minutes, g.params.same_floor_min);
        assert_eq!(r.via_summary(), "direct");
    }

    #[test]
    fn podium_connects_plate_buildings_at_ground() {
        let (h, mut layout) = world();
        // A plate under A and B (not C).
        layout.ground_plates.push(GroundPlate {
            footprint: Footprint {
                vertices: vec![[-20.0, -20.0], [140.0, -20.0], [140.0, 20.0], [-20.0, 20.0]],
            },
            label: None,
            extrude_m: Some(3.5),
            provenance: Provenance::default(),
        });
        let g = RouteGraph::build(
            &h,
            &layout,
            RouteParams::default(),
            TraversalPolicy::default(),
        );
        let a = g.node_index(&"A".into(), &FloorLevel::Ground).unwrap();
        let b = g.node_index(&"B".into(), &FloorLevel::Ground).unwrap();
        let r = g.route(a, b).expect("podium ground hop");
        assert_eq!(r.hops.len(), 1);
        assert_eq!(r.hops[0].kind, RouteEdgeKind::Podium);
        assert_eq!(r.worst_confidence, Confidence::Inferred);
        assert!(r.inferred_weights);
    }
}
