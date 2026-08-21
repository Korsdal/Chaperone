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

### The control plane carries file bytes, and it is authenticated but not authorised

Reads go endpoint → share → model under the user's own token and never touch coord.
But a write snapshots its pre-image to coord (`PUT /blobs`), so coord's blob store
holds file content.

**Admission is enforced.** The default mode is `shared-secret`: a caller must
present the deployment's endpoint token, created on first start in coord's data
directory and given to every laptop as `CHAPR_COORD_TOKEN`. Without it, every route
that serves content, hands out version hashes, or mutates coordination state
answers 401. `/healthz` and the `/admin` page stay open — the first is monitoring,
the second is where the admin token is typed.

> [!WARNING]
> **Authentication is not authorisation.** `GET /blobs/{version}` still applies no
> ACL check, so any *authenticated* caller can fetch any snapshotted version of any
> file coord knows about. The token narrows this from "anyone who can reach the
> port" to "anyone holding the deployment's secret" — which is every endpoint. An
> ACL-aware blob store is not built.

Two further limits worth stating plainly:

- **The acting principal is still asserted, not proven.** The token says *this is
  one of our endpoints*; the `X-Chapr-Principal` header says *acting for this
  user*, and an endpoint holding the secret can name anyone. This is the
  accountability-not-non-repudiation line at the top of this document, and binding
  identity to a verified subject is the enforced-auth work (Negotiate for a
  Kerberos realm, OIDC otherwise).
- **One secret per deployment, so revoking one laptop means rotating for all of
  them.** Deliberate: per-endpoint credentials would need issuance, rotation and a
  registry, and that machinery would be thrown away by the enforced-auth work.

The older `trusted-header` mode remains selectable and accepts **any** non-empty
principal header from **anyone** who can reach the port. It is attribution only.
Choose it only where that is already acceptable, and change modes as a cutover
(`auth_fallback`) rather than a flag day.

*(This section previously said the control plane was not access-controlled at all,
and before that that bytes only ever came through the user's own open. Both were
true when written.)*

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
