//! Configuration file management for adsb-decode.
//!
//! Reads/writes `~/.adsb-decode/config.yaml` with receiver settings,
//! database path, dashboard port, and webhook URL.

use std::path::PathBuf;

use crate::types::AdsbError;

/// Full configuration structure.
#[derive(Debug, Clone)]
pub struct Config {
    pub receiver: ReceiverConfig,
    pub database: DatabaseConfig,
    pub dashboard: DashboardConfig,
    pub webhook: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReceiverConfig {
    pub name: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct DashboardConfig {
    pub host: String,
    pub port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            receiver: ReceiverConfig {
                name: "default".into(),
                lat: None,
                lon: None,
            },
            database: DatabaseConfig {
                path: "data/adsb.db".into(),
            },
            dashboard: DashboardConfig {
                host: "127.0.0.1".into(),
                port: 8080,
            },
            webhook: None,
        }
    }
}

/// Get the config directory path (`~/.adsb-decode/`).
pub fn config_dir() -> PathBuf {
    dirs_home().join(".adsb-decode")
}

/// Get the config file path.
pub fn config_file() -> PathBuf {
    config_dir().join("config.yaml")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Load config from `~/.adsb-decode/config.yaml`.
///
/// Returns default config if file doesn't exist.
pub fn load_config() -> Config {
    let path = config_file();
    if !path.exists() {
        return Config::default();
    }

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Config::default(),
    };

    parse_config(&text).unwrap_or_default()
}

/// Save config to `~/.adsb-decode/config.yaml`.
pub fn save_config(config: &Config) -> Result<PathBuf, AdsbError> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| AdsbError::Config(e.to_string()))?;

    let path = config_file();
    let text = serialize_config(config);
    std::fs::write(&path, text).map_err(|e| AdsbError::Config(e.to_string()))?;

    Ok(path)
}

/// Parse simple YAML-like config text.
fn parse_config(text: &str) -> Option<Config> {
    let mut config = Config::default();
    let mut current_section: Option<String> = None;

    for line in text.lines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }

        let is_indented = line.starts_with("  ") || line.starts_with('\t');

        if let Some((key, val)) = stripped.split_once(':') {
            let key = key.trim();
            let val = val.trim();

            if !is_indented {
                if val.is_empty() {
                    current_section = Some(key.to_string());
                } else {
                    current_section = None;
                    // Top-level key with value
                    match key {
                        "webhook" => config.webhook = parse_string_value(val),
                        _ => {}
                    }
                }
            } else if let Some(ref section) = current_section {
                match section.as_str() {
                    "receiver" => match key {
                        "name" => {
                            if let Some(v) = parse_string_value(val) {
                                config.receiver.name = v;
                            }
                        }
                        "lat" => config.receiver.lat = parse_float_value(val),
                        "lon" => config.receiver.lon = parse_float_value(val),
                        _ => {}
                    },
                    "database" => {
                        if key == "path" {
                            if let Some(v) = parse_string_value(val) {
                                config.database.path = v;
                            }
                        }
                    }
                    "dashboard" => match key {
                        "host" => {
                            if let Some(v) = parse_string_value(val) {
                                config.dashboard.host = v;
                            }
                        }
                        "port" => {
                            if let Some(v) = val.parse::<u16>().ok() {
                                config.dashboard.port = v;
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }

    Some(config)
}

fn parse_string_value(val: &str) -> Option<String> {
    if val == "null" || val == "~" || val.is_empty() {
        return None;
    }
    // Strip quotes
    if (val.starts_with('"') && val.ends_with('"'))
        || (val.starts_with('\'') && val.ends_with('\''))
    {
        return Some(val[1..val.len() - 1].to_string());
    }
    Some(val.to_string())
}

fn parse_float_value(val: &str) -> Option<f64> {
    if val == "null" || val == "~" || val.is_empty() {
        return None;
    }
    val.parse().ok()
}

/// Serialize config to YAML-like text.
fn serialize_config(config: &Config) -> String {
    let mut lines = vec!["# adsb-decode configuration".to_string(), String::new()];

    lines.push("receiver:".into());
    lines.push(format!("  name: \"{}\"", config.receiver.name));
    match config.receiver.lat {
        Some(v) => lines.push(format!("  lat: {v}")),
        None => lines.push("  lat: null".into()),
    }
    match config.receiver.lon {
        Some(v) => lines.push(format!("  lon: {v}")),
        None => lines.push("  lon: null".into()),
    }
    lines.push(String::new());

    lines.push("database:".into());
    lines.push(format!("  path: \"{}\"", config.database.path));
    lines.push(String::new());

    lines.push("dashboard:".into());
    lines.push(format!("  host: \"{}\"", config.dashboard.host));
    lines.push(format!("  port: {}", config.dashboard.port));
    lines.push(String::new());

    match &config.webhook {
        Some(url) => lines.push(format!("webhook: \"{url}\"")),
        None => lines.push("webhook: null".into()),
    }

    lines.join("\n") + "\n"
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.receiver.name, "default");
        assert_eq!(config.dashboard.port, 8080);
        assert!(config.webhook.is_none());
    }

    #[test]
    fn test_parse_config() {
        let text = r#"
receiver:
  name: "mystation"
  lat: 35.5
  lon: -82.5

database:
  path: "/tmp/test.db"

dashboard:
  host: "0.0.0.0"
  port: 9090

webhook: "https://example.com/hook"
"#;
        let config = parse_config(text).unwrap();
        assert_eq!(config.receiver.name, "mystation");
        assert_eq!(config.receiver.lat, Some(35.5));
        assert_eq!(config.receiver.lon, Some(-82.5));
        assert_eq!(config.database.path, "/tmp/test.db");
        assert_eq!(config.dashboard.host, "0.0.0.0");
        assert_eq!(config.dashboard.port, 9090);
        assert_eq!(config.webhook, Some("https://example.com/hook".into()));
    }

    #[test]
    fn test_parse_config_null_values() {
        let text = r#"
receiver:
  name: "test"
  lat: null
  lon: ~

webhook: null
"#;
        let config = parse_config(text).unwrap();
        assert!(config.receiver.lat.is_none());
        assert!(config.receiver.lon.is_none());
        assert!(config.webhook.is_none());
    }

    #[test]
    fn test_roundtrip() {
        let config = Config {
            receiver: ReceiverConfig {
                name: "test".into(),
                lat: Some(35.5),
                lon: Some(-82.5),
            },
            database: DatabaseConfig {
                path: "test.db".into(),
            },
            dashboard: DashboardConfig {
                host: "0.0.0.0".into(),
                port: 9090,
            },
            webhook: Some("https://example.com".into()),
        };
        let text = serialize_config(&config);
        let parsed = parse_config(&text).unwrap();
        assert_eq!(parsed.receiver.name, "test");
        assert_eq!(parsed.receiver.lat, Some(35.5));
        assert_eq!(parsed.dashboard.port, 9090);
        assert_eq!(parsed.webhook, Some("https://example.com".into()));
    }
}
