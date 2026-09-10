# Installing the coordinator as a service

The coordinator is a single static binary (`chapr-coord`) with a built-in setup
wizard and native service integration (E-016). Exactly one runs per environment,
on-prem beside the fileserver. **Coord availability == write availability**, so
run it as a managed service and monitor `/healthz`.

## Windows — the MSI

`chapr-coord-<version>-windows-x86_64.msi` from the Releases page does all of it:
placement, service registration and start, and the inbound firewall rule. The
coordinator writes its own config on first start from the values the installer was
given, and never touches one that already exists.

```powershell
msiexec /i chapr-coord-<version>-windows-x86_64.msi `
        COORD_SHARE=\\FS01\Sales COORD_URL=http://FS01:8787 /qn
```

Every property, the split-volume overrides and the uninstall behaviour are in
[`../msi/README.md`](../msi/README.md).

## Windows — from a source build

Only when there is no MSI to hand: the executable is still a complete installer.

1. **Place the binary**, e.g. `C:\Program Files\Chaperone\chapr-coord.exe`.
2. **Configure** — right-click the executable → **Run as administrator**.

   A bare invocation with no arguments runs the wizard, so there is nothing to
   type and no script to run. (It only serves instead if a `coord.toml` sits in
   the working directory, or if there is no console to prompt on — so scripted and
   SCM invocations are unaffected.) Explicitly:

   ```powershell
   chapr-coord setup
   ```

   Four questions decide the install: the **listen address**, the **hostname the
   laptops connect to**, the **share to coordinate** (picked from the machine's own
   shares), and whether to run the change-watcher. It then probes the host, writes
   `coord.toml` under `%ProgramData%\Chaperone`, creates the admin token, installs
   the Windows service, self-tests `/healthz`, and prints the handover.

   Unattended equivalent — note `--public-url`, which is what a laptop connects to
   and is **not** `--addr`:

   ```powershell
   chapr-coord setup --non-interactive `
     --addr 0.0.0.0:8787 `
     --public-url http://FILESERVER:8787 `
     --share-unc \\FILESERVER\Share `
     --db "sqlite:C:/ProgramData/Chaperone/coord.db?mode=rwc" `
     --blobs C:/ProgramData/Chaperone/blobs `
     --auth trusted-header --backend smb
   ```

   Setup refuses a loopback `--public-url` while `--addr` listens for other
   machines, and refuses a `--share-unc` that is not a UNC path. Both were real
   install failures, so they fail loudly now instead of being written to the config.

3. **Service** — the wizard installs it via the Windows SCM; or manually:

   ```powershell
   chapr-coord run-service    # invoked by the SCM; not run directly
   sc.exe start chapr-coord   # start the installed service
   ```

   The service is named `chapr-coord`; `Chaperone coordination service` is its
   display name. This line said `sc.exe start Chaperone`, which starts nothing.

   (SCM install needs an elevated shell.)
4. **Verify**: `curl http://<host>:8787/healthz` → `ok`.

## Linux (future / cloud coord)

`chapr-coord setup` writes a systemd unit that runs `chapr-coord serve --config …`.
Enable + start it, then check `/healthz`. (The change-watcher's RDCW source is
Windows-only; a Linux coord uses the push endpoint `POST /watch/event`, E-017.)

## Operational notes
- **No Visual C++ redistributable is needed.** The binaries link the CRT
  statically (D-032), because the dynamic default stopped a coordinator from
  starting on a clean Windows Server 2022. The trade: CRT security fixes arrive
  with a Chaperone rebuild, not via Windows Update.
- Back up the SQLite DB **and** the blob store together (audit + history live there).
- TLS: set `[tls]` in the config (rustls); regenerate/rotate certs as usual.
- Auth: `trusted-header` is the MVP mode (accountability). Switch to `negotiate`/
  `oidc` (E-015) for enforced identity without changing anything else — the
  collaboration core is auth-agnostic.
