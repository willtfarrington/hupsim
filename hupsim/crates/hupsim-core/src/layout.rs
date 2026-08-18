//! 3D massing layout: building footprints in a local ENU meter frame.
//! Architecturally simplified (extruded prisms), topologically accurate
//! (real relative positions, adjacency preserved). Geometry is surveyed
//! from the KML hand-mapping (EP-2).

use crate::ids::NodeId;
use crate::provenance::Provenance;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Footprint {
    /// Polygon vertices in meters, CCW, local frame (x = east, y = north).
    pub vertices: Vec<[f32; 2]>,
}

impl Footprint {
    pub fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self {
            vertices: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
        }
    }

    pub fn centroid(&self) -> [f32; 2] {
        let n = self.vertices.len().max(1) as f32;
        let (sx, sy) = self
            .vertices
            .iter()
            .fold((0.0f32, 0.0f32), |(sx, sy), v| (sx + v[0], sy + v[1]));
        [sx / n, sy / n]
    }

    /// Area-weighted centroid via the shoelace formula. Unlike the vertex
    /// mean ([`centroid`](Self::centroid)), this is insensitive to uneven
    /// vertex density and stays at the center of mass of concave outlines
    /// (the vertex mean of PCAM's hand-trace lands outside the polygon).
    pub fn area_centroid(&self) -> [f32; 2] {
        let v = &self.vertices;
        let n = v.len();
        if n < 3 {
            return self.centroid();
        }
        let (mut a2, mut cx, mut cy) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..n {
            let j = (i + 1) % n;
            let w = v[i][0] * v[j][1] - v[j][0] * v[i][1];
            a2 += w;
            cx += (v[i][0] + v[j][0]) * w;
            cy += (v[i][1] + v[j][1]) * w;
        }
        if a2.abs() < 1e-6 {
            return self.centroid();
        }
        [cx / (3.0 * a2), cy / (3.0 * a2)]
    }

    /// Signed area (CCW positive) via the shoelace formula.
    pub fn signed_area(&self) -> f32 {
        let v = &self.vertices;
        let n = v.len();
        if n < 3 {
            return 0.0;
        }
        let mut a = 0.0;
        for i in 0..n {
            let j = (i + 1) % n;
            a += v[i][0] * v[j][1] - v[j][0] * v[i][1];
        }
        a * 0.5
    }

    /// Ensure counter-clockwise winding (required by the extruder).
    pub fn ensure_ccw(&mut self) {
        if self.signed_area() < 0.0 {
            self.vertices.reverse();
        }
    }

    /// Even-odd (ray-cast) point-in-polygon test, winding-agnostic. Points
    /// exactly on the boundary are not guaranteed either way; callers that
    /// need containment keep a margin.
    pub fn contains_point(&self, p: [f32; 2]) -> bool {
        let v = &self.vertices;
        let n = v.len();
        if n < 3 {
            return false;
        }
        let mut inside = false;
        let mut j = n - 1;
        for i in 0..n {
            let (xi, yi) = (v[i][0], v[i][1]);
            let (xj, yj) = (v[j][0], v[j][1]);
            if (yi > p[1]) != (yj > p[1]) {
                let x_cross = xi + (p[1] - yi) / (yj - yi) * (xj - xi);
                if p[0] < x_cross {
                    inside = !inside;
                }
            }
            j = i;
        }
        inside
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildingLayout {
    /// Must match a `Building::id` in the hospital model.
    pub node: NodeId,
    pub footprint: Footprint,
    /// Total above-grade stories to extrude, including the shared
    /// street-level ground story (numbered floors run 1..=levels-1 for
    /// buildings without a topology floor_span).
    pub levels: u32,
    #[serde(default = "default_floor_height")]
    pub floor_height_m: f32,
    #[serde(default)]
    pub ground_elevation_m: f32,
    #[serde(default)]
    pub provenance: Provenance,
}

fn default_floor_height() -> f32 {
    4.0
}

/// A flat reference footprint drawn at ground level (e.g. the true OSM
/// outline of the HUP Main blob beneath the simplified per-wing massing).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroundPlate {
    pub footprint: Footprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Extrusion height in meters (e.g. the HUP Main connective podium).
    /// None keeps the default thin-plinth rendering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extrude_m: Option<f32>,
    #[serde(default)]
    pub provenance: Provenance,
}

/// A skybridge deck polygon connecting two buildings. Geometry lives here
/// (the meters-frame document); the topology graph references a bridge by
/// `id` — two parallel Clifton↔PCAM bridges share the same (from, to) pair,
/// so the pair alone is ambiguous.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bridge {
    /// Stable id, e.g. `bridge.silverstein_clifton`.
    pub id: String,
    pub from: NodeId,
    pub to: NodeId,
    /// Floor level the deck connects (building-local numbering).
    pub level: u32,
    /// Deck height above grade in meters.
    pub deck_height_m: f32,
    /// Deck outline in meters, CCW, local frame (x = east, y = north).
    pub vertices: Vec<[f32; 2]>,
    #[serde(default)]
    pub provenance: Provenance,
}

impl Bridge {
    /// Ensure the deck outline winds counter-clockwise (required by the extruder).
    pub fn ensure_ccw(&mut self) {
        let mut f = Footprint {
            vertices: std::mem::take(&mut self.vertices),
        };
        f.ensure_ccw();
        self.vertices = f.vertices;
    }
}

/// A detached-site annotation drawn as an inset patch. Unused since the
/// out-of-scope sites were archived (EP-1); kept so those layouts parse
/// again if scope re-expands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsetAnnotation {
    /// Building ids drawn inside this inset.
    pub nodes: Vec<NodeId>,
    /// Label explaining the break, e.g. "≈4.8 km SW — no physical connection".
    pub label: String,
    /// Center of the inset patch in the (display) local frame.
    pub center: [f32; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CampusLayout {
    /// Geographic origin of the local frame.
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub buildings: Vec<BuildingLayout>,
    #[serde(default)]
    pub ground_plates: Vec<GroundPlate>,
    /// Skybridge decks (EP-2+). Defaulted so pre-bridge layouts still parse.
    #[serde(default)]
    pub bridges: Vec<Bridge>,
    #[serde(default)]
    pub insets: Vec<InsetAnnotation>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl CampusLayout {
    pub fn building(&self, node: &NodeId) -> Option<&BuildingLayout> {
        self.buildings.iter().find(|b| &b.node == node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_is_ccw() {
        let f = Footprint::rect(0.0, 0.0, 10.0, 5.0);
        assert!(f.signed_area() > 0.0);
        assert_eq!(f.signed_area(), 50.0);
    }

    #[test]
    fn ensure_ccw_flips_cw() {
        let mut f = Footprint {
            vertices: vec![[0.0, 0.0], [0.0, 5.0], [10.0, 5.0], [10.0, 0.0]],
        };
        assert!(f.signed_area() < 0.0);
        f.ensure_ccw();
        assert!(f.signed_area() > 0.0);
    }

    #[test]
    fn area_centroid_of_concave_l_shape() {
        // L = 10×4 bar (area 40, centroid (5,2)) + 4×6 stem (area 24,
        // centroid (2,7)) → (40·(5,2) + 24·(2,7)) / 64 = (3.875, 3.875).
        let l = Footprint {
            vertices: vec![
                [0.0, 0.0],
                [10.0, 0.0],
                [10.0, 4.0],
                [4.0, 4.0],
                [4.0, 10.0],
                [0.0, 10.0],
            ],
        };
        let c = l.area_centroid();
        assert!((c[0] - 3.875).abs() < 1e-4);
        assert!((c[1] - 3.875).abs() < 1e-4);
        // The vertex mean is a different (worse) point on this shape.
        let m = l.centroid();
        assert!((m[0] - c[0]).abs() > 0.5);
    }

    #[test]
    fn contains_point_on_concave_l_shape() {
        let l = Footprint {
            vertices: vec![
                [0.0, 0.0],
                [10.0, 0.0],
                [10.0, 4.0],
                [4.0, 4.0],
                [4.0, 10.0],
                [0.0, 10.0],
            ],
        };
        assert!(l.contains_point([5.0, 2.0])); // in the bar
        assert!(l.contains_point([2.0, 7.0])); // in the stem
        assert!(!l.contains_point([7.0, 7.0])); // in the concave notch
        assert!(!l.contains_point([-1.0, 5.0])); // outside left
        assert!(!l.contains_point([5.0, 4.5])); // just above the bar, right of stem
        assert!(l.contains_point([3.9, 9.9])); // near an inner corner, inside
        // Winding-agnostic: a CW copy answers identically.
        let mut cw = l.clone();
        cw.vertices.reverse();
        assert!(cw.contains_point([5.0, 2.0]));
        assert!(!cw.contains_point([7.0, 7.0]));
    }
}
