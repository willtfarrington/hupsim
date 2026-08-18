//! Geographic projection between lat/lon and the local ENU meter frame.
//!
//! Equirectangular about the layout origin (`CampusLayout.origin_lat/lon`) —
//! at campus scale (<1 km) the error vs a true ENU frame is millimeters,
//! and this exact projection reproduces the frame the existing layout.json
//! footprints (OSM-sourced) were authored in.

/// Meters per degree of latitude (and of longitude at the equator).
pub const METERS_PER_DEG: f64 = 111_320.0;

/// An equirectangular projection frame anchored at a geographic origin.
/// x = east, y = north, meters — matches `layout::Footprint`'s frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoFrame {
    pub origin_lat: f64,
    pub origin_lon: f64,
}

impl GeoFrame {
    pub fn new(origin_lat: f64, origin_lon: f64) -> Self {
        Self {
            origin_lat,
            origin_lon,
        }
    }

    /// lat/lon (degrees) → local meters [x=east, y=north].
    pub fn forward(&self, lat: f64, lon: f64) -> [f64; 2] {
        let x = (lon - self.origin_lon) * self.origin_lat.to_radians().cos() * METERS_PER_DEG;
        let y = (lat - self.origin_lat) * METERS_PER_DEG;
        [x, y]
    }

    /// local meters [x=east, y=north] → (lat, lon) degrees.
    pub fn inverse(&self, x: f64, y: f64) -> (f64, f64) {
        let lat = self.origin_lat + y / METERS_PER_DEG;
        let lon = self.origin_lon + x / (self.origin_lat.to_radians().cos() * METERS_PER_DEG);
        (lat, lon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layout.json origin (39.949, -75.1925).
    fn hup_frame() -> GeoFrame {
        GeoFrame::new(39.949, -75.1925)
    }

    #[test]
    fn round_trip() {
        let f = hup_frame();
        for &(lat, lon) in &[
            (39.949, -75.1925),
            (39.9494247, -75.1938942), // Silverstein SW-ish corner
            (39.9466656, -75.1929227), // PCAM south
        ] {
            let [x, y] = f.forward(lat, lon);
            let (lat2, lon2) = f.inverse(x, y);
            assert!((lat - lat2).abs() < 1e-12, "lat round-trip");
            assert!((lon - lon2).abs() < 1e-12, "lon round-trip");
        }
    }

    /// Frame-agreement fixture: the hand-traced Clifton KML ring, projected
    /// about the layout origin, must land on the OSM Pavilion polygon already
    /// in layout.json (bbox x[-14.7, 140.3] y[-133.9, 14.4]) within ±1.5 m on
    /// the shared west/north/east extremes. (South differs ~17 m — that is a
    /// real hand-trace-vs-OSM extent difference, surfaced by the importer's
    /// cross-check report, not a frame error.)
    #[test]
    fn clifton_kml_ring_agrees_with_osm_pavilion_bbox() {
        let f = hup_frame();
        // (lon, lat) pairs, verbatim from the KML Clifton LinearRing
        // (closing duplicate omitted).
        let ring = [
            (-75.19242435229344, 39.94911688819633),
            (-75.19252020525153, 39.94913399829146),
            (-75.19260715384488, 39.94911112199005),
            (-75.19266709431001, 39.94905006551753),
            (-75.19266704020976, 39.94897583303632),
            (-75.1926379186465, 39.94892050065988),
            (-75.19162573703984, 39.94820017947005),
            (-75.19111606648758, 39.94798109614543),
            (-75.19103315335529, 39.94795434750814),
            (-75.19095023037301, 39.94796158444972),
            (-75.19088872312433, 39.94799098301814),
            (-75.19084325738172, 39.94803833103482),
            (-75.19084652751772, 39.94809754552213),
            (-75.1908871702786, 39.94814874818081),
            (-75.19189017391572, 39.94887397044175),
            (-75.19242435229344, 39.94911688819633),
        ];
        let (mut min_x, mut max_x) = (f64::MAX, f64::MIN);
        let mut max_y = f64::MIN;
        for &(lon, lat) in &ring {
            let [x, y] = f.forward(lat, lon);
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        assert!((min_x - -14.7).abs() < 1.5, "west extreme: got {min_x}");
        assert!((max_x - 140.3).abs() < 1.5, "east extreme: got {max_x}");
        assert!((max_y - 14.4).abs() < 1.5, "north extreme: got {max_y}");
    }
}
