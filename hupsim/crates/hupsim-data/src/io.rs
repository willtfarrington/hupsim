//! Locate + load the data files, with the repo copies embedded as fallback so
//! the compiled binary runs from anywhere.

use crate::compile::{compile, CompileError};
use hupsim_core::prelude::*;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DataError {
    #[error("compile error: {0}")]
    Compile(#[from] CompileError),
    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("scenario error: {0}")]
    Scenario(#[from] hupsim_core::scenario::ScenarioError),
}

pub struct LoadedWorld {
    pub hospital: Hospital,
    pub layout: CampusLayout,
    /// Where the data files were found; None = embedded copies.
    pub source_dir: Option<PathBuf>,
}

/// The repo data files, baked into the binary at compile time.
pub fn embedded() -> (&'static str, &'static str, &'static str, &'static str) {
    (
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/data/hup_topology.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/data/units.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/data/layout.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/data/service_lines.json"
        )),
    )
}

fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(env_dir) = std::env::var("HUPSIM_ASSETS") {
        dirs.push(PathBuf::from(env_dir));
    }
    if let Ok(cwd) = std::env::current_dir() {
        let mut d = cwd;
        for _ in 0..5 {
            dirs.push(d.join("assets").join("data"));
            if !d.pop() {
                break;
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut d = exe;
        d.pop();
        for _ in 0..5 {
            dirs.push(d.join("assets").join("data"));
            if !d.pop() {
                break;
            }
        }
    }
    dirs
}

fn has_all_files(dir: &Path) -> bool {
    [
        "hup_topology.json",
        "units.json",
        "layout.json",
        "service_lines.json",
    ]
    .iter()
    .all(|f| dir.join(f).is_file())
}

fn read(dir: &Path, file: &str) -> Result<String, DataError> {
    let path = dir.join(file);
    std::fs::read_to_string(&path).map_err(|source| DataError::Io {
        path: path.display().to_string(),
        source,
    })
}

/// Load from the first assets dir found (env var → cwd walk → exe walk),
/// falling back to the embedded copies.
pub fn load_default() -> Result<LoadedWorld, DataError> {
    for dir in candidate_dirs() {
        if has_all_files(&dir) {
            let topo = read(&dir, "hup_topology.json")?;
            let units = read(&dir, "units.json")?;
            let layout = read(&dir, "layout.json")?;
            let lines = read(&dir, "service_lines.json")?;
            let (hospital, layout) = compile(&topo, &units, &layout, &lines)?;
            log_warnings(&hospital);
            return Ok(LoadedWorld {
                hospital,
                layout,
                source_dir: Some(dir),
            });
        }
    }
    let (topo, units, layout, lines) = embedded();
    let (hospital, layout) = compile(topo, units, layout, lines)?;
    log_warnings(&hospital);
    Ok(LoadedWorld {
        hospital,
        layout,
        source_dir: None,
    })
}

/// Non-fatal compile findings (needs-ruling service lines, stale mapping
/// entries) go to stderr; the UI also shows them at the data-source indicator.
fn log_warnings(hospital: &Hospital) {
    for w in &hospital.meta.data_warnings {
        eprintln!("hupsim data warning: {w}");
    }
}

/// Save / load scenario files.
pub fn save_scenario(path: &Path, scenario: &Scenario) -> Result<(), DataError> {
    let json = scenario.to_json()?;
    std::fs::write(path, json).map_err(|source| DataError::Io {
        path: path.display().to_string(),
        source,
    })
}

pub fn load_scenario(path: &Path) -> Result<Scenario, DataError> {
    let json = std::fs::read_to_string(path).map_err(|source| DataError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(Scenario::from_json(&json)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_data_loads() {
        let (t, u, l, s) = embedded();
        assert!(compile(t, u, l, s).is_ok());
    }

    #[test]
    fn scenario_file_roundtrip() {
        let (t, u, l, s) = embedded();
        let (hospital, _) = compile(t, u, l, s).unwrap();
        let sc = Scenario::new("roundtrip", hospital);
        let dir = std::env::temp_dir().join("hupsim_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scenario_roundtrip.json");
        save_scenario(&path, &sc).unwrap();
        let back = load_scenario(&path).unwrap();
        assert_eq!(sc, back);
        let _ = std::fs::remove_file(&path);
    }

    /// Remove every field EP-8 introduced, reproducing the exact JSON shape a
    /// pre-EP-8 build would have saved.
    fn strip_ep8_fields(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                for k in [
                    "service_line",
                    "target_rn_ratio",
                    "telemetry_capable",
                    "negative_pressure",
                    "service_lines",
                    "data_warnings",
                ] {
                    map.remove(k);
                }
                for val in map.values_mut() {
                    strip_ep8_fields(val);
                }
            }
            serde_json::Value::Array(arr) => {
                for val in arr.iter_mut() {
                    strip_ep8_fields(val);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn pre_ep8_scenario_save_still_loads() {
        let (t, u, l, s) = embedded();
        let (hospital, _) = compile(t, u, l, s).unwrap();
        let sc = Scenario::new("pre_ep8", hospital);
        let mut v: serde_json::Value = serde_json::from_str(&sc.to_json().unwrap()).unwrap();
        strip_ep8_fields(&mut v);
        let back = Scenario::from_json(&v.to_string()).expect("pre-EP-8 save must load");
        assert!(back
            .hospital
            .units
            .iter()
            .all(|u| u.service_line.is_none() && u.target_rn_ratio.is_none()));
        assert!(back
            .hospital
            .rooms
            .iter()
            .all(|r| !r.telemetry_capable && !r.negative_pressure));
        assert!(back.hospital.service_lines.is_empty());
        assert!(back.hospital.meta.data_warnings.is_empty());
    }
}
