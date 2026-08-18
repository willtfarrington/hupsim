# EP-21 — Seeded-census admission backdating

**Size:** M · **Depends on:** EP-10 (LOS tables — met), EP-16 (report consumers — met),
**EP-23 (met 2026-08-13 — this brief now inherits the ruled LOS regime)** ·
**Blocks:** — (soft input to EP-17: its expected-departures model can condition on
elapsed LOS only if the standing census has admit times; EP-18 demos read better too)

> **EP-23 pickup note (2026-08-13).** The roster this brief backdates against
> changed: 42 units / 1,115 rooms (six units retired to the archive), four new
> ICUs (MICU→Founders 8, SICU→Rhoads 5, HVICU→Pavilion 7 Center, Oncology
> MICU→12 Center), and `los_for` now draws womens_neonatal for Silverstein 7/8
> + Ravdin 6 and oncology for the floor-12 block instead of `line.other`'s
> 96 h default. Two knock-ons to expect here: (1) the immortal-seed saturation
> bites HARDER on the smaller house — at seed 42 a day-2 report window holds
> zero bed admissions/discharges (report_smoke re-pinned to accept ED
> decisions as the corroboration when the window is visible; backdating +
> BacklogDischarges should make real bed churn reappear — consider restoring
> the stricter assert then); (2) `params.rs` still sizes arrivals for "~950
> inpatient beds" — recalibrate the base rate against the 1,115-room roster
> while you are in there, or hand it to EP-17 explicitly.

## Context

`hupsim_data::seed_demo_census` stamps every seeded patient
`admitted_at_min: None` ("present before t0"). Three consumers currently work around
the unknown elapsed LOS, all honestly labeled but all demo-visible:

- **EP-16 report**: the pre-sim rule counts every stable seeded patient as past the
  LOS threshold — a day-2 report on a seeded start lists ~490 anticipated discharges
  (~half the house), labeled "(admitted pre-sim)". Honest, but not what a morning
  huddle sheet looks like.
- **EP-15 experiments**: `run_experiment` backfills a **fresh-LOS** discharge for
  everyone in-house because elapsed time is unknown (bias identical in both arms,
  documented in `experiment.rs`).
- **Boarding wait context**: seeded ED boarders show "pre-sim" instead of hours waiting.

There is also a fourth artifact of `None`: seeded patients are **immortal** in the live
sim — nothing ever schedules their discharge, so the seeded house saturates to 100%
everywhere within a day (noted as a trap in the EP-13/EP-15 test notes). The params
sizing comment targets λ·LOS ≈ 85% chronic-full, not a pegged 100%; the saturation is
an artifact of the missing timestamps, not a modeling choice.

Fix: start the world **mid-stream**, the way a real hospital snapshot is — synthetic
negative admit times drawn from the same EP-10 LOS tables, plus scheduled exits for the
standing census.

## In scope

1. **Backdating in `seed_demo_census`** (`hupsim-data/src/seed.rs`), deterministic from
   the existing seed parameter (same seed ⇒ identical stamps, pinned by test):
   - **Inpatient units**: draw a total LOS `L` from the unit's service-line log-normal
     (`params::los_for`), **length-biased** — stays in progress at a random instant
     oversample long stays; for LN(median, σ) the size-biased draw is the same σ with
     median × e^(σ²) — then elapsed = U·L, stamp `admitted_at_min = −elapsed` (in
     minutes). Label the construction synthetic where the other params are labeled.
   - **ED bays**: episode-scale backdating, hours not days —
     `ED_TREAT_RELEASE_LOS`-scale elapsed for non-boarders; boarders get a
     decision-to-admit + boarding-wait-scale draw so day-0 boarding waits read as a
     plausible few hours. Refine exact scales at pickup.
   - Negative sim minutes are safe by construction elsewhere (`is_weekend`/`hour_of`
     use `rem_euclid`; the matrix column is f32 with `NO_ADMIT_MIN = −∞` as the only
     sentinel) — verify with a test, don't assume.
2. **Standing-census exits**: a one-shot `SimProcess` in `hupsim-sim` (working name
   `BacklogDischarges`) that on its first step schedules a guarded discharge
   (`expect_patient`) for every occupied inpatient bed with a known admit time, at
   **residual LOS**: total-LOS resample conditioned above elapsed (bounded retries,
   fallback = prompt discharge shaped by `DISCHARGE_HOUR_WEIGHTS`), then disables
   itself. Registered default-on in `SimDrive::new` beside the EP-10 processes. This
   restores the intended ~85% breathing regime from a seeded start and makes the
   report's "likely DC" column correspond to real upcoming exits.
3. **EP-15 backfill unification**: `run_experiment`'s fresh-LOS backfill is the same
   job — reuse the residual-LOS mechanism and update the bias note in `experiment.rs`.
   Still identical in both arms; the A/A null-diff test must stay green untouched.
4. **EP-16 interactions** (mostly free, verify and pin):
   - Anticipated discharges on a seeded day-1/2 become LOS-gated — a plausible
     morning-list fraction of the stable census, with `discharges_pre_sim` ≈ 0 (the
     pre-sim rule stays supported for hand-admitted/edited patients).
   - The day-1 overnight-admits section will correctly include backdated arrivals whose
     stamps fall inside the trailing window — intended (the snapshot pretends the
     hospital predates t0); the log-corroboration line already flags "≥ lower bounds"
     when the log can't reach back that far. Pin with a test, change nothing.
5. **Tests**: stamp determinism; stamps negative with sane elapsed spreads per unit
   type; scenario save/load round-trips negative stamps (schema v1 untouched — the
   field is already `Option<f64>`); EP-16 report on the real seeded world at day-1
   07:00 (anticipated discharges > 0 and ≪ census, `discharges_pre_sim == 0`, boarder
   waits finite); occupancy from a seeded start stays off the 100% rail over a
   multi-day run; EP-15 A/A null-diff green; update `report_smoke` expectations.

## Out of scope

- The typed sim event ledger (noted in the EP-17 brief for decision at pickup — the
  EP-16 log-corroboration scan migrates only if/when that exists).
- Changing the EP-16 discharge-readiness thresholds or report format.
- Backdating patients admitted via the editor/inspector (they correctly stamp "now").
- Any UI surface for tuning the backdating parameters (params stay compile-time,
  EP-10 owner decision).

## Verification / acceptance

- `cargo test --workspace` green, including the updated smoke expectations.
- Run the app fresh (seeded Tuesday): census decays off 100% into a breathing
  ~chronic-full regime over the first sim day; open 📋 Report near day-1 07:00 — the
  anticipated-discharge list is a plausible morning list (not ~half the house), no
  "pre-sim" flood, boarding waits show hours; export still matches the drawer numbers
  at the same instant (EP-16 acceptance re-run).
