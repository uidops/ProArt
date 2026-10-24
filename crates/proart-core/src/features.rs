//! The ProArt OSD as DDC/CI features.
//!
//! Mirrors the ProArt monitor menu: ProArt Preset, Blue Light Filter, ProArt
//! Palette, Image, Sound, Input Select and System Setup. Each [`Feature`] is
//! one OSD entry, and is either
//!
//! * **MCCS** - defined by the VESA standard (opcode and value meaning are
//!   fixed), supported iff the monitor answers a read; or
//! * **vendor** - an ASUS-specific item. ASUS publishes no opcode table, but
//!   ASUS's own `asus-display-control` CLI reference and a public decompile
//!   of DisplayWidget Center document many of them. Where an opcode *and*
//!   values are documented they are built in and only used if the monitor
//!   answers a read. Anything else keeps its fixed *names* and its
//!   opcode/values are **learned** from the monitor (see [`crate::learn`]).
//!
//! A learned entry always wins over a built-in one, and nothing is ever
//! written to a vendor opcode that is neither documented nor learned.

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Preset,
    Blue,
    Palette,
    Image,
    Sound,
    Input,
    System,
}

impl Group {
    pub const ALL: [Group; 7] = [
        Group::Preset,
        Group::Blue,
        Group::Palette,
        Group::Image,
        Group::Sound,
        Group::Input,
        Group::System,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Group::Preset => "preset",
            Group::Blue => "blue",
            Group::Palette => "palette",
            Group::Image => "image",
            Group::Sound => "sound",
            Group::Input => "input",
            Group::System => "system",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Group::Preset => "ProArt Preset",
            Group::Blue => "Blue Light Filter",
            Group::Palette => "ProArt Palette",
            Group::Image => "Image",
            Group::Sound => "Sound",
            Group::Input => "Input Select",
            Group::System => "System Setup",
        }
    }

    pub fn parse(text: &str) -> Option<Group> {
        let t = text.trim().to_lowercase();
        Group::ALL.into_iter().find(|g| g.id() == t || g.label().to_lowercase() == t)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Slider,
    Choice,
    Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Mccs,
    Vendor,
}

/// ASUS VCP table generation: read 0xEF, high byte of max is 2 on the new table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Gen {
    #[default]
    Legacy,
    New,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub name: &'static str,
    pub code: u16,
}

const fn c(name: &'static str, code: u16) -> Choice {
    Choice { name, code }
}

#[derive(Clone, Debug)]
pub struct Feature {
    pub id: &'static str,
    pub group: Group,
    pub label: &'static str,
    pub kind: Kind,
    pub source: Source,
    /// Default opcode (vendor features without `documented`: only a hint).
    pub vcp: Option<u8>,
    /// Extra opcodes written together with `vcp` (linked sliders).
    pub vcps: &'static [u8],
    pub max: u16,
    pub choices: Vec<Choice>,
    /// Vendor value names to learn when `choices` is empty.
    pub options: &'static [&'static str],
    /// Opcode per VCP table generation (overrides `vcp`).
    pub vcp_gen: &'static [(Gen, u8)],
    /// Only on this table generation.
    pub only_gen: Option<Gen>,
    /// Opcode and values come from ASUS's own tooling (usable without learning).
    pub documented: bool,
    pub dangerous: bool,
    /// Checked on a real ASUS PA278QV.
    pub verified: bool,
    pub action_value: u16,
    pub note: &'static str,
}

impl Feature {
    pub fn vendor(&self) -> bool {
        self.source == Source::Vendor
    }

    pub fn opcode(&self, gen: Gen) -> Option<u8> {
        self.vcp_gen.iter().find(|(g, _)| *g == gen).map(|(_, v)| *v).or(self.vcp)
    }

    fn base(id: &'static str, group: Group, label: &'static str, kind: Kind, source: Source) -> Feature {
        Feature {
            id,
            group,
            label,
            kind,
            source,
            vcp: None,
            vcps: &[],
            max: 100,
            choices: Vec::new(),
            options: &[],
            vcp_gen: &[],
            only_gen: None,
            documented: false,
            dangerous: false,
            verified: false,
            action_value: 1,
            note: "",
        }
    }
}

// ----------------------------------------------------------------- value tables --

/// VCP 0x14 colour temperature on ProArt (DisplayWidget Center). A monitor only
/// accepts the subset its capabilities string lists.
pub const COLOR_TEMPS: &[Choice] = &[
    c("5000 K", 0x04),
    c("5500 K", 0x0E),
    c("6500 K", 0x05),
    c("7500 K", 0x06),
    c("9300 K", 0x08),
    c("12000 K", 0x10),
    c("P3-Theater", 0x0F),
    c("M Model", 0x11),
];

/// VCP 0xE3 calibrated ProArt preset, written as `(preset << 8) | 0xFF`.
pub const PROART_PRESETS: &[Choice] = &[
    c("Native", 0x01FF),
    c("sRGB", 0x05FF),
    c("Adobe RGB", 0x08FF),
    c("Rec. 2020", 0x09FF),
    c("DCI-P3", 0x0AFF),
    c("DICOM", 0x0BFF),
    c("Rec. 709", 0x0EFF),
];

/// VCP 0x72 gamma: high byte = gamma x 50 (80 = 1.8 ... 160 = 2.6).
pub const GAMMAS: &[Choice] = &[
    c("1.8", 80 << 8),
    c("2.0", 100 << 8),
    c("2.2", 120 << 8),
    c("2.4", 140 << 8),
    c("2.6", 160 << 8),
];

/// VCP 0xE4 (ProArt) input range, per the ASUS CLI reference.
pub const INPUT_RANGES: &[Choice] = &[
    c("Auto", 0),
    c("Full", 1),
    c("Limited 16-235", 2),
    c("Limited 16-254", 3),
    c("SDI Full", 4),
];

/// VCP 0xCC OSD language: the codes ASUS ProArt capabilities strings list.
pub const LANGUAGES: &[Choice] = &[
    c("Chinese (Traditional)", 0x01),
    c("English", 0x02),
    c("French", 0x03),
    c("German", 0x04),
    c("Italian", 0x05),
    c("Japanese", 0x06),
    c("Korean", 0x07),
    c("Portuguese", 0x08),
    c("Russian", 0x09),
    c("Spanish", 0x0A),
    c("Turkish", 0x0C),
    c("Chinese (Simplified)", 0x0D),
    c("Croatian", 0x11),
    c("Czech", 0x12),
    c("Dutch", 0x14),
    c("Hungarian", 0x1A),
    c("Polish", 0x1E),
    c("Romanian", 0x1F),
    c("Thai", 0x23),
    c("Ukrainian", 0x24),
    c("Vietnamese", 0x25),
    c("Persian", 0x26),
    c("Indonesian", 0x27),
];

/// VCP 0x60 input source: the connectors ASUS ProArt monitors have, with the
/// values ASUS's own CLI documents (15 DP-1, 16 DP-2, 17 HDMI-1 ...).
pub const INPUT_SOURCES: &[Choice] = &[
    c("DVI 1", 0x03),
    c("DisplayPort 1", 0x0F),
    c("DisplayPort 2", 0x10),
    c("HDMI 1", 0x11),
    c("HDMI 2", 0x12),
    c("HDMI 3", 0x13),
    c("HDMI 4", 0x14),
    c("USB-C 1", 0x1A),
    c("USB-C 2", 0x1B),
];

const PRESET_MODES: &[&str] = &[
    "Standard Mode",
    "sRGB Mode",
    "Rec. 709 Mode",
    "DCI-P3 Mode",
    "Scenery Mode",
    "Reading Mode",
    "Darkroom Mode",
    "Rapid Rendering Mode",
];

const ONOFF: &[&str] = &["Off", "On"];
const OFF_ON: &[Choice] = &[c("Off", 0), c("On", 1)];

fn blue_levels() -> Vec<Choice> {
    vec![c("Level 0", 0), c("Level 1", 1), c("Level 2", 2), c("Level 3", 3), c("Level 4", 4)]
}

// ------------------------------------------------------------------- the list --

fn mccs_slider(id: &'static str, group: Group, label: &'static str, vcp: u8) -> Feature {
    Feature { vcp: Some(vcp), ..Feature::base(id, group, label, Kind::Slider, Source::Mccs) }
}

fn documented_choice(id: &'static str, group: Group, label: &'static str, vcp: u8, choices: &[Choice]) -> Feature {
    Feature {
        vcp: Some(vcp),
        choices: choices.to_vec(),
        documented: true,
        ..Feature::base(id, group, label, Kind::Choice, Source::Vendor)
    }
}

fn learn_choice(id: &'static str, group: Group, label: &'static str, options: &'static [&'static str]) -> Feature {
    Feature { options, ..Feature::base(id, group, label, Kind::Choice, Source::Vendor) }
}

fn learn_slider(id: &'static str, group: Group, label: &'static str) -> Feature {
    Feature::base(id, group, label, Kind::Slider, Source::Vendor)
}

fn build() -> Vec<Feature> {
    use Group::*;
    vec![
        // -- ProArt Preset -----------------------------------------------------
        // PA278QV-class monitors keep the preset in 0xDC. ASUS does not publish
        // the value table: it is learned (`proart learn preset`), and the
        // PA278QV's table ships in MODEL_PROFILES. Calibrated ProArt models
        // use 0xE3 with documented values.
        Feature {
            vcp: Some(0xDC),
            note: "0xDC mode register; value-to-mode table is learned",
            ..learn_choice("preset", Preset, "ProArt Preset", PRESET_MODES)
        },
        Feature {
            note: "calibrated ProArt models; 0xE3 = preset<<8|0xFF",
            ..documented_choice("preset_calibrated", Preset, "ProArt Preset (calibrated)", 0xE3, PROART_PRESETS)
        },
        // -- Blue Light Filter -----------------------------------------------------
        Feature {
            note: "0xE6 reads on the PA278QV but its writes had no effect in the modes tried",
            ..documented_choice("blue_light", Blue, "Blue Light Filter", 0xE6, &blue_levels())
        },
        // -- ProArt Palette --------------------------------------------------------
        Feature { verified: true, ..mccs_slider("brightness", Palette, "Brightness", 0x10) },
        Feature { verified: true, ..mccs_slider("contrast", Palette, "Contrast", 0x12) },
        Feature { note: "MCCS 0x8A; not implemented by the PA278QV", ..mccs_slider("saturation", Palette, "Saturation", 0x8A) },
        Feature { note: "MCCS 0x90; not implemented by the PA278QV", ..mccs_slider("hue", Palette, "Hue", 0x90) },
        Feature {
            vcp: Some(0x14),
            choices: COLOR_TEMPS.to_vec(),
            verified: true,
            note: "PA278QV: 5000 K write verified; locked (ignored) in sRGB mode",
            ..Feature::base("color_temp", Palette, "Color Temp.", Kind::Choice, Source::Mccs)
        },
        Feature { note: "not implemented by the PA278QV", ..documented_choice("gamma", Palette, "Gamma", 0x72, GAMMAS) },
        Feature {
            vcps: &[0x6E, 0x70],
            verified: true,
            note: "red, green and blue black level written together",
            ..mccs_slider("black_level", Palette, "Black Level", 0x6C)
        },
        Feature { verified: true, ..mccs_slider("color_red", Palette, "Color: Red", 0x16) },
        Feature { verified: true, ..mccs_slider("color_green", Palette, "Color: Green", 0x18) },
        Feature { verified: true, ..mccs_slider("color_blue", Palette, "Color: Blue", 0x1A) },
        Feature {
            vcp: Some(0x08),
            dangerous: true,
            note: "MCCS 0x08 restore factory colour defaults (answers on the PA278QV; not fired in testing)",
            ..Feature::base("color_reset", Palette, "Color Reset", Kind::Action, Source::Mccs)
        },
        // -- Image -----------------------------------------------------------------
        Feature { verified: true, ..mccs_slider("sharpness", Image, "Sharpness", 0x87) },
        Feature {
            vcp: Some(0xE0),
            documented: true,
            note: "0xE0 overdrive index; the OSD shows it x20 on ProArt",
            ..learn_slider("trace_free", Image, "Trace Free")
        },
        learn_choice("aspect_control", Image, "Aspect Control", &["Full", "4:3", "1:1"]),
        Feature {
            note: "0xE4 on ProArt models (GamePlus timer on gaming ones)",
            ..documented_choice("input_range", Image, "Input Range", 0xE4, INPUT_RANGES)
        },
        learn_choice("vivid_pixel", Image, "Vivid Pixel", ONOFF),
        Feature {
            only_gen: Some(Gen::Legacy),
            note: "0xE1 on the legacy table; it is Power Saving on the new one",
            ..documented_choice("ascr", Image, "ASCR", 0xE1, OFF_ON)
        },
        // -- Sound -----------------------------------------------------------------
        Feature { verified: true, ..mccs_slider("volume", Sound, "Volume", 0x62) },
        Feature {
            vcp: Some(0x8D),
            note: "1 = mute, 2 = unmute (MCCS); falls back to volume 0 when 0x8D is not implemented",
            ..Feature::base("mute", Sound, "Mute", Kind::Action, Source::Mccs)
        },
        learn_choice("sound_source", Sound, "Source", &["Line In", "HDMI", "DisplayPort"]),
        // -- Input Select ----------------------------------------------------------
        Feature {
            vcp: Some(0x60),
            max: 0xFF,
            choices: INPUT_SOURCES.to_vec(),
            dangerous: true,
            verified: true,
            ..Feature::base("input", Input, "Input Select", Kind::Choice, Source::Mccs)
        },
        Feature {
            note: "ASUS capabilities flag it (FC token 0x10) but no opcode is documented",
            ..learn_choice("auto_detect", Input, "AutoDetection", ONOFF)
        },
        // -- System Setup ----------------------------------------------------------
        Feature {
            note: "a bit in the 0xFD toggle register on gaming/OLED models",
            ..learn_choice("adaptive_sync", System, "Adaptive-Sync", ONOFF)
        },
        Feature {
            vcp_gen: &[(Gen::Legacy, 0xEE), (Gen::New, 0xE1)],
            note: "0xEE on the legacy table, 0xE1 on the new one",
            ..documented_choice("power_saving", System, "Power Saving", 0xEE, OFF_ON)
        },
        Feature { vcp: Some(0xE9), ..learn_slider("osd_timeout", System, "OSD Setup: Timeout") },
        Feature { vcp: Some(0xE9), ..learn_slider("osd_transparency", System, "OSD Setup: Transparency") },
        Feature {
            note: "ASUS lists further patterns for 0xE7 (0x60-0x72) that are unnamed",
            ..documented_choice("quickfit", System, "QuickFit", 0xE7, OFF_ON)
        },
        Feature {
            vcp: Some(0xCC),
            choices: LANGUAGES.to_vec(),
            verified: true,
            ..Feature::base("language", System, "Language", Kind::Choice, Source::Mccs)
        },
        Feature {
            vcp: Some(0x61),
            note: "0x61 appears in ProArt capabilities (values 1-3), meaning undocumented",
            ..learn_choice("dp_stream", System, "DisplayPort Stream", &["DisplayPort 1.1", "DisplayPort 1.2"])
        },
        // PA278QV reads 1 (max 2) with the buttons working. MCCS words 1 as
        // "OSD disabled", ASUS's CLI documents KeyLock 0 off / 1 on on an
        // unpublished opcode. This mapping is inferred from the live state,
        // not verified by writing; hence the confirmation.
        Feature {
            vcp: Some(0xCA),
            choices: vec![c("Off", 1), c("On", 2)],
            dangerous: true,
            note: "inferred: the monitor reads 1 while its buttons work",
            ..Feature::base("key_lock", System, "Key Lock", Kind::Choice, Source::Mccs)
        },
        learn_choice("power_indicator", System, "Power Indicator", ONOFF),
    ]
}

static FEATURES: OnceLock<Vec<Feature>> = OnceLock::new();

pub fn all() -> &'static [Feature] {
    FEATURES.get_or_init(build)
}

pub fn in_group(group: Group) -> impl Iterator<Item = &'static Feature> {
    all().iter().filter(move |f| f.group == group)
}

pub fn get(id: &str) -> Option<&'static Feature> {
    all().iter().find(|f| f.id == id)
}

fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Find a feature by id or label, ignoring case, spaces and punctuation.
pub fn find(text: &str) -> Option<&'static Feature> {
    let t = norm(text);
    all().iter().find(|f| norm(f.id) == t).or_else(|| all().iter().find(|f| norm(f.label) == t))
}

/// Match a typed value to a choice name: exact, then prefix, then substring.
pub fn match_choice<'a, I: IntoIterator<Item = (&'a str, u16)>>(choices: I, text: &str) -> Option<u16> {
    let want = norm(text);
    let all: Vec<(String, u16)> = choices.into_iter().map(|(n, c)| (norm(n), c)).collect();
    let tests: [fn(&str, &str) -> bool; 3] = [
        |n, w| n == w,
        |n, w| n.starts_with(w),
        |n, w| n.contains(w),
    ];
    for test in tests {
        let hits: Vec<u16> = all.iter().filter(|(n, _)| test(n, &want)).map(|(_, c)| *c).collect();
        if hits.len() == 1 {
            return Some(hits[0]);
        }
    }
    None
}

// ------------------------------------------------------------ model profiles --

/// Learned-on-hardware mappings shipped per model (matched on the EDID name).
/// Same shape as a user's learned entry, which always wins.
pub struct ProfileEntry {
    pub feature: &'static str,
    pub vcp: u8,
    pub values: &'static [(&'static str, u16)],
}

pub struct ModelProfile {
    pub model: &'static str,
    pub entries: &'static [ProfileEntry],
}

pub const MODEL_PROFILES: &[ModelProfile] = &[ModelProfile {
    model: "PA278QV",
    entries: &[ProfileEntry {
        // learned from the monitor's OSD on a real PA278QV (value read back from 0xDC)
        feature: "preset",
        vcp: 0xDC,
        values: &[
            ("Standard Mode", 0),
            ("sRGB Mode", 13),
            ("Rec. 709 Mode", 33),
            ("DCI-P3 Mode", 35),
            ("Scenery Mode", 11),
            ("Reading Mode", 23),
            ("Darkroom Mode", 24),
            ("Rapid Rendering Mode", 34),
        ],
    }],
}];

pub fn profile_entry(model_names: &[&str], feature_id: &str) -> Option<&'static ProfileEntry> {
    for name in model_names {
        let upper = name.to_uppercase();
        for p in MODEL_PROFILES {
            if upper.contains(p.model) {
                if let Some(e) = p.entries.iter().find(|e| e.feature == feature_id) {
                    return Some(e);
                }
            }
        }
    }
    None
}

// -------------------------------------------------------------- information --

pub struct InfoItem {
    pub label: &'static str,
    pub vcp: u8,
    pub format: InfoFormat,
}

#[derive(Clone, Copy)]
pub enum InfoFormat {
    VersionBytes,
    Plain,
    CentiHertz,
}

pub const INFO_ITEMS: &[InfoItem] = &[
    InfoItem { label: "Firmware", vcp: 0xC9, format: InfoFormat::VersionBytes },
    InfoItem { label: "Usage Hours", vcp: 0xC0, format: InfoFormat::Plain },
    InfoItem { label: "DDC/CI (MCCS) Version", vcp: 0xDF, format: InfoFormat::VersionBytes },
    InfoItem { label: "Refresh Rate", vcp: 0xAE, format: InfoFormat::CentiHertz },
];

pub fn format_info(format: InfoFormat, value: u16) -> String {
    match format {
        InfoFormat::VersionBytes => format!("{}.{}", value >> 8, value & 0xFF),
        InfoFormat::CentiHertz => format!("{:.2} Hz", value as f64 / 100.0),
        InfoFormat::Plain => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn registry_covers_the_proart_osd() {
        let ids: HashSet<_> = all().iter().map(|f| f.id).collect();
        assert_eq!(ids.len(), all().len(), "feature ids are unique");
        for id in [
            "preset", "blue_light", "brightness", "contrast", "saturation", "hue", "color_temp", "gamma",
            "black_level", "color_red", "color_reset", "sharpness", "trace_free", "aspect_control",
            "input_range", "vivid_pixel", "ascr", "volume", "mute", "sound_source", "input", "auto_detect",
            "adaptive_sync", "power_saving", "osd_timeout", "quickfit", "language", "dp_stream", "key_lock",
            "power_indicator",
        ] {
            assert!(ids.contains(id), "{id}");
        }
        let names: HashSet<_> = get("preset").unwrap().options.iter().copied().collect();
        for n in ["Standard Mode", "sRGB Mode", "Rec. 709 Mode", "DCI-P3 Mode", "Scenery Mode", "Reading Mode",
                  "Darkroom Mode", "Rapid Rendering Mode"] {
            assert!(names.contains(n), "{n}");
        }
        assert_eq!(get("blue_light").unwrap().choices.len(), 5);
    }

    #[test]
    fn documented_values_match_asus_tables() {
        assert_eq!(GAMMAS[2], c("2.2", 0x7800)); // 2.2 reads back as 0x7800 on ASUS
        assert_eq!(PROART_PRESETS[1].code, 0x05FF);
    }

    #[test]
    fn find_ignores_case_and_punctuation() {
        assert_eq!(find("Color Temp.").unwrap().id, "color_temp");
        assert_eq!(find("blue-light").unwrap().id, "blue_light");
        assert_eq!(find("BLUE_LIGHT_FILTER").unwrap().id, "blue_light");
        assert!(find("nope").is_none());
    }

    #[test]
    fn match_choice_prefers_exact_then_prefix() {
        let list = [("6500 K", 5u16), ("65000 K", 9)];
        assert_eq!(match_choice(list.iter().map(|(n, c)| (*n, *c)), "6500k"), Some(5));
        let hdmi = INPUT_SOURCES.iter().map(|c| (c.name, c.code));
        assert_eq!(match_choice(hdmi, "hdmi1"), Some(0x11));
        let hdmi = INPUT_SOURCES.iter().map(|c| (c.name, c.code));
        assert_eq!(match_choice(hdmi, "hdmi"), None, "ambiguous");
    }

    #[test]
    fn pa278qv_profile_is_found_by_edid_name() {
        let e = profile_entry(&["PA278QV"], "preset").unwrap();
        assert_eq!(e.vcp, 0xDC);
        assert!(e.values.contains(&("sRGB Mode", 13)));
        assert!(profile_entry(&["FakeDisplay"], "preset").is_none());
    }

    #[test]
    fn power_saving_opcode_depends_on_generation() {
        let f = get("power_saving").unwrap();
        assert_eq!(f.opcode(Gen::Legacy), Some(0xEE));
        assert_eq!(f.opcode(Gen::New), Some(0xE1));
    }
}
