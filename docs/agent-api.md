# The Skein review API (#213)

Epic #52 exists because the agent could not read review comments.
#211 gave the diff a lifetime, #212 gave the reviewer somewhere to
write, and this is the piece that closes the loop: a localhost HTTP
server inside the running Skein process, exposed as an **MCP endpoint**,
so an agent in a room can read the comments on its own work, answer
them, and say what it did about them.

It **cannot resolve a thread.** That is the point, not an omission —
see [The one thing it will not do](#the-one-thing-it-will-not-do).

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
     └─ GET    /api/diff               get_diff
```

`/mcp` is what Claude Code and opencode talk to. `/api/*` is the same
five verbs as ordinary JSON, for the `skein` CLI epic #52 D10 defers;
both go through the same functions in `agent_api/verbs.rs`.

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

**The token is the scope.** No request names a room, so a token can only
ever reach the room it was minted for — there is nothing to tamper with.
`X-Skein-Harness` is *attribution only*: it decides whose byline a reply
carries and never what may be read, so a wrong or missing value costs
the comment its name and nothing else.

A token lives no longer than the Skein process that handed it out. Every
PTY dies with the app, so nothing legitimate is still holding one after
a restart — and a token that ended up in an old transcript stops working
at the next boot.

## The verbs

All five are room-scoped by the token. MCP tool names; Claude Code
presents them as `mcp__skein__<name>`.

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

## The two things it will not do

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

## Errors

Loud, and distinguishable (#176):

| status | meaning |
| :-- | :-- |
| `400` | malformed JSON, or an `MCP-Protocol-Version` we do not speak |
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

## Driving it today

Automatic config injection is #215. Until it lands, wire it by hand.

**Claude Code** — `.mcp.json` in the room's worktree (both harnesses
inherit the variables, and Claude Code expands `${VAR}` when it reads
the file):

```json
{
  "mcpServers": {
    "skein": {
      "type": "http",
      "url": "${SKEIN_REVIEW_URL}",
      "headers": {
        "Authorization": "Bearer ${SKEIN_REVIEW_TOKEN}",
        "X-Skein-Harness": "${SKEIN_HARNESS_ID}"
      }
    }
  }
}
```

Then `/mcp` inside the harness should list five tools and no resolve.
`.mcp.json` is worktree-local, so add it to `.git/info/exclude` unless
you want it in the review's own diff.

**opencode** — the same, in `opencode.json`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "skein": {
      "type": "remote",
      "url": "{env:SKEIN_REVIEW_URL}",
      "enabled": true,
      "headers": {
        "Authorization": "Bearer {env:SKEIN_REVIEW_TOKEN}",
        "X-Skein-Harness": "{env:SKEIN_HARNESS_ID}"
      }
    }
  }
}
```

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
| `agent_api/state.rs` | shared state, the `skein://review-changed` event, `HarnessIdentity` |
| `agent_api/auth.rs` | `Origin`, bearer, token → room, archived/revoked |
| `agent_api/verbs.rs` | the six verbs — the whole testable core |
| `agent_api/mcp.rs` | JSON-RPC, the tool schemas, the resolve and approve refusals |
| `agent_api/http.rs` | the routes |
| `agent_api/tests.rs` | scoping, both prohibitions, lifecycle, real HTTP |
| `review_surface/signoff.rs` | the sign-off itself, and the staleness rule (#214) |

Anchoring is **not** reimplemented here: `get_comment` and `get_diff`
call `review_surface::query::file_impl`, the same function the pane
calls, so the agent and the reviewer can never be looking at two
different answers to the same question.
