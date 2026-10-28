# opencode v2 recon (#617)

What opencode v2 changes compared with v1 at every place Skein integrates
with opencode, how Skein should support v1 and v2 side by side, and the
follow-up issues that implement it.

**Versions tested (2026-10-28):** v1 = 1.18.35 (the local install) and the
v1.19.5 source; v2 = `@opencode/cli` **2.2.0** (npm `latest`, published
2026-10-27), on Windows 11, in an isolated npm prefix with isolated
`HOME`/`USERPROFILE`/XDG dirs. The user's own v1 install, config and data
were checked unchanged afterwards.

**v2 release status:** v2 is the current release, not a beta. npm published
`@opencode/cli` 2.0.0 on 2026-09-11 and 2.2.0 on 2026-10-27. The only GitHub
Release object on the v2 line is v2.2.0, marked non-prerelease, and no GA
announcement was found. The migration guide's "beta" and "may wipe data"
wording is stale: the current docs and changelog contain no wipe warning,
and a real first start on a v1 database deleted only three empty v1 tables
(see [Store](#3-on-disk-store)).

**Sources.** Each finding is tagged:

- **[run]**: observed on the real v2.2.0 install.
- **[src]**: read in `anomalyco/opencode` at tag `v1.19.5` or `v2.2.0`. The
  repository moved from `sst/opencode`. v1 code lives in
  `packages/opencode/` and `packages/core/`. v2 code lives in
  `packages/cli/`, `packages/core/`, `packages/schema/`, `packages/protocol/`
  and `packages/server/`.
- **[docs]**: from opencode.ai/v2/docs (migrate-v1, cli, cli/web, api,
  config, mcp-servers, agents, troubleshooting) or opencode.ai/changelog.
- **[skein]**: Skein's own code, with path:line.
- **INFERRED**: reasoned from the above, not observed.

## Summary

v2 breaks nearly every runtime contract Skein has with opencode. Config
loading and the database file location are the exceptions.

- **The TUI no longer embeds a server Skein can address.** Without flags it
  talks to a shared per-user background service. `--standalone` starts a
  private child server on a random port with a password Skein cannot learn.
  `--port`, `--hostname` and `--agent` are rejected on the TUI command.
- **The server API is new.** Every route is under `/api/*` and needs Basic
  auth. SSE frames carry `data`, not `properties`. `message.updated`,
  `message.part.updated`, `session.updated` and `question.*` are no longer
  emitted. Their replacements are `session.step/text/tool.*`,
  `session.execution.*` and `form.*`.
- **The store is the same file but a new schema.** v2 opens the same
  `opencode.db`, migrates v1 sessions **once** into `session_v2`, and writes
  messages to `session_message`. Skein's v1 queries read nothing v2 writes.
- **What still works.** `OPENCODE_CONFIG` and Skein's v1-shaped MCP entry
  still work: v2 normalises the entry. `--session` still exists.

The recommendation keeps **one `opencode` kind**: detect the major version
per spawn, then pick a v1 or v2 adapter. The v2 adapter runs a
**Skein-owned `opencode serve` sidecar** per harness, with a minted password,
and the TUI runs in the PTY with `--server`.

## 1. Touch points

Each touch point is rated **unchanged**, **changed** or **gone**.

### 1.1 Spawn and resume argv: changed

Skein emits `opencode --port <p> --hostname 127.0.0.1 [--agent <a>]
[--session <sid> | --continue]` [skein] `app/src/harnessCmd.ts:77-87`,
`:171-177`.

| Flag on the default (TUI) command | v1 | v2.2.0 |
|---|---|---|
| `--port`, `--hostname` | yes [src] `cli/network.ts` | **gone**: `Unrecognized flag: --port`, help printed, then exit [run] |
| `--agent`, `--model` | yes [src] `cli/cmd/tui.ts` | **gone** from the TUI. Kept on `run` and `mini` [run][src] |
| `--session/-s` | resume an existing session | **exists**, but now means "Session ID to continue, **or to create if it does not exist**" [run][src] |
| `--continue/-c` | yes | yes [run] |
| new | none | `--standalone` (private server), `--server <url>` (connect to a server), `--auto`, `--prompt` [run] |

- `serve` keeps `--port`, `--hostname` and `--cors`, and adds `--service`
  and `--stdio` [run].
- `serve --port P --hostname 127.0.0.1` binds as asked [run].
- `attach` and `agent` do not exist in v2 [run].

### 1.2 Live telemetry over SSE: changed (endpoint, auth, framing, names)

Skein's v1 contract [skein] `app/src-tauri/src/harness_events_opencode/frames.rs:177-335`,
`connection.rs:65` and `session.rs:104-121`:

- It reads `GET /event` and `GET /session/<id>`.
- It reads `{type, properties}` frames.
- It matches `session.status`, `session.created`, `message.updated`,
  `message.part.updated`, `message.part.delta`, `permission.asked`,
  `permission.replied`, `question.asked`, `question.replied` and
  `question.rejected`.
- `harness_actions_opencode.rs` also reads `message.part.updated` tool,
  patch, compaction and reasoning parts, plus `session.updated`.

v2.2.0:

- **Endpoint.** `GET /api/event` is SSE. `/event` and `/global/event`
  return the web UI's HTML [run]. `GET /api/session/{id}` replaces
  `/session/<id>` [run][src].
- **Auth.** Basic auth with user `opencode` on every `/api/*` route; an
  unauthenticated request gets 401 [run].
  - A foreground `serve` honours `OPENCODE_PASSWORD` and
    `OPENCODE_SERVER_PASSWORD`. Without either, it picks a random password
    and prints it [run].
  - A TUI given `--server` reads `OPENCODE_PASSWORD`. Without it the TUI
    exits with `Server at … requires a password; set OPENCODE_PASSWORD`
    [run].
- **Framing.** Frames are
  `data: {"id":"evt_…","created":ms,"type":…,"location":{"directory":…},"data":{…},"durable":{…}}`
  [run].
  - The payload is under **`data`**, not `properties`.
  - The first frame is `server.connected`; after that the server sends
    `: heartbeat` comments.
  - The stream is "volatile by contract: a slow consumer overflows and
    fails the stream, and events during disconnection are missed" [docs]
    (api). `?types=` filters it [src].
- **Event names.** Event names are defined in
  `packages/schema/src/event-manifest.ts` `ServerDefinitions` [src]. The 25
  types observed in a real tool and question session were [run]:
  `server.connected`, `project.updated`, `session.created`,
  `session.execution.{started,succeeded}`, `session.inbox.{enqueued,delivered}`,
  `session.instructions.updated`, `session.step.{started,streamed,ended}`,
  `session.text.{started,delta,ended}`, `session.reasoning.{started,delta,ended}`,
  `session.tool.{input.started,input.ended,called,success}`,
  `session.usage.updated`, `permission.asked`, `form.created` and
  `form.replied`.

| v1 event Skein uses | v2.2.0 |
|---|---|
| `session.status` `{status:{type: busy\|idle}}` | Defined, as `{sessionID, status: idle\|busy\|retry}` [src], but **not observed** in the run. Busy and done showed up as `session.execution.started` and `session.execution.succeeded`, with `failed` and `interrupted` also defined [run][src]. `session.idle` is marked deprecated [src]. |
| `session.created` `{info.parentID}` | Exists, with `slug` and `version` [run]. Subagent child sessions keep a `parentID` on create (`POST /api/session` accepts `parentID` [run]). Whether the event payload carries it is **not verified**. |
| `session.updated` | **Gone** from the stream [src]. `session.renamed`, `session.archived`, `session.agent.selected` and `session.model.selected` replace parts of it [src]. |
| `message.updated` (user role, agent) | **Gone** [src][run]. A user prompt arrives as `session.inbox.enqueued` and then `session.inbox.delivered` [run]. Agent changes arrive as `session.agent.selected` [src]. |
| `message.part.updated` (tool, patch, reasoning, compaction) | **Gone** [src][run]. Replaced by `session.tool.input.*`, `.called` `{input}`, `.success` `{content, metadata}`, `.failed` `{error}` and `.progress`; by `session.reasoning.*` and `session.text.*`; by `session.compaction.*`; and by `session.step.ended` `{cost, tokens, finish}` [src][run]. |
| `message.part.delta` | Replaced by `session.text.delta`, `session.reasoning.delta` and `session.tool.input.delta` [src]. |
| `permission.asked` `{id, sessionID, …}` | Exists. `id` (`per_…`) and `sessionID` are kept. The body changed to `{action, resources[], save, source:{type:"tool", messageID, id}}` [run]. Skein reads only `id` and `sessionID`, so that part is **unchanged for Skein**. |
| `permission.replied` `{sessionID, requestID}` | Exists with the same keys [src]. |
| `question.asked` / `question.replied` / `question.rejected` | **Gone.** Replaced by `form.created` `{form:{id:"frm_…", sessionID, metadata:{kind:"question"}, fields…}}`, `form.replied` `{id, sessionID, answer}` and `form.cancelled` `{id, sessionID}` [run][src]. A form's `sessionID` can be absent for MCP-elicitation forms [src]. Note that the request id key changes from `requestID` to `id`. |

**Impact on #672.** The question-hold fix (commit `b8a4b12`) keys on
`question_asked` and `question_resolved` in
`app/src/harnessEventsOpencode.ts` [skein]. A v2 adapter has to emit those
same two variants from `form.created` (`metadata.kind == "question"`) and
`form.replied` / `form.cancelled`, keyed on `form.id`. The frontend and
`pendingPrompts.ts` then work unchanged.

### 1.3 On-disk store: changed (same file, new tables)

Skein reads `<home>/.local/share/opencode/opencode.db` read-only. The
tables are `session` (columns listed at
`crates/skein-harness/src/opencode.rs:116-118`), `message` (`data` JSON:
`role`, `agent`, `cost`, `tokens`…) and `part` [skein] (`opencode.rs`,
`harness_actions_opencode.rs:456`, `:504`).

v2.2.0:

- **Same location.** The path is `<xdg data>/opencode/opencode.db`;
  `OPENCODE_DB` overrides it [src] (`packages/cli/src/database-path.ts`)
  [run] (`debug paths`).
- **Fresh v2 database.** It has 21 tables [run]. Among them are
  `session_v2` (adds fork, idle, viewed and suspend columns),
  `session_message` (`id, session_id, type, seq, time_*, data` JSON) and
  `event`. It has **no** `session`, `message` or `part` table.
- **v2 on an existing v1 database.** The first open runs a one-shot
  migration (`kv` key `migration.v1-v2 = {"phase":"completed"}`) [run][src]
  (`core/src/database/v1-migration.bun.ts`):
  - It copies `session` rows into `session_v2`.
  - It adds the v2 tables.
  - It drops three **empty** v1 tables: `data_migration`,
    `session_context_epoch` and `session_input`.
  - v1's `session`, `message`, `part` and `project` tables and their rows
    survive.
  - v1 afterwards still lists and exports its sessions [run].
  - This was tested on a one-session database only (see §5).
- **The migration does not re-run.** A session v1 creates **after** that
  first v2 start is in `session` but never reaches `session_v2`. It is
  invisible to v2 [run].
- **v2 writes nothing v1 reads.** Its sessions go to `session_v2` and its
  messages to `session_message` (INFERRED from the schema, not run with v1
  on a v2-written session).
- **Cost and tokens** are on `session.step.ended` events and in
  `session_message` data. The `session_v2` row also carries totals [run]
  (`GET /api/session/{id}` returns `cost` and `tokens`).

Skein's existing queries therefore still run against a migrated database.
They see v1 history but none of v2's sessions or messages. Against a fresh
v2 database they fail, because there is no `session` table.

### 1.4 Resume probes: changed

`app/src-tauri/src/resume.rs:58-63` (the `opencode_session_exists` command)
calls `crates/skein-harness/src/opencode.rs:169`, which runs
`SELECT 1 FROM session WHERE id = ?1 AND time_archived IS NULL` [skein].

- Under v2 that table is v1-only, so the probe must read `session_v2`
  instead. Its `time_archived` column was not checked.
- The probe matters more in v2: `--session <unknown id>` **creates** a
  session with that id instead of failing [run]. A fake `ses_…` id was
  accepted and listed afterwards.
- A wrong id therefore produces a silently empty conversation, not the
  error v1 gave.
- Session ids keep the v1 shape: `ses_` + 12 hex + 14 base62 [run].

### 1.5 Config injection (#215): unchanged for Skein

Skein sets `OPENCODE_CONFIG` (`app/src-tauri/src/harness_config.rs:87-91`
for the variable, `:122` for the file) to
`app/src-tauri/harness-config/opencode/opencode.json`. That file contains one
`mcp.skein` entry: `type: "remote"`, `url` and `headers` built from
`{env:SKEIN_REVIEW_URL}`, `{env:SKEIN_REVIEW_TOKEN}` and
`{env:SKEIN_HARNESS_ID}`, and `enabled: true`. It has no `plugin` key
[skein].

- **`OPENCODE_CONFIG` is still read** [src] (`core/src/config.ts`) [run].
- **The v1 MCP shape loads.** It is normalised to `disabled: false`,
  `{env:}` is substituted, and a connect attempt hits the substituted URL
  [run].
- **The docs' shape fails in 2.2.0.** The docs describe `mcp.servers.<name>`
  with `disabled` [docs], but in 2.2.0 that input shape loads and then
  connect returns 404 `McpServerNotFoundError` [run]. So **keep the v1 shape**.
- **Plugin changes do not affect Skein.** `plugin` → `plugins` and "v1
  plugins do not run in v2" [docs] change nothing, because Skein ships no
  plugin.
- **The env reaches a foreground `serve`.** A `serve` started with the env
  shows the `skein` entry in `/api/config` [run]. That is the process the
  recommended design injects into.
- **Not confirmed for `--standalone`.** Its child server's config could not
  be read: 401, password unknown.
- **The shared background service is a problem.** It is per user, started by
  whichever client came first, with *that* client's environment [run]
  (`service.json`). It cannot carry a per-room token. That alone rules it
  out for Skein.

### 1.6 Agent discovery (#246): changed

Skein runs `opencode agent list` and parses `name (primary|subagent|all)`
lines. It globs `<home>/.config/opencode/agent` and `<cwd>/.opencode/agent`,
and reads the config `agent` key [skein]
(`crates/skein-harness/src/agents/opencode.rs:66-138`).

v2.2.0:

- **`agent list` is gone** [run]. `opencode debug agents` prints a JSON
  array of `{id, name, description, mode, hidden, permissions[], request}`
  [run]. `GET /api/agent` returns the same list [run].
- **Disk enrichment.** v2 reads `agents/` (plural) and still lists agents
  from the v1 `agent/` dir: both `foo` and `bar` were listed [run]. The
  config key is now `agents`, and the v1 `agent` and `mode` maps are
  translated [docs] (migrate-v1). v2's own output already carries the
  description, so the enrichment glob only needs to learn the plural dir.
- **Selecting an agent** is no longer a TUI flag. There are two API routes
  [run] (openapi):
  - `POST /api/session` accepts `{id, parentID, title, agent, model, location, …}`.
  - `POST /api/session/{id}/agent` `{agent}` switches it.

  Skein can therefore pre-create the session with the chosen agent, then
  open the TUI on it. Whether a caller-chosen `id` is honoured, which would
  let Skein pre-allocate the id the way it does Claude's `--session-id`, is
  INFERRED from the spec only: the route accepts the field, and nobody
  tested it.

### 1.7 Post-exit shell follow (#517): changed

`pty/procscan.rs:82-86` matches the process basename `opencode` or
`opencode.exe` and parses `--session`, `--port`, `--hostname`, `--model`,
`--agent` and `--continue`. `pid_listens_on` confirms the port [skein].

- **Process name is unchanged.** Every v2 process is a native
  `opencode.exe`, ~253 MB, with no node or bun shim [run].
- **Argv changed.**
  - `--session` follows as before.
  - `--port` no longer exists on the TUI.
  - A user typing bare `opencode` in the shell gets the shared service, or
    a private `serve --stdio --port 0` child with `--standalone`. Skein
    holds the password for neither [run], so there is no live status.
  - `--server <url>` can be followed only if the user also exported the
    password Skein minted.
- **Net effect:** under v2 the session follows and the live status does not,
  until Restart harness, the same as v1's bare-`opencode` case today.
- **Not checked:** whether the detached `serve --service` process shows up
  as a descendant of the shell.

### 1.8 Harness kind and version detection: new

- Skein never checks the opencode version today [skein]. The only version
  probes are for `claude` and `hermes`.
- `opencode --version` prints `1.18.35` (bare) on v1 and `opencode v2.2.0`
  on v2 [run]. An unanchored `(\d+)\.(\d+)\.(\d+)` reads both, with the
  major version as the first group. The probe spawns the binary (the v2
  exe is ~253 MB), so it has to be cached.
- The v2 package's two bins, `opencode` and `opencode2`, are the **same
  file**, `bin/opencode.exe` [run].
- npm's `opencode-ai` package stays on the v1 line (latest 1.19.5). Its
  `next`/`tui-v2` tags carry an `opencode2` bin [run]. `@opencode-ai/cli`
  only has 0.0.0-beta builds [run].
- **v2 replaces v1 on one machine.** v2 claims the `opencode` command, and
  "the V2 curl installer replaces the V1 binary" [docs] (migrate-v1). Plain
  PATH lookup can't give one machine both versions.
- Coexisting on one machine needs an explicit program path per harness.
  Skein has no such setting today [skein] (`spawn_settings.rs` has only the
  shell path).
- The user's need is per **machine**: one machine on v1, another on v2.
  Per-spawn detection covers that without a new setting.

### 1.9 Other coupling

- **Composer reader**
  (`app/src/promptScreen.ts:101` `readOpencode`, dispatched at `:142`; its
  fixtures are captured opencode 1.18.32 screens, `promptScreen.test.ts:117`)
  [skein]. The v2 TUI layout is **not verified**: no screen of it was
  captured. It needs a screenshot from a real v2 pane before anyone relies
  on `screenPhase`-style reads for v2.
- **"last message to Y"** (worded in `app/src/harnessAgent.ts`) comes from
  v1 user-role `message.updated`, parsed for `info.agent` in
  `app/src-tauri/src/harness_events_opencode/frames.rs:219-241` [skein]. In v2 it would come from
  `session.agent.selected` [src], which was not observed in the run.
- **The `remote` kind** runs the user's own tool string with no telemetry
  [skein] (`docs/remote-harness.md`). It is unaffected.

## 2. Support strategy

Supporting both versions is a requirement. The question is how.

### Recommended: one `opencode` kind, version detected per spawn, v1 and v2 adapters

The spawn probe is `<resolved program> --version`, cached by resolved path
plus file size and mtime. It returns the major version, and the spawn path
picks the adapter set from it:

- **v1** is today's code path, unchanged.
- **v2** works like this:
  1. Skein mints a password and allocates a port.
  2. It starts `opencode serve --port P --hostname 127.0.0.1` as a **non-PTY
     sidecar child** owned by the harness. The child gets `OPENCODE_PASSWORD`,
     `OPENCODE_CONFIG` (#215) and the `SKEIN_*` variables.
  3. On a fresh spawn, Skein creates the session through `POST /api/session`
     with `{agent, location: {directory: cwd}}`.
  4. The PTY runs `opencode --server http://127.0.0.1:P --session <sid>`, with
     `OPENCODE_PASSWORD` in its env.
  5. The SSE adapter subscribes to `/api/event` with Basic auth. It filters
     by `data.sessionID` and the session tree, and translates into the
     **existing `OpencodeEvent` variants**. Phase, badges, #672's question
     hold, mail and nudges then need no frontend change.

  Steps 1, 2 and 4 are verified [run]; the rest are INFERRED design.

The harness record gets one new optional field (`#[serde(default)]`), the
major version seen at the last spawn. A resume can then tell a v1 session id
from a v2 one, because both have the same shape, and handle the migration
case in §3.

Cost by complexity:

- One version probe, plus one new field.
- One new sidecar lifecycle. The sidecar is killed with the PTY, on restart
  and on app exit, and an orphan sweep is needed because PTYs die with
  Skein.
- One new SSE parser and translator over a new event vocabulary.
- One new store reader for `session_v2` and `session_message`.
- Two small CLI deltas: `debug agents` and the procscan argv.

All of it sits behind the version switch, so v1 behaviour cannot regress.
The main risk is the sidecar lifecycle on Windows. It is the first time
Skein owns a long-lived non-PTY child.

### Alternatives

- **A separate `opencode2` kind.** It doesn't remove detection, because v2
  also installs itself as `opencode`. A v2 machine's existing "opencode"
  rooms would silently become v2 and need the same adapter switch anyway.
  It adds:
  - a `HARNESS_KINDS` entry and picker row;
  - a second program name that v1 machines lack;
  - a kind migration for existing harnesses;
  - every `kind === "opencode"`-style call site, though `capabilities` gates
    most of them.

  Strictly more work than the recommendation, for one benefit: the version
  is explicit per harness instead of observed. The stored-version field
  delivers that benefit within one kind.
- **v2 only.** This drops the v1 adapter. It is ruled out by the
  requirement, and it would break the v1 machine on the next Skein update.
- **v2 via `--standalone` instead of a sidecar.** Fewer processes, but the
  child's password is internal. `OPENCODE_PASSWORD` and
  `OPENCODE_SERVER_PASSWORD` are both ignored [run], so Skein cannot read
  the stream. That would mean no telemetry and no permission, question or
  phase signal. Not viable unless upstream adds a way to set it.
- **v2 via the shared background service** (read `service.json` for the URL
  and password). This is one process for every harness, but it runs with
  whoever started it first's environment. It cannot carry each room's
  `OPENCODE_CONFIG` or bearer token, so #215 breaks. It also mixes every
  project's events on one stream. Not viable for config injection.

### Coding against `@opencode/client`?

No; code against the HTTP contract, and pin it with fixtures instead.

- `@opencode/client` 2.2.0 is a **TypeScript** client (Effect and Promise
  flavours) generated from the Effect HttpApi definitions in
  `packages/protocol`, not from a hand-maintained spec [src]
  (`packages/client/script/generate.ts`).
- The derived `openapi.json` (served at `/openapi.json`, 156 operations
  [run]) is a usable **route** contract. But it types the event stream as an
  opaque string (`V2EventEncoded`). Event shapes are typed only in the TS
  schema package (`packages/schema/src/*.ts`) [src].
- Parts of the API reference call it an "Experimental HttpApi surface"
  [docs].
- Skein's adapters are Rust, so the client package cannot be used directly.
  The plan:
  1. Treat `openapi.json` at a pinned tag as the route contract.
  2. Treat `packages/schema` at the same tag as the event contract.
  3. Capture real `/api/event` frames as test fixtures, the way the v1
     parsers are pinned today.
  4. Re-verify on each v2 minor release.
- Unknown event types must stay silent, as they do in v1.

## 3. Migration: a stored v1 `Harness.sessionId` once v2 is on PATH

Tested on a real migrated database. Case 1 and the v1-side listing in case
3 are [run]. Case 2's resume behaviour and case 3's claim about v2 sessions
are INFERRED and need a check before issue 5 relies on them:

1. **The session existed when v2 first opened the database.** Most stored
   ids fall in this case.
   - It was migrated into `session_v2` under the **same id**.
   - `opencode --session <id>` opens it.
   - `GET /api/session/<id>/message` returns the v1 messages.
   - No new session is created.
   - Resume works with history.
2. **v1 created the session after v2's one-shot migration**, for example
   when a machine went back to v1 for a while.
   - It is not in `session_v2`, so v2 does not list it.
   - INFERRED from the fake-id run: `--session <id>` would silently create
     a **new empty** session under that id.
   - Skein must catch this before spawning. If the id is in `session` but
     not in `session_v2`, say so and offer a fresh session. Never resume
     into an empty one.
   - v2 has `session export`/`session import` commands, a possible recovery
     path that was not tested.
3. **Back to v1 after v2 ran.** v1's own sessions still list and export
   [run]. Sessions v2 created are not in v1's tables (INFERRED from the
   schema), so a v2-era `sessionId` resumed under v1 fails as "session not
   found". Today's v1 probe already handles that by dropping the id.

The stored-version field from §2 lets Skein word these cases honestly
instead of guessing.

## 4. Follow-up issues

Filed 2026-10-28 as epic **#674** with children 1–8 below as #675–#682, in
order. Item 9 is parked in `docs/backlog.md`.

Grouping: a new epic issue, **"opencode v2 support"**, with #617
closed by this recon and linked from the epic. #617 should not be retitled
into the epic. It is a research issue whose done-condition this doc meets,
and an epic body that lists children reads better than a rewritten research
issue. Labels: `area:harness-telemetry`, `enhancement`.

Each issue below gives its scope and what it depends on.

1. **opencode: detect the major version per spawn.** Probe `--version` on
   the resolved program, cached by path, size and mtime. Add an optional
   `Harness` field recording the version at the last spawn (`serde(default)`,
   plus the `db.rs` Room struct). Expose it to the spawn path and the
   status-bar popover.
   - Depends on: none. Every other issue builds on it.
2. **opencode v2: spawn through a Skein-owned `serve` sidecar.**
   - Mint a password and allocate a port; spawn `serve` as a non-PTY child
     with `OPENCODE_PASSWORD`, `OPENCODE_CONFIG` and the `SKEIN_*`
     variables.
   - Create the session through `POST /api/session` with
     `{agent, location}`; the PTY runs `opencode --server … --session <sid>`.
     This issue owns passing the stored `Harness.agent` into that call.
   - Lifecycle: kill with the PTY, on Restart harness and on app exit; sweep
     orphans at boot; handle the sidecar dying while the TUI lives.
   - v2 arms of `cmdForKind` and `resumeCmd`.
   - Depends on: 1.
3. **opencode v2: SSE adapter for `/api/event`.**
   - Basic auth, `data` framing, and a filter by session tree.
   - Map `session.execution.*`, `session.status`, `session.step.*` and
     `session.tool.*` to phase.
   - Map `permission.asked` and `permission.replied`.
   - Map `form.created` (`metadata.kind == "question"`) and `form.replied` /
     `form.cancelled` to the existing `question_asked` /
     `question_resolved`. This keeps #672.
   - Map `session.tool.*` to the existing tool-start event the activity
     store uses, and put the frame and tool-shape parsing in a module that
     issue 4 reuses.
   - Map `session.agent.selected` to "last message to Y".
   - Reconnect baseline (#175).
   - Fixtures captured from 2.2.0.
   - Depends on: 2.
4. **opencode v2: Live Context action rows and backfill.** Write live
   `harness_actions` rows from `session.tool.*`, `session.step.ended`
   (cost/tokens) and the inbox and text events (user prompts). Backfill and
   cost from `session_v2` and `session_message` in `crates/skein-harness`.
   This needs a recon of the `session_message.data` shapes first, as part of
   the issue.
   - Depends on: 3, for the connection, the fixtures and the shared
     frame and tool-shape parsing.
5. **opencode v2: resume probe and v1-session migration handling.**
   - Probe `session_v2` under v2.
   - First verify §3 case 2 for real, since it is INFERRED.
   - Detect a v1-only id (in `session`, missing from `session_v2`). Refuse
     to resume it silently into an empty session; offer a fresh one, and try
     `session export`/`import` as recovery.
   - Use the recorded version to word the outcome.
   - Depends on: 1, 2.
6. **opencode v2: agent discovery via `debug agents`.** Parse the JSON list
   (`mode`, `hidden`). Teach disk enrichment the plural `agents/` dir and
   the `agents` config key, which apply to v1 too. Discovery only: passing
   the chosen agent to v2 belongs to issue 2.
   - Depends on: 1.
7. **opencode v2: post-exit shell follow.** Parse `--server`,
   `--standalone` and `--session` in `procscan.rs`. Follow the session
   always, and the live status only for `--server <Skein's sidecar URL>`.
   Show a hint otherwise.
   - Depends on: 2, 3.
8. **opencode v2: composer reader and screen checks.** Capture the v2 TUI
   layout (screenshot from a real pane) and either extend `promptScreen.ts`
   or turn its opencode capability off for v2.
   - Depends on: 1.
9. *(Optional; park in `docs/backlog.md` unless asked)* **Per-kind program
   path setting**, for a single machine that keeps v1 and v2 installed
   under different paths.

Suggested order: 1 → 2 → 3 (v2 becomes usable with live status) → 5 → 6 →
4 → 7 → 8.

## 5. Open items not verified

- A real v1 database with a large history migrated by v2. Only a
  one-session database was tested.
- Whether `session.created` carries `parentID` for subagent child sessions
  in v2, and whether v2 subagents create child sessions at all.
- Whether `session.status` is emitted at all in normal v2 operation.
- The `session_message.data` JSON shapes.
- The v2 TUI's screen layout.
- Whether the standalone child inherits `OPENCODE_CONFIG`. This is moot
  under the recommendation.
- macOS and Linux. Everything above was run on Windows. The service port
  differs there [docs]: 49374 vs 28894.
