//! Coord configuration (E-016).
//!
//! Resolution precedence, lowest to highest: **built-in defaults → TOML config
//! file (written by `chapr-coord setup`) → environment variables**. So an admin
//! keeps a readable `coord.toml`, and an operator can still override any single
//! value with a `CHAPR_COORD_*` env var without editing the file.
//!
//! The same env-var names/defaults that coord has always used are preserved, so
//! nothing that set them before changes behaviour.

use chapr_proto::BackendKind;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// TLS material for serving HTTPS (concept §13.1 transport). Both paths are
/// PEM files the admin/PKI provides — the wizard does not generate certs.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

/// A backend route: a canonical path prefix (a share root, e.g. `\\srv\share`)
/// mapped to the backend kind that owns it. Longest-prefix wins at `resolve`
/// (§14). Empty by default — the single global `Config::backend` covers the
/// common one-backend deployment; routes exist for static mixed-backend
/// topology. This is the one path-shaped bit of the discovery seam (invariant
/// 5): `prefix` becomes a scope when identity generalizes to an opaque locator.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct BackendRoute {
    pub prefix: String,
    pub kind: BackendKind,
}

/// Fully-resolved coord configuration.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub db_url: String,
    pub addr: String,
    pub blob_root: String,
    /// Connection auth mode: `disabled` | `trusted-header` | `negotiate`.
    pub auth: String,
    pub reap_secs: u64,
    pub gc_secs: u64,
    pub watch_dir: Option<String>,
    pub share_unc: Option<String>,
    pub tls: Option<TlsConfig>,
    /// The global backend kind announced on `resolve` when no route matches (§14).
    pub backend: BackendKind,
    /// Longest-prefix backend routes for static mixed-backend topology. Empty by
    /// default.
    pub backend_routes: Vec<BackendRoute>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            db_url: "sqlite:chapr-coord.db".into(),
            addr: "127.0.0.1:8787".into(),
            blob_root: "chapr-blobs".into(),
            auth: "disabled".into(),
            reap_secs: 30,
            gc_secs: 86_400,
            watch_dir: None,
            share_unc: None,
            tls: None,
            backend: BackendKind::default(),
            backend_routes: Vec::new(),
        }
    }
}

impl Config {
    /// Load config: defaults, overlaid by an optional TOML file, overlaid by env.
    pub fn load(path: Option<&Path>) -> Result<Config, ConfigError> {
        let mut cfg = match path {
            Some(p) => {
                let text = std::fs::read_to_string(p)
                    .map_err(|e| ConfigError(format!("reading {}: {e}", p.display())))?;
                toml::from_str(&text)
                    .map_err(|e| ConfigError(format!("parsing {}: {e}", p.display())))?
            }
            None => Config::default(),
        };
        cfg.apply_overrides(|k| std::env::var(k).ok());
        Ok(cfg)
    }

    /// Overlay env-style overrides via a lookup fn (real env in prod; a map in
    /// tests, so precedence is testable without touching process globals).
    fn apply_overrides(&mut self, get: impl Fn(&str) -> Option<String>) {
        if let Some(v) = get("CHAPR_COORD_DB") {
            self.db_url = v;
        }
        if let Some(v) = get("CHAPR_COORD_ADDR") {
            self.addr = v;
        }
        if let Some(v) = get("CHAPR_COORD_BLOBS") {
            self.blob_root = v;
        }
        if let Some(v) = get("CHAPR_COORD_AUTH") {
            self.auth = v;
        }
        if let Some(n) = get("CHAPR_COORD_REAP_SECS").and_then(|v| v.parse().ok()) {
            self.reap_secs = n;
        }
        if let Some(n) = get("CHAPR_COORD_GC_SECS").and_then(|v| v.parse().ok()) {
            self.gc_secs = n;
        }
        if let Some(v) = get("CHAPR_COORD_WATCH_DIR") {
            self.watch_dir = Some(v);
        }
        if let Some(v) = get("CHAPR_COORD_SHARE_UNC") {
            self.share_unc = Some(v);
        }
        if let (Some(cert_path), Some(key_path)) =
            (get("CHAPR_COORD_TLS_CERT"), get("CHAPR_COORD_TLS_KEY"))
        {
            self.tls = Some(TlsConfig { cert_path, key_path });
        }
        if let Some(k) = get("CHAPR_COORD_BACKEND").and_then(|v| v.parse().ok()) {
            self.backend = k;
        }
    }

    /// Serialise to TOML for the wizard to write.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}

/// Config load error (bad path or malformed TOML).
#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "config error: {}", self.0)
    }
}
impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn overrides(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn defaults_are_the_historical_values() {
        let c = Config::default();
        assert_eq!(c.addr, "127.0.0.1:8787");
        assert_eq!(c.reap_secs, 30);
        assert_eq!(c.gc_secs, 86_400);
        assert!(c.tls.is_none());
    }

    #[test]
    fn toml_overlays_defaults_partially() {
        // A partial file: unspecified fields fall back to defaults (#[serde(default)]).
        let c: Config = toml::from_str("addr = \"0.0.0.0:9000\"\nauth = \"trusted-header\"\n").unwrap();
        assert_eq!(c.addr, "0.0.0.0:9000");
        assert_eq!(c.auth, "trusted-header");
        assert_eq!(c.db_url, "sqlite:chapr-coord.db"); // default preserved
    }

    #[test]
    fn env_overrides_win_over_file_and_default() {
        let mut c: Config = toml::from_str("addr = \"0.0.0.0:9000\"\n").unwrap();
        c.apply_overrides(overrides(&[
            ("CHAPR_COORD_ADDR", "10.0.0.5:8787"),
            ("CHAPR_COORD_GC_SECS", "3600"),
            ("CHAPR_COORD_TLS_CERT", "/etc/chapr/c.pem"),
            ("CHAPR_COORD_TLS_KEY", "/etc/chapr/k.pem"),
        ]));
        assert_eq!(c.addr, "10.0.0.5:8787"); // env beat the file
        assert_eq!(c.gc_secs, 3600); // env beat the default
        assert_eq!(
            c.tls,
            Some(TlsConfig {
                cert_path: "/etc/chapr/c.pem".into(),
                key_path: "/etc/chapr/k.pem".into()
            })
        );
    }

    #[test]
    fn tls_needs_both_cert_and_key() {
        let mut c = Config::default();
        c.apply_overrides(overrides(&[("CHAPR_COORD_TLS_CERT", "only-cert.pem")]));
        assert!(c.tls.is_none(), "one half of a TLS pair does not enable TLS");
    }

    #[test]
    fn toml_round_trips() {
        let c = Config {
            tls: Some(TlsConfig {
                cert_path: "c.pem".into(),
                key_path: "k.pem".into(),
            }),
            watch_dir: Some("\\\\srv\\share".into()),
            ..Config::default()
        };
        let back: Config = toml::from_str(&c.to_toml()).unwrap();
        assert_eq!(c, back);
    }
}
