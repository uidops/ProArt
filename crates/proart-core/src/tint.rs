//! Software picture adjustments: warmth (a blue-light filter like Night
//! Shift / f.lux), extra dimming below the monitor's minimum and a gamma
//! curve, all applied through the display's colour tables. They work on any
//! monitor, independent of DDC/CI, and last while the app runs.
//!
//! Colour tables are per channel, so channel mixing (saturation, hue,
//! greyscale) is not possible this way.

use serde::{Deserialize, Serialize};

/// Level names, matching the monitor's own Blue Light Filter levels.
pub const LEVELS: &[&str] = &["Level 0", "Level 1", "Level 2", "Level 3", "Level 4"];
/// Colour temperature of each level; level 0 is off.
const LEVEL_KELVIN: [u32; 5] = [NEUTRAL_KELVIN, 5500, 4700, 4000, 3400];
pub const MAX_LEVEL: u8 = 4;

/// The display's own white point: no warming.
pub const NEUTRAL_KELVIN: u32 = 6500;
/// Warmest setting of the warmth slider.
pub const MIN_KELVIN: u32 = 2700;
/// Extra dimming stops here so the screen never goes black.
pub const MAX_DIM: u8 = 80;
/// Gamma the display is assumed to have; the neutral setting.
pub const NEUTRAL_GAMMA: f32 = 2.2;
pub const GAMMAS: &[f32] = &[1.8, 2.0, 2.2, 2.4, 2.6];

/// Software picture of one display.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoftPicture {
    /// Colour temperature the picture is warmed to; 6500 = unchanged.
    pub kelvin: u32,
    /// Extra dimming in percent, 0 to `MAX_DIM`.
    pub dim: u8,
    /// Target gamma; 2.2 = unchanged.
    pub gamma: f32,
}

impl Default for SoftPicture {
    fn default() -> Self {
        SoftPicture { kelvin: NEUTRAL_KELVIN, dim: 0, gamma: NEUTRAL_GAMMA }
    }
}

impl SoftPicture {
    /// Within the supported ranges.
    pub fn clamped(self) -> SoftPicture {
        SoftPicture {
            kelvin: self.kelvin.clamp(MIN_KELVIN, NEUTRAL_KELVIN),
            dim: self.dim.min(MAX_DIM),
            gamma: if self.gamma.is_finite() { self.gamma.clamp(1.0, 3.0) } else { NEUTRAL_GAMMA },
        }
    }

    pub fn is_neutral(&self) -> bool {
        *self == SoftPicture::default()
    }

    /// Red, green and blue gains (0-1): warmth times dimming.
    pub fn gains(&self) -> [f32; 3] {
        let dim = 1.0 - self.dim.min(MAX_DIM) as f32 / 100.0;
        kelvin_gains(self.kelvin).map(|g| g * dim)
    }

    /// Power applied to the colour table: `gamma / 2.2`.
    pub fn exponent(&self) -> f32 {
        self.clamped().gamma / NEUTRAL_GAMMA
    }

    /// The level whose colour temperature this is, if any.
    pub fn level(&self) -> Option<u8> {
        LEVEL_KELVIN.iter().position(|k| *k == self.kelvin).map(|i| i as u8)
    }
}

pub fn level_kelvin(level: u8) -> u32 {
    LEVEL_KELVIN[level.min(MAX_LEVEL) as usize]
}

/// Gains that warm the display's white to `kelvin`; `[1, 1, 1]` at 6500 K or above.
pub fn kelvin_gains(kelvin: u32) -> [f32; 3] {
    if kelvin >= NEUTRAL_KELVIN {
        return [1.0; 3];
    }
    let (white, warm) = (blackbody(NEUTRAL_KELVIN), blackbody(kelvin.max(1000)));
    [0, 1, 2].map(|c| (warm[c] / white[c]).clamp(0.0, 1.0))
}

/// Approximate RGB (0-255) of a black body at `kelvin` (Tanner Helland's fit).
fn blackbody(kelvin: u32) -> [f32; 3] {
    let t = kelvin as f32 / 100.0;
    let r = if t <= 66.0 { 255.0 } else { 329.698_73 * (t - 60.0).powf(-0.133_204_76) };
    let g = if t <= 66.0 { 99.470_8 * t.ln() - 161.119_57 } else { 288.122_17 * (t - 60.0).powf(-0.075_514_85) };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    [r, g, b].map(|v| v.clamp(0.0, 255.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_picture_is_neutral() {
        let p = SoftPicture::default();
        assert!(p.is_neutral());
        assert_eq!(p.gains(), [1.0, 1.0, 1.0]);
        assert_eq!(p.exponent(), 1.0);
        assert_eq!(p.level(), Some(0));
    }

    #[test]
    fn higher_levels_cut_more_blue_and_keep_red() {
        let mut last = kelvin_gains(level_kelvin(0));
        for level in 1..=MAX_LEVEL {
            let g = kelvin_gains(level_kelvin(level));
            assert_eq!(g[0], 1.0, "red stays at full");
            assert!(g[2] < last[2] && g[1] < last[1], "level {level} is warmer than {}", level - 1);
            assert!(g[2] < g[1], "blue is cut more than green");
            last = g;
        }
        assert!(kelvin_gains(level_kelvin(MAX_LEVEL))[2] > 0.3, "level 4 is warm, not orange");
        assert_eq!(level_kelvin(9), level_kelvin(MAX_LEVEL), "clamped");
    }

    #[test]
    fn dimming_scales_every_channel_and_stops_short_of_black() {
        let p = SoftPicture { dim: 50, ..SoftPicture::default() };
        assert_eq!(p.gains(), [0.5, 0.5, 0.5]);
        let max = SoftPicture { dim: 200, ..SoftPicture::default() };
        assert!((max.gains()[0] - 0.2).abs() < 1e-6, "dimming is capped at {MAX_DIM} %");
        assert_eq!(max.clamped().dim, MAX_DIM);
    }

    #[test]
    fn gamma_maps_to_an_exponent_over_2_2() {
        let p = SoftPicture { gamma: 2.4, ..SoftPicture::default() };
        assert!((p.exponent() - 2.4 / 2.2).abs() < 1e-6);
        assert_eq!(SoftPicture { gamma: f32::NAN, ..SoftPicture::default() }.clamped().gamma, NEUTRAL_GAMMA);
    }

    #[test]
    fn a_custom_warmth_is_not_a_level() {
        assert_eq!(SoftPicture { kelvin: 4000, ..SoftPicture::default() }.level(), Some(3));
        assert_eq!(SoftPicture { kelvin: 3000, ..SoftPicture::default() }.level(), None);
        assert_eq!(SoftPicture { kelvin: 9000, ..SoftPicture::default() }.clamped().kelvin, NEUTRAL_KELVIN);
    }
}
