# Security notes

Chaperone is a **collaboration engine that values security and traceability — not a
security tool.** The audit trail proves a user is responsible for their agents
(accountability), not court-grade non-repudiation. Read the limitations below as
what they are: deliberate, documented v1 positions, not oversights.

The endpoint runs as the logged-in user and ACLs are enforced by that token on the
direct filesystem path — there is no impersonation or delegation layer. Every
lease, write, restore, and history entry is stamped with the acting AD principal.
The audit trail is a primary deliverable, not a byproduct.

## Three things to know explicitly

### The existence leak (documented v1 limitation)

`coord.resolve(path)` returns version, size, and mtime regardless of the caller's
ACL, because the watcher indexes as a service account. Accepted for a
flat-permission department; the v2 fix is an ACL-aware index.

### The control plane carries file bytes, and it is not access-controlled in v1

> [!WARNING]
> Treat reachability of coord as equivalent to read access to file history.

Reads still go endpoint → share → model under the user's own token and never touch
coord. But a write snapshots its pre-image to coord (`PUT /blobs`), so coord's blob
store holds file content, and `GET /blobs/{version}` applies no ACL check — nor
does any other coord route, by design in the MVP's `trusted-header` posture, which
authenticates nothing. Anyone who can reach coord's port can fetch any snapshotted
version.

This is acceptable only because coord sits on the internal network beside the
fileserver. Put enforced auth ahead of any deployment where that is not true.

*(This section previously claimed bytes only ever come through the user's own open.
That stopped being true when pre-image snapshots started flowing to coord.)*

### Cross-agent prompt injection

`chapr.read` wraps returned content in an explicit untrusted-data envelope, and the
tool description states that the content is data from a shared drive, possibly
written by another party, and never to be treated as instructions.

> [!CAUTION]
> The envelope is a mitigation, not a guarantee. It depends on the model heeding
> it, and a shared drive is a place where one agent's output becomes another
> agent's input. Anything on the share should be treated as untrusted by whatever
> reads it.

## Reporting something

Open an issue. There is no separate embargo process — this is a small project, and
pretending otherwise would be theatre. If you would rather not file publicly, say
so in an issue without detail and we will find another channel.
