//! One-shot KML importer CLI (see `hupsim_data::kml` for the logic).
//!
//! Regenerates `assets/data/layout.json` from the owner's Google Earth Pro
//! hand-mapping and writes the delta/cross-check report next to it:
//!
//! ```text
//! cargo run -p hupsim-data --features tools --bin kml_import
//! ```
//!
//! Refuses to run when the layout already matches the KML (all centroid
//! deltas ~0) — a re-run would overwrite `kml_import_report.md`'s historical
//! survey deltas with zeros. Pass `--force` to overwrite anyway.

use hupsim_data::kml::{import, Mapping};
use hupsim_data::layoutdoc;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kml_import: error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let force = std::env::args().any(|a| a == "--force");

    // CARGO_MANIFEST_DIR = crates/hupsim-data; the KML lives in the project's
    // "source material" folder, one directory above the hupsim workspace root.
    let ws: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let kml_path = ws
        .join("..")
        .join("source material")
        .join("hospital of the university of pennsylvania.kml");
    let mapping_path = ws.join("assets").join("data").join("kml_mapping.json");
    let layout_path = ws.join("assets").join("data").join("layout.json");
    let report_path = ws.join("assets").join("data").join("kml_import_report.md");

    let kml = std::fs::read_to_string(&kml_path)?;
    let mapping: Mapping = serde_json::from_str(&std::fs::read_to_string(&mapping_path)?)?;
    let layout_json = std::fs::read_to_string(&layout_path)?;

    let outcome = import(&kml, &mapping, &layout_json)?;

    if outcome.max_centroid_delta_m < 0.05 && !force {
        return Err("all centroid deltas are ~0: layout.json already matches the KML. \
             Re-running would erase the historical survey deltas in kml_import_report.md. \
             Pass --force to overwrite anyway."
            .into());
    }

    let json = layoutdoc::to_layout_json(&outcome.layout)?;
    std::fs::write(&layout_path, json)?;
    std::fs::write(&report_path, &outcome.report)?;

    println!("wrote {}", layout_path.display());
    println!("wrote {}\n", report_path.display());
    print!("{}", outcome.report);
    Ok(())
}
