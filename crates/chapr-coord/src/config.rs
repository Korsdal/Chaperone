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
    /// The base URL laptops actually connect to — a hostname, not a bind address.
    ///
    /// `addr` binds a socket; this is what a client types. `0.0.0.0` and
    /// `127.0.0.1` are correct for the former and useless as the latter, and
    /// conflating the two handed a real customer `http://127.0.0.1:8787` as the
    /// value to configure on every laptop. They are two different facts about a
    /// deployment and they now have two fields.
    ///
    /// `None` means "derive it": this machine's hostname plus `addr`'s port. Read
    /// it through [`Config::advertised_url`], never by reaching for `addr`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
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
    /// A second authenticator, tried when the primary rejects (slice 6).
    ///
    /// The safe way to change auth modes: run the new one as `auth` with the old
    /// one here, watch the real endpoints come through on the new mode, then
    /// remove this. Without it, a misconfigured cutover is discovered through
    /// support calls from users whose writes started failing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_fallback: Option<String>,
    /// Principals allowed to administer this coordinator (D-029's role model).
    ///
    /// Enforced only when the auth mode actually authenticates (`negotiate`,
    /// `oidc` — E-015). Under `trusted-header` the principal is asserted by the
    /// client and unverifiable, so this is recorded but authorizes nothing; the
    /// admin token is what gates mutations there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admin_principals: Vec<String>,
    /// Config keys currently being overridden by the environment.
    ///
    /// Not part of the config format — it is derived at load. It exists because
    /// `apply_overrides` layers `CHAPR_COORD_*` **on top of** the file, so a
    /// settings UI that let someone edit an env-overridden field would save a
    /// value that is silently discarded on the next load. Recording which keys
    /// are captive lets the UI say so instead of lying.
    #[serde(skip)]
    pub overridden_by_env: Vec<&'static str>,
    /// The file this config was read from, if any.
    ///
    /// Not part of the config *format* — `skip` keeps it out of both directions of
    /// the TOML, so `to_toml` never emits it and a file containing it is not
    /// rejected. It exists because "which file is this coordinator running from"
    /// is the first question in any support call, and the admin overview answers
    /// it without anyone opening a shell.
    #[serde(skip)]
    pub source: Option<std::path::PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            db_url: "sqlite:chapr-coord.db".into(),
            addr: "127.0.0.1:8787".into(),
            public_url: None,
            blob_root: "chapr-blobs".into(),
            auth: "disabled".into(),
            reap_secs: 30,
            gc_secs: 86_400,
            watch_dir: None,
            share_unc: None,
            tls: None,
            backend: BackendKind::default(),
            backend_routes: Vec::new(),
            auth_fallback: None,
            admin_principals: Vec::new(),
            overridden_by_env: Vec::new(),
            source: None,
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
        cfg.source = path.map(|p| p.to_path_buf());
        cfg.apply_overrides(|k| std::env::var(k).ok());
        Ok(cfg)
    }

    /// Overlay env-style overrides via a lookup fn (real env in prod; a map in
    /// tests, so precedence is testable without touching process globals).
    ///
    /// Every override taken is recorded in [`Self::overridden_by_env`] under the
    /// **config field name**, not the variable name — the settings UI marks fields,
    /// and the operator reading it thinks in fields.
    fn apply_overrides(&mut self, get: impl Fn(&str) -> Option<String>) {
        let mut captive = Vec::new();
        if let Some(v) = get("CHAPR_COORD_DB") {
            self.db_url = v;
            captive.push("db_url");
        }
        if let Some(v) = get("CHAPR_COORD_ADDR") {
            self.addr = v;
            captive.push("addr");
        }
        if let Some(v) = get("CHAPR_COORD_PUBLIC_URL") {
            self.public_url = Some(v);
            captive.push("public_url");
        }
        if let Some(v) = get("CHAPR_COORD_BLOBS") {
            self.blob_root = v;
            captive.push("blob_root");
        }
        if let Some(v) = get("CHAPR_COORD_AUTH") {
            self.auth = v;
            captive.push("auth");
        }
        if let Some(n) = get("CHAPR_COORD_REAP_SECS").and_then(|v| v.parse().ok()) {
            self.reap_secs = n;
            captive.push("reap_secs");
        }
        if let Some(n) = get("CHAPR_COORD_GC_SECS").and_then(|v| v.parse().ok()) {
            self.gc_secs = n;
            captive.push("gc_secs");
        }
        if let Some(v) = get("CHAPR_COORD_WATCH_DIR") {
            self.watch_dir = Some(v);
            captive.push("watch_dir");
        }
        if let Some(v) = get("CHAPR_COORD_SHARE_UNC") {
            self.share_unc = Some(v);
            captive.push("share_unc");
        }
        if let (Some(cert_path), Some(key_path)) =
            (get("CHAPR_COORD_TLS_CERT"), get("CHAPR_COORD_TLS_KEY"))
        {
            self.tls = Some(TlsConfig { cert_path, key_path });
            captive.push("tls");
        }
        if let Some(k) = get("CHAPR_COORD_BACKEND").and_then(|v| v.parse().ok()) {
            self.backend = k;
            captive.push("backend");
        }
        self.overridden_by_env = captive;
    }

    /// Serialise to TOML for the wizard to write.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// The coordinator's own data directory — where the database lives.
    ///
    /// Also where the admin token goes, which is the point: this directory is
    /// already restricted to administrators and the service account by the
    /// installer (D-029), so the token's confidentiality is a protection that
    /// already exists rather than a new secret-management problem.
    pub fn data_dir(&self) -> Option<std::path::PathBuf> {
        db_file_path(&self.db_url)
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .filter(|d| !d.as_os_str().is_empty())
    }

    /// The URL to hand a laptop — the **only** place a client-facing URL is built.
    ///
    /// Everything that tells a human or a laptop where the coordinator is goes
    /// through here. The handover used to format `cfg.addr` directly, which is how
    /// `http://127.0.0.1:8787` ended up being the value an administrator was told
    /// to configure on every machine: a bind address is not a URL, and the two
    /// were the same string.
    pub fn advertised_url(&self) -> String {
        if let Some(u) = self
            .public_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            return u.trim_end_matches('/').to_string();
        }
        let scheme = if self.tls.is_some() { "https" } else { "http" };
        // `addr` is a validated SocketAddr, so the last `:` segment is the port
        // for both `1.2.3.4:8787` and `[::]:8787`.
        let port = self.addr.rsplit(':').next().unwrap_or("8787");
        let host = machine_hostname().unwrap_or_else(|| "localhost".to_string());
        format!("{scheme}://{host}:{port}")
    }

    /// Which fields differ between `self` and `other`, by config field name.
    ///
    /// Used to tell an operator what a save actually changed, split into what took
    /// effect and what is waiting for a restart.
    pub fn changed_fields(&self, other: &Config) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut note = |cond: bool, name: &'static str| {
            if cond {
                out.push(name);
            }
        };
        note(self.db_url != other.db_url, "db_url");
        note(self.addr != other.addr, "addr");
        note(self.public_url != other.public_url, "public_url");
        note(self.blob_root != other.blob_root, "blob_root");
        note(self.auth != other.auth, "auth");
        note(self.auth_fallback != other.auth_fallback, "auth_fallback");
        note(self.admin_principals != other.admin_principals, "admin_principals");
        note(self.reap_secs != other.reap_secs, "reap_secs");
        note(self.gc_secs != other.gc_secs, "gc_secs");
        note(self.watch_dir != other.watch_dir, "watch_dir");
        note(self.share_unc != other.share_unc, "share_unc");
        note(self.tls != other.tls, "tls");
        note(self.backend != other.backend, "backend");
        note(self.backend_routes != other.backend_routes, "backend_routes");
        out
    }

    /// Write this config to `path` without risking a truncated file.
    ///
    /// Temp-then-rename, which is the *opposite* of the rule for files on the share
    /// (a rename there carries the source ACL and strips the target's). This is
    /// coord's own local config on its own volume, and it is the file that decides
    /// whether the service can start at all — a half-written one is a coordinator
    /// that will not come back up.
    pub fn write_to(&self, path: &std::path::Path) -> Result<(), ConfigError> {
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml())
            .map_err(|e| ConfigError(format!("writing {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            ConfigError(format!("replacing {}: {e}", path.display()))
        })
    }

    /// Reject a configuration that cannot mean what it says.
    ///
    /// Only rules that are *structurally* wrong live here — whether a path exists
    /// is `setup::probe`'s job, and it runs against a candidate before it is saved.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // `disabled` never fails, so a fallback behind it is unreachable. Saving
        // that would look like a configured cutover and be a no-op.
        if self.auth == "disabled" && self.auth_fallback.is_some() {
            return Err(ConfigError(
                "auth = \"disabled\" never rejects a request, so auth_fallback would never be \
                 reached. Remove the fallback, or make the primary a mode that authenticates."
                    .into(),
            ));
        }
        if self.auth_fallback.as_deref() == Some(self.auth.as_str()) {
            return Err(ConfigError(
                "auth_fallback is the same mode as auth, which tests nothing".into(),
            ));
        }
        for name in [Some(self.auth.as_str()), self.auth_fallback.as_deref()]
            .into_iter()
            .flatten()
        {
            if !matches!(name, "disabled" | "trusted-header" | "negotiate") {
                return Err(ConfigError(format!(
                    "unknown auth mode {name:?}; expected disabled, trusted-header or negotiate"
                )));
            }
        }
        let bind: std::net::SocketAddr = self.addr.parse().map_err(|_| {
            ConfigError(format!("addr {:?} is not a host:port address", self.addr))
        })?;

        // The mistake this catches is not a typo, it is a category error: handing
        // out the bind address as the URL. It is rejected rather than warned about
        // because the resulting config *looks* finished — the service starts, the
        // admin page works on the box, and the failure surfaces only as every
        // laptop being unable to connect.
        if let Some(raw) = self
            .public_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            if !raw.starts_with("http://") && !raw.starts_with("https://") {
                return Err(ConfigError(format!(
                    "public_url {raw:?} needs a scheme — e.g. http://{raw}"
                )));
            }
            const UNREACHABLE: &[&str] = &["0.0.0.0", "127.0.0.1", "::", "::1", "localhost"];
            let host = url_host(raw);
            // A deliberate loopback-only deployment is legitimate (a developer box,
            // a single-machine demo), and there `addr` says so too. It is only a
            // contradiction when the listener is reachable and the URL is not.
            if UNREACHABLE.iter().any(|h| h.eq_ignore_ascii_case(host)) && !bind.ip().is_loopback()
            {
                return Err(ConfigError(format!(
                    "public_url {raw:?} names {host:?}, which no other machine can reach, but \
                     addr {:?} is listening for them. Use the coordinator's hostname — the name \
                     the laptops resolve — not its bind address.",
                    self.addr
                )));
            }
        }
        Ok(())
    }
}

/// The host part of an `http(s)://host[:port][/path]` URL, for validation only.
///
/// Deliberately not a URL parser: coord has no need of one, and the single fact
/// wanted here is "which name did the administrator write down".
fn url_host(url: &str) -> &str {
    let rest = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // IPv6 literals are bracketed, so the brackets — not a colon — delimit the
    // host: `[::1]:8787` must yield `::1`, never `[`.
    if let Some(inner) = authority
        .strip_prefix('[')
        .and_then(|a| a.split_once(']'))
        .map(|(inner, _)| inner)
    {
        return inner;
    }
    authority.split(':').next().unwrap_or(authority)
}

/// This machine's name — what the laptops will actually be connecting to.
///
/// Lives here rather than in the wizard because [`Config::advertised_url`] needs
/// it at every read, not only at install time.
pub fn machine_hostname() -> Option<String> {
    let key = if cfg!(windows) { "COMPUTERNAME" } else { "HOSTNAME" };
    std::env::var(key)
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|h| !h.is_empty())
        })
}

/// Fields a running coordinator can adopt without a restart.
///
/// Deliberately short. Everything else is wired into something built once at
/// bring-up — the bound listener, the open pool, the watcher's OS thread, the
/// background tickers — and pretending otherwise would be the worst kind of
/// setting: one that reports success and changes nothing.
///
/// `auth` is here **because** the admin token is independent of the auth mode. You
/// cannot lock yourself out, which is what makes changing it live safe, and what
/// makes an auth cutover something you can attempt rather than commit to blind.
/// `public_url` is here for a different reason from the auth fields: nothing in
/// the running service reads it. It is display data — the handover, the admin
/// overview, the value an operator copies to a laptop — so correcting a wrong one
/// should not cost a restart of the coordinator every laptop depends on.
pub const RELOADABLE_FIELDS: &[&str] = &["auth", "auth_fallback", "public_url"];

/// The on-disk SQLite file a `db_url` names, if it names one.
///
/// `sqlite:C:/data/coord.db?mode=rwc` → the path; `sqlite::memory:` → `None`.
pub(crate) fn db_file_path(db_url: &str) -> Option<std::path::PathBuf> {
    let rest = db_url.strip_prefix("sqlite:")?;
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    let rest = rest.split('?').next()?;
    // A leading ':' is a SQLite pseudo-target (`:memory:`), not a path.
    if rest.is_empty() || rest.starts_with(':') {
        return None;
    }
    Some(std::path::PathBuf::from(rest))
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

    #[test]
    fn env_overrides_are_recorded_so_the_settings_ui_can_say_so() {
        // The trap this exists for: overrides land *on top of* the file, so a UI
        // that let someone edit an overridden field would save a value that is
        // silently discarded on the next load.
        let mut cfg = Config::default();
        cfg.apply_overrides(overrides(&[
            ("CHAPR_COORD_AUTH", "negotiate"),
            ("CHAPR_COORD_ADDR", "0.0.0.0:9999"),
        ]));
        assert_eq!(cfg.auth, "negotiate");
        assert!(cfg.overridden_by_env.contains(&"auth"));
        assert!(cfg.overridden_by_env.contains(&"addr"));
        assert!(
            !cfg.overridden_by_env.contains(&"blob_root"),
            "only keys actually taken from the environment are captive"
        );
    }

    #[test]
    fn public_url_is_captive_when_the_environment_sets_it() {
        let mut cfg = Config::default();
        cfg.apply_overrides(overrides(&[(
            "CHAPR_COORD_PUBLIC_URL",
            "http://FILESRV01:8787",
        )]));
        assert_eq!(cfg.public_url.as_deref(), Some("http://FILESRV01:8787"));
        assert!(cfg.overridden_by_env.contains(&"public_url"));
    }

    #[test]
    fn advertised_url_uses_an_explicit_value_verbatim() {
        let cfg = Config {
            addr: "0.0.0.0:8787".into(),
            public_url: Some("http://FILESRV01:8787/".into()),
            ..Default::default()
        };
        // Trailing slash trimmed, because callers append `/admin` and `/healthz`.
        assert_eq!(cfg.advertised_url(), "http://FILESRV01:8787");
        assert_eq!(cfg.advertised_url(), format!("{}", cfg.advertised_url()));
    }

    #[test]
    fn advertised_url_derives_host_and_port_when_unset() {
        let cfg = Config {
            addr: "0.0.0.0:9191".into(),
            public_url: None,
            ..Default::default()
        };
        let url = cfg.advertised_url();
        // The port comes from `addr`; the host does not — that is the whole point.
        assert!(url.ends_with(":9191"), "{url}");
        assert!(url.starts_with("http://"), "{url}");
        assert!(!url.contains("0.0.0.0"), "derived the bind address as a host: {url}");

        // TLS decides the scheme, so the handover is not http on an https service.
        let secure = Config {
            tls: Some(TlsConfig {
                cert_path: "c.pem".into(),
                key_path: "k.pem".into(),
            }),
            ..cfg
        };
        assert!(secure.advertised_url().starts_with("https://"));
    }

    #[test]
    fn validate_refuses_a_loopback_url_on_a_reachable_listener() {
        for host in ["0.0.0.0", "127.0.0.1", "localhost", "[::1]"] {
            let cfg = Config {
                addr: "0.0.0.0:8787".into(),
                public_url: Some(format!("http://{host}:8787")),
                ..Default::default()
            };
            let err = cfg
                .validate()
                .expect_err("{host} is not reachable from another machine");
            assert!(err.to_string().contains("laptops resolve"), "{err}");
        }
    }

    /// A single-machine deployment is legitimate, and there the loopback URL is
    /// the truth. The rule is about the *contradiction*, not about loopback.
    #[test]
    fn validate_accepts_a_deliberate_loopback_deployment() {
        let cfg = Config {
            addr: "127.0.0.1:8787".into(),
            public_url: Some("http://127.0.0.1:8787".into()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    #[test]
    fn validate_refuses_a_public_url_without_a_scheme() {
        let cfg = Config {
            addr: "0.0.0.0:8787".into(),
            public_url: Some("FILESRV01:8787".into()),
            ..Default::default()
        };
        let err = cfg.validate().expect_err("a bare host:port is not a URL");
        assert!(err.to_string().contains("needs a scheme"), "{err}");
    }

    #[test]
    fn url_host_extracts_the_name_an_admin_wrote_down() {
        assert_eq!(url_host("http://FILESRV01:8787"), "FILESRV01");
        assert_eq!(url_host("https://coord.example.dk/admin"), "coord.example.dk");
        assert_eq!(url_host("http://[::1]:8787"), "::1");
        assert_eq!(url_host("http://[fe80::1]"), "fe80::1");
        assert_eq!(url_host("FILESRV01"), "FILESRV01");
    }

    #[test]
    fn public_url_is_reloadable_but_addr_is_not() {
        // `public_url` is display data — nothing in the running service reads it,
        // so a wrong one should not cost a restart. `addr` binds the listener.
        assert!(RELOADABLE_FIELDS.contains(&"public_url"));
        assert!(!RELOADABLE_FIELDS.contains(&"addr"));
    }

    #[test]
    fn changed_fields_notices_a_new_public_url() {
        let a = Config::default();
        let b = Config {
            public_url: Some("http://FILESRV01:8787".into()),
            ..Config::default()
        };
        assert!(a.changed_fields(&b).contains(&"public_url"));
    }

    #[test]
    fn nothing_is_captive_without_env_vars() {
        let mut cfg = Config::default();
        cfg.apply_overrides(overrides(&[]));
        assert!(cfg.overridden_by_env.is_empty());
    }

    #[test]
    fn data_dir_is_the_database_directory() {
        let cfg = Config {
            db_url: "sqlite:C:/ProgramData/Chaperone/coord.db?mode=rwc".into(),
            ..Default::default()
        };
        assert_eq!(
            cfg.data_dir(),
            Some(std::path::PathBuf::from("C:/ProgramData/Chaperone"))
        );
        // An in-memory database has no directory — and so no admin token, which
        // makes the admin surface fail closed rather than open.
        let mem = Config {
            db_url: "sqlite::memory:".into(),
            ..Default::default()
        };
        assert_eq!(mem.data_dir(), None);
    }

    #[test]
    fn validate_refuses_a_fallback_that_can_never_be_reached() {
        // `disabled` never rejects, so a fallback behind it is dead config that
        // looks like a configured cutover.
        let cfg = Config {
            auth: "disabled".into(),
            auth_fallback: Some("trusted-header".into()),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.0.contains("never rejects"), "{}", err.0);
    }

    #[test]
    fn validate_refuses_a_fallback_identical_to_the_primary() {
        let cfg = Config {
            auth: "trusted-header".into(),
            auth_fallback: Some("trusted-header".into()),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_refuses_an_unknown_mode_and_a_bad_address() {
        let bad_mode = Config {
            auth: "kerberos-ish".into(),
            ..Default::default()
        };
        assert!(bad_mode.validate().unwrap_err().0.contains("unknown auth mode"));

        let bad_addr = Config {
            addr: "not-an-address".into(),
            ..Default::default()
        };
        assert!(bad_addr.validate().unwrap_err().0.contains("host:port"));
    }

    #[test]
    fn validate_accepts_a_real_cutover() {
        let cfg = Config {
            auth: "negotiate".into(),
            auth_fallback: Some("trusted-header".into()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

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
