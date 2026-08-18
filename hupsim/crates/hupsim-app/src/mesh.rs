//! Polygon triangulation + prism extrusion for building footprints.
//! Ear clipping handles the concave OSM outlines (Pavilion bar, PCAM, blob).
//! Coordinate mapping: data is ENU (x=east, y=north); Bevy world is
//! x=east, y=up, z=south — so `world = (x, h, -y)`.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};

fn cross(o: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn point_in_triangle(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let d1 = cross(p, a, b);
    let d2 = cross(p, b, c);
    let d3 = cross(p, c, a);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

/// Ear-clipping triangulation of a simple CCW polygon (indices into `pts`).
pub fn triangulate(pts: &[[f32; 2]]) -> Vec<[u32; 3]> {
    let n = pts.len();
    if n < 3 {
        return vec![];
    }
    let mut idx: Vec<u32> = (0..n as u32).collect();
    let mut tris: Vec<[u32; 3]> = Vec::with_capacity(n - 2);

    while idx.len() > 3 {
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let ip = idx[(i + m - 1) % m] as usize;
            let ic = idx[i] as usize;
            let inx = idx[(i + 1) % m] as usize;
            let (a, b, c) = (pts[ip], pts[ic], pts[inx]);
            // Convex corner in a CCW polygon (allow near-collinear as ears).
            if cross(a, b, c) < -1e-6 {
                continue;
            }
            // No other remaining vertex may sit inside the candidate ear.
            let blocked = idx.iter().any(|&j| {
                let j = j as usize;
                j != ip && j != ic && j != inx && point_in_triangle(pts[j], a, b, c)
            });
            if blocked {
                continue;
            }
            tris.push([ip as u32, ic as u32, inx as u32]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // Degenerate input: fall back to a fan so we always render something.
            for i in 1..idx.len() - 1 {
                tris.push([idx[0], idx[i], idx[i + 1]]);
            }
            return tris;
        }
    }
    tris.push([idx[0], idx[1], idx[2]]);
    tris
}

/// Extrude a CCW footprint into a flat-shaded prism spanning `y0..y1` (Bevy Y).
/// Vertices are duplicated per face with explicit normals.
pub fn extrude_polygon(pts: &[[f32; 2]], y0: f32, y1: f32) -> Mesh {
    let tris = triangulate(pts);
    let n = pts.len();
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let world = |p: [f32; 2], y: f32| -> [f32; 3] { [p[0], y, -p[1]] };

    // Top cap (+Y). The ENU→Bevy map (x,y)→(x,h,-y) PRESERVES apparent
    // winding seen from above (+Y), so CCW input triangles are emitted as-is.
    let base = positions.len() as u32;
    for p in pts {
        positions.push(world(*p, y1));
        normals.push([0.0, 1.0, 0.0]);
    }
    for t in &tris {
        indices.extend_from_slice(&[base + t[0], base + t[1], base + t[2]]);
    }

    // Bottom cap (-Y): reversed so it faces down.
    let base = positions.len() as u32;
    for p in pts {
        positions.push(world(*p, y0));
        normals.push([0.0, -1.0, 0.0]);
    }
    for t in &tris {
        indices.extend_from_slice(&[base + t[0], base + t[2], base + t[1]]);
    }

    // Walls: one quad per edge, outward normal = CCW edge rotated right.
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let e = [b[0] - a[0], b[1] - a[1]];
        let len = (e[0] * e[0] + e[1] * e[1]).sqrt().max(1e-6);
        // Outward normal of a CCW polygon edge in ENU: (ey, -ex) / len.
        let nrm_enu = [e[1] / len, -e[0] / len];
        let nrm = [nrm_enu[0], 0.0, -nrm_enu[1]];
        let base = positions.len() as u32;
        positions.push(world(a, y0));
        positions.push(world(b, y0));
        positions.push(world(b, y1));
        positions.push(world(a, y1));
        for _ in 0..4 {
            normals.push(nrm);
        }
        // Outward-facing winding (counter-clockwise seen from outside).
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_indices(Indices::U32(indices))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poly_area(pts: &[[f32; 2]]) -> f32 {
        let n = pts.len();
        let mut a = 0.0;
        for i in 0..n {
            let j = (i + 1) % n;
            a += pts[i][0] * pts[j][1] - pts[j][0] * pts[i][1];
        }
        a * 0.5
    }

    fn tri_area_sum(pts: &[[f32; 2]], tris: &[[u32; 3]]) -> f32 {
        tris.iter()
            .map(|t| {
                let (a, b, c) = (
                    pts[t[0] as usize],
                    pts[t[1] as usize],
                    pts[t[2] as usize],
                );
                cross(a, b, c).abs() * 0.5
            })
            .sum()
    }

    #[test]
    fn square_triangulates_to_two() {
        let sq = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let tris = triangulate(&sq);
        assert_eq!(tris.len(), 2);
        assert!((tri_area_sum(&sq, &tris) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn concave_l_shape_preserves_area() {
        let l = [
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 4.0],
            [4.0, 4.0],
            [4.0, 10.0],
            [0.0, 10.0],
        ];
        let tris = triangulate(&l);
        assert_eq!(tris.len(), 4); // n - 2
        assert!((tri_area_sum(&l, &tris) - poly_area(&l)).abs() < 1e-3);
    }

    #[test]
    fn emitted_winding_matches_authored_normals() {
        // Every emitted triangle's geometric normal must agree with its
        // authored vertex normal — otherwise backface culling hides the face.
        let sq = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let mesh = extrude_polygon(&sq, 0.0, 5.0);
        let pos: Vec<[f32; 3]> = match mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() {
            bevy::mesh::VertexAttributeValues::Float32x3(v) => v.clone(),
            _ => panic!("positions must be f32x3"),
        };
        let nrm: Vec<[f32; 3]> = match mesh.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap() {
            bevy::mesh::VertexAttributeValues::Float32x3(v) => v.clone(),
            _ => panic!("normals must be f32x3"),
        };
        let idx: Vec<u32> = match mesh.indices().unwrap() {
            Indices::U32(v) => v.clone(),
            _ => panic!("u32 indices expected"),
        };
        for tri in idx.chunks(3) {
            let (a, b, c) = (
                bevy::math::Vec3::from(pos[tri[0] as usize]),
                bevy::math::Vec3::from(pos[tri[1] as usize]),
                bevy::math::Vec3::from(pos[tri[2] as usize]),
            );
            let geometric = (b - a).cross(c - a);
            let authored = bevy::math::Vec3::from(nrm[tri[0] as usize]);
            assert!(
                geometric.dot(authored) > 0.0,
                "triangle {tri:?} winds against its authored normal {authored:?}"
            );
        }
    }

    #[test]
    fn pavilion_osm_footprint_triangulates_cleanly() {
        // The real OSM Pavilion polygon from layout.json (concave bar).
        let pav = [
            [140.3, -110.8],
            [121.1, -115.2],
            [120.8, -130.4],
            [104.8, -133.9],
            [76.4, -89.1],
            [-14.7, -5.7],
            [-12.8, 10.3],
            [4.4, 14.4],
            [53.0, -13.4],
            [139.6, -93.9],
        ];
        // ensure CCW first (mirrors compile-time normalization)
        let mut p = pav.to_vec();
        if poly_area(&p) < 0.0 {
            p.reverse();
        }
        let tris = triangulate(&p);
        assert_eq!(tris.len(), p.len() - 2);
        assert!((tri_area_sum(&p, &tris) - poly_area(&p).abs()).abs() / poly_area(&p).abs() < 1e-3);
    }

    #[test]
    fn perelman_imported_footprint_triangulates_cleanly() {
        // "The Perelman pin": the KML hand-trace of PCAM — 25 unique
        // vertices, concave, ~3 m notch — is the worst polygon in the set.
        // It is not stored in layout.json (PCAM keeps its OSM footprint), so
        // derive it exactly as kml_import does: project about the layout
        // origin, drop the closing duplicate, round to 0.1 m, ensure CCW.
        let frame = hupsim_core::geo::GeoFrame::new(39.949, -75.1925);
        // (lon, lat), verbatim from the KML Perelman LinearRing (closing
        // duplicate omitted).
        let ring = [
            (-75.1924880287942, 39.94841345634788),
            (-75.1925637529345, 39.94817991124216),
            (-75.19250944020567, 39.94810959241757),
            (-75.19258411793032, 39.94807809023887),
            (-75.19266644749815, 39.94818565881329),
            (-75.19295608268339, 39.9480568848091),
            (-75.19287537487611, 39.94795157627576),
            (-75.19304462548979, 39.94787527151047),
            (-75.19318466867929, 39.94790451797555),
            (-75.19369757567165, 39.94766851883635),
            (-75.19292269814424, 39.94666564277773),
            (-75.19262735932575, 39.94680069419981),
            (-75.1927093974483, 39.94690538296566),
            (-75.19266461660563, 39.94692564060827),
            (-75.19263241200987, 39.94688846016216),
            (-75.19256850664416, 39.94691859143119),
            (-75.19266469953867, 39.94704300137822),
            (-75.19200758189204, 39.94734029605699),
            (-75.19204640842158, 39.9473905473425),
            (-75.19177939786057, 39.94751058516462),
            (-75.19180725988066, 39.94754748224749),
            (-75.19166302327973, 39.94761309359014),
            (-75.19227620903256, 39.94839881825892),
            (-75.1923226963431, 39.94837709311648),
            (-75.19238133508841, 39.94845494606891),
        ];
        let mut p: Vec<[f32; 2]> = ring
            .iter()
            .map(|&(lon, lat)| {
                let [x, y] = frame.forward(lat, lon);
                [
                    (x * 10.0).round() as f32 / 10.0,
                    (y * 10.0).round() as f32 / 10.0,
                ]
            })
            .collect();
        assert_eq!(p.len(), 25, "25 unique vertices");
        if poly_area(&p) < 0.0 {
            p.reverse();
        }
        let tris = triangulate(&p);
        assert_eq!(tris.len(), p.len() - 2, "no fan fallback on the concave trace");
        assert!((tri_area_sum(&p, &tris) - poly_area(&p).abs()).abs() / poly_area(&p).abs() < 1e-3);
    }

    #[test]
    fn podium_ring_triangulates_cleanly() {
        // "The podium pin" (EP-7): the derived union-podium ring, verbatim
        // from layout.json ground_plates[0]. Concave (inter-wing shoulders
        // survive the closing radius), so it exercises real ear-clipping.
        // If podium_gen is re-run with different parameters this literal goes
        // stale — the failure is the reminder to re-pin it.
        let ring = [
            [-248.3, 87.6],
            [-205.5, 79.3],
            [-195.3, 88.8],
            [-184.5, 90.1],
            [-166.0, 83.3],
            [-158.3, 75.8],
            [-126.8, 69.8],
            [-121.0, 61.3],
            [-121.8, 47.3],
            [-119.8, 44.6],
            [-44.3, 32.1],
            [-42.0, 35.1],
            [-28.0, 118.3],
            [-22.0, 128.1],
            [-17.8, 153.1],
            [-21.0, 157.1],
            [-233.8, 192.3],
            [-236.0, 189.6],
            [-251.3, 90.8],
        ];
        let mut p = ring.to_vec();
        assert_eq!(p.len(), 19, "pinned vertex count");
        if poly_area(&p) < 0.0 {
            p.reverse();
        }
        let tris = triangulate(&p);
        assert_eq!(tris.len(), p.len() - 2, "no fan fallback on the podium ring");
        assert!((tri_area_sum(&p, &tris) - poly_area(&p).abs()).abs() / poly_area(&p).abs() < 1e-3);
    }
}
