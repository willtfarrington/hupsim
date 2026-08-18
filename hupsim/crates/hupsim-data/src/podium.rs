//! Podium footprint derivation (EP-7). The connective HUP Main podium plate
//! is computed from the union of the eight `wing.*` footprints rather than
//! hand-traced, so it can never drift from the surveyed wing geometry.
//!
//! Pipeline: rasterize the wing polygons onto a fine grid → exact Euclidean
//! distance transform (Felzenszwalb) → morphological **closing with a net
//! outward buffer** (dilate by `closing + buffer`, erode by `closing`) →
//! flood-fill interior holes → Moore boundary trace → Douglas-Peucker +
//! collinear/short-edge merge → 0.1 m rounding. Closing (not plain dilation)
//! is what makes the outline calm: it fills courtyards and the 9–26 m
//! inter-wing bays that no simplification tolerance could remove, while the
//! outer edge stays at exactly `buffer` meters from the wing walls.
//!
//! Deterministic: same input and params ⇒ byte-identical output. Pure math,
//! no dependencies; tools-feature only (never in the runtime graph).

use hupsim_core::layout::{CampusLayout, Footprint};
use std::fmt::Write as _;

/// Tuning knobs for the derivation. Defaults are the committed values —
/// change them only via `podium_gen` and re-pin the downstream tests.
#[derive(Debug, Clone)]
pub struct PodiumParams {
    /// Raster cell size in meters.
    pub cell_m: f64,
    /// Net outward buffer from the wing walls.
    pub buffer_m: f64,
    /// Morphological closing radius (bay/courtyard fill scale).
    pub closing_m: f64,
    /// Douglas-Peucker tolerance.
    pub simplify_eps_m: f64,
    /// Vertices deviating from straight by less than this are merged away.
    pub collinear_max_deg: f64,
    /// Edges shorter than this are merged away.
    pub min_edge_m: f64,
}

impl Default for PodiumParams {
    fn default() -> Self {
        Self {
            cell_m: 0.25,
            buffer_m: 3.0,
            closing_m: 15.0,
            simplify_eps_m: 1.5,
            collinear_max_deg: 5.0,
            min_edge_m: 3.0,
        }
    }
}

/// Every wing vertex must sit at least this far inside the derived ring.
const MIN_CLEARANCE_M: f64 = 1.0;
/// Hard cap on output complexity — "clean, not overfitted".
const MAX_VERTICES: usize = 28;

#[derive(Debug)]
pub struct PodiumOutcome {
    /// The derived plate outline: simple, CCW, 0.1 m-rounded.
    pub ring: Vec<[f32; 2]>,
    /// Raster area of the bare wing union (pre-closing), m².
    pub union_area_m2: f64,
    /// Shoelace area of the final ring, m².
    pub area_m2: f64,
    /// Smallest distance from any wing boundary sample (≤1 m spacing along
    /// every wing edge) to the ring boundary.
    pub min_wing_clearance_m: f64,
    /// Markdown provenance/repro report for `assets/data/podium_report.md`.
    pub report: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PodiumError {
    #[error("layout contains no wing.* buildings to derive the podium from")]
    NoWings,
    #[error("podium raster has {0} connected components, expected 1 (raise closing_m)")]
    NotSingleComponent(usize),
    #[error("simplified podium ring self-intersects")]
    SelfIntersecting,
    #[error(
        "wing boundary point ({x:.1}, {y:.1}) of {node} ends up outside the podium ring \
         or closer than {min:.1} m to its edge (clearance {clearance:.2} m)"
    )]
    WingBoundaryOutside { node: String, x: f32, y: f32, clearance: f64, min: f64 },
    #[error("simplified ring has {0} vertices, max {MAX_VERTICES} — raise closing_m or simplify_eps_m")]
    TooManyVertices(usize),
    #[error("ring edge of {len:.2} m is shorter than min_edge_m after simplification")]
    EdgeTooShort { len: f64 },
    #[error("final ring area {area:.0} m² is smaller than the wing union {union:.0} m²")]
    AreaShrank { area: f64, union: f64 },
}

/// Derive the podium plate ring from `layout`'s `wing.*` footprints.
pub fn derive_podium(
    layout: &CampusLayout,
    params: &PodiumParams,
) -> Result<PodiumOutcome, PodiumError> {
    let wings: Vec<(&str, Vec<[f64; 2]>)> = layout
        .buildings
        .iter()
        .filter(|b| b.node.as_str().starts_with("wing."))
        .map(|b| {
            let poly = b.footprint.vertices.iter().map(|v| [v[0] as f64, v[1] as f64]).collect();
            (b.node.as_str(), poly)
        })
        .collect();
    if wings.is_empty() {
        return Err(PodiumError::NoWings);
    }

    // --- raster frame: bbox padded past the largest morphological radius ---
    let cell = params.cell_m;
    let dilate_r = params.closing_m + params.buffer_m;
    let pad = dilate_r + 3.0 * cell;
    let (mut minx, mut miny, mut maxx, mut maxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (_, poly) in &wings {
        for p in poly {
            minx = minx.min(p[0]);
            miny = miny.min(p[1]);
            maxx = maxx.max(p[0]);
            maxy = maxy.max(p[1]);
        }
    }
    let (x0, y0) = (minx - pad, miny - pad);
    let nx = ((maxx + pad - x0) / cell).ceil() as usize + 1;
    let ny = ((maxy + pad - y0) / cell).ceil() as usize + 1;
    let center = |i: usize, j: usize| [x0 + (i as f64 + 0.5) * cell, y0 + (j as f64 + 0.5) * cell];

    // --- rasterize the union of wing polygons (cell centers, even-odd) ---
    let mut occ = vec![false; nx * ny];
    for (_, poly) in &wings {
        let (mut px0, mut py0, mut px1, mut py1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in poly {
            px0 = px0.min(p[0]);
            py0 = py0.min(p[1]);
            px1 = px1.max(p[0]);
            py1 = py1.max(p[1]);
        }
        let i0 = (((px0 - x0) / cell).floor().max(0.0)) as usize;
        let j0 = (((py0 - y0) / cell).floor().max(0.0)) as usize;
        let i1 = (((px1 - x0) / cell).ceil() as usize).min(nx - 1);
        let j1 = (((py1 - y0) / cell).ceil() as usize).min(ny - 1);
        for j in j0..=j1 {
            for i in i0..=i1 {
                if !occ[j * nx + i] && point_in_poly(center(i, j), poly) {
                    occ[j * nx + i] = true;
                }
            }
        }
    }
    let union_area_m2 = occ.iter().filter(|&&b| b).count() as f64 * cell * cell;
    if union_area_m2 == 0.0 {
        return Err(PodiumError::NoWings);
    }

    // --- closing with net buffer: dilate(closing+buffer) then erode(closing) ---
    let d2 = distance_sq_grid(&occ, nx, ny);
    let r1 = dilate_r / cell;
    let dilated: Vec<bool> = d2.iter().map(|&d| d.sqrt() <= r1).collect();
    let inv: Vec<bool> = dilated.iter().map(|&b| !b).collect();
    let d2i = distance_sq_grid(&inv, nx, ny);
    let r2 = params.closing_m / cell;
    let mut closed: Vec<bool> = d2i.iter().map(|&d| d.sqrt() > r2).collect();

    // --- fill interior holes (anything not flood-reachable from the border) ---
    fill_holes(&mut closed, nx, ny);

    let components = count_components(&closed, nx, ny);
    if components != 1 {
        return Err(PodiumError::NotSingleComponent(components));
    }

    // --- trace + simplify ---
    let contour = trace_boundary(&closed, nx, ny);
    let world: Vec<[f64; 2]> = contour.iter().map(|&(i, j)| center(i, j)).collect();
    let mut ring = simplify_ring(world, params);

    // CCW + 0.1 m rounding (the project-wide coordinate convention).
    if shoelace(&ring) < 0.0 {
        ring.reverse();
    }
    let ring_f32: Vec<[f32; 2]> = ring
        .iter()
        .map(|p| [((p[0] * 10.0).round() / 10.0) as f32, ((p[1] * 10.0).round() / 10.0) as f32])
        .collect();

    // --- validation: the tool must never emit junk ---
    if ring_f32.len() > MAX_VERTICES {
        return Err(PodiumError::TooManyVertices(ring_f32.len()));
    }
    let ring64: Vec<[f64; 2]> = ring_f32.iter().map(|p| [p[0] as f64, p[1] as f64]).collect();
    let n = ring64.len();
    for i in 0..n {
        let len = dist(ring64[i], ring64[(i + 1) % n]);
        if len < params.min_edge_m * 0.66 {
            return Err(PodiumError::EdgeTooShort { len });
        }
    }
    if ring_self_intersects(&ring64) {
        return Err(PodiumError::SelfIntersecting);
    }
    let fp = Footprint { vertices: ring_f32.clone() };
    let mut min_clearance = f64::MAX;
    // Simplification (DP + collinear/short-edge merges) can pull the ring
    // inward BETWEEN wing vertices, so containment is checked along the whole
    // wing boundary at ≤1 m sampling, not just at the vertices.
    for (node, poly) in &wings {
        let np = poly.len();
        for i in 0..np {
            let (a, b) = (poly[i], poly[(i + 1) % np]);
            let steps = (dist(a, b) / MIN_CLEARANCE_M).ceil().max(1.0) as usize;
            for s in 0..steps {
                let t = s as f64 / steps as f64;
                let p = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
                let inside = fp.contains_point([p[0] as f32, p[1] as f32]);
                let clearance = ring_distance(&ring64, p);
                if !inside || clearance < MIN_CLEARANCE_M {
                    return Err(PodiumError::WingBoundaryOutside {
                        node: node.to_string(),
                        x: p[0] as f32,
                        y: p[1] as f32,
                        clearance: if inside { clearance } else { -clearance },
                        min: MIN_CLEARANCE_M,
                    });
                }
                min_clearance = min_clearance.min(clearance);
            }
        }
    }
    let area_m2 = shoelace(&ring64);
    if area_m2 < union_area_m2 {
        return Err(PodiumError::AreaShrank { area: area_m2, union: union_area_m2 });
    }

    let report = build_report(params, &wings, union_area_m2, area_m2, min_clearance, &ring_f32);
    Ok(PodiumOutcome { ring: ring_f32, union_area_m2, area_m2, min_wing_clearance_m: min_clearance, report })
}

// ---------------------------------------------------------------------------
// Raster machinery
// ---------------------------------------------------------------------------

fn point_in_poly(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let n = poly.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if (yi > p[1]) != (yj > p[1]) && p[0] < xi + (p[1] - yi) / (yj - yi) * (xj - xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Exact squared Euclidean distance (in cells) to the nearest `true` cell,
/// via Felzenszwalb–Huttenlocher's two-pass 1D transform.
fn distance_sq_grid(occ: &[bool], nx: usize, ny: usize) -> Vec<f64> {
    const INF: f64 = 1e20;
    let m = nx.max(ny);
    let (mut f, mut d) = (vec![0.0f64; m], vec![0.0f64; m]);
    let mut tmp = vec![0.0f64; nx * ny];
    for i in 0..nx {
        for j in 0..ny {
            f[j] = if occ[j * nx + i] { 0.0 } else { INF };
        }
        dt1d(&f[..ny], &mut d[..ny]);
        for j in 0..ny {
            tmp[j * nx + i] = d[j];
        }
    }
    let mut out = vec![0.0f64; nx * ny];
    for j in 0..ny {
        f[..nx].copy_from_slice(&tmp[j * nx..j * nx + nx]);
        dt1d(&f[..nx], &mut d[..nx]);
        out[j * nx..j * nx + nx].copy_from_slice(&d[..nx]);
    }
    out
}

fn dt1d(f: &[f64], d: &mut [f64]) {
    let n = f.len();
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f64; n + 1];
    let mut k = 0usize;
    z[0] = -1e20;
    z[1] = 1e20;
    for q in 1..n {
        loop {
            let vk = v[k] as f64;
            let s = ((f[q] + (q * q) as f64) - (f[v[k]] + vk * vk)) / (2.0 * q as f64 - 2.0 * vk);
            if s <= z[k] {
                k -= 1;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = 1e20;
                break;
            }
        }
    }
    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let dq = q as f64 - v[k] as f64;
        d[q] = dq * dq + f[v[k]];
    }
}

/// Set to `true` every `false` region not flood-reachable from the border.
fn fill_holes(grid: &mut [bool], nx: usize, ny: usize) {
    let mut outside = vec![false; nx * ny];
    let mut stack: Vec<usize> = Vec::new();
    for i in 0..nx {
        for &j in &[0, ny - 1] {
            let k = j * nx + i;
            if !grid[k] && !outside[k] {
                outside[k] = true;
                stack.push(k);
            }
        }
    }
    for j in 0..ny {
        for &i in &[0, nx - 1] {
            let k = j * nx + i;
            if !grid[k] && !outside[k] {
                outside[k] = true;
                stack.push(k);
            }
        }
    }
    while let Some(k) = stack.pop() {
        let (i, j) = (k % nx, k / nx);
        let mut push = |k2: usize| {
            if !grid[k2] && !outside[k2] {
                outside[k2] = true;
                stack.push(k2);
            }
        };
        if i > 0 {
            push(k - 1);
        }
        if i + 1 < nx {
            push(k + 1);
        }
        if j > 0 {
            push(k - nx);
        }
        if j + 1 < ny {
            push(k + nx);
        }
    }
    for k in 0..nx * ny {
        if !grid[k] && !outside[k] {
            grid[k] = true;
        }
    }
}

fn count_components(grid: &[bool], nx: usize, ny: usize) -> usize {
    let mut seen = vec![false; nx * ny];
    let mut stack: Vec<usize> = Vec::new();
    let mut components = 0;
    for start in 0..nx * ny {
        if grid[start] && !seen[start] {
            components += 1;
            seen[start] = true;
            stack.push(start);
            while let Some(k) = stack.pop() {
                let (i, j) = (k % nx, k / nx);
                let mut push = |k2: usize| {
                    if grid[k2] && !seen[k2] {
                        seen[k2] = true;
                        stack.push(k2);
                    }
                };
                if i > 0 {
                    push(k - 1);
                }
                if i + 1 < nx {
                    push(k + 1);
                }
                if j > 0 {
                    push(k - nx);
                }
                if j + 1 < ny {
                    push(k + nx);
                }
            }
        }
    }
    components
}

/// Moore-neighbor boundary trace. Returns boundary cell coordinates in order.
fn trace_boundary(grid: &[bool], nx: usize, ny: usize) -> Vec<(usize, usize)> {
    let at = |x: isize, y: isize| -> bool {
        x >= 0 && y >= 0 && x < nx as isize && y < ny as isize && grid[y as usize * nx + x as usize]
    };
    // First occupied cell scanning y then x: its west neighbor is empty.
    let (mut sx, mut sy) = (0isize, 0isize);
    'outer: for y in 0..ny as isize {
        for x in 0..nx as isize {
            if at(x, y) {
                sx = x;
                sy = y;
                break 'outer;
            }
        }
    }
    // Moore neighborhood in clockwise order (y up), starting west.
    const NB: [(isize, isize); 8] =
        [(-1, 0), (-1, 1), (0, 1), (1, 1), (1, 0), (1, -1), (0, -1), (-1, -1)];
    let (mut cx, mut cy) = (sx, sy);
    let mut back = 0usize; // index into NB pointing at the empty backtrack cell
    let mut contour = vec![(cx as usize, cy as usize)];
    let cap = 4 * nx * ny;
    for _ in 0..cap {
        let mut found = None;
        for s in 1..=8 {
            let idx = (back + s) % 8;
            if at(cx + NB[idx].0, cy + NB[idx].1) {
                found = Some(idx);
                break;
            }
        }
        let Some(fidx) = found else { break }; // isolated single cell
        // The previously-scanned (empty) neighbor becomes the new backtrack;
        // consecutive Moore-ring cells are always adjacent to the new cell.
        let prev_empty = (fidx + 7) % 8;
        let (ex, ey) = (cx + NB[prev_empty].0, cy + NB[prev_empty].1);
        let (nxc, nyc) = (cx + NB[fidx].0, cy + NB[fidx].1);
        let (bdx, bdy) = (ex - nxc, ey - nyc);
        back = NB.iter().position(|&(a, b)| (a, b) == (bdx, bdy)).expect("adjacent");
        cx = nxc;
        cy = nyc;
        // First return to the start cell closes the loop. (Closed shapes here
        // are morphologically fat — no one-cell isthmus revisits the start.)
        if (cx, cy) == (sx, sy) {
            break;
        }
        contour.push((cx as usize, cy as usize));
    }
    contour
}

// ---------------------------------------------------------------------------
// Simplification
// ---------------------------------------------------------------------------

fn simplify_ring(ring: Vec<[f64; 2]>, params: &PodiumParams) -> Vec<[f64; 2]> {
    if ring.len() < 8 {
        return ring;
    }
    // Split the closed ring at its min-x / max-x extremes — both survive DP,
    // so each open chain can be simplified independently.
    let imin = (0..ring.len())
        .min_by(|&a, &b| ring[a][0].total_cmp(&ring[b][0]).then(ring[a][1].total_cmp(&ring[b][1])))
        .unwrap();
    let mut r = ring;
    r.rotate_left(imin);
    let imax = (0..r.len()).max_by(|&a, &b| r[a][0].total_cmp(&r[b][0])).unwrap();
    let mut chain_a = r[..=imax].to_vec();
    let mut chain_b = r[imax..].to_vec();
    chain_b.push(r[0]);
    chain_a = douglas_peucker(&chain_a, params.simplify_eps_m);
    chain_b = douglas_peucker(&chain_b, params.simplify_eps_m);
    let mut out = chain_a;
    out.pop(); // imax kept by chain_b
    out.extend(chain_b);
    out.pop(); // closing duplicate of the start vertex
    collinear_merge(&mut out, params.collinear_max_deg);
    min_edge_merge(&mut out, params.min_edge_m);
    out
}

fn douglas_peucker(pts: &[[f64; 2]], eps: f64) -> Vec<[f64; 2]> {
    let n = pts.len();
    if n <= 2 {
        return pts.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (mut best, mut best_d) = (a, -1.0f64);
        for i in a + 1..b {
            let d = seg_dist(pts[i], pts[a], pts[b]);
            if d > best_d {
                best_d = d;
                best = i;
            }
        }
        if best_d > eps {
            keep[best] = true;
            stack.push((a, best));
            stack.push((best, b));
        }
    }
    pts.iter().zip(&keep).filter(|(_, &k)| k).map(|(p, _)| *p).collect()
}

/// Remove ring vertices whose turn deviates from straight-on by less than
/// `max_deg` (smallest deviation first, until none remain).
fn collinear_merge(ring: &mut Vec<[f64; 2]>, max_deg: f64) {
    let threshold = max_deg.to_radians();
    while ring.len() > 4 {
        let n = ring.len();
        let (mut best, mut best_dev) = (usize::MAX, threshold);
        for i in 0..n {
            let (p, c, q) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
            let a = [c[0] - p[0], c[1] - p[1]];
            let b = [q[0] - c[0], q[1] - c[1]];
            let (la, lb) = ((a[0] * a[0] + a[1] * a[1]).sqrt(), (b[0] * b[0] + b[1] * b[1]).sqrt());
            if la == 0.0 || lb == 0.0 {
                best = i;
                break;
            }
            let cross = (a[0] * b[1] - a[1] * b[0]) / (la * lb);
            let dot = (a[0] * b[0] + a[1] * b[1]) / (la * lb);
            let dev = cross.atan2(dot).abs();
            if dev < best_dev {
                best_dev = dev;
                best = i;
            }
        }
        if best == usize::MAX {
            break;
        }
        ring.remove(best);
    }
}

/// Merge away edges shorter than `min_edge`: for the shortest offending edge,
/// drop whichever endpoint deviates least from the line through its own
/// neighbors.
fn min_edge_merge(ring: &mut Vec<[f64; 2]>, min_edge: f64) {
    while ring.len() > 4 {
        let n = ring.len();
        let (mut worst, mut worst_len) = (usize::MAX, min_edge);
        for i in 0..n {
            let len = dist(ring[i], ring[(i + 1) % n]);
            if len < worst_len {
                worst_len = len;
                worst = i;
            }
        }
        if worst == usize::MAX {
            break;
        }
        let (i, j) = (worst, (worst + 1) % n);
        let dev_i = seg_dist(ring[i], ring[(i + n - 1) % n], ring[j]);
        let dev_j = seg_dist(ring[j], ring[i], ring[(j + 1) % n]);
        ring.remove(if dev_i <= dev_j { i } else { j });
    }
}

// ---------------------------------------------------------------------------
// Small geometry helpers (f64 ring space)
// ---------------------------------------------------------------------------

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Distance from point `p` to segment `ab`.
fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (abx, aby) = (b[0] - a[0], b[1] - a[1]);
    let len2 = abx * abx + aby * aby;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / len2).clamp(0.0, 1.0)
    };
    dist(p, [a[0] + t * abx, a[1] + t * aby])
}

fn shoelace(ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    let mut a = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        a += ring[i][0] * ring[j][1] - ring[j][0] * ring[i][1];
    }
    a * 0.5
}

fn ring_distance(ring: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let n = ring.len();
    (0..n).map(|i| seg_dist(p, ring[i], ring[(i + 1) % n])).fold(f64::MAX, f64::min)
}

fn ring_self_intersects(ring: &[[f64; 2]]) -> bool {
    let n = ring.len();
    for i in 0..n {
        for j in i + 1..n {
            // Skip adjacent segments (they share an endpoint by construction).
            if j == i || (j + 1) % n == i || (i + 1) % n == j {
                continue;
            }
            if segs_intersect(ring[i], ring[(i + 1) % n], ring[j], ring[(j + 1) % n]) {
                return true;
            }
        }
    }
    false
}

fn segs_intersect(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let orient = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| -> f64 {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    let (o1, o2) = (orient(a, b, c), orient(a, b, d));
    let (o3, o4) = (orient(c, d, a), orient(c, d, b));
    if ((o1 > 0.0) != (o2 > 0.0)) && ((o3 > 0.0) != (o4 > 0.0)) {
        return true;
    }
    // Collinear touch/overlap between non-adjacent segments is also a defect.
    let on_seg = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| -> bool {
        orient(p, q, r).abs() < 1e-9
            && r[0] >= p[0].min(q[0]) - 1e-9
            && r[0] <= p[0].max(q[0]) + 1e-9
            && r[1] >= p[1].min(q[1]) - 1e-9
            && r[1] <= p[1].max(q[1]) + 1e-9
    };
    on_seg(a, b, c) || on_seg(a, b, d) || on_seg(c, d, a) || on_seg(c, d, b)
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

fn build_report(
    params: &PodiumParams,
    wings: &[(&str, Vec<[f64; 2]>)],
    union_area_m2: f64,
    area_m2: f64,
    min_clearance: f64,
    ring: &[[f32; 2]],
) -> String {
    let mut r = String::new();
    writeln!(r, "# Podium derivation report").unwrap();
    writeln!(r).unwrap();
    writeln!(
        r,
        "Written by `podium_gen` (EP-7). The podium plate in `layout.json` is the union of the \
         `wing.*` footprints below, morphologically closed and buffered, then simplified. \
         Regenerate with:"
    )
    .unwrap();
    writeln!(r).unwrap();
    writeln!(r, "```").unwrap();
    writeln!(r, "cargo run -p hupsim-data --features tools --bin podium_gen").unwrap();
    writeln!(r, "```").unwrap();
    writeln!(r).unwrap();
    writeln!(r, "## Parameters").unwrap();
    writeln!(r).unwrap();
    writeln!(r, "| cell (m) | buffer (m) | closing (m) | simplify ε (m) | collinear (°) | min edge (m) |").unwrap();
    writeln!(r, "|---|---|---|---|---|---|").unwrap();
    writeln!(
        r,
        "| {} | {} | {} | {} | {} | {} |",
        params.cell_m,
        params.buffer_m,
        params.closing_m,
        params.simplify_eps_m,
        params.collinear_max_deg,
        params.min_edge_m
    )
    .unwrap();
    writeln!(r).unwrap();
    writeln!(r, "## Inputs (surveyed wing footprints, layout.json)").unwrap();
    writeln!(r).unwrap();
    writeln!(r, "| node | vertices | area (m²) |").unwrap();
    writeln!(r, "|---|---|---|").unwrap();
    for (node, poly) in wings {
        writeln!(r, "| `{}` | {} | {:.0} |", node, poly.len(), shoelace(poly).abs()).unwrap();
    }
    writeln!(r).unwrap();
    writeln!(r, "## Result").unwrap();
    writeln!(r).unwrap();
    writeln!(r, "- raster union area (pre-closing): {union_area_m2:.0} m²").unwrap();
    writeln!(r, "- final ring: {} vertices, {area_m2:.0} m²", ring.len()).unwrap();
    writeln!(r, "- min wing-boundary clearance to the ring edge: {min_clearance:.2} m").unwrap();
    writeln!(
        r,
        "- the full wing boundary (sampled at ≤1 m along every edge) is inside the ring \
         (validated at derivation time)"
    )
    .unwrap();
    r
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hupsim_core::ids::NodeId;
    use hupsim_core::layout::BuildingLayout;

    fn layout_of(rects: &[(&str, f32, f32, f32, f32)]) -> CampusLayout {
        CampusLayout {
            origin_lat: 39.949,
            origin_lon: -75.1925,
            buildings: rects
                .iter()
                .map(|&(node, x0, y0, x1, y1)| BuildingLayout {
                    node: NodeId(node.into()),
                    footprint: Footprint::rect(x0, y0, x1, y1),
                    levels: 5,
                    floor_height_m: 4.0,
                    ground_elevation_m: 0.0,
                    provenance: Default::default(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn two_gapped_rects_close_into_one_clean_ring() {
        // 10 m gap < closing radius ⇒ single ring covering both.
        let layout =
            layout_of(&[("wing.a", 0.0, 0.0, 40.0, 20.0), ("wing.b", 50.0, 0.0, 90.0, 20.0)]);
        let out = derive_podium(&layout, &PodiumParams::default()).unwrap();
        assert!(out.ring.len() <= MAX_VERTICES);
        assert!(out.area_m2 > out.union_area_m2);
        let fp = Footprint { vertices: out.ring.clone() };
        assert!(fp.signed_area() > 0.0, "CCW");
        // Both rects and the gap between them are covered.
        for p in [[0.0, 0.0], [90.0, 20.0], [45.0, 10.0]] {
            assert!(fp.contains_point(p), "{p:?} must be inside");
        }
        assert!(out.min_wing_clearance_m >= MIN_CLEARANCE_M);
    }

    #[test]
    fn courtyard_hole_is_filled() {
        // Four 15 m-thick walls around a 30×30 courtyard.
        let layout = layout_of(&[
            ("wing.s", 0.0, 0.0, 60.0, 15.0),
            ("wing.n", 0.0, 45.0, 60.0, 60.0),
            ("wing.w", 0.0, 0.0, 15.0, 60.0),
            ("wing.e", 45.0, 0.0, 60.0, 60.0),
        ]);
        let out = derive_podium(&layout, &PodiumParams::default()).unwrap();
        let fp = Footprint { vertices: out.ring.clone() };
        assert!(fp.contains_point([30.0, 30.0]), "courtyard center must be filled");
    }

    #[test]
    fn derivation_is_deterministic() {
        let layout =
            layout_of(&[("wing.a", 0.0, 0.0, 40.0, 20.0), ("wing.b", 50.0, 0.0, 90.0, 20.0)]);
        let a = derive_podium(&layout, &PodiumParams::default()).unwrap();
        let b = derive_podium(&layout, &PodiumParams::default()).unwrap();
        assert_eq!(a.ring, b.ring);
        assert_eq!(a.report, b.report);
    }

    #[test]
    fn layout_without_wings_is_an_error() {
        let layout = layout_of(&[("bldg.pavilion", 0.0, 0.0, 40.0, 20.0)]);
        assert!(matches!(
            derive_podium(&layout, &PodiumParams::default()),
            Err(PodiumError::NoWings)
        ));
    }
}
