//! What the GUI remembers between runs.
//!
//! Environment variables still set the defaults, so a machine already
//! configured for the headless binary opens the GUI on its own settings;
//! anything the user then changes in the window is what persists. Settings
//! the window does not expose (the pixel-scale prior, `FAINT_LIGHT_FAKE`)
//! keep coming from the environment.

use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Kept as text: an unparseable address should be something the user can
    /// see and correct in the field, not something silently reset.
    pub bind: String,
    pub port: u16,
    pub index_dir: PathBuf,
    pub cache_gb: f64,
    pub solve_timeout_secs: u64,
    /// Start the HTTP server as soon as the window opens.
    pub autostart: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        let cfg = Config::from_env();
        Settings {
            bind: cfg.bind.to_string(),
            port: cfg.port,
            index_dir: cfg.index_dir,
            cache_gb: cfg.cache_gb,
            solve_timeout_secs: cfg.solve_timeout.as_secs(),
            autostart: true,
        }
    }
}

impl Settings {
    /// Load the saved settings, falling back to the environment-derived
    /// defaults if the file is missing or unreadable.
    pub fn load() -> Settings {
        let path = Settings::path();
        match std::fs::read_to_string(&path) {
            // A Windows editor may have left a UTF-8 BOM; JSON has no room
            // for one, and losing every setting to an invisible character
            // is a poor way to find that out.
            Ok(text) => match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("ignoring unreadable {}: {e}", path.display());
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Settings::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self)?)
    }

    /// The server configuration these settings describe. Fields the window
    /// does not own come from the environment.
    pub fn to_config(&self) -> Config {
        Config {
            bind: self.bind_addr(),
            port: self.port,
            index_dir: self.index_dir.clone(),
            cache_gb: self.cache_gb,
            solve_timeout: Duration::from_secs(self.solve_timeout_secs.max(1)),
            ..Config::from_env()
        }
    }

    /// The bind address, or all interfaces if the text is not an address.
    pub fn bind_addr(&self) -> IpAddr {
        self.bind
            .trim()
            .parse()
            .unwrap_or(IpAddr::from([0, 0, 0, 0]))
    }

    pub fn path() -> PathBuf {
        config_dir().join("faint_light").join("gui.json")
    }
}

/// Per-user configuration directory: `%APPDATA%` on Windows,
/// `$XDG_CONFIG_HOME` (else `~/.config`) elsewhere.
pub fn config_dir() -> PathBuf {
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata);
        }
    } else if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg);
    }
    home().map_or_else(|| PathBuf::from("."), |h| h.join(".config"))
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survives_a_json_round_trip() {
        let s = Settings {
            port: 9001,
            bind: "127.0.0.1".into(),
            ..Settings::default()
        };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.port, 9001);
        assert_eq!(back.bind_addr(), IpAddr::from([127, 0, 0, 1]));
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // Settings written by an older build must still load.
        let back: Settings = serde_json::from_str(r#"{"port": 8100}"#).unwrap();
        assert_eq!(back.port, 8100);
        assert_eq!(back.cache_gb, Settings::default().cache_gb);
    }

    #[test]
    fn a_broken_bind_address_falls_back_to_all_interfaces() {
        let s = Settings {
            bind: "not an address".into(),
            ..Settings::default()
        };
        assert_eq!(s.bind_addr(), IpAddr::from([0, 0, 0, 0]));
    }

    #[test]
    fn settings_the_window_does_not_own_come_from_the_environment() {
        let s = Settings {
            port: 9999,
            ..Settings::default()
        };
        let cfg = s.to_config();
        assert_eq!(cfg.port, 9999);
        assert_eq!(cfg.scale_lo, Config::from_env().scale_lo);
        assert_eq!(cfg.fake, Config::from_env().fake);
    }
}
