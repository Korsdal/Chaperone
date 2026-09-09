# Packaging: endpoint MCPB bundles

**MCPB is one delivery path, not the only one.** The endpoint is a plain MCP server
over stdio configured entirely by environment variables, so any MCP host can drive
it; `.mcpb` is **Claude Desktop's** install format specifically. For any other host
— the Claude Code CLI included — the artifact is the bare `chapr-endpoint` binary,
and `chapr-endpoint print-config <host>` prints the registration for it. This
directory is only about the Desktop bundle. Keep that distinction in mind before
adding anything here that assumes it is the sole install route.

The bundle is a one-click MCP install that Claude Desktop handles (Settings →
Extensions): a zip of a `manifest.json` + the compiled `chapr-endpoint` binary.
Because Chaperone derives identity from the OS logon (D-024), a user configures only
**two** things: the coordinator URL, and the **coordinated location** — the share
path the endpoint is allowed to act on, which is also announced to the model so it
routes writes through Chaperone (E-025). Both are asked for at install time and
**neither has a default** — see *Configuring a bundle* below for why that is
deliberate rather than an omission.

> Spec: <https://github.com/modelcontextprotocol/mcpb> · manifest reference:
> `MANIFEST.md` in that repo. CLI: `npm i -g @anthropic-ai/mcpb`.

## One bundle per client OS

The MCPB is per **client OS**, not per fileserver — each bundle carries the
backend(s) that OS can drive; the coordinator announces which backend to use and
the endpoint confirms it locally. Releases build all three from this one template:
`win32` (SMB), `linux` and `darwin` (POSIX). macOS is compile-verified only — it
has never been run.

| Placeholder | Windows (SMB) | Linux/macOS (POSIX) |
|---|---|---|
| `{{PLATFORM}}` | `win32` | `linux` / `darwin` |
| `{{EXE_SUFFIX}}` | `.exe` | *(empty)* |
| `{{BUNDLE_NAME}}` | `chaperone-endpoint` | `chaperone-endpoint` |

## Build

The builder takes either an already-instantiated `manifest.json` (`-Manifest`) or the template
plus the values to fill it with (`-Template`). CI uses the second form; the first exists for a
bundle built for one machine, which is the only place pre-filled defaults belong.

```powershell
# From the template — what the release workflow runs:
./build-mcpb.ps1 -Template ./manifest.template.json -Version 0.1.1 -Pack `
                 -Output ./chaperone-endpoint.mcpb

# From an instantiated manifest — what a customer deployable does:
./build-mcpb.ps1 -Manifest ./manifest.json -Pack
```

It compiles the release binary, assembles `build/` (`manifest.json` + `server/chapr-endpoint[.exe]`
+ `LICENSE` + `NOTICE`), then `validate`s and `pack`s it. Omit `-Pack` to stop after assembly.
`-Output <path>` writes the packed bundle straight to a chosen location.

**It does not cross-compile.** `-Platform` (`win32` | `linux` | `darwin`) defaults to the host and
must match it: the script packages the binary cargo just built for *this* machine, and a bundle
labelled `linux` while carrying a `.exe` installs cleanly and then fails to start. The release
workflow therefore builds each OS's bundle on that OS's runner. On PowerShell 7 (Linux/macOS) the
staged binary is `chmod +x`'d, because `Copy-Item` does not carry the mode across.

Placeholders are filled from the parameters, and a leftover `{{PLACEHOLDER}}` is a hard error —
`mcpb validate` is perfectly happy to accept the literal string `{{VERSION}}` as a version.

Released bundles are built by `.github/workflows/release.yml` on a `v*` tag and attached to a draft
GitHub Release. Building one by hand is for local testing and for customer deployables.

**Upgrading an installed bundle.** Claude Desktop identifies a bundle by `name` and `version`, so
two builds carrying the same version are indistinguishable to it and a user can end up running a
stale binary. Released bundles are safe here — the release workflow refuses a tag that does not
match the workspace version, so every published `.mcpb` has a distinct one. **Hand-built bundles
between releases are not**: tell users to remove the existing extension before installing one,
rather than letting them discover the stale binary.

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

## Configuring a bundle — read this before adding a `default`

**The template ships no defaults, and `coordinated_root` is required.** Both came
out of a cowork session against a live share, and the reasoning matters more than
the rule:

- **A `default` only ever reaches a first-time installer.** Stored `user_config`
  is keyed by extension, not by version, so anyone who installed an earlier
  bundle keeps their old value — and a *corrected* default never reaches them.
  During that session a root typo'd as `charptest` survived several edits and a
  new bundle, and the string appeared nowhere in the build. Nothing in the
  endpoint can fix this; only not shipping a wrong value can.
- **A wrong-but-plausible default installs cleanly and coordinates nothing**,
  which is a quieter failure than an empty required field. Hence
  `coordinated_root` is `required: true`: an unconfined endpoint should be a
  deliberate choice, not a blank nobody noticed.
- **Deliberate asymmetry with the binary.** `chapr-endpoint` itself still *warns
  and continues* with no `CHAPR_ROOT`, and should: the bare executable ships for
  every OS (D-035) and a binary that refused to start until configured would be
  unusable for a Claude Code or Cursor user. The manifest is a different
  audience — someone installing a one-click bundle — and can demand more.

So where do the values come from? **The coordinator.** IT runs coord's setup and
distributes what it prints; that handover is the supported path, and it is why
this template asks rather than guesses.

**A private, single-machine bundle may set defaults** — a rig or a demo laptop
where the bundle is never handed to anyone else. Do not do it in anything you
distribute, and never put a real `coord_token` in a bundle you publish: that
publishes the credential.

## Notes
- Keep guidance for packagers **here, not in a `description`**. The host renders
  `description` verbatim in the install dialog, so notes addressed to ourselves
  were read by whoever installed the bundle.
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
