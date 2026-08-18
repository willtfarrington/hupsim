//! App-level resources and state shared between the 3D view, the unit view,
//! and the egui panels.

use bevy::prelude::*;
use hupsim_core::prelude::*;
use hupsim_data::LoadedWorld;
use hupsim_sim::prelude::*;
use std::path::PathBuf;

/// The whole model world: scenario (hospital + clock + seed), 3D layout, and
/// the lookup index. `version` bumps on every mutation so render/aggregate
/// systems know when to refresh.
#[derive(Resource)]
pub struct AppWorld {
    pub scenario: Scenario,
    pub layout: CampusLayout,
    pub index: HospitalIndex,
    pub version: u64,
    pub source_dir: Option<PathBuf>,
    pub agg_mode: AggregateMode,
}

impl AppWorld {
    pub fn new(loaded: LoadedWorld) -> Self {
        let mut scenario = Scenario::new("HUP — live editing session", loaded.hospital);
        scenario.meta.description =
            "Interactive scenario; save via the toolbar to keep your edits.".into();
        let index = HospitalIndex::build(&scenario.hospital);
        Self {
            scenario,
            layout: loaded.layout,
            index,
            version: 1,
            source_dir: loaded.source_dir,
            agg_mode: AggregateMode::Mean,
        }
    }

    pub fn hospital(&self) -> &Hospital {
        &self.scenario.hospital
    }

    /// Rebuild the index after structural edits (room add/remove).
    pub fn reindex(&mut self) {
        self.index = HospitalIndex::build(&self.scenario.hospital);
        self.version += 1;
    }
}

/// What the inspector panel is focused on.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Focus {
    #[default]
    None,
    Building(NodeId),
    Floor(NodeId, FloorLevel),
    Unit(UnitId),
    Room(RoomId),
}

#[derive(Resource, Default)]
pub struct Selection(pub Focus);

/// Floor slab currently under the pointer in the 3D view.
#[derive(Resource, Default)]
pub struct HoverInfo(pub Option<(NodeId, FloorLevel)>);

/// Connection (skybridge deck or flow ribbon) currently under the pointer,
/// as an index into `hospital.connections`.
#[derive(Resource, Default)]
pub struct ConnHover(pub Option<usize>);

/// Request a full 3D scene rebuild (set after loading a scenario, whose
/// building/floor set may differ from the entities on screen).
#[derive(Resource, Default)]
pub struct SceneRebuild(pub bool);

/// Which unit the 2D detail view shows.
#[derive(Resource, Default)]
pub struct OpenUnit(pub Option<UnitId>);

#[derive(States, Debug, Clone, Copy, Default, Eq, PartialEq, Hash)]
pub enum ViewMode {
    #[default]
    Campus3D,
    UnitDetail,
}

/// Simulation engine + registration bookkeeping (which registered process is
/// which, for the toolbar toggles).
#[derive(Resource)]
pub struct SimDrive {
    pub engine: SimEngine,
    pub idx_drift: usize,
    pub idx_flow: usize,
    pub idx_diurnal: usize,
    pub idx_ed: usize,
}

impl SimDrive {
    pub fn new(seed: u64, hospital: &Hospital, layout: &CampusLayout) -> Self {
        let mut engine = SimEngine::new(seed);
        // Standing-census exits (EP-21) go first: the one-shot fires before
        // the flow processes on the opening tick, so the seeded house gets
        // its residual-LOS discharge schedule before anyone new is admitted.
        engine.register(Box::new(BacklogDischarges::default()), true);
        // Realism processes (EP-10) default ON — the clock starts paused, so
        // nothing moves until the user presses play. The flat demo flow stays
        // registered as an off-by-default control signal.
        engine.register(Box::new(AcuityDrift::default()), true);
        engine.register(Box::new(DemoFlow::default()), false);
        engine.register(Box::new(DiurnalFlow::default()), true);
        engine.register(Box::new(EdPressure::default()), true);
        // Timed transport (EP-6): RNG-free, registered last (index 5) and
        // always enabled — the toolbar toggle drives `engine.transport.timed`
        // instead of the process flag, so in-flight patients still arrive
        // after a mid-run switch-off.
        engine.register(Box::new(BedTransport), true);
        engine.transport.timed = true;
        engine.transport.graph = Some(Self::route_graph(hospital, layout));
        Self {
            engine,
            idx_drift: 1,
            idx_flow: 2,
            idx_diurnal: 3,
            idx_ed: 4,
        }
    }

    /// Build the EP-6 route graph: default weights, unverified edges (the
    /// HUP Main↔Clifton tunnel) excluded per the confidence policy.
    pub fn route_graph(hospital: &Hospital, layout: &CampusLayout) -> RouteGraph {
        RouteGraph::build(
            hospital,
            layout,
            RouteParams::default(),
            TraversalPolicy::default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The idx_* fields are hand-maintained against the registration order
    /// (EP-21 shifted every index by one) — pin each index to its process by
    /// name, so a future reorder can't silently point the toolbar toggles or
    /// the compare tab's ProcessToggles capture at the wrong process.
    #[test]
    fn simdrive_indices_match_their_processes() {
        let loaded = hupsim_data::load_default().expect("embedded data compiles");
        let sim = SimDrive::new(1, &loaded.hospital, &loaded.layout);
        let name = |i: usize| sim.engine.processes[i].process.name().to_string();
        assert_eq!(sim.engine.processes.len(), 6);
        assert_eq!(name(0), BacklogDischarges::default().name());
        assert_eq!(name(sim.idx_drift), AcuityDrift::default().name());
        assert_eq!(name(sim.idx_flow), DemoFlow::default().name());
        assert_eq!(name(sim.idx_diurnal), DiurnalFlow::default().name());
        assert_eq!(name(sim.idx_ed), EdPressure::default().name());
        assert_eq!(name(5), BedTransport.name(), "transport registered last");
        // Defaults: everything on except the demo control flow.
        assert!(sim.engine.processes[0].enabled, "standing-census exits default on");
        assert!(!sim.engine.processes[sim.idx_flow].enabled, "demo flow defaults off");
        assert!(sim.engine.transport.timed, "timed transport defaults on");
    }
}
