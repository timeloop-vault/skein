# Chapter 5 — Recon notes

Verified on macOS arm64 (Darwin 25.2.0) on 2026-05-03 against:

- `claude` 2.1.126 (`Claude Code`) at `/Users/<user>/.local/bin/claude`
- `opencode` 1.14.28 at `/opt/homebrew/bin/opencode`

Phase 2 / 3 / 4 should cite the section numbers below rather than
re-deriving any of this.

---

## 1. Claude — CLI argv shapes

`claude --help` reveals four resume-related flags. The two we care
about:

- `-r, --resume [value]` — resume a conversation by session id, or
  open the interactive picker if no value is passed.
- **`--session-id <uuid>` — *pre-allocate* a session id when starting
  a new conversation.** Must be a valid UUID. This is the headline
  finding of phase 1 (see §5).

Auxiliary, unused by chapter 5:

- `-c, --continue` — continue the most recent in cwd. Phase 5a's
  current behaviour.
- `--fork-session` — when resuming, create a new id instead of
  reusing.
- `--from-pr` — resume a PR-linked session.
- `--no-session-persistence` — disable session save (would defeat
  resume, do not pass).

We did **not** run `claude --resume <bogus-uuid>` end-to-end —
spawning a real Claude session inside this terminal would hijack
the shell. Phase 4's pre-flight check (file existence on disk) makes
the question moot.

## 2. opencode — CLI argv shapes

`opencode --help` resume-related flags:

- `-s, --session <id>` — continue by session id. **No pre-allocation
  equivalent of Claude's `--session-id`.** Phase 2 has to capture
  the id after the binary has written it.
- `-c, --continue` — continue last session (phase 5a's current
  behaviour for opencode).
- `--fork` — fork the session when continuing (use with `--continue`
  or `--session`).

There's also an `opencode session list` subcommand. On this machine
it returned nothing despite the db containing 3 rows — likely
filtered by current directory / workspace. We ignore it; phase 2
queries `opencode.db` directly via rusqlite (same pattern as Skein's
existing `db.rs`).

## 3. Claude — on-disk session storage

Path: `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl`

Path encoding (verified against 17 existing project dirs on this
machine):

- `/` → `-`
- Path starts with `/`, so encoded form starts with `-`.
- Original dashes in path components survive unchanged.
  → encoding is **lossy**: `/foo-bar/baz` and `/foo/bar/baz` both
  encode to `-foo-bar-baz`. Claude's problem, not ours.

Examples:

```
/Users/<user>                        → -Users-<user>
/Users/<user>/git/private/skein      → -Users-<user>-git-private-skein
/Users/<user>/git/example/feature-x
                                       → -Users-<user>-git-example-feature-x
```

We did **not** verify spaces or non-ASCII characters. Skein only
ever spawns Claude in our user-picked cwd; if the user picks a
folder with spaces, the encoding may differ from what we'd guess.
This becomes a non-issue if we adopt §5's `--session-id`
pre-allocation, since Skein then never has to *find* a Claude
session file by encoded path — only check whether *one specific*
filename exists.

## 4. opencode — on-disk session storage

Path: `~/.local/share/opencode/opencode.db` (sqlite, drizzle-managed)

Schema of the relevant table:

```sql
CREATE TABLE `session` (
  `id` text PRIMARY KEY,             -- format: "ses_<26-char-ulid>"
  `project_id` text NOT NULL,
  `parent_id` text,
  `slug` text NOT NULL,              -- e.g. "playful-canyon"
  `directory` text NOT NULL,         -- raw cwd, no encoding
  `title` text NOT NULL,
  `version` text NOT NULL,
  `share_url` text,
  ...summary fields...
  `time_created` integer NOT NULL,   -- epoch ms
  `time_updated` integer NOT NULL,
  `time_compacting` integer,
  `time_archived` integer,           -- soft-delete marker (NULL = active)
  `workspace_id` text,
  ...
);
```

Useful queries:

```sql
-- Phase 2: what session ids exist for this cwd before / after spawn?
SELECT id FROM session
WHERE directory = ? AND time_archived IS NULL
ORDER BY time_created DESC;

-- Phase 4: does this stored id still exist?
SELECT 1 FROM session
WHERE id = ? AND time_archived IS NULL
LIMIT 1;
```

`directory` stores the raw cwd verbatim — no path encoding to worry
about.

## 5. Headline finding — `--session-id` pre-allocation simplifies phase 2

Claude's `--session-id <uuid>` flag means Skein can **pick the UUID
itself** when spawning a fresh Claude harness:

1. Generate a fresh UUID v4 in JS (or Rust).
2. Spawn `claude --session-id <our-uuid>`.
3. Store `our-uuid` on the harness immediately.
4. On Skein restart, spawn `claude --resume <our-uuid>`.

No polling, no snapshot-then-diff, no parsing. The flow for Claude
becomes synchronous and deterministic.

This **only** applies to Claude. opencode has no equivalent flag, so
phase 2 keeps the snapshot-then-diff design (§4's queries) for
opencode.

The chapter 5 plan should be revised to split phase 2 into:

- **Phase 2a — Claude:** pre-allocate UUID at spawn time via
  `--session-id`. Trivial; one-line change to `cmdForKind` for the
  Claude branch and a `sessionId` field on the harness.
- **Phase 2b — opencode:** snapshot-then-diff via the `session`
  table queries above. Keeps the existing 5-second polling design
  from the original plan.

## 6. Claude JSONL format (informational)

For phase 4 *file existence* is sufficient — we never need to parse
this. Captured here so a future chapter 6/7 / future-us doesn't have
to look it up.

The first three lines of an in-progress Claude session are typically:

```jsonl
{"type":"last-prompt","leafUuid":"3bae1650-...","sessionId":"23d38993-..."}
{"type":"permission-mode","permissionMode":"acceptEdits","sessionId":"23d38993-..."}
{"parentUuid":null,"isSidechain":false,"type":"system","subtype":"bridge_status","content":"/remote-control is active. ...","timestamp":"2026-05-03T12:39:17.916Z","uuid":"3bae1650-...","userType":"external","entrypoint":"cli","cwd":"/Users/<user>/git/private/skein","sessionId":"23d38993-...","version":"2.1.126","gitBranch":"main"}
```

Useful fields if we ever do parse:

- `sessionId` — matches the filename UUID.
- `cwd` — the *resolved* cwd Claude saw.
- `gitBranch` — could surface in Skein's session-tab UI eventually.
- `version` — Claude version that wrote the file.

## 7. Implications for the chapter 5 plan

Rewrite phase 2 along the §5 split (phase 2a Claude / phase 2b
opencode). Phase 3 and phase 4 stay broadly as drafted, but Claude's
file-existence check is now `<encoded-cwd>/<our-uuid>.jsonl` —
we know the exact path because we picked the UUID, so the encoding
robustness concern from §3 disappears.

Out of scope for this chapter (still): Windows / Linux path
verification (chapter 7), real-time invalidation, gh copilot resume
(no resume mode).
