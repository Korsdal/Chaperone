# The Windows coordinator MSI

How a coordinator reaches a Windows fileserver (**D-049**, design note in
`specs/coord-windows-packaging-0910.md`).

The package is **entirely declarative**: it places the binary, registers the
service with this deployment's values on its command line, opens the port, and
starts it. **There is no custom action.**

On its **first start** — and only then — the service writes `coord.toml` from
those arguments, creates its data directory and restricts it. It uses the same
`config_from_args` the prompts and the browser page use, so there is still one
definition of a valid config and the three front ends cannot drift. An existing
config is never touched, so a restart or an upgrade cannot overwrite an
administrator's edits.

That is a correction, not a preference. The package used to run `chapr-coord
setup` as a deferred custom action before registering the service. On an
interactive install that action did not run and did not say so, leaving a
registered service pointing at a config nobody had written: the installer sat on
"Starting services" while the service failed, correctly, on a missing file. The
one piece of work everything else depends on does not belong in a step that can
be skipped in silence.

## Build

Once per machine:

```powershell
dotnet tool install --global wix --version 6.0.2
wix extension add --global WixToolset.Firewall.wixext/6.0.2
```

Then:

```powershell
./build-msi.ps1 -Version 0.1.4            # packages build/chapr-coord.exe
./build-msi.ps1 -Version 0.1.4 -Build     # cargo build --release first
```

`-Version` is mandatory and is checked against the workspace version. Versioning
is a human decision here; the script will not pick one.

## Install

```powershell
msiexec /i chapr-coord-0.1.4.msi ^
        COORD_SHARE=\\FS01\share ^
        COORD_URL=http://FS01:8787 ^
        /qn /l*v install.log
```

Afterwards, the values every laptop needs:

```powershell
& "C:\Program Files\Chaperone\chapr-coord.exe" handover --config C:\ProgramData\Chaperone\coord.toml
```

**A silent install prints nothing**, and setup's output — both tokens, the
coordinator URL — is where those values are announced. `handover` is how they are
recovered, which is why it exists (D-048).

## Properties

| Property | Default | Notes |
|---|---|---|
| `COORD_DATA_DIR` | `C:\ProgramData\Chaperone` | Config, database, blobs, tokens, TLS — the one property that places a coordinator. **Never under Program Files** — an uninstall or repair would take the audit trail with it |
| `COORD_DB` | under `COORD_DATA_DIR` | Split-volume override. Must be a `sqlite:` URL with **forward slashes** — setup refuses anything else |
| `COORD_BLOBS` | under `COORD_DATA_DIR` | Split-volume override |
| `COORD_PORT` | `8787` | Drives both the bind address and the firewall rule |
| `COORD_ADDR` | `0.0.0.0:[COORD_PORT]` | Not the config crate's `127.0.0.1` default: a fileserver's coordinator exists to be reached |
| `COORD_URL` | derived from hostname | What laptops type |
| `COORD_SHARE` | — | UNC of the coordinated share |
| `COORD_WATCH_DIR` | — | |
| `COORD_AUTH` | `shared-secret` | |
| `COORD_BACKEND` | `smb` | |
| `COORD_TLS` | `none` | `generate` for a self-signed certificate |
| `COORD_TLS_CERT` / `COORD_TLS_KEY` | — | An internal CA's certificate, which is the better answer |
| `COORD_TLS_HOSTNAME` | machine name | |
| `COORD_FIREWALL` | `1` | `0` to skip the inbound rule |

**The installer does not ask for any of these.** A double-click installs a
running coordinator with defaults; the share is then set on the admin page's
Settings tab, which has the field. Adding a settings page was attempted and
abandoned: splicing a dialog into `WixUI_Minimal` means publishing a control
event on `WelcomeEulaDlg`, which pulls that dialog's fragment in twice and fails
the build on a duplicate `CheckBox` key. WiX's supported route is to copy the
whole dialog set into the project and edit it, and the set ships as a compiled
`wixlib` with no source to copy — so asking here would mean authoring a UI set
from scratch.

**TLS works from an endpoint as of I-018's fix**: the client now reads the
machine's own certificate store as well as the public one, so
`COORD_TLS=generate` plus installing the generated `coord.crt` in each laptop's
Trusted Root store is a working deployment — and `CHAPR_COORD_CA_CERT` on the
endpoint is the alternative for a machine where that store cannot be reached.
That distribution step is a fleet action; name it, or it silently does not happen
and every laptop refuses the connection.

## What upgrade and uninstall do

- **Upgrade** (`MajorUpgrade`) replaces the binary and re-registers the service.
  `coord.toml` is left exactly as the administrator has it — the service only
  writes a config when there is none. It also does not migrate the schema; the
  binary does that itself at start-up (C0), which is the half an installer
  cannot do.
- **Uninstall** removes the service, the binary and the firewall rule and
  **keeps every byte** in `COORD_DATA_DIR`. Nothing under that path is authored
  as an MSI component, so no uninstall or repair can reach it. That is D-048's
  promise made structural rather than remembered.

## Verify

**CI runs this on every push.** The `packaging` job installs the MSI on a hosted
Windows runner — a clean machine that runs elevated, which is exactly what this
script needs — and runs the whole thing with `-Port 18899 -Uninstall`. So the
installer is exercised on every commit rather than whenever someone has an
elevated shell free. The residual gap, stated rather than hidden: the *exact* MSI
that gets published is never installed before publication. It is a fresh compile
of a verified commit, and the release is a draft a human reviews.

`./verify-msi.ps1 -Uninstall` installs on the current machine, asserts the things
that had never executed outside compilation — service registered, auto-start,
running, both tokens on disk, firewall rule, `/healthz` answering, bound to all
interfaces — then uninstalls and asserts the data survived. **Elevated, and it
refuses to run over an existing install.**

`-DataDir` and `-Port` drive the install as well as the assertions, so a run with
non-default values proves the single-property claim rather than only the default
path.

### On a fileserver, not a laptop

The VM needs **nothing installed**: Windows Installer is part of the OS, the MSI
carries the binary, and the binary is static (D-032). No .NET, no Rust, no WiX, no
internet — which matters on a lab switch that has none. Copy two files across: the
`.msi` and `verify-msi.ps1`.

```powershell
# elevated, on the server
C:\rig\verify-msi.ps1 -Msi C:\rig\chapr-coord-0.1.4.msi `
                      -Port 18899 -Share \\CHAPR-FS\chaprtest `
                      -Url http://CHAPR-FS:18899 -Uninstall
```

Then the check no single machine can make, from a client: `Invoke-WebRequest
http://<server>:<port>/healthz`. That is what proves the firewall rule, and it is
the one thing a local `/healthz` cannot tell you.

**Known limitation: the MSI does not adopt a hand-installed service.** A machine
that already has a `chapr-coord` service registered — every coordinator installed
before this package existed — fails the install with MSI error **1923**, which
says nothing useful. Remove the old registration first (`sc.exe delete
chapr-coord`, elevated) and install fresh; the data directory is untouched by
either step. Turning 1923 into a sentence needs a registry search plus a
rescheduled `LaunchConditions`, and is not written.
