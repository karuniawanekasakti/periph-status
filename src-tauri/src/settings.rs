use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Send a notification when the battery is <= this value (percent).
    pub threshold: u8,
    /// Polling interval in seconds.
    pub interval_secs: u64,
    pub notify: bool,
    /// Also show paired devices that are currently disconnected.
    pub show_disconnected: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            threshold: 20,
            interval_secs: 60,
            notify: true,
            show_disconnected: false,
        }
    }
}

impl Settings {
    pub const MIN_INTERVAL: u64 = 15;
    pub const MAX_INTERVAL: u64 = 3600;

    /// Clamp values that are out of a sensible range.
    pub fn sanitized(mut self) -> Self {
        self.threshold = self.threshold.clamp(1, 95);
        self.interval_secs = self.interval_secs.clamp(Self::MIN_INTERVAL, Self::MAX_INTERVAL);
        self
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps() {
        let s = Settings {
            threshold: 0,
            interval_secs: 1,
            ..Default::default()
        }
        .sanitized();
        assert_eq!((s.threshold, s.interval_secs), (1, Settings::MIN_INTERVAL));
    }

    #[test]
    fn missing_fields_use_defaults() {
        let s: Settings = serde_json::from_str(r#"{"threshold": 30}"#).unwrap();
        assert_eq!(s.threshold, 30);
        assert_eq!(s.interval_secs, 60);
        assert!(s.notify);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("periph-status-test-{}", std::process::id()));
        let path = dir.join("settings.json");
        let s = Settings {
            threshold: 35,
            interval_secs: 120,
            notify: false,
            show_disconnected: true,
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
        let _ = std::fs::remove_dir_all(dir);
    }
}
