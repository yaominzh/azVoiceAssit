use crate::config::{MIN_SILENCE_MS, SPEECH_THRESHOLD, SYSTEM_PROMPT};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub system_prompt: String,
    pub silence_ms: u32,
    pub speech_threshold: f32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            system_prompt: SYSTEM_PROMPT.to_string(),
            silence_ms: MIN_SILENCE_MS,
            speech_threshold: SPEECH_THRESHOLD,
        }
    }
}

impl AppSettings {
    pub fn validate(mut self) -> Self {
        self.silence_ms = self.silence_ms.clamp(300, 5000);
        self.speech_threshold = self.speech_threshold.clamp(0.1, 0.9);
        self
    }

    pub fn load_from(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<AppSettings>(&s).ok())
            .unwrap_or_default()
            .validate()
    }

    pub fn load() -> Self {
        Self::load_from(&crate::config::settings_path())
    }

    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&crate::config::settings_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_config_constants() {
        let s = AppSettings::default();
        assert_eq!(s.system_prompt, SYSTEM_PROMPT);
        assert_eq!(s.silence_ms, MIN_SILENCE_MS);
        assert!((s.speech_threshold - SPEECH_THRESHOLD).abs() < 1e-6);
    }

    #[test]
    fn validate_clamps_out_of_range() {
        let s = AppSettings {
            system_prompt: "x".into(),
            silence_ms: 99_999,
            speech_threshold: 5.0,
        };
        let v = s.validate();
        assert_eq!(v.silence_ms, 5000);
        assert!((v.speech_threshold - 0.9).abs() < 1e-6);
    }

    #[test]
    fn validate_leaves_valid_values_unchanged() {
        let s = AppSettings {
            system_prompt: "ok".into(),
            silence_ms: 700,
            speech_threshold: 0.5,
        };
        let v = s.clone().validate();
        assert_eq!(v, s);
    }

    #[test]
    fn roundtrip_json() {
        let s = AppSettings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("azva_test_settings");
        let path = dir.join("settings.json");
        let s = AppSettings {
            system_prompt: "Test prompt".into(),
            silence_ms: 500,
            speech_threshold: 0.6,
        };
        s.save_to(&path).unwrap();
        let loaded = AppSettings::load_from(&path);
        assert_eq!(loaded, s);
    }
}
