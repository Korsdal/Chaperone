// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! `chapr-endpoint print-config <host>` — emit this binary's MCP client config.
//!
//! ## Why the binary does this
//!
//! Chaperone's endpoint is a plain MCP server: `rmcp` over stdio, configured
//! entirely by environment variables. Nothing about it is Anthropic-specific —
//! `server.rs` never names a vendor, and "run this command with these environment
//! variables" is something every MCP host can express. So the endpoint already
//! works anywhere MCP does; what was missing was a way to *say so* without making
//! each user hand-assemble a config from prose.
//!
//! The `.mcpb` bundle stays the one-click path for Claude Desktop, but `.mcpb` is
//! **Desktop's install format**. A user who prefers the Claude Code CLI, or any
//! other MCP host, needs a command to point at instead of a bundle to open. This
//! subcommand is that: the binary knows its own absolute path and its own
//! environment contract, so it can print the exact config rather than a template
//! with holes to fill.
//!
//! It is the same idea as D-032 on the coordinator side — *the executable is the
//! installer* — applied to the endpoint's client-side registration.
//!
//! ## What it deliberately does not do
//!
//! It never writes to another application's configuration file. That breaks the
//! moment the host changes its schema, needs an uninstall path to be honest, and
//! silently edits files the user did not ask us to touch. Printing is inspectable
//! and reversible; installing is neither.
//!
//! ## stdout is the artifact, stderr is the advice
//!
//! The config alone goes to stdout, so `print-config generic > .mcp.json` produces
//! a usable file. Everything else — placeholder warnings, what to do next — goes to
//! stderr. The same split the MCP server itself observes, and for the same reason:
//! whatever is on stdout is being consumed by something.

use std::collections::BTreeMap;

/// The MCP server name Chaperone registers under.
///
/// Letters, numbers, hyphens and underscores only: Claude Code treats the key as
/// the server name and rejects anything else, and other hosts are no more liberal.
pub const SERVER_NAME: &str = "chaperone";

/// Host keys `print-config` understands.
///
/// Deliberately short. `generic` is the portable answer — the `mcpServers` shape
/// is what Claude Desktop, Claude Code's `.mcp.json`, and most other clients all
/// read. `claude-code` exists only because its CLI can register a server in one
/// command, which is genuinely easier than editing a file.
///
/// Other hosts are not enumerated on purpose: their formats move, we do not test
/// them, and listing a vendor here would imply verified support.
pub const HOSTS: &[&str] = &["generic", "claude-code"];

/// Placeholder used when the environment does not say what the real value is.
/// Deliberately not a loopback address: a coordinator URL of `127.0.0.1` is the
/// exact mistake D-032 called out, where every laptop is handed a URL that only
/// resolves on the machine that printed it.
const COORD_URL_PLACEHOLDER: &str = "http://coord-host:8787";
const ROOT_PLACEHOLDER: &str = r"\\FILESRV\AICollab";

/// The environment a rendered config will set, plus where each value came from.
pub struct Settings {
    /// Absolute path to this binary, as the host will have to invoke it.
    pub command: String,
    /// `CHAPR_COORD_URL` and `CHAPR_ROOT`: always emitted, because a config
    /// missing them is not usable, and an endpoint with no root is unconfined.
    pub coord_url: String,
    pub root: String,
    /// Everything else, emitted only when this process actually has it set. A
    /// config printed on a configured machine reproduces that machine; one printed
    /// on a fresh machine stays minimal instead of restating defaults.
    pub extra: BTreeMap<String, String>,
    /// Names whose value is a placeholder rather than something we were told.
    pub placeholders: Vec<String>,
}

impl Settings {
    /// The full environment block, in a stable order.
    fn env(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert("CHAPR_COORD_URL".to_string(), self.coord_url.clone());
        m.insert("CHAPR_ROOT".to_string(), self.root.clone());
        m.extend(self.extra.iter().map(|(k, v)| (k.clone(), v.clone())));
        m
    }
}

/// What a render produced: the config itself, and anything the human should know.
#[derive(Debug)]
pub struct Rendered {
    /// Goes to stdout, alone, so it can be redirected into a file.
    pub config: String,
    /// Goes to stderr.
    pub notes: Vec<String>,
}

/// Read the settings out of the process environment.
///
/// Split from [`render`] the way `identity::logged_in_principal` is split from
/// `identity::principal_from_parts`: the impure part is this function and nothing
/// else, so every rendering case is unit-testable without mutating process env.
pub fn settings_from_env() -> Settings {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());

    // A host launches this binary by absolute path; `argv[0]` may be a bare name
    // resolved through PATH, which would produce a config that only works from
    // whatever directory it was printed in.
    let command = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "chapr-endpoint.exe".to_string()
            } else {
                "chapr-endpoint".to_string()
            }
        });

    let mut placeholders = Vec::new();
    let coord_url = get("CHAPR_COORD_URL").unwrap_or_else(|| {
        placeholders.push("CHAPR_COORD_URL".to_string());
        COORD_URL_PLACEHOLDER.to_string()
    });
    let root = get("CHAPR_ROOT").unwrap_or_else(|| {
        placeholders.push("CHAPR_ROOT".to_string());
        ROOT_PLACEHOLDER.to_string()
    });

    let mut extra = BTreeMap::new();
    for k in [
        "CHAPR_BACKEND",
        // The deployment's shared secret. Emitted only when this process has it,
        // like the rest of `extra` — so `print-config` on a configured machine
        // reproduces that machine, and on a fresh one stays silent rather than
        // printing a placeholder that looks like a credential.
        "CHAPR_COORD_TOKEN",
        "CHAPR_PRINCIPAL",
        "CHAPR_MAX_INLINE_BYTES",
        "CHAPR_DIAG_LOG",
        "RUST_LOG",
    ] {
        if let Some(v) = get(k) {
            extra.insert(k.to_string(), v);
        }
    }

    Settings {
        command,
        coord_url,
        root,
        extra,
        placeholders,
    }
}

/// Render the config for `host`. Pure — every input arrives as an argument.
///
/// `Err` carries a message naming the valid hosts, so an unknown key is a usable
/// answer rather than a bare failure.
pub fn render(host: &str, s: &Settings) -> Result<Rendered, String> {
    let config = match host {
        "generic" => render_generic(s),
        "claude-code" => render_claude_code(s),
        other => {
            return Err(format!(
                "unknown host {other:?}. Valid hosts: {}",
                HOSTS.join(", ")
            ))
        }
    };
    Ok(Rendered {
        config,
        notes: notes_for(host, s),
    })
}

/// The `mcpServers` block. Portable: Claude Desktop's config file, Claude Code's
/// `.mcp.json`, and most other clients read this same shape.
///
/// No `"type"` field, deliberately — an entry without one is read as stdio, which
/// is what this is, and hosts differ on whether they accept `"stdio"` there at all.
fn render_generic(s: &Settings) -> String {
    let env: serde_json::Map<String, serde_json::Value> = s
        .env()
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect();
    let doc = serde_json::json!({
        "mcpServers": {
            SERVER_NAME: {
                "command": s.command,
                "env": env,
            }
        }
    });
    // Pretty, not compact: this is going into a config file a human will edit.
    let mut out = serde_json::to_string_pretty(&doc)
        .unwrap_or_else(|e| unreachable!("a map of strings cannot fail to serialize: {e}"));
    out.push('\n');
    out
}

/// The Claude Code CLI one-liner.
///
/// Shape per its documentation: options first, then the server name, then `--`,
/// then the command. The `--` matters — everything after it is the server's own
/// command line, which is what keeps a path with a leading dash from being read as
/// a flag.
fn render_claude_code(s: &Settings) -> String {
    let mut parts = vec![
        "claude".to_string(),
        "mcp".to_string(),
        "add".to_string(),
        "--transport".to_string(),
        "stdio".to_string(),
    ];
    for (k, v) in s.env() {
        parts.push("--env".to_string());
        parts.push(shell_quote(&format!("{k}={v}")));
    }
    parts.push(SERVER_NAME.to_string());
    parts.push("--".to_string());
    parts.push(shell_quote(&s.command));
    format!("{}\n", parts.join(" "))
}

/// Quote an argument only when it needs it.
///
/// Paths on Windows routinely contain spaces (`C:\Program Files\…`), and an
/// unquoted one silently becomes two arguments — the config then looks right and
/// fails to launch. Double quotes rather than single: they work in `cmd`,
/// PowerShell and POSIX shells alike, and no value here can contain one.
fn shell_quote(s: &str) -> String {
    if !s.is_empty() && !s.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'') {
        return s.to_string();
    }
    format!("\"{}\"", s.replace('"', "\\\""))
}

fn notes_for(host: &str, s: &Settings) -> Vec<String> {
    let mut notes = Vec::new();
    if !s.placeholders.is_empty() {
        notes.push(format!(
            "{} not set in this environment — the output contains a placeholder. \
             Replace it before use.",
            s.placeholders.join(" and ")
        ));
    }
    if s.placeholders.iter().any(|p| p == "CHAPR_ROOT") {
        notes.push(
            "CHAPR_ROOT is the coordinated location. It confines the endpoint and is \
             announced to the model; leaving it unset starts an unconfined endpoint."
                .to_string(),
        );
    }
    match host {
        "generic" => notes.push(
            "Merge this into your host's MCP config: Claude Desktop's \
             claude_desktop_config.json, .mcp.json at a project root for Claude Code, \
             or the equivalent for any other MCP client."
                .to_string(),
        ),
        "claude-code" => notes.push(
            "Add --scope project to share it with a repository, or see \
             `print-config generic` for the .mcp.json form."
                .to_string(),
        ),
        _ => {}
    }
    notes
}

/// Entry point for the subcommand. Returns the process exit code.
///
/// `None` for the host lists the valid keys and succeeds: someone who typed
/// `print-config` with nothing else is asking what their options are, and that is
/// not an error.
pub fn run(host: Option<&str>) -> u8 {
    let settings = settings_from_env();
    let Some(host) = host else {
        eprintln!("Usage: chapr-endpoint print-config <{}>", HOSTS.join("|"));
        eprintln!();
        eprintln!("Chaperone's endpoint is a standard MCP server over stdio, so any MCP");
        eprintln!("host can drive it. `generic` prints the portable mcpServers block.");
        eprintln!();
        eprintln!("  command: {}", settings.command);
        return 0;
    };
    match render(host, &settings) {
        Ok(r) => {
            print!("{}", r.config);
            for n in r.notes {
                eprintln!("note: {n}");
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            command: "/opt/chaperone/chapr-endpoint".to_string(),
            coord_url: "https://coord-01:8787".to_string(),
            root: r"\\FILESRV\AICollab".to_string(),
            extra: BTreeMap::new(),
            placeholders: Vec::new(),
        }
    }

    #[test]
    fn every_advertised_host_renders() {
        for h in HOSTS {
            let r = render(h, &settings()).unwrap_or_else(|e| panic!("{h} failed: {e}"));
            assert!(!r.config.trim().is_empty(), "{h} rendered nothing");
        }
    }

    /// The whole point of the subcommand: the config names *this* binary. A
    /// template with a placeholder path would be no better than documentation.
    #[test]
    fn every_host_names_the_resolved_binary_and_the_coordinator() {
        for h in HOSTS {
            let out = render(h, &settings()).unwrap().config;
            assert!(out.contains("/opt/chaperone/chapr-endpoint"), "{h}: no path");
            assert!(out.contains("https://coord-01:8787"), "{h}: no coord url");
        }
    }

    #[test]
    fn generic_is_valid_json_with_the_portable_shape() {
        let out = render("generic", &settings()).unwrap().config;
        let v: serde_json::Value = serde_json::from_str(&out).expect("not valid JSON");
        let entry = &v["mcpServers"][SERVER_NAME];
        assert_eq!(entry["command"], "/opt/chaperone/chapr-endpoint");
        assert_eq!(entry["env"]["CHAPR_COORD_URL"], "https://coord-01:8787");
        assert_eq!(entry["env"]["CHAPR_ROOT"], r"\\FILESRV\AICollab");
        // An entry with no `type` is read as stdio; hosts disagree on whether an
        // explicit "stdio" is even accepted, so emitting one narrows portability.
        assert!(entry.get("type").is_none(), "type should be omitted");
    }

    #[test]
    fn optional_settings_appear_only_when_set() {
        let out = render("generic", &settings()).unwrap().config;
        assert!(!out.contains("RUST_LOG"), "unset value should be omitted");

        let mut s = settings();
        s.extra.insert("RUST_LOG".into(), "debug".into());
        s.extra.insert("CHAPR_BACKEND".into(), "posix".into());
        let v: serde_json::Value =
            serde_json::from_str(&render("generic", &s).unwrap().config).unwrap();
        let env = &v["mcpServers"][SERVER_NAME]["env"];
        assert_eq!(env["RUST_LOG"], "debug");
        assert_eq!(env["CHAPR_BACKEND"], "posix");
    }

    /// `--` separates the CLI's own flags from the server's command line. Without
    /// it a path could be parsed as a flag, so its position is load-bearing.
    #[test]
    fn claude_code_puts_the_command_after_a_double_dash() {
        let out = render("claude-code", &settings()).unwrap().config;
        let (flags, cmd) = out.split_once(" -- ").expect("no `--` separator");
        assert!(cmd.contains("chapr-endpoint"));
        assert!(flags.contains("--transport stdio"));
        assert!(flags.contains("--env CHAPR_COORD_URL=https://coord-01:8787"));
        // The name goes last among the options, immediately before the separator.
        assert!(flags.trim_end().ends_with(SERVER_NAME), "got: {flags}");
    }

    /// A `C:\Program Files\…` path unquoted becomes two arguments, and the config
    /// then looks correct and refuses to launch.
    #[test]
    fn a_path_with_spaces_is_quoted() {
        let mut s = settings();
        s.command = r"C:\Program Files\Chaperone\chapr-endpoint.exe".to_string();
        let out = render("claude-code", &s).unwrap().config;
        assert!(
            out.contains(r#""C:\Program Files\Chaperone\chapr-endpoint.exe""#),
            "got: {out}"
        );
        // And a value that needs no quoting does not get any.
        assert!(out.contains("--env CHAPR_COORD_URL=https://coord-01:8787"));
    }

    #[test]
    fn an_unknown_host_lists_the_valid_ones() {
        let e = render("emacs", &settings()).unwrap_err();
        assert!(e.contains("emacs"), "should name what was asked for: {e}");
        for h in HOSTS {
            assert!(e.contains(h), "should list {h}: {e}");
        }
    }

    #[test]
    fn placeholders_are_flagged_rather_than_passed_off_as_configuration() {
        let mut s = settings();
        s.coord_url = COORD_URL_PLACEHOLDER.to_string();
        s.placeholders = vec!["CHAPR_COORD_URL".to_string()];
        let notes = render("generic", &s).unwrap().notes;
        assert!(
            notes.iter().any(|n| n.contains("CHAPR_COORD_URL")),
            "an unset coordinator URL must be called out: {notes:?}"
        );
    }

    /// An unconfined endpoint is a real posture change, so it is called out
    /// separately from the generic placeholder warning.
    #[test]
    fn an_unset_root_warns_about_confinement() {
        let mut s = settings();
        s.placeholders = vec!["CHAPR_ROOT".to_string()];
        let notes = render("generic", &s).unwrap().notes;
        assert!(
            notes.iter().any(|n| n.contains("unconfined")),
            "got: {notes:?}"
        );
    }
}
