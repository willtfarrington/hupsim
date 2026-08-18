//! The green → blue → red stability spectrum, interpolated piecewise in Oklab
//! so the transitions stay perceptually clean instead of passing through muddy
//! RGB midpoints. Green = stable, blue = watcher/intermediate, red = unstable.

use serde::{Deserialize, Serialize};

/// sRGB color, components in 0.0..=1.0 (gamma-encoded, i.e. what you'd write in CSS).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    pub const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    pub fn from_u8(r: u8, g: u8, b: u8) -> Self {
        Self::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
    }

    /// Linear-light components (for renderers that want linear color, e.g. Bevy).
    pub fn to_linear(self) -> [f32; 3] {
        [srgb_to_linear(self.r), srgb_to_linear(self.g), srgb_to_linear(self.b)]
    }

    pub fn to_array(self) -> [f32; 3] {
        [self.r, self.g, self.b]
    }
}

/// Spectrum anchors (sRGB).
const GREEN: Rgb = Rgb::new(0.161, 0.682, 0.380); // #29AE61 — stable
const BLUE: Rgb = Rgb::new(0.204, 0.435, 0.878); // #346FE0 — watcher
const RED: Rgb = Rgb::new(0.870, 0.196, 0.184); // #DE322F — unstable/critical

/// Vacant room / empty aggregate.
pub const COLOR_VACANT: Rgb = Rgb::new(0.72, 0.72, 0.74);
/// Out-of-service room.
pub const COLOR_OOS: Rgb = Rgb::new(0.38, 0.38, 0.42);
/// Building/wing with no patient rooms (topological context in the 3D view).
pub const COLOR_BEDLESS: Rgb = Rgb::new(0.82, 0.80, 0.76);

/// Map instability in `0.0..=1.0` onto the green→blue→red spectrum.
/// 0.0 → green, 0.5 → blue, 1.0 → red. Values are clamped.
pub fn stability_color(instability: f32) -> Rgb {
    let t = instability.clamp(0.0, 1.0);
    if t <= 0.5 {
        lerp_oklab(GREEN, BLUE, t * 2.0)
    } else {
        lerp_oklab(BLUE, RED, (t - 0.5) * 2.0)
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB → Oklab (Björn Ottosson's reference constants).
fn to_oklab(c: Rgb) -> [f32; 3] {
    let [r, g, b] = c.to_linear();
    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();
    [
        0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
        1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
        0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
    ]
}

fn from_oklab(lab: [f32; 3]) -> Rgb {
    let [l, a, b] = lab;
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let l3 = l_ * l_ * l_;
    let m3 = m_ * m_ * m_;
    let s3 = s_ * s_ * s_;
    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3;
    let b = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;
    Rgb::new(
        linear_to_srgb(r.clamp(0.0, 1.0)),
        linear_to_srgb(g.clamp(0.0, 1.0)),
        linear_to_srgb(b.clamp(0.0, 1.0)),
    )
}

fn lerp_oklab(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let la = to_oklab(a);
    let lb = to_oklab(b);
    from_oklab([
        la[0] + (lb[0] - la[0]) * t,
        la[1] + (lb[1] - la[1]) * t,
        la[2] + (lb[2] - la[2]) * t,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Rgb, b: Rgb) -> bool {
        (a.r - b.r).abs() < 0.02 && (a.g - b.g).abs() < 0.02 && (a.b - b.b).abs() < 0.02
    }

    #[test]
    fn endpoints_hit_anchors() {
        assert!(close(stability_color(0.0), GREEN));
        assert!(close(stability_color(0.5), BLUE));
        assert!(close(stability_color(1.0), RED));
    }

    #[test]
    fn out_of_range_clamps() {
        assert_eq!(stability_color(-1.0), stability_color(0.0));
        assert_eq!(stability_color(2.0), stability_color(1.0));
    }

    #[test]
    fn oklab_roundtrip() {
        for c in [GREEN, BLUE, RED, Rgb::new(0.5, 0.5, 0.5)] {
            assert!(close(from_oklab(to_oklab(c)), c));
        }
    }

    #[test]
    fn spectrum_moves_red_up_green_down() {
        // Red channel should broadly rise and green broadly fall across the ramp.
        let lo = stability_color(0.05);
        let hi = stability_color(0.95);
        assert!(hi.r > lo.r);
        assert!(hi.g < lo.g);
    }
}
