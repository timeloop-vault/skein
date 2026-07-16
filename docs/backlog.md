# Backlog

Parked ideas. Things we've thought about but decided to defer beyond the
active chapter. Lives outside any chapter so it survives context resets.

Format: short title, one-line "what", brief "why parked." When something
moves into an active phase, delete it here.

## UX surfaces

- **Settings panel proper** — API keys per harness kind, default
  starting harness, default worktree placement, permission mode. The
  cog modal currently covers theme/density/font/scale only because
  we've kept the surface tiny.
- **Real onboarding tour** — chapter 1's tour was fixture-driven and
  got deleted in chapter 2 phase 1. A real "create your first room"
  walk-through is a separate product surface, not a fixture re-skin.
- **Worktree cleanup on archive** — closing a room (chapter 6 phase 2)
  doesn't `git worktree remove` the worktree dir. Add a "delete
  worktree on close" toggle when the surface justifies it.

## Display polish

- **Syntax highlighting in the diff view** — current diff is plain
  monospace with `+`/`-` colouring. `web-tree-sitter` would add
  per-language highlighting.
- **BYOH permission UX** — only meaningful if we ever build our own
  agent loop. Each harness handles its own permission flow today.

## Infra

- **Multi-window** — single window is fine for v0.
- **OS code signing** — chapter 8 shipped unsigned installers with a
  Tauri-signed auto-updater; real Apple / Authenticode signing waits
  until an audience justifies the certificate cost.

## From the grok-build recon (2026-07-16)

Ideas surfaced by `docs/grok-build-recon-2026-07-16.md` (file citations
there). Parked because each is real product surface, not a quick win.

- **Queue/steer a message to a busy harness** — three verbs from grok:
  Enter = queue for next turn, send-now = cancel-and-send, interject =
  merge into the running turn (never lost: idle fallback + stranded
  flush). Skein has no way to hand text to a busy harness today.
  Minimum viable: type into the PTY when idle, hold/queue when busy.
- **Per-room checkpoints via git-ref snapshots** — scratch-index
  `read-tree HEAD` + `add -A` + `commit-tree -p HEAD` + `update-ref`;
  never touches HEAD/index/worktree. Snapshot at harness idle
  boundaries to `refs/skein/rooms/<id>/<n>` → diffable rollback nearly
  free; snapshot-before-archive would make close=archive fully
  reversible. Waits on the review surface (#52/#106) to give it a UI.
- **Remote release policy: min-version floor + announcements** — fetch
  a small policy JSON at boot and check *before opening skein.db*; the
  #167 boot-wipe is the textbook incident. Channel pointer files
  support deliberate rollback (tauri-updater won't downgrade by
  default). Announcements ("data-migrating release coming — update
  first") are a ~200-line lift. Waits until the audience justifies
  running a policy endpoint.
- **Notification hooks** — user-configured shell command per event
  (ROOM/HARNESS/EVENT env vars) as the Slack/webhook escape hatch;
  complements the existing badge/toast/OS pipeline.
- **Room guardrail presets / sandboxed rooms** — presets
  (attended/unattended/review-only) materialized as flags + a
  `.claude/settings.json` written into the worktree (grok reads
  Claude's file, so one artifact configures both harnesses). Long
  range: kernel sandbox confined to the worktree instead of prompts
  (grok's `[sandbox] auto_allow_bash` duality). Waits on #76-style
  unattended use being real.
- **Fork a room including uncommitted work** — grok's fast-worktree
  clones dirty+untracked state via CoW copy (`PreserveWorkingTree`);
  "branch this room mid-experiment" is the product shape. Waits on a
  concrete need.
- **Terminal ring buffer + replay** — backend per-PTY ring (grok: 256
  KiB + byte offsets) would let cross-room previews (#76) and webview
  reloads render scrollback without keeping xterm.js mounted per
  harness. Do after the 16 ms output-coalescing issue lands.

## Migration candidates

- **React → Preact** — Preact is ~3 kB vs React's ~45 kB with a
  near-compatible API (`react` aliases to `preact/compat`). Shrinks the
  bundle meaningfully without touching component code. `poe-inspect-2`
  uses this exact recipe under Tauri, so it's well-trodden. Wait until
  the UI shape settles — chapters 6 and 7 reshape a lot of it.
