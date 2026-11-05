//! Persistent configuration store for API keys and settings.
//!
//! Users save credentials once, e.g.:
//!
//!     aurifer config set IG_API_KEY <value>
//!
//! Values are stored in a per-user JSON file and loaded into the process
//! environment at startup, so all existing `std::env::var(...)` call sites
//! keep working unchanged.
//!
//! Resolution order (highest priority first):
//!   1. Variables the user set explicitly in the shell
//!   2. Variables loaded from `.env`
//!   3. Values saved via `aurifer config set` / the web settings page
//!   4. Hard-coded defaults in the code

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Well-known keys the user is likely to want to persist.
/// Used for display in the settings UI — nothing enforces these names.
pub const KNOWN_KEYS: &[(&str, &str)] = &[
    ("IG_API_KEY",                  "IG Markets API key"),
    ("IG_USERNAME",                 "IG Markets username"),
    ("IG_PASSWORD",                 "IG Markets password"),
    ("IG_BASE_URL",                 "IG API base URL (leave unset for demo)"),
    ("IG_GOLD_EPIC",                "Market EPIC (default: CS.D.CFDGOLD.CFDGC.IP)"),
    ("IG_ORDER_SIZE",               "Order size per trade (default: 1.0)"),
    ("IG_CACHE_DIR",                "Where to cache IG historical bars (default: ./cache)"),
    ("IG_WEEKLY_DATA_POINT_BUDGET", "Weekly data points budget (default: 10000)"),
    ("AURIFER_CACHE_DIR",           "Where to cache CSV data (default: ./cache)"),
    ("AURIFER_CSV",                 "Path to a manual CSV file to use instead of downloading"),
];

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StoredConfig {
    #[serde(default)]
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CredentialRow {
    pub key: String,
    pub value_masked: String,
    pub is_secret: bool,
    pub is_set: bool,
    pub description: Option<String>,
    /// True if the process env already has a value for this key — used by
    /// the web UI to show where the effective value actually comes from.
    pub from_env_or_dotenv: bool,
}

pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("AURIFER_CONFIG_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    if let Ok(d) = std::env::var("XDG_CONFIG_HOME") {
        if !d.is_empty() {
            return PathBuf::from(d).join("aurifer");
        }
    }
    #[cfg(target_os = "macos")]
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join("Library/Application Support/aurifer");
    }
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata).join("aurifer");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config").join("aurifer");
    }
    PathBuf::from(".aurifer")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn load() -> Result<StoredConfig> {
    let path = config_path();
    if !path.exists() {
        return Ok(StoredConfig::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading config file {}", path.display()))?;
    let cfg: StoredConfig = serde_json::from_str(&text)
        .with_context(|| format!("parsing config file {}", path.display()))?;
    Ok(cfg)
}

pub fn save(cfg: &StoredConfig) -> Result<PathBuf> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating config dir {}", dir.display()))?;
    let path = dir.join("config.json");
    let text = serde_json::to_string_pretty(cfg)?;
    std::fs::write(&path, text)
        .with_context(|| format!("writing config file {}", path.display()))?;
    // Best-effort tightening: 0600 on Unix. Silently ignored elsewhere.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

fn normalize_key(k: &str) -> String {
    k.trim().to_ascii_uppercase()
}

pub fn set(key: &str, value: &str) -> Result<PathBuf> {
    let mut cfg = load()?;
    cfg.values.insert(normalize_key(key), value.to_string());
    save(&cfg)
}

/// Save to disk AND set the process env var. The env update takes effect
/// for any code that reads env vars **after** this call. Already-initialized
/// resources (e.g. an established IG connection) will not pick up the
/// change until the app is restarted.
#[allow(unused_unsafe)]
pub fn set_and_apply(key: &str, value: &str) -> Result<PathBuf> {
    let path = set(key, value)?;
    unsafe {
        std::env::set_var(normalize_key(key), value);
    }
    Ok(path)
}

pub fn get(key: &str) -> Result<Option<String>> {
    let cfg = load()?;
    Ok(cfg.values.get(&normalize_key(key)).cloned())
}

pub fn unset(key: &str) -> Result<(bool, PathBuf)> {
    let mut cfg = load()?;
    let removed = cfg.values.remove(&normalize_key(key)).is_some();
    let path = save(&cfg)?;
    Ok((removed, path))
}

/// List all credentials for display in the settings UI.
///
/// Known keys are always included (even if not set, so the user can
/// discover them); custom keys are appended alphabetically. Secret values
/// are always masked — the API never returns raw secrets.
pub fn list_all() -> Result<Vec<CredentialRow>> {
    let cfg = load()?;
    let mut rows: Vec<CredentialRow> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for &(key, desc) in KNOWN_KEYS {
        let is_set = cfg.values.contains_key(key);
        let secret = is_secret(key);
        let value_masked = match cfg.values.get(key) {
            Some(v) if secret => mask(v),
            Some(v) => v.clone(),
            None => String::new(),
        };
        rows.push(CredentialRow {
            key: key.to_string(),
            value_masked,
            is_secret: secret,
            is_set,
            description: Some(desc.to_string()),
            from_env_or_dotenv: std::env::var_os(key).is_some(),
        });
        seen.insert(key.to_string());
    }

    for (key, val) in &cfg.values {
        if seen.contains(key.as_str()) {
            continue;
        }
        let secret = is_secret(key);
        let value_masked = if secret { mask(val) } else { val.clone() };
        rows.push(CredentialRow {
            key: key.clone(),
            value_masked,
            is_secret: secret,
            is_set: true,
            description: None,
            from_env_or_dotenv: std::env::var_os(key).is_some(),
        });
    }

    rows.sort_by(|a, b| {
        let a_pos = KNOWN_KEYS
            .iter()
            .position(|&(k, _)| k == a.key.as_str())
            .unwrap_or(usize::MAX);
        let b_pos = KNOWN_KEYS
            .iter()
            .position(|&(k, _)| k == b.key.as_str())
            .unwrap_or(usize::MAX);
        a_pos.cmp(&b_pos).then_with(|| a.key.cmp(&b.key))
    });

    Ok(rows)
}

/// Load stored values into the process environment — but only for keys
/// not already set (by the shell or by `.env`). Call this AFTER
/// `dotenvy::dotenv()` so that `.env` wins over saved settings.
#[allow(unused_unsafe)]
pub fn apply_to_env() {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("⚠️  could not load stored config: {e}");
            return;
        }
    };
    for (k, v) in &cfg.values {
        if std::env::var_os(k).is_none() {
            // `set_var` is safe in Rust 2021 but `unsafe` in Rust 2024;
            // the `#[allow(unused_unsafe)]` on the fn covers both.
            unsafe {
                std::env::set_var(k, v);
            }
        }
    }
}

/// Should the value for `key` be masked when listed?
pub fn is_secret(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    k.contains("KEY")
        || k.contains("PASSWORD")
        || k.contains("PASSWD")
        || k.contains("TOKEN")
        || k.contains("SECRET")
}

/// Show only the first 4 characters, mask the rest.
pub fn mask(v: &str) -> String {
    let n = v.chars().count();
    if n <= 4 {
        return "*".repeat(n);
    }
    let visible: String = v.chars().take(4).collect();
    format!("{}{}", visible, "*".repeat(n - 4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_short_and_long() {
        assert_eq!(mask(""), "");
        assert_eq!(mask("ab"), "**");
        assert_eq!(mask("abcd"), "****");
        assert_eq!(mask("abcde"), "abcd*");
        assert_eq!(mask("my-api-key-123"), "my-a**********");
    }

    #[test]
    fn secrets_are_flagged() {
        assert!(is_secret("IG_API_KEY"));
        assert!(is_secret("IG_PASSWORD"));
        assert!(is_secret("fxmacrodata_api_key"));
        assert!(!is_secret("IG_USERNAME"));
        assert!(!is_secret("IG_BASE_URL"));
    }

    #[test]
    fn keys_are_normalized() {
        assert_eq!(normalize_key("ig_api_key"), "IG_API_KEY");
        assert_eq!(normalize_key("  IG_PASSWORD  "), "IG_PASSWORD");
    }
}