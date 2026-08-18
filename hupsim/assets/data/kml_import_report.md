# KML import report

Source: `Google Earth Pro hand-mapping (owner), 2026-08` — projected equirectangularly about the layout origin (39.949, -75.1925).

## Wing footprints: old (hand-seeded rectangle) → new (surveyed trace)

| node | old centroid (m) | new centroid (m) | delta (m) |
|---|---|---|---|
| `wing.silverstein` | (-66.5, 125.0) | (-86.6, 61.0) | 67.1 (dx -20.1, dy -64.0) |
| `wing.ravdin` | (-66.5, 65.0) | (-47.0, 98.2) | 38.6 (dx +19.5, dy +33.2) |
| `wing.rhoads` | (-208.0, 41.0) | (-213.7, 97.8) | 57.1 (dx -5.7, dy +56.8) |
| `wing.white` | (-272.0, 62.0) | (-37.1, 143.5) | 248.6 (dx +234.9, dy +81.5) |
| `wing.dulles` | (-125.0, 72.0) | (-85.6, 117.9) | 60.4 (dx +39.4, dy +45.9) |
| `wing.gates` | (-162.5, 152.5) | (-151.4, 167.3) | 18.5 (dx +11.1, dy +14.8) |
| `wing.founders` | (-132.5, 77.5) | (-148.0, 102.7) | 29.6 (dx -15.5, dy +25.2) |
| `wing.maloney` | (-230.5, 149.0) | (-222.7, 147.5) | 8.0 (dx +7.8, dy -1.5) |

## Cross-check: KML hand-trace vs kept OSM footprint

These footprints were NOT replaced (`replace_footprint: false`) — the OSM polygons stay authoritative; the traces are cross-check inputs and bridge anchors.

### `bldg.pavilion` ("Clifton Center (previously labeled as "Pavilion")")

- centroid offset (trace − OSM): dx -5.6 m, dy +11.5 m, dist 12.8 m
- area: OSM 7329 m² vs trace 6511 m² (delta -11.2%)
- bbox deltas (trace − OSM): west +0.4, east +1.1, south +17.5, north +0.5 m

### `bldg.pcam` ("Perelman Center for Advanced Medicine")

- centroid offset (trace − OSM): dx +3.8 m, dy -26.3 m, dist 26.6 m
- area: OSM 14274 m² vs trace 16171 m² (delta +13.3%)
- bbox deltas (trace − OSM): west -4.6, east -10.8, south -9.4, north +0.0 m

## Bridges (new `bridges[]` array)

- `bridge.clifton_pcam_1` — bldg.pavilion ↔ bldg.pcam, level 2, deck 8 m, 4 vertices, deck area 74.5 m²
- `bridge.clifton_pcam_2` — bldg.pavilion ↔ bldg.pcam, level 2, deck 8 m, 4 vertices, deck area 98.9 m²
- `bridge.silverstein_clifton` — wing.silverstein ↔ bldg.pavilion, level 3, deck 12 m, 4 vertices, deck area 157.6 m²
