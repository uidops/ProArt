//! Settings: one JSON document in the platform config directory.
//!
//! Display keys and the `feature_map` / `unsupported_features` fields use the
//! same shape as the Python "ProArt Bar" config, so [`Settings::import_legacy`]
//! carries learned mappings, names and presets over.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CONFIG_VERSION: u32 = 1;

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PROART_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "macos")]
    {
        home().join("Library/Application Support/ProArt")
    }
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home).join("ProArt")
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"))
            .join("proart")
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// Where the Python "ProArt Bar" kept its config.
pub fn legacy_config_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home().join("Library/Application Support/ProArt Bar/config.json")
    }
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home).join("ProArt Bar/config.json")
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"))
            .join("proart-bar/config.json")
    }
}

/// An opcode (and, for choices, a value per name) learned from the monitor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Learned {
    pub vcp: u8,
    #[serde(default)]
    pub values: BTreeMap<String, u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayPrefs {
    /// User override of the EDID name; empty = EDID name.
    pub name: String,
    pub enabled: bool,
    /// Features hidden for this display.
    pub hidden_features: Vec<String>,
    /// Last known values, shown before the first read.
    pub value_cache: BTreeMap<String, u16>,
    pub feature_map: BTreeMap<String, Learned>,
    /// ProArt features this display does not expose (a learn run found nothing).
    pub unsupported_features: Vec<String>,
}

impl Default for DisplayPrefs {
    fn default() -> Self {
        DisplayPrefs {
            name: String::new(),
            enabled: true,
            hidden_features: Vec::new(),
            value_cache: BTreeMap::new(),
            feature_map: BTreeMap::new(),
            unsupported_features: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// auto | light | dark
    pub theme: String,
    /// Feature ids shown as quick controls in the menu-bar panel, in order.
    pub quick_features: Vec<String>,
    pub show_value_labels: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        UiSettings {
            theme: "auto".into(),
            quick_features: ["preset", "brightness", "contrast", "volume", "input"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            show_value_labels: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalSettings {
    pub sync_all: bool,
    pub live_values: bool,
    pub live_interval_ms: u64,
    pub write_debounce_ms: u64,
    pub confirm_dangerous: bool,
    pub launch_at_login: bool,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        GlobalSettings {
            sync_all: false,
            live_values: true,
            live_interval_ms: 2000,
            write_debounce_ms: 120,
            confirm_dangerous: true,
            launch_at_login: false,
        }
    }
}

/// A named snapshot of monitor settings.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub icon: String,
    /// display key -> feature id -> value
    pub values: BTreeMap<String, BTreeMap<String, u16>>,
    /// dontChange | turnOn | turnOff
    pub power: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub ui: UiSettings,
    pub global: GlobalSettings,
    pub displays: BTreeMap<String, DisplayPrefs>,
    pub presets: Vec<Preset>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: CONFIG_VERSION,
            ui: UiSettings::default(),
            global: GlobalSettings::default(),
            displays: BTreeMap::new(),
            presets: Vec::new(),
        }
    }
}

impl Settings {
    pub fn display_prefs(&mut self, key: &str) -> &mut DisplayPrefs {
        self.displays.entry(key.to_string()).or_default()
    }

    pub fn prefs(&self, key: &str) -> DisplayPrefs {
        self.displays.get(key).cloned().unwrap_or_default()
    }

    /// Load the config; first run imports the Python app's config if present
    /// and writes the result so the import happens once.
    pub fn load() -> Settings {
        let path = config_path();
        let existed = path.exists();
        let s = Self::load_from(&path, Some(&legacy_config_path()));
        if !existed {
            let _ = s.save_to(&path);
        }
        s
    }

    pub fn load_from(path: &Path, legacy: Option<&Path>) -> Settings {
        match fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Settings>(&text) {
                Ok(s) => s,
                Err(_) => {
                    // corrupt: keep a backup, start fresh
                    let _ = fs::rename(path, path.with_extension("json.corrupt"));
                    Settings::default()
                }
            },
            Err(_) => legacy
                .and_then(|p| fs::read_to_string(p).ok())
                .and_then(|t| Settings::import_legacy(&t))
                .unwrap_or_default(),
        }
    }

    /// Import displays (names, learned mappings, "not available" marks) and
    /// presets from a Python "ProArt Bar" config document.
    pub fn import_legacy(text: &str) -> Option<Settings> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let mut s = Settings::default();
        if let Some(displays) = v.get("displays").and_then(|d| d.as_object()) {
            for (key, d) in displays {
                let mut p = DisplayPrefs {
                    name: d.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    enabled: d.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true),
                    ..DisplayPrefs::default()
                };
                if let Some(h) = d.get("hidden_controls").and_then(|x| x.as_array()) {
                    p.hidden_features = h.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                }
                if let Some(c) = d.get("value_cache").and_then(|x| x.as_object()) {
                    for (k, val) in c {
                        if let Some(n) = val.as_u64() {
                            p.value_cache.insert(k.clone(), n.min(u16::MAX as u64) as u16);
                        }
                    }
                }
                if let Some(m) = d.get("feature_map") {
                    if let Ok(map) = serde_json::from_value::<BTreeMap<String, Learned>>(m.clone()) {
                        p.feature_map = map;
                    }
                }
                if let Some(u) = d.get("unsupported_features").and_then(|x| x.as_array()) {
                    p.unsupported_features = u.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                }
                s.displays.insert(key.clone(), p);
            }
        }
        if let Some(presets) = v.get("presets") {
            if let Ok(list) = serde_json::from_value::<Vec<Preset>>(presets.clone()) {
                s.presets = list;
            }
        }
        Some(s)
    }

    pub fn save(&self) -> std::io::Result<PathBuf> {
        self.save_to(&config_path())
    }

    /// Atomic write: temp file in the same directory, then rename.
    pub fn save_to(&self, path: &Path) -> std::io::Result<PathBuf> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".config.{}.tmp", std::process::id()));
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
            f.write_all(b"\n")?;
        }
        fs::rename(&tmp, path)?;
        Ok(path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults_for_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let mut s = Settings::default();
        s.global.sync_all = true;
        s.display_prefs("d1").feature_map.insert(
            "blue_light".into(),
            Learned { vcp: 0xE6, values: [("Level 1".to_string(), 1u16)].into() },
        );
        s.save_to(&path).unwrap();
        let loaded = Settings::load_from(&path, None);
        assert!(loaded.global.sync_all);
        assert_eq!(loaded.displays["d1"].feature_map["blue_light"].vcp, 0xE6);
        // a config from an older/newer build with missing or extra keys still loads
        fs::write(&path, r#"{"global":{"sync_all":true},"unknown":1}"#).unwrap();
        let l = Settings::load_from(&path, None);
        assert!(l.global.sync_all && l.global.live_values);
    }

    #[test]
    fn corrupt_config_is_backed_up_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        fs::write(&path, "{not json").unwrap();
        let s = Settings::load_from(&path, None);
        assert!(s.displays.is_empty());
        assert!(dir.path().join("c.json.corrupt").exists());
    }

    #[test]
    fn imports_the_python_apps_learned_mappings() {
        let legacy = r#"{
          "version": 1,
          "displays": { "edid-06b3-2701-1010101": {
              "name": "", "enabled": true, "hidden_controls": ["sharpness"],
              "value_cache": {"brightness": 50},
              "feature_map": {"preset": {"vcp": 220, "values": {"sRGB Mode": 13}}},
              "unsupported_features": ["sound_source"] } },
          "presets": [{"id": "p1", "name": "Movie", "icon": "x", "power": "dontChange",
                       "values": {"edid-06b3-2701-1010101": {"brightness": 40}}}]
        }"#;
        let s = Settings::import_legacy(legacy).unwrap();
        let d = &s.displays["edid-06b3-2701-1010101"];
        assert_eq!(d.feature_map["preset"].vcp, 220);
        assert_eq!(d.feature_map["preset"].values["sRGB Mode"], 13);
        assert_eq!(d.unsupported_features, vec!["sound_source"]);
        assert_eq!(d.hidden_features, vec!["sharpness"]);
        assert_eq!(d.value_cache["brightness"], 50);
        assert_eq!(s.presets[0].values["edid-06b3-2701-1010101"]["brightness"], 40);
    }
}
