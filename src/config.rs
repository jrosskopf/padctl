use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{AppError, Result};

pub const CONFIG_FILE: &str = "padctl.toml";

/// Persisted device configuration saved to `padctl.toml` in CWD.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Last successfully connected device address (AA:BB:CC:DD:EE:FF).
    #[serde(default)]
    pub device_addr: Option<String>,

    /// BLE advertised name to scan for when no address is known.
    #[serde(default = "default_device_name")]
    pub device_name: String,

    /// Google Fit OAuth 2.0 client ID (from Google Cloud Console).
    #[serde(default)]
    pub fit_client_id: Option<String>,

    /// Google Fit OAuth 2.0 client secret.
    #[serde(default)]
    pub fit_client_secret: Option<String>,

    /// Persisted refresh token; set automatically after first OAuth flow.
    #[serde(default)]
    pub fit_refresh_token: Option<String>,
}

fn default_device_name() -> String {
    "WalkingPad".into()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            device_addr: None,
            device_name: default_device_name(),
            fit_client_id: None,
            fit_client_secret: None,
            fit_refresh_token: None,
        }
    }
}

impl AppConfig {
    /// Pure parse from a TOML string — used directly in tests.
    /// An empty string parses to defaults (all fields are optional).
    pub fn load_from_str(s: &str) -> Result<Self> {
        toml::from_str(s).map_err(|e| AppError::Config(e.to_string()))
    }

    /// Serialize to a TOML string.
    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string(self).map_err(|e| AppError::Config(e.to_string()))
    }

    /// Load from `padctl.toml` in CWD. Returns defaults if the file is absent.
    pub fn load() -> Result<Self> {
        Self::load_from_path(CONFIG_FILE)
    }

    /// Load from an arbitrary path (useful for testing with temp files).
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => Self::load_from_str(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(AppError::Io(e)),
        }
    }

    /// Persist to `padctl.toml` in CWD.
    pub fn save(&self) -> Result<()> {
        self.save_to_path(CONFIG_FILE)
    }

    /// Persist to an arbitrary path (useful for testing).
    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let s = self.to_toml_string()?;
        std::fs::write(path, s)?;
        Ok(())
    }

    pub fn load_with_warning() -> (Self, Option<String>) {
        match Self::load() {
            Ok(cfg) => (cfg, None),
            Err(err) => (
                Self::default(),
                Some(format!("Config load failed: {err}. Using defaults.")),
            ),
        }
    }

    pub fn set_selected_device(&mut self, addr: &str, name: &str) {
        self.device_addr = Some(addr.into());
        self.device_name = name.into();
    }

    pub fn save_selected_device(&mut self, addr: &str, name: &str) -> Result<()> {
        self.set_selected_device(addr, name);
        self.save()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Iter 1: defaults when file/string is empty ────────────────────────

    #[test]
    fn test_config_defaults_from_empty_str() {
        let cfg = AppConfig::load_from_str("").unwrap();
        assert_eq!(cfg.device_name, "WalkingPad");
        assert!(cfg.device_addr.is_none());
    }

    #[test]
    fn test_config_defaults_device_name() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.device_name, "WalkingPad");
    }

    // ── Iter 2: save + load round-trip ───────────────────────────────────

    #[test]
    fn test_config_round_trip_with_addr() {
        let cfg = AppConfig {
            device_addr: Some("AA:BB:CC:DD:EE:FF".into()),
            device_name: "KingSmith Z1".into(),
            ..AppConfig::default()
        };
        let s = cfg.to_toml_string().unwrap();
        let loaded = AppConfig::load_from_str(&s).unwrap();
        assert_eq!(loaded.device_addr.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
        assert_eq!(loaded.device_name, "KingSmith Z1");
    }

    #[test]
    fn test_config_round_trip_no_addr() {
        let cfg = AppConfig::default();
        let s = cfg.to_toml_string().unwrap();
        let loaded = AppConfig::load_from_str(&s).unwrap();
        assert!(loaded.device_addr.is_none());
        assert_eq!(loaded.device_name, "WalkingPad");
    }

    // ── Iter 3: addr persisted in TOML string ────────────────────────────

    #[test]
    fn test_config_addr_appears_in_toml() {
        let cfg = AppConfig {
            device_addr: Some("11:22:33:44:55:66".into()),
            ..AppConfig::default()
        };
        let s = cfg.to_toml_string().unwrap();
        assert!(s.contains("11:22:33:44:55:66"), "TOML:\n{s}");
        let loaded = AppConfig::load_from_str(&s).unwrap();
        assert_eq!(loaded.device_addr, Some("11:22:33:44:55:66".into()));
    }

    // ── File I/O: missing file → defaults ────────────────────────────────

    #[test]
    fn test_config_load_missing_file_returns_defaults() {
        let cfg = AppConfig::load_from_path("/tmp/padctl_nonexistent_abc123.toml").unwrap();
        assert_eq!(cfg.device_name, "WalkingPad");
        assert!(cfg.device_addr.is_none());
    }

    // ── File I/O: save + load via real file ──────────────────────────────

    #[test]
    fn test_config_save_and_load_file() {
        let path = std::env::temp_dir().join("padctl_test_config.toml");
        let cfg = AppConfig {
            device_addr: Some("AA:BB:CC:DD:EE:FF".into()),
            device_name: "TestPad".into(),
            ..AppConfig::default()
        };
        cfg.save_to_path(&path).unwrap();
        let loaded = AppConfig::load_from_path(&path).unwrap();
        assert_eq!(loaded.device_addr.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
        assert_eq!(loaded.device_name, "TestPad");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_config_partial_toml_uses_defaults_for_missing_fields() {
        // Only device_name set — device_addr should default to None
        let cfg = AppConfig::load_from_str(r#"device_name = "MyPad""#).unwrap();
        assert_eq!(cfg.device_name, "MyPad");
        assert!(cfg.device_addr.is_none());
    }

    #[test]
    fn test_config_save_selected_device_updates_in_memory() {
        let mut cfg = AppConfig::default();

        cfg.set_selected_device("11:22:33:44:55:66", "DeskPad");
        assert_eq!(cfg.device_addr.as_deref(), Some("11:22:33:44:55:66"));
        assert_eq!(cfg.device_name, "DeskPad");
    }

    #[test]
    fn test_config_load_with_warning_on_invalid_toml() {
        let path = std::env::temp_dir().join("padctl_test_invalid.toml");
        std::fs::write(&path, "device_name = [").unwrap();

        let loaded = AppConfig::load_from_path(&path);
        assert!(loaded.is_err());

        std::fs::remove_file(&path).ok();
    }
}
