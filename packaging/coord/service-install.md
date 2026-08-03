# Installing the coordinator as a service

The coordinator is a single static binary (`chapr-coord`) with a built-in setup
wizard and native service integration (E-016). Exactly one runs per environment,
on-prem beside the fileserver. **Coord availability == write availability**, so
run it as a managed service and monitor `/healthz`.

## Windows (this project)

1. **Place the binary**, e.g. `C:\Program Files\Chaperone\chapr-coord.exe`.
2. **Configure** — interactive wizard (recommended):

   ```powershell
   chapr-coord setup
   ```

   It probes the host, writes a `coord.toml`, offers to install the Windows
   service, runs a `/healthz` self-test, and prints the endpoint (MCPB) settings.
   Unattended equivalent:

   ```powershell
   chapr-coord setup --non-interactive `
     --addr 0.0.0.0:8787 `
     --db "sqlite:C:/ProgramData/Chaperone/coord.db?mode=rwc" `
     --blobs C:/ProgramData/Chaperone/blobs `
     --auth trusted-header --backend smb
   ```

3. **Service** — the wizard installs it via the Windows SCM; or manually:

   ```powershell
   chapr-coord run-service   # invoked by the SCM; not run directly
   sc.exe start Chaperone     # start the installed service
   ```

   (SCM install needs an elevated shell.)
4. **Verify**: `curl http://<host>:8787/healthz` → `ok`.

## Linux (future / cloud coord)

`chapr-coord setup` writes a systemd unit that runs `chapr-coord serve --config …`.
Enable + start it, then check `/healthz`. (The change-watcher's RDCW source is
Windows-only; a Linux coord uses the push endpoint `POST /watch/event`, E-017.)

## Operational notes
- Back up the SQLite DB **and** the blob store together (audit + history live there).
- TLS: set `[tls]` in the config (rustls); regenerate/rotate certs as usual.
- Auth: `trusted-header` is the MVP mode (accountability). Switch to `negotiate`/
  `oidc` (E-015) for enforced identity without changing anything else — the
  collaboration core is auth-agnostic.
