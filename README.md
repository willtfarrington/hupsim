# hupsim — a room-level capacity model of the HUP campus

> **Independent project — please read before citing.** hupsim is a personal
> portfolio project. It is **not affiliated with, endorsed by, or produced for**
> the Hospital of the University of Pennsylvania, Penn Medicine / UPHS, or the
> University of Pennsylvania, and the author was not acting on behalf of any
> institution in building it. Every *structural* fact (buildings, wings, units,
> room ranges, bridges) comes from cited **public** sources — wayfinding
> directories, job postings, program pages, OpenStreetMap, a hand-traced Google
> Earth survey — and each carries a confidence tag. Every *dynamic* quantity
> (patients, census, lengths of stay, staffing ratios, transport minutes,
> forecasts) is **simulated** from stated parameters; none is a measurement of
> the real hospital, and no patient or operational data of any kind was used.
> The case studies are **methodological demonstrations**, not operational
> assessments of HUP — a sentence like "closing the largest ICU strands 11
> patients" is a statement about the model, not about the institution. See
> [Model honesty & future directions](hupsim/docs/analyses/05-model-honesty-and-future-directions.md).

**License:** MIT for the code, data files, KML survey, and documentation — see
[LICENSE](LICENSE).

## Project status (2026-08-13)

**Scope: single campus — HUP West Philadelphia only.** The earlier idea of expanding to
the division's other sites (PPMC, GSPP/Rittenhouse, HUP Cedar) was considered and
dropped; out-of-scope data gets archived, not deleted — it lives in
`hupsim/assets/data/archive/out_of_scope.json` (see roadmap EP-1). The real institution
includes those other sites; only the West Philadelphia campus is modeled.

**Both roadmap phases are complete** (EP-0 … EP-18): the single-campus refit, then the
phase-2 informatics layer — clinical data enrichment with owner-ruled service lines
(EP-8/EP-23), room matrix + rolling statistics (EP-9), diurnal/ED/acuity sim realism
with backdated seeded census (EP-10/EP-21), RN staffing and workload (EP-11), unit and
hospital dashboards (EP-12/EP-13), the placement engine (EP-14), timed transport over
the route graph (EP-6), surge A/B experiments (EP-15), the bed-huddle morning report
(EP-16), the calibrated census forecast fan (EP-17), and the written case-study layer
(EP-18).

- **Code:** [hupsim/README.md](hupsim/README.md) — the Rust workspace (Bevy desktop app +
  data tools). Build and run from inside it: `cd hupsim && cargo run --release`.
- **Start here:** [hupsim/docs/analyses/README.md](hupsim/docs/analyses/README.md) —
  four reproducible case studies (transport, PCAM/CHPS access, surge experiments,
  forecast calibration) plus the model-honesty/future-directions capstone.
- **Roadmap:** [roadmap/README.md](roadmap/README.md) — every executed brief; open items
  and post-ruling threads tracked at the bottom. *(The commit hashes recorded there refer
  to the project's private development history, which was not carried into this public
  repository — see the note at the top of the roadmap.)*
- **Geometry source of truth:** `source material/hospital of the university of pennsylvania.kml`
  (hand-mapped in Google Earth Pro: 10 buildings + 3 skybridges). Imported into
  `hupsim/assets/data/layout.json` by the EP-2 tool; runtime never parses KML.
