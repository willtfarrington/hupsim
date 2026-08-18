# Podium derivation report

Written by `podium_gen` (EP-7). The podium plate in `layout.json` is the union of the `wing.*` footprints below, morphologically closed and buffered, then simplified. Regenerate with:

```
cargo run -p hupsim-data --features tools --bin podium_gen
```

## Parameters

| cell (m) | buffer (m) | closing (m) | simplify ε (m) | collinear (°) | min edge (m) |
|---|---|---|---|---|---|
| 0.25 | 3 | 15 | 1.5 | 5 | 3 |

## Inputs (surveyed wing footprints, layout.json)

| node | vertices | area (m²) |
|---|---|---|
| `wing.silverstein` | 10 | 1918 |
| `wing.ravdin` | 4 | 1650 |
| `wing.founders` | 8 | 3546 |
| `wing.gates` | 8 | 988 |
| `wing.maloney` | 10 | 1529 |
| `wing.rhoads` | 10 | 1710 |
| `wing.dulles` | 10 | 2636 |
| `wing.white` | 4 | 756 |

## Result

- raster union area (pre-closing): 14730 m²
- final ring: 19 vertices, 24260 m²
- min wing-boundary clearance to the ring edge: 1.51 m
- the full wing boundary (sampled at ≤1 m along every edge) is inside the ring (validated at derivation time)
