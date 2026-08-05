# Packaging: endpoint MCPB bundles

The Chaperone endpoint ships to laptops as an **MCPB** — a one-click MCP bundle
that Claude Desktop installs (Settings → Extensions). An `.mcpb` is a zip of a
`manifest.json` + the compiled `chapr-endpoint` binary. Because Chaperone derives
identity from the OS logon (D-024), the **only** thing a user configures is the
coordinator URL.

> Spec: <https://github.com/modelcontextprotocol/mcpb> · manifest reference:
> `MANIFEST.md` in that repo. CLI: `npm i -g @anthropic-ai/mcpb`.

## One bundle per client OS

The MCPB is per **client OS**, not per fileserver — each bundle carries the
backend(s) that OS can drive; the coordinator announces which backend to use and
the endpoint confirms it locally. For this project we ship **Windows** (SMB); the
same template produces macOS/Linux (POSIX) bundles.

| Placeholder | Windows (SMB) | Linux/macOS (POSIX) |
|---|---|---|
| `{{PLATFORM}}` | `win32` | `linux` / `darwin` |
| `{{EXE_SUFFIX}}` | `.exe` | *(empty)* |
| `{{BUNDLE_NAME}}` | `chaperone-endpoint` | `chaperone-endpoint` |

## Build

1. Copy `manifest.template.json` → `manifest.json` and fill the `{{PLACEHOLDERS}}`.
2. Run the builder:

   ```powershell
   ./build-mcpb.ps1 -Manifest ./manifest.json -Pack
   ```

   It compiles the release binary, assembles `build/` (`manifest.json` +
   `server/chapr-endpoint.exe`), then `validate`s + `pack`s it (omit `-Pack` to
   stop after assembly and run `mcpb pack` yourself).
3. Distribute the resulting `.mcpb`. Users double-click it in Claude Desktop and
   enter the coordinator URL when prompted.

## Signing & provenance
Claude Desktop reports **signature** (code-signed & trusted?) and **provenance**
(in Anthropic's public directory?) independently. An internal tool won't be in the
public directory — expected, not an error.

> **Known limitation (mcpb CLI 2.1.2):** signing is effectively **broken** —
> `node-forge` throws "PKCS#7 signature verification not yet implemented", so
> `mcpb verify` reports every bundle as unsigned, the produced signature doesn't
> validate with openssl, and Claude Desktop shows signed bundles as "not signed"
> (verified empirically). **Ship unsigned for now** and accept the unverified-
> publisher note; revisit when the toolchain is fixed and validate against Claude
> Desktop's actual requirements before investing in a code-signing cert. Managed
> fleets can pre-approve the `.mcpb` via Intune/GPO instead.

## Notes
- The user-facing `coord_url` field has **no default** (so a non-technical user
  can't click past it onto localhost). A packager MAY set `default` to the *real*
  coordinator URL to pre-fill it — never localhost.
- `manifest_version` is set to `0.3` (the value in the upstream MANIFEST.md
  example, and it passes `mcpb validate`). If a newer CLI reports otherwise, bump it.
- **No comment keys in the manifest.** The template used to carry a `"//"` note and
  `mcpb validate` (CLI 2.1.2) rejects it outright: `Unrecognized key(s) in object:
  '//'`. The schema is closed, so any explanatory key fails validation — keep notes
  here in the README instead. `build-mcpb.ps1` now checks `validate`'s exit code, so
  this fails at build time rather than shipping a bundle that breaks on install.
- The binary is **not** committed here — it is compiled at build time from the
  Chaperone workspace, so the bundle always matches a known source build.
- A concrete instantiation for one customer lives in that customer's own deployable repo
  (`endpoint/manifest.json` + `endpoint/build.ps1`).
