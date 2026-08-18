//! EP-9 benchmark: scalar vs rayon-parallel `RoomMatrix` transforms on the
//! real compiled hospital (1,115 rooms) and a synthetic ~50k-room world (the
//! "scales to a health system" datapoint).
//!
//! Not a correctness test (those live in hupsim-core); `#[ignore]`d so it
//! never taxes the normal suite. Run it with:
//!
//! ```text
//! cargo test -p hupsim-data --release --features parallel -- --ignored --nocapture
//! ```
//!
//! Without `--features parallel` it still runs and prints scalar-only
//! timings. Times are best-of-N wall clock (µs); the par/scalar cross-checks
//! assert integer-exact equality and last-ulp-tolerant float equality.

use hupsim_core::index::HospitalIndex;
use hupsim_core::matrix::{OccState, RoomMatrix};
use hupsim_core::model::Hospital;
use std::hint::black_box;
use std::time::Instant;

/// Best-of-`iters` wall time in µs, after two warmup calls.
fn best_us(iters: usize, mut f: impl FnMut()) -> f64 {
    for _ in 0..2 {
        f();
    }
    let mut best = f64::INFINITY;
    for _ in 0..iters {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64() * 1e6);
    }
    best
}

/// The real compiled hospital with the deterministic demo census seeded.
fn real_world() -> (Hospital, HospitalIndex) {
    let (topo, units, layout, lines) = hupsim_data::io::embedded();
    let (mut h, _) = hupsim_data::compile(topo, units, layout, lines).expect("embedded compiles");
    let idx = HospitalIndex::build(&h);
    hupsim_data::seed_demo_census(&mut h, &idx, 0xC0FFEE, 0.0);
    (h, idx)
}

/// Replicate the real rooms `factor`× (fresh ids, same units/statuses) —
/// unit count and occupancy mix stay realistic while rows scale.
fn synthetic_world(factor: usize) -> (Hospital, HospitalIndex) {
    let (mut h, _) = real_world();
    let originals = h.rooms.clone();
    for k in 1..factor {
        h.rooms.extend(originals.iter().map(|r| {
            let mut r = r.clone();
            r.id = format!("rep{k}.{}", r.id).into();
            r
        }));
    }
    let idx = HospitalIndex::build(&h);
    (h, idx)
}

struct Row {
    transform: &'static str,
    scalar_us: f64,
    par_us: Option<f64>,
}

/// Time a `par_*` expression, or `None` without the feature — a macro so the
/// expression is never even typechecked in a scalar-only build.
macro_rules! par_bench {
    ($e:expr) => {{
        #[cfg(feature = "parallel")]
        {
            Some(best_us(100, || {
                black_box($e);
            }))
        }
        #[cfg(not(feature = "parallel"))]
        {
            None
        }
    }};
}

fn bench_world(h: &Hospital, idx: &HospitalIndex) -> Vec<Row> {
    let n_units = h.units.len();
    let n_lines = h.service_lines.len();
    let mut m = RoomMatrix::build(h, idx);
    let mut rows = Vec::new();

    rows.push(Row {
        transform: "build (full)",
        scalar_us: best_us(20, || {
            black_box(RoomMatrix::build(black_box(h), black_box(idx)));
        }),
        par_us: None,
    });
    rows.push(Row {
        transform: "refresh (fast path)",
        scalar_us: best_us(100, || {
            black_box(m.refresh(black_box(h), black_box(idx)));
        }),
        par_us: None,
    });

    let m = RoomMatrix::build(h, idx);
    rows.push(Row {
        transform: "unit_aggregates",
        scalar_us: best_us(100, || {
            black_box(m.unit_aggregates(n_units));
        }),
        par_us: par_bench!(m.par_unit_aggregates(n_units)),
    });
    rows.push(Row {
        transform: "rollup_by_unit_type",
        scalar_us: best_us(100, || {
            black_box(m.rollup_by_unit_type());
        }),
        par_us: par_bench!(m.par_rollup_by_unit_type()),
    });
    rows.push(Row {
        transform: "rollup_by_service_line",
        scalar_us: best_us(100, || {
            black_box(m.rollup_by_service_line(n_lines));
        }),
        par_us: par_bench!(m.par_rollup_by_service_line(n_lines)),
    });
    rows.push(Row {
        transform: "hospital_stats",
        scalar_us: best_us(100, || {
            black_box(m.hospital_stats());
        }),
        par_us: par_bench!(m.par_hospital_stats()),
    });
    rows.push(Row {
        transform: "count_by_unit (vacant)",
        scalar_us: best_us(100, || {
            black_box(m.count_by_unit(n_units, |i| m.occupancy[i] == OccState::Vacant));
        }),
        par_us: par_bench!(m.par_count_by_unit(n_units, |i| m.occupancy[i] == OccState::Vacant)),
    });
    rows.push(Row {
        transform: "masked_mean_std (instability)",
        scalar_us: best_us(100, || {
            let mask = m.occupied_mask();
            black_box(hupsim_core::matrix::masked_mean_std(&m.instability, &mask));
        }),
        par_us: None,
    });

    cross_check(&m, n_units, n_lines);
    rows
}

#[cfg(feature = "parallel")]
fn cross_check(m: &RoomMatrix, n_units: usize, n_lines: usize) {
    let (s, p) = (m.unit_aggregates(n_units), m.par_unit_aggregates(n_units));
    for (a, b) in s.iter().zip(&p) {
        assert_eq!(
            (a.capacity, a.census, a.out_of_service, a.boarding),
            (b.capacity, b.census, b.out_of_service, b.boarding)
        );
        let close = |x: Option<f32>, y: Option<f32>| match (x, y) {
            (Some(x), Some(y)) => (x - y).abs() < 1e-4,
            (None, None) => true,
            _ => false,
        };
        assert!(close(a.mean_instability, b.mean_instability));
        assert!(close(a.worst_instability, b.worst_instability));
    }
    assert_eq!(m.hospital_stats(), m.par_hospital_stats());
    assert_eq!(
        m.count_by_unit(n_units, |i| m.occupancy[i] == OccState::Vacant),
        m.par_count_by_unit(n_units, |i| m.occupancy[i] == OccState::Vacant)
    );
    let _ = n_lines;
}

#[cfg(not(feature = "parallel"))]
fn cross_check(_m: &RoomMatrix, _n_units: usize, _n_lines: usize) {}

fn print_table(title: &str, n_rooms: usize, rows: &[Row]) {
    println!("\n### {title} — {n_rooms} rooms\n");
    println!("| transform | scalar (µs) | parallel (µs) | speedup |");
    println!("|---|---:|---:|---:|");
    for r in rows {
        match r.par_us {
            Some(p) => println!(
                "| {} | {:.1} | {:.1} | {:.2}× |",
                r.transform,
                r.scalar_us,
                p,
                r.scalar_us / p
            ),
            None => println!("| {} | {:.1} | — | — |", r.transform, r.scalar_us),
        }
    }
}

#[test]
#[ignore = "timing benchmark — run: cargo test -p hupsim-data --release --features parallel -- --ignored --nocapture"]
fn matrix_transform_benchmark() {
    let (h, idx) = real_world();
    assert!(
        h.rooms.len() > 1000,
        "expected the real ~1,115-room hospital, got {}",
        h.rooms.len()
    );
    let rows = bench_world(&h, &idx);
    print_table("Real hospital", h.rooms.len(), &rows);

    let (h50, idx50) = synthetic_world(40);
    let rows50 = bench_world(&h50, &idx50);
    print_table("Synthetic health system", h50.rooms.len(), &rows50);

    println!(
        "\n(parallel feature: {}; best-of-N wall clock; see EP-9 brief for the recorded run)",
        cfg!(feature = "parallel")
    );
}
