//! Automation settings and the pure logic behind them: the blue-light
//! schedule, presets at set times, brightness from ambient light and
//! sleep/wake power. The manager's automation thread and the app's key and
//! app watchers act on these; nothing here talks to a monitor.
//!
//! Times of day are minutes after local midnight.

use serde::{Deserialize, Serialize};

use crate::tint::NEUTRAL_KELVIN;

pub const DAY: u16 = 24 * 60;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Automation {
    pub blue_schedule: BlueSchedule,
    pub timed_presets: Vec<TimedPreset>,
    pub app_presets: Vec<AppPreset>,
    pub keys: KeySettings,
    pub ambient: Ambient,
    pub power: PowerAutomation,
}

/// Warm every display between two times, fading in and out.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BlueSchedule {
    pub enabled: bool,
    pub start: u16,
    pub end: u16,
    pub kelvin: u32,
    pub fade_min: u16,
}

impl Default for BlueSchedule {
    fn default() -> Self {
        BlueSchedule { enabled: false, start: 21 * 60, end: 7 * 60, kelvin: 3400, fade_min: 30 }
    }
}

/// Apply a preset every day at a time.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TimedPreset {
    pub id: String,
    pub enabled: bool,
    pub at: u16,
    /// Preset id.
    pub preset: String,
}

/// Apply a preset while an app is in front, and go back afterwards.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppPreset {
    pub bundle_id: String,
    pub name: String,
    /// Preset id.
    pub preset: String,
}

/// The Mac's brightness and volume keys control the monitor.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KeySettings {
    /// Brightness keys, for the display under the pointer.
    pub brightness: bool,
    /// Volume and mute keys, while sound plays through the monitor.
    pub volume: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbientSource {
    /// The Mac's ambient light sensor.
    #[default]
    Sensor,
    /// Follow the built-in display's brightness (which follows the sensor
    /// when the Mac's automatic brightness is on).
    Builtin,
}

/// Monitor brightness from the room's light.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Ambient {
    pub enabled: bool,
    pub source: AmbientSource,
    /// Brightness in the dark and in bright light (percent).
    pub min: u8,
    pub max: u8,
}

impl Default for Ambient {
    fn default() -> Self {
        Ambient { enabled: false, source: AmbientSource::Sensor, min: 10, max: 90 }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerAutomation {
    /// Turn monitors off when the Mac's screens sleep or it locks.
    pub off_on_sleep: bool,
    /// Turn them on again when it wakes or unlocks.
    pub on_on_wake: bool,
}

// ---------------------------------------------------------------- schedule --

/// Minutes from `from` forward to `to`, across midnight.
fn forward(from: f32, to: f32) -> f32 {
    (to - from).rem_euclid(DAY as f32)
}

/// Colour temperature the schedule asks for at `now`, or None outside it.
/// The filter fades in over `fade_min` after the start and out over
/// `fade_min` before the end (shortened to half the window if needed).
pub fn schedule_kelvin(s: &BlueSchedule, now: f32) -> Option<u32> {
    if !s.enabled || s.start == s.end {
        return None;
    }
    let window = forward(s.start as f32, s.end as f32);
    let into = forward(s.start as f32, now);
    if into >= window {
        return None;
    }
    let fade = (s.fade_min as f32).min(window / 2.0);
    let left = window - into;
    let t = if fade <= 0.0 { 1.0 } else { (into / fade).min(left / fade).min(1.0) };
    let k = NEUTRAL_KELVIN as f32 + (s.kelvin.min(NEUTRAL_KELVIN) as f32 - NEUTRAL_KELVIN as f32) * t;
    Some(k.round() as u32)
}

/// The timed preset to apply when the clock moved from `last` to `now`
/// (exclusive, inclusive): the latest one passed, so waking from sleep
/// applies only the one that should be in effect.
pub fn due_preset(entries: &[TimedPreset], last: f32, now: f32) -> Option<&TimedPreset> {
    let span = forward(last, now);
    if span == 0.0 {
        return None;
    }
    entries
        .iter()
        .filter(|e| e.enabled && !e.preset.is_empty())
        .filter_map(|e| {
            let since_last = forward(last, e.at as f32);
            (since_last > 0.0 && since_last <= span).then_some((since_last, e))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, e)| e)
}

// ----------------------------------------------------------------- ambient --

/// Brightness for a light level: logarithmic, `min` in the dark up to `max`
/// at 1000 lux and above.
pub fn ambient_brightness(lux: f32, min: u8, max: u8) -> u8 {
    let (lo, hi) = (min.min(max) as f32, max.max(min) as f32);
    let frac = ((lux.max(0.0) + 1.0).log10() / 1001f32.log10()).clamp(0.0, 1.0);
    (lo + (hi - lo) * frac).round() as u8
}

/// Brightness that follows the built-in display (0-1).
pub fn follow_brightness(builtin: f32, min: u8, max: u8) -> u8 {
    let (lo, hi) = (min.min(max) as f32, max.max(min) as f32);
    (lo + (hi - lo) * builtin.clamp(0.0, 1.0)).round() as u8
}

/// "21:05" for minutes after midnight.
pub fn format_time(minutes: u16) -> String {
    let m = minutes % DAY;
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Step a time of day by `delta` minutes, wrapping around midnight.
pub fn step_time(minutes: u16, delta: i32) -> u16 {
    (minutes as i32 + delta).rem_euclid(DAY as i32) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn night() -> BlueSchedule {
        BlueSchedule { enabled: true, start: 21 * 60, end: 7 * 60, kelvin: 3400, fade_min: 30 }
    }

    fn at(h: u16, m: u16) -> f32 {
        (h * 60 + m) as f32
    }

    #[test]
    fn the_schedule_spans_midnight_and_fades() {
        let s = night();
        assert_eq!(schedule_kelvin(&s, at(20, 59)), None);
        assert_eq!(schedule_kelvin(&s, at(21, 0)), Some(6500), "starts neutral");
        assert_eq!(schedule_kelvin(&s, at(21, 15)), Some(4950), "half way through the fade");
        assert_eq!(schedule_kelvin(&s, at(21, 30)), Some(3400));
        assert_eq!(schedule_kelvin(&s, at(2, 0)), Some(3400), "after midnight");
        assert_eq!(schedule_kelvin(&s, at(6, 45)), Some(4950), "fading out");
        assert_eq!(schedule_kelvin(&s, at(7, 0)), None);
        assert_eq!(schedule_kelvin(&s, at(12, 0)), None);
    }

    #[test]
    fn a_disabled_or_empty_schedule_does_nothing() {
        assert_eq!(schedule_kelvin(&BlueSchedule { enabled: false, ..night() }, at(23, 0)), None);
        assert_eq!(schedule_kelvin(&BlueSchedule { start: 600, end: 600, ..night() }, at(10, 0)), None);
    }

    #[test]
    fn a_short_window_halves_the_fade() {
        let s = BlueSchedule { start: 22 * 60, end: 22 * 60 + 20, fade_min: 60, ..night() };
        assert_eq!(schedule_kelvin(&s, at(22, 10)), Some(3400), "peak in the middle");
        let none = BlueSchedule { fade_min: 0, ..night() };
        assert_eq!(schedule_kelvin(&none, at(21, 0)), Some(3400), "no fade: full at once");
    }

    fn timed(at: u16, preset: &str) -> TimedPreset {
        TimedPreset { id: preset.into(), enabled: true, at, preset: preset.into() }
    }

    #[test]
    fn a_timed_preset_fires_once_when_its_minute_passes() {
        let list = [timed(8 * 60, "day"), timed(20 * 60, "evening")];
        assert_eq!(due_preset(&list, at(7, 59), at(8, 0)).unwrap().preset, "day");
        assert!(due_preset(&list, at(8, 0), at(8, 1)).is_none(), "not again");
        assert!(due_preset(&list, at(8, 0), at(8, 0)).is_none());
    }

    #[test]
    fn after_sleep_only_the_latest_passed_preset_fires() {
        let list = [timed(8 * 60, "day"), timed(20 * 60, "evening")];
        assert_eq!(due_preset(&list, at(7, 0), at(21, 0)).unwrap().preset, "evening");
        assert_eq!(due_preset(&list, at(19, 0), at(9, 0)).unwrap().preset, "day", "across midnight");
        let off = [TimedPreset { enabled: false, ..timed(8 * 60, "day") }];
        assert!(due_preset(&off, at(7, 59), at(8, 0)).is_none());
    }

    #[test]
    fn ambient_light_maps_logarithmically() {
        assert_eq!(ambient_brightness(0.0, 10, 90), 10);
        assert_eq!(ambient_brightness(1000.0, 10, 90), 90);
        assert_eq!(ambient_brightness(50_000.0, 10, 90), 90);
        let dim_room = ambient_brightness(30.0, 10, 90);
        assert!(dim_room > 30 && dim_room < 60, "30 lux is a dim room, got {dim_room}");
        assert_eq!(ambient_brightness(1000.0, 90, 10), 90, "min/max swapped");
        assert_eq!(follow_brightness(0.5, 20, 80), 50);
    }

    #[test]
    fn times_format_and_step_around_midnight() {
        assert_eq!(format_time(21 * 60 + 5), "21:05");
        assert_eq!(step_time(23 * 60 + 45, 15), 0);
        assert_eq!(step_time(0, -15), 23 * 60 + 45);
    }
}
