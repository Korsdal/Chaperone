# What Chaperone can attribute, and how to check it on your host

Every lease, write, restore and history entry is stamped with the acting OS
principal — *which user*. What the audit trail cannot currently say is *which
conversation*: a Chaperone session is one endpoint **run**, `sess-{pid}`, and a
host that multiplexes several conversations over one MCP server process puts all
of them in that one session.

That is a limit of MCP as it stands, not a gap waiting to be filled. A stdio
server is handed nothing that identifies a conversation: there is no session
concept in the stdio transport, `rmcp::SessionId` is a server-minted UUID for an
HTTP header, and none of `_meta`'s reserved keys is a conversation id.

**One thing is host-dependent and therefore worth measuring on yours.**
**SEP-414** adds the W3C `traceparent` to a
request's `_meta`. A host that sends a trace id which is stable across one
conversation's tool calls and differs between conversations is, in effect,
telling Chaperone which conversation acted. Whether a given host does that is an
empirical question, so the endpoint answers it in its own log.

## Running the measurement

1. **Point your host at an endpoint build.** `chapr-endpoint print-config generic`
   prints the `mcpServers` block, with `CHAPR_COORD_URL` and `CHAPR_ROOT` filled
   in from the environment if they are set. For Claude Desktop, merge it into
   `claude_desktop_config.json`; for Claude Code, into `.mcp.json` at a project
   root. Restart the host so it re-launches the server.

2. **Make a tool call in one conversation**, then **a second call in the same
   conversation**, then **a third in a different conversation of the same
   application run** (do not restart the host in between — restarting is what
   makes a new `sess-{pid}`, and would answer a different question). Any tool
   will do; the observation happens before the call is dispatched, so a call that
   fails still counts.

3. **Read the endpoint's stderr**, where all its logging goes — MCP speaks
   JSON-RPC over stdout, so nothing is ever written there. Hosts capture it to a
   file; Claude Desktop keeps per-server logs under its own `logs` directory.
   Grep for `traceprobe`.

## Reading the answer

| What the log shows | What it means |
|---|---|
| `no traceparent` and no context lines at all | The host offers no trace context. A session stays per endpoint run. |
| **One** `new trace context`, then silence across the conversation — and a **different** `trace_id` for the second conversation | A per-conversation identifier. This is the case that would let the audit trail name a conversation. |
| A `new trace context` on **every** tool call | The host mints a trace per request. Correct tracing behaviour, and useless as a conversation id. |
| `stopped reporting trace contexts` | Thirty-two distinct contexts seen, so the answer is already "per request"; the cap keeps the probe from filling the log. |
| **Nothing at all** | The hook did not run — the wrong build, or no tool call was made. Silence is *not measured*, not a negative. |

Two properties of the log to keep in mind while reading it. Requests are served
as concurrent tasks, so **line order is not call order** — read the set of lines
a run produced, not their sequence. And the `no traceparent` line reports that
*a* call carried no trace context, not that every call did; a host may populate
`_meta` for some calls only, and both kinds of line can appear in one run.

Only `_meta` **key names** are logged, never values, apart from `traceparent`
itself. The probe changes no behaviour: it reads, it logs, and it does not touch
the session id, a lease, a version check or an exit code.

The engineering reasoning, including why the observation is keyed on the trace id
rather than the whole header, is in `crates/chapr-endpoint/src/traceprobe.rs`.
