# Epic #50 L2c-2 — opencode recon notes

Verified on macOS arm64 (Darwin 25.2.0) on 2026-05-19 against
`opencode` 1.14.39 at `/opt/homebrew/bin/opencode`.

Spike goal from `docs/epic-50-l2c-plan.md`:

> Half-day spike on the HTTP-vs-sqlite question. Lands as a recon
> doc. Then a tight L2c-2 plan + PR.

Headline: **opencode exposes a built-in HTTP server with a Server-
Sent Events endpoint at `/event`, and the events are exactly the
signals we need.** No need for sqlite polling. L2c-2 becomes a
small adapter that mirrors L2c-1's shape — different data source,
same architecture.

---

## 1. The server is built into every opencode invocation

The default `opencode` TUI command is a thin client wrapping its own
embedded HTTP server. Help text:

```
opencode [project]           start opencode tui                       [default]
opencode attach <url>        attach to a running opencode server
opencode serve               starts a headless opencode server

Options:
  --port      port to listen on                       [number] [default: 0]
  --hostname  hostname to listen on             [string] [default: "127.0.0.1"]
  --mdns      enable mDNS service discovery     [boolean] [default: false]
```

`--port 0` (the default) makes the server pick a random free port at
startup. Passing `--port <N>` pins the server to a known port —
this is the path Skein will use, because we need to know the URL
before we can subscribe to `/event`.

`opencode serve` is the same thing without the TUI — useful for
debugging but not what Skein consumes. The integration is simpler:
Skein launches the TUI normally and connects to its embedded server.

## 2. The event stream

`GET /event` returns a `text/event-stream` SSE response. Verified
with `curl -sN http://127.0.0.1:<port>/event`:

```
HTTP/1.1 200 OK
Cache-Control: no-cache
X-Content-Type-Options: nosniff
Content-Type: text/event-stream
Connection: keep-alive
Vary: Origin
X-Accel-Buffering: no
transfer-encoding: chunked
```

Each line is `data: {json-object}\n\n` — standard SSE framing. The
JSON shape:

```json
{
  "id": "evt_e3fcf10500024jqhYyRBb7EsJc",
  "type": "session.status",
  "properties": { "sessionID": "ses_…", "status": { "type": "idle" } }
}
```

The first event after connect is always `server.connected` (with
empty properties). Heartbeats fire as `server.heartbeat` events on
an interval. Connection survives idle sessions — no need for
client-side reconnect logic during normal operation.

## 3. Event types observed during a real session

Drove a real `POST /session/<id>/message` ("Say hi in 3 words")
against `claude-haiku-4-5` and captured the full SSE stream. All
event types observed, ordered by their typical sequence in a turn:

| `type`                  | Meaning |
|-------------------------|---------|
| `server.connected`      | Sent once on `/event` connect. |
| `server.heartbeat`      | Periodic keepalive while the connection is open. |
| `session.created`       | New session record. |
| `session.updated`       | Session metadata changed (title, time, etc.). |
| `session.status`        | **The signal.** `properties.status.type` is `"busy"` while a turn is running and `"idle"` when it ends. Multiple `busy` events fire across a turn; one `idle` fires at the end. |
| `session.idle`          | Fires once when the session transitions to idle. Redundant with `session.status` `idle` for our purposes — they fire back-to-back. |
| `session.diff`          | Filesystem diff snapshot the session is tracking (workdir changes). |
| `message.updated`       | New or updated message record (user or assistant). |
| `message.part.updated`  | A "part" of a message changed. Part types include `text`, `step-start`, `step-finish`, `tool` (see §4). |
| `message.part.delta`    | Streamed text token append (per-character / per-fragment for the assistant's text part). |
| `mcp.tools.changed`     | An MCP server's tool list changed; not user-facing. |

The full sequence for a single turn (with one assistant text reply,
no tool calls) was:

```
session.status busy           ← turn started
session.updated
session.diff
message.updated               ← assistant placeholder
session.status busy
message.part.updated step-start
message.part.updated text     ← text part created
message.part.delta            ← repeated, one per token chunk
message.part.delta
message.part.delta
message.part.delta
message.part.updated text     ← text part finalised
message.part.updated step-finish reason=stop
message.updated               ← assistant complete
message.updated
session.status busy           ← (one trailing busy)
session.status idle           ← turn over, awaiting user
session.idle
session.updated
session.diff
message.updated
```

The mapping for Skein's policy layer is therefore trivial:

- `session.status` with `status.type === "busy"` → `running`
- `session.status` with `status.type === "idle"` → `waiting`

Everything else is either redundant for state-machine purposes or
useful for L7 (the future cross-harness activity feed — `message.part.updated`
with `part.type === "tool"` is the "h1b just used the Edit tool"
signal).

## 4. Discovering the port

Skein needs to know which port opencode's embedded server listens
on, because the SSE URL is `http://127.0.0.1:<port>/event`.

Three options, in order of preference:

### A. Pin the port via `--port <N>` (recommended)

Skein allocates a free TCP port itself (bind to `127.0.0.1:0`, read
the assigned port, immediately drop the listener) and passes
`--port <N>` to opencode. Pros:

- Deterministic — no port-discovery step.
- One process per harness; no extra subprocess to manage.
- Resume case works identically: `opencode --port <N> --session <id>`.

Race window: between dropping the listener and opencode binding,
another process could grab the port. In practice this is the same
~milliseconds-wide race that every "find free port" pattern has,
and recoverable: if `opencode` fails to bind, the PTY exits with
the error visible in the terminal.

### B. Parse the port from opencode's startup output

`opencode serve` prints `opencode server listening on http://127.0.0.1:<port>`
to stderr. The TUI doesn't seem to print it in normal mode (we
didn't drive a full TUI start-up — needs verification), so this
path only works for the `serve` subcommand. Less appealing.

### C. mDNS discovery

The `--mdns` flag advertises the server via mDNS. Robust but adds
a dep we don't need given option A works.

**Decision:** Option A. Skein already does port allocation for the
PTY layer indirectly (via the system); adding a one-shot free-port
helper in Rust is ~15 LOC.

## 5. Resume + session-id pre-allocation

opencode still doesn't have a `--session-id` pre-allocation flag
(verified — only `-s, --session <id>` to load an existing one and
`-c, --continue` for last in cwd). So chapter 5's snapshot-then-diff
capture still applies to fresh opencode spawns.

But L2c-2 helps here too: the `session.created` event fires with
`properties.sessionID` set, in real time, the moment opencode
creates a session. Skein could capture the session id from the SSE
stream instead of polling sqlite. Cleaner, no race window with
sqlite WAL flushes.

This is a follow-up — not blocking L2c-2's main goal (waiting
detection) — but worth filing as a chapter-5 phase-2b refinement.

## 6. Sub-agents / sidechain

opencode supports sub-agents (`opencode agent` subcommand). We did
not exercise these in the recon; whether their events appear with
distinct sessionID values or via some `isSidechain`-equivalent flag
is unknown. For L2c-2 v1 we filter to `sessionID === our session`
and let any unknown event types be no-ops; sub-agent handling can
be a v1.1 refinement once we see one fire.

## 7. Permission prompts

opencode has a permission system but the recon didn't trigger one.
The general shape we'd expect: a `message.part.updated` with some
`type: "permission"` (or similar) and then no further events until
the user answers. Worth dogfooding once L2c-2 ships to confirm.

## 8. Connection lifecycle

What the L2c-2 adapter has to handle:

- **Server not yet listening on spawn.** The PTY child has just
  started; opencode needs a moment to bind. The adapter should
  retry the SSE connect for a few seconds before giving up. Same
  pattern as L2c-1's "file doesn't exist yet" branch.
- **Server dies cleanly when the user `/exits`.** SSE connection
  closes naturally. Maps to PTY exit; the activity store's
  `exited` phase wins regardless.
- **Server crashes / network blip.** SSE delivers a connection
  close. Adapter should reconnect a few times; permanent failure →
  detach authoritative source → fall back to L2a.

For v1: best-effort reconnect with a small backoff (max 3 attempts,
~1 s apart). If reconnect fails, leave the adapter detached. The
L2a idle heuristic takes over and the harness still works.

## 9. Authentication

`opencode serve` warns on startup:

```
Warning: OPENCODE_SERVER_PASSWORD is not set; server is unsecured.
```

But our server is bound to `127.0.0.1` and only consumed in-process
by Skein. Not a concern for the local-only case. If we ever want to
support remote opencode (the `opencode attach <url>` shape), we'd
need to handle the `Basic` auth password.

For v1: ignore. localhost only.

## 10. Sketch of the L2c-2 implementation

Architecture mirrors L2c-1 exactly:

```
opencode embedded HTTP server (random port, or pinned via --port)
                         │
                         ▼  /event SSE stream
Rust adapter (harness_events_opencode.rs)
                         │  emits OpencodeEvent over per-harness Channel
                         ▼
Frontend translator (harnessEvents.ts gains an opencode branch)
                         │  translates → setRunningFromAdapter / setWaitingFromAdapter
                         ▼
       harnessActivity store (existing, kind-agnostic)
```

Files to touch (estimate):

- `app/src-tauri/src/harness_events_opencode.rs` — new, ~250 LOC
  incl. tests. Uses `reqwest` (already a transitive dep via
  tauri-plugin-updater) or just raw `hyper` for the SSE client.
  Parser uses `serde_json::Value` — same pattern as L2c-1.
- `app/src-tauri/src/lib.rs` — register two Tauri commands. ~15 LOC.
- `app/src/harnessEvents.ts` — gain an `attachOpencodeEvents` helper
  and translator branch. ~30 LOC.
- `app/src/LiveTerminal.tsx` — attach for `kind === "opencode"`
  alongside the existing claude case. ~5 LOC.
- `app/src/App.tsx` — opencode-spawn path needs to allocate the
  port and pass `--port <N>`. ~20 LOC, in `cmdForKind`.

Tests: the SSE parser is straightforward to unit-test (feed it a
canned event stream, assert the emitted enum values). The full
end-to-end (real opencode child) is too heavyweight for unit tests
— we dogfood by driving a real session.

PR title (eventual): `Epic #50 L2c-2: opencode event-stream adapter
(waiting via SSE)`.

## 11. Open questions for the L2c-2 plan

1. **HTTP client library.** Add `reqwest` (or `eventsource-client`)
   as a direct dep, or write a minimal SSE reader by hand?
   `reqwest` is heavy if it's not already pulled directly; the SSE
   protocol is simple enough to handle inline. Lean toward
   handwritten minimal client to keep the dep tree tight.
2. **Reconnect policy.** Confirmed needed for crash recovery, but
   the exact backoff curve is bikeshed territory. Start with "3
   attempts, 1 s apart, then give up" and tune from real-world
   failures.
3. **Capture session id from `session.created` event?** Plan above
   suggests yes (synergy with chapter 5 capture), but it's a
   follow-up — not blocking L2c-2 v1.
4. **MCP tool changes via `mcp.tools.changed`.** Useful for L7
   activity feed, ignore for L2c-2 v1.
