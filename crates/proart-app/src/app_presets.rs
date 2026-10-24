//! Presets per app: while a listed app is in front its preset is applied,
//! and the settings from before come back when another app comes forward.

use proart_core::automation::AppPreset;

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Nothing,
    /// Save the current settings (to restore later), then apply this preset.
    SaveAndApply(String),
    /// Another listed app came forward: apply its preset.
    Apply(String),
    /// No listed app in front any more: restore the saved settings.
    Restore,
}

#[derive(Default)]
pub struct AppSwitcher {
    active: Option<String>,
}

impl AppSwitcher {
    /// `front` came to the front. ProArt itself is ignored, so opening it
    /// while a listed app is active does not undo that app's preset.
    pub fn on_front(&mut self, front: &str, rules: &[AppPreset], own: Option<&str>) -> Step {
        if Some(front) == own || self.active.as_deref() == Some(front) {
            return Step::Nothing;
        }
        match rules.iter().find(|r| r.bundle_id == front && !r.preset.is_empty()) {
            Some(rule) => {
                let first = self.active.is_none();
                self.active = Some(front.to_string());
                if first {
                    Step::SaveAndApply(rule.preset.clone())
                } else {
                    Step::Apply(rule.preset.clone())
                }
            }
            None if self.active.take().is_some() => Step::Restore,
            None => Step::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(bundle: &str, preset: &str) -> AppPreset {
        AppPreset { bundle_id: bundle.into(), name: bundle.into(), preset: preset.into() }
    }

    #[test]
    fn a_listed_app_applies_its_preset_and_leaving_restores() {
        let rules = [rule("com.adobe.Lightroom", "srgb"), rule("com.apple.FinalCut", "rec709")];
        let mut sw = AppSwitcher::default();
        assert_eq!(sw.on_front("com.apple.Safari", &rules, None), Step::Nothing);
        assert_eq!(sw.on_front("com.adobe.Lightroom", &rules, None), Step::SaveAndApply("srgb".into()));
        assert_eq!(sw.on_front("com.adobe.Lightroom", &rules, None), Step::Nothing, "still in front");
        assert_eq!(sw.on_front("com.apple.FinalCut", &rules, None), Step::Apply("rec709".into()), "keeps the first saved settings");
        assert_eq!(sw.on_front("com.apple.Safari", &rules, None), Step::Restore);
        assert_eq!(sw.on_front("com.apple.Finder", &rules, None), Step::Nothing);
    }

    #[test]
    fn opening_proart_does_not_undo_the_app_preset() {
        let rules = [rule("com.adobe.Lightroom", "srgb")];
        let mut sw = AppSwitcher::default();
        sw.on_front("com.adobe.Lightroom", &rules, Some("com.proart.app"));
        assert_eq!(sw.on_front("com.proart.app", &rules, Some("com.proart.app")), Step::Nothing);
        assert_eq!(sw.on_front("com.adobe.Lightroom", &rules, Some("com.proart.app")), Step::Nothing);
    }
}
