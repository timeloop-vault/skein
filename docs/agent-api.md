# The Skein review API (#213)

Epic #52 exists because the agent could not read review comments.
#211 gave the diff a lifetime, #212 gave the reviewer somewhere to
write, and this is the piece that closes the loop: a localhost HTTP
server inside the running Skein process, exposed as an **MCP endpoint**,
so an agent in a room can read the comments on its own work, answer
them, and say what it did about them.

It **cannot resolve a thread.** That is the point, not an omission —
see [What it will not do](#what-it-will-not-do).

## Shape

```
Skein process
 └─ 127.0.0.1:<ephemeral>              bound in setup(), 127.0.0.1 only
     ├─ POST   /mcp                    MCP streamable HTTP — the harnesses
     ├─ GET    /mcp                    405: no server→client stream
     ├─ DELETE /mcp                    405: stateless, no sessions
     ├─ GET    /api/health             unauthenticated, carries nothing private
     ├─ GET    /api/comments           list_comments
     ├─ GET    /api/comments/{id}      get_comment
     ├─ POST   /api/comments/{id}/reply
     ├─ POST   /api/comments/{id}/addressed
     ├─ POST   /api/comments/{id}/resolve   403, always
     ├─ GET    /api/diff               get_diff
     ├─ POST   /api/messages           send_message
     ├─ GET    /api/messages           read_messages
     ├─ GET    /api/messages/history   message_history
     ├─ POST   /api/rooms              create_room
     ├─ GET    /api/rooms              list_rooms
     ├─ GET    /api/rooms/find         find_rooms_for_path
     ├─ GET    /api/rooms/{room_id}    get_room
     ├─ GET    /api/harnesses          list_harnesses
     ├─ POST   /api/harness/permission     see below — not an agent verb
     └─ POST   /api/harness/session-start  see below — not an agent verb
```

`/mcp` is what Claude Code and opencode talk to. `/api/*` is the same
verbs as ordinary JSON, for the `skein` CLI epic #52 D10 defers; both go
through the same functions in `agent_api/verbs.rs`.

The port is **ephemeral** and changes every launch. Nothing has to agree
on a number in advance because the URL reaches the harness in its
environment (below), and both harnesses expand `${VAR}` inside their MCP
config.

## Identity

Every harness Skein spawns gets four variables:

| variable | what it is |
| :-- | :-- |
| `SKEIN_REVIEW_URL` | `http://127.0.0.1:<port>/mcp` |
| `SKEIN_REVIEW_TOKEN` | the **room's** bearer token |
| `SKEIN_ROOM_ID` | the room, for logs and prompts |
| `SKEIN_HARNESS_ID` | this harness, for attribution |

Two headers carry them:

```
Authorization: Bearer <SKEIN_REVIEW_TOKEN>
X-Skein-Harness: <SKEIN_HARNESS_ID>
```

**The token is the scope**, with the exception of the cross-room read
verbs below. No request names a room, so a token can only
ever reach the room it was minted for — there is nothing to tamper with.
`X-Skein-Harness` is *attribution only*: it decides whose byline a reply
carries and never what may be read, so a wrong or missing value costs
the comment its name and nothing else.

A token lives no longer than the Skein process that handed it out. Every
PTY dies with the app, so nothing legitimate is still holding one after
a restart — and a token that ended up in an old transcript stops working
at the next boot.

## The verbs

All fourteen verbs carry the token, but not all fourteen are scoped by
it to the calling room. Nine of them read or act only within that
room; `create_room` opens a *different* room, though still only from
the calling room's token, which is what its own rate cap below is
keyed to; `find_rooms_for_path`, `list_rooms`, `get_room` and
`list_harnesses` break the pattern the other way — each reads across
every room regardless of which room the token names, see the scope
note under "Finding rooms by path" below and "Listing rooms and
harnesses" further down. MCP tool names; Claude Code
presents them as `mcp__plugin_skein_api__<name>` — the server is
registered by the plugin Skein injects (#215), keyed `api` in the
plugin's `.mcp.json`, and plugin-provided MCP servers carry a
`plugin_<plugin>_<server>` prefix. opencode presents them as
`skein_<name>`, from the `skein` key in `opencode.json`.

### `list_comments`

`{ status?: "unresolved" | "all", file?: string }` — defaults to
unresolved, which is the list of what still needs doing. Each thread
comes back with its file and **current** line range (re-anchored through
the same path the pane uses), whether the anchor has gone outdated,
whether there is a standing `addressed` claim on it, and the whole
conversation with per-comment authors (`reviewer`, or a harness byline
like `claude · main`).

`unresolved_total` is the room's count, not the filter's.

### `get_comment`

`{ thread_id }` — the thread plus the code it is about:

- `anchor_lines` — the code the comment was written against. This is the
  anchor itself, not a cache of it; for an outdated thread it is the
  only honest account of what was meant.
- `current_context` — that region of the file as it stands today, with
  1-based line numbers and `>` marking the commented lines.
- `diff_context` — the hunk the comment lands in, as unified diff.

### `get_diff`

`{ file?, scope?: "branch" | "pending" | "commit", commit_sha? }` —
unified patch text, not a hunk tree. `branch` (the default) is
everything this branch does against its base, committed and uncommitted
together; `pending` is #211's not-yet-reviewed set; `commit` needs a
`commit_sha`. A whole-branch diff is capped at 256 KB and reports
`truncated: true` rather than trailing off — ask per file.

### `reply`

`{ thread_id, body }` — posts a comment with `author_kind = "agent"` and
`author_id = <harness id>`. It appears in the review pane immediately:
the write emits `skein://review-changed`, because a comment touches only
sqlite and no filesystem watcher would ever fire.

### `mark_addressed`

`{ thread_id, commit_sha?, note? }` — records that the agent handled a
comment, with the commit that did it when there is one. **A claim, not a
closure.** The pane renders it as a quiet badge beside the resolve
control, never instead of it. A later comment *from the reviewer* clears
the claim automatically: if they are still talking, it evidently is not
settled.

### `review_status`

No arguments. Whether the reviewer has **signed off** (#214) — the gate
to read before merging, pushing, or opening a pull request.

```json
{
  "approved": false,
  "stale": true,
  "approved_sha": "4196f17e…",
  "head_sha": "2bcae5a1…",
  "commits_since_signoff": 2,
  "unresolved_count": 1,
  "unaddressed_count": 0,
  "guidance": "The reviewer approved an earlier commit, and HEAD has moved since…"
}
```

`approved` is the only field a decision should read, and it is true
**only when the sign-off names the commit HEAD points at now**. A
sign-off approves a commit, not a room: an agent that commits after
being approved invalidates its own clearance, which is `stale`. That is
the honest outcome — an approval silently stretching over code nobody
read would be worse than no approval.

`guidance` says the same thing in words, because a model reading
`approved: false, stale: true` otherwise has to infer why.

**Skein does not land the branch.** It never merges, pushes, or opens a
pull request: which strategy, which forge, and what a PR body looks like
are things this repository already says — in `.claude/skills/`, in
`CLAUDE.md` — and the agent reads them. Skein owns the one fact that
lives nowhere else, which is whether the human said yes.

## The mailbox (#327, epic #275)

Two more verbs, and a different shape from the six above: they are not
about review at all, and a `send_message` almost always targets a room
that is not the caller's own. A sqlite table, `harness_messages`, holds
one row per message: `id`, the recipient (`room_id`, `harness_id`) and
the sender (`from_room_id`, `from_harness_id` — the latter `NULL` when
the sender's request carried no `X-Skein-Harness`), `body`,
`created_ms` and `read_ms`.

### `send_message`

`{ to, body }` — send a short message to another harness: a sibling in
this room, or a room named by its id. A room id is resolved **at send
time** to that room's **lead harness** — the first harness in the
room's order that can read mail (see below) — so the stored row names a
concrete recipient rather than a room whose lead harness might change
before anyone reads it. Returns `message_id`, the resolved
`to_room_id`/`to_harness_id`, and `to_room_name`/`to_harness_name` for a
readable confirmation.

Refused, with the exact reason as text:

- `"agent messaging is turned off in Settings"` — the kill switch, below
- `"a message needs a body"` — an empty body
- `"a message body is capped at 65536 bytes; this one is N bytes"`
- `"send_message needs a to"` — an empty or whitespace `to`
- `"no harness or room \"<to>\""` — `to` matches neither a harness id
  (searched across every room) nor a room id
- `"<room> is archived and cannot receive messages"`
- `"<harness> cannot read messages: <reason>"` — the target exists but
  fails the "who can read mail" check below
- `"<room> has no harness that can read messages"` — `to` was a room id
  and no harness in it qualifies
- `"rate limit: this room has sent N messages in the last minute (cap 30)"`
- `"<harness> already has N unread messages (cap 100); it needs to read
  before it can receive more"`

A successful send emits `skein://mail-changed { roomId, harnessId }`
naming the **recipient's** room and harness. It also records two
`harness_actions` rows (#329), so the Live Context feed shows who
talked to whom without exposing the body: `message_in` on the
recipient (`room_id`/`harness_id` = the resolved target) and
`message_out` on the sender (`harness_id` empty when the caller had no
`X-Skein-Harness`), same timestamp, same payload —
`message_id`/`from_room_id`/`from_room_name`/`from_harness_id`/
`from_harness_label`/`to_room_id`/`to_room_name`/`to_harness_id`/
`to_harness_label`. A separate Tauri command, `mail_unread(room_id,
harness_id)`, gives a harness's unread count and distinct sender room
names for a badge, without marking anything read.

### `read_messages`

`{ include_read? }` — every unread message for the *calling* harness,
oldest first, marked read as they are returned. `X-Skein-Harness` is
**required** here, unlike everywhere else it is attribution-only: this
verb has to know whose inbox to read, not just whose byline to stamp on
a reply, so a missing header is refused rather than silently reading
nobody's mail (`"read_messages needs X-Skein-Harness to say which
harness is asking"`). Pass `include_read: true` to get the whole
history instead of only the unread tail — still-unread rows in that set
are marked read the same as a normal call.

Each message carries `from_room_id`/`from_harness_id` and, when the
sender still exists, `from_room_name`/`from_harness_name`; a `null`
harness fields means the sender's own request carried no
`X-Skein-Harness`, or that harness or its room is gone since. Refused
the same way as `send_message` when messaging is off in Settings.

A call that actually marks something read emits `skein://mail-changed
{ roomId, harnessId }` for the **caller's own** mailbox; a poll that
finds nothing new fires nothing.

### `message_history` (#364)

Why a sibling verb rather than teaching `read_messages` a flag: that
verb marks mail read as a side effect, and its default — the unread
tail — must not change out from under callers relying on it; a history
read that never touches read state, and so never disturbs the unread
nudge or badge, needs its own verb.

`{ with?, since?, limit?, direction? }`, all optional:

- `with` — a room id or harness id. Given, only mail exchanged with
  that counterpart, both directions; omitted, everything in scope.
- `since` — a number is `created_ms`, exclusive; a string is a message
  id, meaning strictly after that message. The id must be one the
  caller can already see, or the call is refused rather than silently
  starting from the beginning.
- `limit` — default 100, max 500.
- `direction` — `"in"` | `"out"` | `"both"`, default `"in"`.

Scope is the token's, the same as `read_messages`: inbound is the
calling harness's own inbox (`X-Skein-Harness` required, same as
`read_messages`); outbound is every message sent from the caller's
**room** — never another room's outbox. A message from a sibling
harness in the same room addressed to the caller counts once, as
inbound, never twice.

Returns `{ messages, hasMore }`, oldest first, the first `limit`
messages after `since` — page forward by passing the last message's
`id` as the next call's `since`. Each message: `id`, `direction`
(`"inbound"` | `"outbound"`), `from_room_id`, `from_room_name?`,
`from_harness_id?`, `from_harness_name?`, `to_room_id`,
`to_room_name?`, `to_harness_id`, `to_harness_name?`, `body`,
`created_ms`, and `read_ms?` — on an **outbound** row, `read_ms` says
whether the recipient has read it yet.

Refused the same way as `read_messages` when messaging is off in
Settings. Never marks anything read, and emits no
`skein://mail-changed` — nothing about the mailbox's state changed.

Deliberately not built: a `latest_per_room` shape the original issue
sketched. `list_rooms { created_by: "me" }` already carries each child
room's `last_status` — the latest line and timestamp — so a second
verb doing the same summary would be redundant; reach for
`message_history` only once you want the thread itself.

### Who can read mail

A harness can receive a message only if all of this holds:

- its kind is `claude` or `opencode` — any other kind has no MCP
  connection at all, so there is nothing to receive on;
- #215 config injection is switched on **for that kind** in Settings;
- if it runs a named agent (#246/#247), that agent's own `tools`
  allowlist does not hide MCP tools. An agent whose definition cannot be
  found or read on disk is **not** refused for it — a degraded lookup
  must never be the reason a message is silently refused.

The same check picks a room's lead harness: the first harness in room
order that passes it, not necessarily the first harness in the room.

### Caps and the kill switch

| cap | value |
| :-- | :-- |
| message body | 64 KiB |
| sends | 30 per token (i.e. per room) per rolling minute |
| unread per receiver | 100 |

Each cap errors loudly, as text above, rather than queueing or
truncating. Settings → Shell & environment has a toggle beside the #215
injection switches, **"Let agents message other harnesses"**
(`allowAgentMessaging` in `settings.json`, default on): off refuses both
verbs by name, with the reason, exactly like the injection toggles
refuse the review tools when off.

### Two things worth writing down

- **`X-Skein-Harness` is attribution and routing here, not authority** —
  the same rule as everywhere else in this API (#213). Any harness
  holding the room's bearer token can present any harness id in that
  room and read that harness's mail; there is no per-harness secret
  underneath the room token. This is accepted: the token's *room* is
  the scope, and addressing a harness inside it was never meant to be a
  security boundary.
- **A message body is attacker-reachable text.** An agent that has just
  read an issue, a PR comment or a web page can forward some or all of
  it verbatim in a `send_message` call. Nothing on the receiving end
  auto-approves or auto-acts on a message because it arrived over this
  API; every message is stored with its sender so a bad one is
  traceable; and a harness reading its mailbox should weigh a message
  the way it would weigh anything else it did not write itself — a
  request from another agent, never an instruction from the reviewer.

## Opening a room (#330)

A third shape, past review and mail: `create_room` is the one verb that
lets an agent start *another* agent working, unattended, in a room of
its own. It is guarded more heavily than anything else in this API —
see "Caps and the kill switch" just below.

### `create_room`

```
{ path?, branchMode?, branch?, baseBranch?, task, kind?, agent?, prompt? }
```

| arg | default when omitted |
| :-- | :-- |
| `path` | the calling room's own repo root, falling back to its cwd |
| `branchMode` | `"worktree"` (the other choice is `"current"`) |
| `branch` | the user's own branch-name template, applied to a slug of `task` |
| `baseBranch` | the repo's current branch guess. May name a local branch or a remote-tracking ref (e.g. `"origin/main"`), read as of Skein's last fetch — Skein never fetches on its own; a local branch of the same name wins if both exist |
| `task` | required — short label for the room's tab |
| `kind` | the user's own default harness kind for this folder |
| `agent` | the user's own default agent for that kind, or the folder's remembered agent (#247/#248) — omit rather than guessing a name, since an unresolvable one refuses the whole call |
| `prompt` | omitted → the room opens idle. Given → queued as the new harness's first mailbox message the moment it exists |

Returns `roomId`, `name`, `cwd`, `repo`, `branch`, `harnessId`, `kind`,
`agent`, `sessionId` (nullable — a non-git folder has no repo or
branch, and an opencode harness's session id is only captured
asynchronously after spawn), `messageId` (present only when `prompt`
was given and successfully queued), and `baseBehindUpstream` (present
only when the worktree's base was a *local* branch behind its
upstream — a lower bound, since Skein never fetches; a remote-tracking
base has nothing to be behind). Seeing it means: pull first, or pass
a remote-tracking ref such as `"origin/main"` as `baseBranch` and
retry. HTTP route: `POST /api/rooms`, taking `CreateRoomArgs` directly
as the body.

**The flow** mirrors the New Room dialog rather than reinventing it,
so a room an agent opens looks exactly like one a human would have
gotten for the same folder:

1. Rust validates what it can locally — `task` non-empty, `branchMode`
   one of the two known values, `kind` one of the known harness kinds,
   `path` absolute and a real directory, a git checkout when
   `branchMode` is `"worktree"` — then checks the guards below.
2. It round-trips `"create_room.resolve"` to the webview (#328's
   `AgentApiState::request_frontend`): the frontend applies the user's
   own default kind for the folder, the per-kind default agent (#248),
   the folder's own remembered agent, and #247's agent validation —
   the same defaulting `useNewRoomForm.tsx` runs for a human typing
   into New Room.
3. If a `prompt` was given, the #327 mailbox "who can read mail" rule
   is checked against the *resolved* `(kind, agent)` — before anything
   is created, so a prompt that could never be delivered (an
   unreachable kind, injection turned off) is refused as
   `"cannot queue the prompt: the new harness …"` rather than leaving
   an idle room behind.
4. Only then does it round-trip `"create_room"`, which runs the New
   Room dialog's own shared worktree-creation path with
   `activate: false` — the room opens in the background and does not
   take over the user's screen.
5. If `prompt` was given, it is queued as a mailbox message *from the
   calling harness* once the room exists, and delivered the way any
   mailbox message is delivered: #329's nudge, once the new harness is
   `waiting`.

Refused, with the exact reason as text (a representative, not
exhaustive, list):

- `"task can't be empty"`
- `"unknown branchMode …"` / `"unknown kind …"`
- `"agent room creation is turned off in Settings"` — the kill switch, below
- `"rate limit: this room has attempted 5 room creations in the last minute (cap 5)"`
- `"agents have already opened N open rooms in the repository group <repoRoot> (cap 20 per group); close or archive some before opening another"`
- `"agents have already opened N open rooms outside any repository group (cap 20 for ungrouped rooms); close or archive some before opening another"`
- `"{path} is not a git checkout, so branchMode \"worktree\" cannot be used (try \"current\")"`
- `"a prompt was given, but agent messaging is turned off in Settings, so it could never be delivered"`
- `"cannot queue the prompt: the new harness …"` — the #327 "who can read mail" check, run against the resolved kind/agent
- whatever the frontend round trip itself refuses for — an unresolvable folder, a colliding branch name, an unknown agent name — surfaces verbatim

### Caps and the kill switch

| cap | value |
| :-- | :-- |
| room creations | 5 per calling room per rolling minute |
| open (non-archived), agent-opened rooms per repository group | 20 |
| open (non-archived), agent-opened rooms with no repository (ungrouped bucket) | 20 |
| prompt | refused outright when agent messaging is off in Settings |

The open-room ceiling (#375) only counts rooms `create_room` itself
opened — a room the user made by hand, or one from before #330 with no
`createdBy` at all, never counts, and never blocks an agent — and it is
scoped **per repository group**, the same group the room strip groups
on: a room's normalized `repoRoot`, or one shared "ungrouped" bucket
for rooms with none. The scope checked is the group the *new* room
would join, resolved from `path` before anything is created, so a
runaway agent can fan out inside one repository's group but can't
starve every other project of room slots at the same time.

Settings → Shell & environment has **"Let agents open rooms"**
(`allowAgentRoomCreation` in `settings.json`, default on), right
beside the messaging toggle: off refuses `create_room` by name, with
the reason, the same way the messaging and #215 injection toggles
refuse their own verbs.

## Finding rooms by path (#354, epic #266 slice A)

`find_rooms_for_path` is the first verb in this API whose answer is
**not** scoped to the calling room — read the scope note below before
relying on it from anywhere that assumed every verb was room-local.

### `find_rooms_for_path`

`{ path }` — every room, open or archived, whose worktree touches
`path`. HTTP mirror: `GET /api/rooms/find?path=<p>`, same bearer token
as every other route.

```json
{
  "rooms": [
    {
      "room_id": "…",
      "name": "…",
      "cwd": "…",
      "repo_root": "…",
      "branch": "…",
      "archived": false,
      "safe_to_remove": false,
      "match": "cwd"
    }
  ],
  "unreadable_rooms": 0
}
```

- `match` is `"cwd"` when `path` normalises to exactly the room's
  `cwd`; `"inside_room"` when `path` is a path-segment-boundary
  descendant of the room's `cwd` (checking a file inside a worktree);
  or `"contains_room"` when the room's `cwd` is a path-segment-boundary
  descendant of `path` (checking a `<repo>-wt` parent that holds
  several worktrees) — the direction matters for a folder-removal
  decision, so it is never a bare "one contains the other". Exact
  matches sort first.
- `repo_root` is `null` for a room outside any git checkout.
- `safe_to_remove` is `false` for every **open** room — an open room
  means don't touch this folder, full stop — and `true` only for an
  **archived** one. The verb only reads: `archive_room`,
  `remove_worktree`, `delete_room` and `close_room` remain refused by
  name, unchanged by this issue.
- Path comparison uses the same normalisation room grouping already
  relies on (`app/src/roomGroups.ts`'s `normalizePath`): backslashes to
  forward slashes, one trailing separator stripped, case-folded, on
  every platform, and matched on path-segment boundaries so `foo` never
  matches `foobar`. An empty `path` is refused as an invalid argument.
- Reads Skein's persisted room table, which the app rewrites on every
  room change including archive, so the answer reflects live open/
  archived state for as long as Skein keeps running.
- `unreadable_rooms` counts rooms Skein holds but could not read as
  this response was built — a persisted row that failed to parse, or
  one already parked in quarantine (#167) — and so could not be
  checked against `path` at all. It is non-zero only when Skein's own
  room table has damage; a healthy install always reports 0. When it
  is non-zero, `rooms` may be missing an entry: do not read a path's
  absence from `rooms` as permission to remove that folder.

### Scope note

Every other verb's scope **is** the calling token — see Identity,
above. `find_rooms_for_path` is deliberately the exception, settled on
epic #266 by #275: the bearer token guards against a replay from a
leaked log, not against the user's own agents, and a single director
room spanning several projects is a wanted shape, not a hole to close.
The rest of #266 slice A — `list_rooms` / `get_room`, #356 — reuses
this same rule, so this note will end up describing three verbs, not
one.

Consumer: the `worktree-sweep` skill's "which rooms own this folder"
check is switching to this verb (#357).

## Listing rooms and harnesses (#356, epic #266 slice A)

Three more read-only verbs, cross-room for the same reason
`find_rooms_for_path` is: a director that opened several rooms with
`create_room` needs to see them, and no single room's token could
scope that question. `list_rooms` alone takes a room-scoped filter,
`created_by: "me"`, checked against the *caller's* room rather than
widening what the token can see.

Every phase these three verbs report (`lifecycle` on `list_rooms`,
`phase` on `get_room` and `list_harnesses`) comes from the same
round trip to the webview `create_room` uses (#328), asked fresh on
every call with a 3-second timeout. Anything the backend cannot vouch
for — no webview, a timeout, a harness the answer never named, an
archived room, or a value outside the phases the frontend's own
activity store defines — reads `"unknown"`, never a guess and never
`"idle"`: that is a real phase, and reporting it without an answer
would be a lie Skein never told itself.

### `list_rooms`

`{ created_by?: "me" }` — every room Skein holds, across every
project, archived rooms included and never dropped: "that room landed
and was archived" is exactly what a director needs to see after its
own context is compacted. Any `created_by` value other than `"me"` is
refused rather than silently ignored, since a typo here would
otherwise read as "show me everything" and a director would never
notice its filter never applied.

Each room reports `room_id`, `name`, `cwd`, `repo_root`, `branch`,
`archived`, `harness_count`, `created_by` (`{ room_id, harness_id }`,
present only for a room `create_room` opened), `base_sha` and
`prompt_first_line` (both recorded at create time by `create_room`,
so absent on a room from before #330 or one the user opened by hand),
`lead_harness_id` (the harness `send_message` would actually reach if
addressed to this room id — absent when nothing in the room can read
mail), `lifecycle` (`"archived"`, a live phase, or `"unknown"`), and
`last_status` — the first line and timestamp of the **last message
that room sent the caller**, absent if it never has. `last_status`
alone is enough for a director to rebuild its table without replaying
any mail.

`unreadable_rooms` counts persisted rooms Skein holds but could not
parse (the same count `find_rooms_for_path` reports) — non-zero only
when Skein's own room table has damage, and a sign that `rooms` may be
short.

### `get_room`

`{ room }` — one room by id, open or archived, created by anyone, not
only rooms the caller opened. An id nobody recognises, or one that
resolves to an archived room, is refused loudly and by name — never a
bare empty result, since #356's whole point is a director inspecting a
room it does not own.

Reports the same room fields as `list_rooms` (minus `lifecycle` and
`last_status`, which are cross-room-specific), plus `harnesses` — each
with `harness_id`, `name`, `kind`, `agent`, `session_id` and its live
`phase` — and a `signoff` block: the **same sign-off `review_status`
returns**, produced by the same function, so `get_room` can never
report a different answer than that room would give about itself.
`signoff_unavailable` (with `signoff` then absent) explains why there
is none — today, only because the room has no worktree.

There is no `review_status { room }` alias: `get_room` is the one
read path for another room's sign-off.

### `list_harnesses`

`{ room? }` — every harness in every open room, or, with `room` given,
every harness in that one room, refused loudly by name if it does not
exist or is archived. Archived rooms are otherwise skipped entirely
when `room` is omitted: none of their harnesses have a live process to
ask a phase of. Each entry names its `room_id`/`room_name` alongside
the same per-harness fields `get_room` reports.

### Scope note

`list_rooms`, `get_room` and `list_harnesses` all read across every
room the same way `find_rooms_for_path` does — see the scope note
under "Finding rooms by path" above, which this reuses rather than
restating. `list_rooms`'s `created_by: "me"` is a filter on the
result, not a narrowing of the token's own reach: an unfiltered call
still lists every room on the machine.

Still refused for every room these three can see, the same as
everywhere else in this API: `resolve`, `approve`/`sign_off`/
`mark_approved`, and `archive_room`/`remove_worktree`/`delete_room`/
`close_room`. A director can watch a gate; it can never open one, on
its own room or anyone else's.

Out of scope for #356: a room's message history — now `message_history`
above (#364) — and notifying a director when a child room's sign-off
changes; these verbs are pull, not push.

## The permission-required signal (#86)

`POST /api/harness/permission` is not an agent verb — it carries no MCP
tool, and no agent ever calls it on purpose. It exists so "permission
required" can be a first-class activity phase instead of folding into
end-of-turn `waiting`.

The caller is the `PermissionRequest` command hook Skein's Claude Code
plugin injects (#215): the hook fires only when a permission dialog is
actually shown, POSTs its own JSON payload to this route, and always
exits 0 so a curl failure never fails the tool call it is reporting on.
opencode has no hook mechanism to match; its harness derives the same
phase straight from the `permission.asked` / `question.asked` events on
its own `/event` stream, and never calls this route at all.

Same two headers as every other route — `Authorization: Bearer` and
`X-Skein-Harness` — but here `X-Skein-Harness` is not mere attribution:
the harness id **is** what the event is about, so it must be present
*and* name a harness the room actually contains, or the answer is `400`
rather than a silent no-op.

The body is read leniently: only `tool_name`, `agent_type` and
`agent_id` are pulled out of it (all optional), and a body that is not
JSON at all still answers `204` — the hook's payload shape belongs to
Claude Code, not to Skein, and a future shape change must not start
breaking the harness's turn. `tool_input` is never read or logged; a
permission dialog is often asking about the very thing that would be a
secret. `agent_id` is present only when the hook fires inside a
subagent — it is what lets Skein tell whose dialog is open, so one
subagent's tool result can't clear another's (#276, epic #298).

A successful call emits `skein://harness-permission` —
`{ roomId, harnessId, toolName: string | null, agentType: string | null, agentId: string | null }`
— and answers `204 No Content`. The route also writes one
`tracing::info!` per request (#176).

## The launch signal (#273)

`POST /api/harness/session-start` is not an agent verb either. A
freshly spawned Claude harness has no JSONL transcript at all until the
first prompt, and the transcript tail is the phase store's
authoritative source everywhere else — so a harness sat there before
its first prompt never left `spawning`. This route gives it a way out.

The caller is the `SessionStart` command hook Skein's Claude Code
plugin injects (#215): it fires on every session start, resume, clear,
compact and fork, with no matcher restricting it to only the first.
That is deliberate rather than an oversight — the frontend only ever
acts on a harness still in `spawning`, so a later fire (a `clear` or a
`compact` mid-session) is a free no-op. opencode has no hook mechanism
to match; it already reports liveness over its own `/event` SSE stream
and never calls this route.

Same two headers as the permission route, and `X-Skein-Harness` is
authoritative the same way: absent or unknown is `400`, not a silent
no-op.

The body is Claude Code's own hook payload — it also carries
`session_id`, `cwd`, `transcript_path`, `permission_mode` and sometimes
`model` — and none of it is Skein's to police. Only `source` is read,
purely for the log line, and a body that is not JSON at all still
answers `204`: a future payload shape change must not start breaking a
harness's launch.

Duplicate fires must stay harmless, and not only because of the
matcher-less hook above: upstream anthropics/claude-code#78455 reports
`SessionStart` firing twice within a few hundred ms for the same
project, once for a "phantom" session that never materialises, with a
payload indistinguishable from the real one. Nothing here or downstream
may hang a consume-once side effect on this route — emitting the event
twice is fine.

A successful call emits `skein://harness-session-start` —
`{ roomId, harnessId }` — and answers `204 No Content`.

## What it will not do

There is no `resolve` and no `approve`, and there never will be here.
An agent that can close its own comments removes the only gate in the
loop; one that can sign off its own work removes it a level higher. In
both cases the reviewer would find their approval on code they never
read.

Each is refused three ways, on purpose:

- absent from `tools/list`;
- refused **by name** in `tools/call` — `resolve`, `resolve_thread`,
  `close_thread`, `mark_resolved`, and `approve`, `sign_off`,
  `mark_approved` and friends all come back with a reason. A model told
  a tool is merely *missing* goes looking for another way in; a model
  told *whose decision it is* stops.
- `POST /api/comments/{id}/resolve` and `POST /api/status` exist and
  answer `403`, so the answer reads as design rather than as "not
  implemented yet".

**Creating a room is allowed; destroying one is not.** `create_room`
(#330) is deliberately not a symmetric pair with some `close_room` or
`archive_room` — the line it draws is *whose decision it is*, not read
vs write. Opening a room is reversible and visible: it lands as an
ordinary room the user can see, close, or ignore. Closing one is not
the caller's call to make, least of all the room's own occupant.
`archive_room`, `remove_worktree`, `delete_room` and `close_room` are
refused **by name** in `tools/call`, the same three-way treatment as
`resolve`/`approve` above — the reason names whose decision it is
rather than leaving a model to go looking for another way to tear a
worktree down (a raw `rm -rf`, say). This is the same standing rule
the rest of Skein already follows: it performs no git mutations of its
own (`CLAUDE.md`), and #330 does not carve out an exception just
because the *thing* being destroyed is a room instead of a commit.

## Errors

Loud, and distinguishable (#176):

| status | meaning |
| :-- | :-- |
| `400` | malformed JSON, an `MCP-Protocol-Version` we do not speak, or (on the permission and session-start routes only) a missing/unknown `X-Skein-Harness` |
| `401` | no bearer token, or one that was never minted |
| `403` | a revoked token, a non-localhost `Origin`, or `resolve` / `approve` |
| `404` | the token's room is gone |
| `405` | `GET`/`DELETE /mcp` — no stream, no sessions |
| `410` | the room was archived while the agent held the review open |

A *tool* that ran and refused is different: it comes back inside a
successful JSON-RPC result with `isError: true` and the reason as text.
A protocol-level error is handled by the client's plumbing and never
reaches the model, which would then simply try again.

Non-localhost `Origin` is rejected because a page in the user's browser
can otherwise reach 127.0.0.1 by DNS rebinding — the MCP spec requires
the check, and the listener binds 127.0.0.1 only besides.

## How it reaches the harness

Nothing to configure: #215 wires this at spawn. Skein ships a small
config bundle as an app resource and points each CLI at it for that
session only.

| harness | what Skein does |
| :-- | :-- |
| Claude Code | appends `--plugin-dir <resources>/harness-config/claude-plugin` |
| opencode | sets `OPENCODE_CONFIG=<resources>/harness-config/opencode/opencode.json` |

The bundle's configs interpolate the variables above — `${VAR}` for
Claude Code, `{env:VAR}` for opencode — so the ephemeral port still
never has to be agreed in advance.

**Nothing is written into the worktree.** No `.mcp.json` to exclude
from the review's own diff, no MCP trust prompt on first run, and
nothing left pointing at a dead port once the room is archived.

**Both mechanisms are additive.** `--plugin-dir` loads alongside the
plugins and connectors you installed (it shadows only a plugin of the
same name); `OPENCODE_CONFIG` is *merged* between your global config
and the project's, so your providers and credentials survive and a
repo's own `opencode.json` still wins. **Settings → Shell &
environment** shows exactly what is injected and can switch either off
— turning the opencode one off is how you reclaim `OPENCODE_CONFIG`
for a config file of your own.

The Claude Code plugin also carries a `skein-review` skill describing
the review loop, and a `skein-mail` skill covering the mailbox and
`create_room`. Skills are loaded lazily, so neither costs anything
until used.

Inside a Claude Code harness, `/mcp` should list the server and
`review_status` should answer. The tools are named
`mcp__plugin_skein_api__<verb>` — plugin-provided MCP servers carry
a `plugin_<plugin>_<server>` prefix. opencode names them
`skein_<verb>`. An agent's own `tools` allowlist (#246/#247) that still
names the old `mcp__plugin_skein_review__…` / `skein-review_…` tools
must be updated to the new names, or it silently hides the whole
server.

**By hand, from inside a harness** — useful for checking the server is
actually up:

```bash
curl -s "${SKEIN_REVIEW_URL%/mcp}/api/health"
curl -s -H "Authorization: Bearer $SKEIN_REVIEW_TOKEN" \
        "${SKEIN_REVIEW_URL%/mcp}/api/comments"
```

Settings → About shows the bound port, or says why there is none.

## Where the code is

| file | what it owns |
| :-- | :-- |
| `agent_api/state.rs` | shared state, the `skein://review-changed`, `skein://harness-permission`, `skein://harness-session-start` and `skein://mail-changed` (#327) events, `HarnessIdentity` |
| `agent_api/auth.rs` | `Origin`, bearer, token → room, archived/revoked |
| `agent_api/verbs.rs` | the fourteen verbs — the whole testable core, including the mailbox (#327, plus `message_history`, #364) and the cross-room reads (#354, #356) |
| `agent_api/mcp.rs` | JSON-RPC, the tool schemas, the resolve and approve refusals |
| `agent_api/http.rs` | the routes, including `/api/messages` and `/api/messages/history` (#327, #364), `/api/rooms`/`/api/rooms/{id}`/`/api/harnesses` (#356), `/api/harness/permission` (#86) and `/api/harness/session-start` (#273) |
| `agent_api/tests.rs` | scoping, both prohibitions, lifecycle, real HTTP |
| `review_surface/signoff.rs` | the sign-off itself, and the staleness rule (#214) |
| `harness_config.rs` | what Skein injects at spawn so a CLI finds all this (#215) |
| `harness-config/` | the shipped plugin + opencode config themselves |

Anchoring is **not** reimplemented here: `get_comment` and `get_diff`
call `review_surface::query::file_impl`, the same function the pane
calls, so the agent and the reviewer can never be looking at two
different answers to the same question.
