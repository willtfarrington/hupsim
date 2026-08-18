# EP-0 — Baseline & hygiene

**Size:** S · **Depends on:** nothing · **Blocks:** everything else

## Context

The working tree has been dirty since commit `754e5aa feat: add simulation framework for
HUP model` — ~15 modified files across the hupsim workspace plus `scratch.md` at the repo
root, and two untracked smoke-test artifacts. Before any refit work starts, the current
state must be captured as a baseline so every subsequent endpoint produces a clean,
reviewable diff.

**The git root is one level above the hupsim workspace** (the `hupsim` repository root).
All git commands run there. *(Until 2026-08-15 this project lived inside the
multi-project `PRIVATE-1/` repo, with the workspace two–three levels below the git root
and unrelated directories alongside — hence the original advice to scope paths
deliberately.)*

## In scope

1. **Baseline commit** of all pending hupsim changes (everything under the project
   directory — now the repository root — this includes
   the modified `readme.md` and the new `roadmap/` folder, which are planning artifacts:
   commit them, don't edit them) with a message like `chore(hupsim): pre-refit baseline`.
   Commit `scratch.md` separately (or in the same commit if trivial — inspect the diff
   first).
2. **Delete the stale duplicate** `hup_topology.json` at the project-directory root
   (the copy beside `hupsim/`, one level ABOVE the workspace).
   It is byte-identical to `hupsim/assets/data/hup_topology.json`
   (27,371 bytes, md5 `15b2ba9b833b39ab121397990221c73b`) and is **git-tracked**, so use
   `git rm`. The app can never load it: `has_all_files`
   (`hupsim/crates/hupsim-data/src/io.rs:74-78`) requires all three JSONs together in one
   directory, and the root-side dir holds only the topology file. Verify byte-identity
   before removing;
   if it has diverged, STOP and reconcile into the assets copy first.
3. **Delete + gitignore** `hupsim/smoke_out.txt` and `hupsim/smoke_err.txt` (untracked
   debug artifacts). Append `smoke_*.txt` to `hupsim/.gitignore` (it exists; current
   entries: `/target`, `Cargo.lock.orig`, `*.pdb`).
4. **Commit the KML** `hospital of the university of pennsylvania.kml` (then at the
   project-directory root, beside `hupsim/`) — it is the authoritative
   geometry source for EP-2. *(Post-EP-18 note: the KML now lives in
   `source material/hospital of the university of pennsylvania.kml`, still one level
   above the workspace.)*

## Out of scope

- Any code, data-content, or asset edits. This endpoint only commits, removes, and
  ignores. (Committing the `roadmap/` folder and `readme.md` is in scope per item 1;
  editing their content is not.)

## Verification / acceptance

- `cargo test --workspace` (run inside `hupsim/`) green **before** the baseline commit —
  the baseline must be a known-good state. If tests fail, commit anyway with the failure
  noted in the commit message (the baseline documents reality), and record the failure
  prominently for the next endpoint.
- After all commits: `git status` clean (from the repo root).
- `git log --oneline -5` shows the baseline commit(s); the KML is tracked
  (`git ls-files | grep -i kml` shows it).
- The duplicate root-side `hup_topology.json` is gone; `hupsim/assets/data/hup_topology.json`
  is untouched.
