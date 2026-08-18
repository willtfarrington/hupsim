//! One-shot podium derivation CLI (see `hupsim_data::podium` for the logic).
//!
//! Replaces the single ground plate in `assets/data/layout.json` with the
//! union-derived podium ring and writes the provenance report next to it:
//!
//! ```text
//! cargo run -p hupsim-data --features tools --bin podium_gen
//! ```
//!
//! Deterministic: re-running against unchanged wing footprints produces a
//! byte-identical layout.json (safe to run as an idempotency check).

use hupsim_core::layout::{CampusLayout, Footprint, GroundPlate};
use hupsim_core::provenance::{Confidence, Era, Provenance};
use hupsim_data::layoutdoc;
use hupsim_data::podium::{derive_podium, PodiumParams};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// One story: the podium is the shared street-level Ground story, leaving
/// the numbered floors (1+) of every wing visible above the rim (the wings
/// share a 3.8 m story height).
const PODIUM_EXTRUDE_M: f32 = 3.5;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("podium_gen: error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // CARGO_MANIFEST_DIR = crates/hupsim-data.
    let ws: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let layout_path = ws.join("assets").join("data").join("layout.json");
    let report_path = ws.join("assets").join("data").join("podium_report.md");

    let mut layout: CampusLayout =
        serde_json::from_str(&std::fs::read_to_string(&layout_path)?)?;
    if layout.ground_plates.len() != 1 {
        return Err(format!(
            "expected exactly 1 ground plate in layout.json, found {} — \
             this tool only knows how to replace the HUP Main podium plate",
            layout.ground_plates.len()
        )
        .into());
    }

    let params = PodiumParams::default();
    let outcome = derive_podium(&layout, &params)?;
    let wing_count =
        layout.buildings.iter().filter(|b| b.node.as_str().starts_with("wing.")).count();

    layout.ground_plates[0] = GroundPlate {
        footprint: Footprint { vertices: outcome.ring.clone() },
        label: Some("HUP Main podium — derived union of the eight wing footprints".into()),
        extrude_m: Some(PODIUM_EXTRUDE_M),
        provenance: Provenance {
            confidence: Confidence::Inferred,
            source: Some(format!(
                "podium_gen: union of the {wing_count} surveyed wing.* footprints, \
                 +{} m buffer, {} m morphological closing, {} m simplify \
                 (see assets/data/podium_report.md)",
                params.buffer_m, params.closing_m, params.simplify_eps_m
            )),
            era: Era::Timeless,
            note: Some(
                "one-story connective podium = the shared street-level Ground \
                 story; the wing towers sit fully inside the plate by \
                 construction and their numbered floors start above the rim"
                    .into(),
            ),
        },
    };

    std::fs::write(&layout_path, layoutdoc::to_layout_json(&layout)?)?;
    std::fs::write(&report_path, &outcome.report)?;

    println!("wrote {}", layout_path.display());
    println!("wrote {}\n", report_path.display());
    print!("{}", outcome.report);
    Ok(())
}
