# grok-build recon — what Skein can learn from xAI's coding agent

**Date:** 2026-07-16
**Subject:** [github.com/xai-org/grok-build](https://github.com/xai-org/grok-build) at commit `c68e39f` (2026-07-16) — xAI/SpaceXAI's open-sourced terminal AI coding agent (`grok`). Pure Rust, ~75 workspace crates, ~1.3M LOC, synced from their monorepo.
**Method:** 16 parallel research agents (11 dimensions + completeness critic + 4 follow-ups), each briefed on Skein's architecture and roadmap, all findings verified in code (not docs) with file citations. Citations below are paths inside the grok-build repo unless prefixed with `app/` or `crates/skein-git` (Skein paths).

grok-build is a *harness* (the thing Skein drives), not an IDE (the thing Skein is) — but it contains, in shipped production form, the backend of almost every surface on Skein's roadmap: a harness-agnostic protocol (#116), a hunk-level review engine (#52/#106), a cross-session dashboard (#76), worktree/checkpoint machinery (#182 + backlog), and a PTY test harness (#169). This doc maps the learnings onto the audit §5 arcs, then gives the recipe for adding grok as a Skein harness kind.

---

## 1. The headline: grok is "ACP-first" — and that answers #116's strategic question

grok-build's single most important architectural fact: **its own TUI is a client of its own agent runtime over the Agent Client Protocol (ACP)**, even in-process. The agent (`MvpAgent`, `crates/codegen/xai-grok-shell/src/agent/mvp_agent/`) is only ever driven over ACP; the TUI (in-memory channels), `grok agent stdio` (editors: Zed, Neovim, Emacs, marimo), a WebSocket server, headless `grok -p`, and a multi-client leader daemon are all the *same* JSON-RPC vocabulary over different transports. There is no PTY-scraping anywhere in the product.

- ACP here = Zed's upstream `agent-client-protocol` Rust crate v0.10.4 (protocol V1), plus an `x.ai/*` extension namespace (~100 methods: git writes incl. commit/stage-by-content, worktrees, full PTYs multiplexed over the protocol, hunk-level review, session fork/rewind, dashboard roster).
- The protocol carries everything Skein's telemetry layer reconstructs today: message/thought chunks, `ToolCall`/`ToolCallUpdate` **with structured diffs**, `Plan` (todo list) as protocol data, permission requests with typed options, retry/compaction events, usage.
- Crucially for integration: **grok persists each session as its ACP update stream** — `~/.grok/sessions/<encoded-cwd>/<sid>/updates.jsonl`, one `{timestamp, method, params}` envelope per line. Resume (`session/load`) is literally a replay of that file. So the persisted format = the wire format = the telemetry format.

**What this means for #116** (harness-adapter consolidation): adopt **ACP's vocabulary as Skein's internal normalized event model** — `SessionUpdate` / `ToolCallUpdate(+Diff)` / `Plan` / `request_permission` / turn events — with per-harness *transports* at the edge: JSONL tail for Claude Code, SSE for opencode, JSONL tail (or ACP stdio) for grok. Ecosystem check: Gemini CLI speaks ACP natively; Claude Code only via Zed's `claude-code-acp` adapter; opencode's is in progress. So "ACP as the only transport" is premature, but "ACP as the vocabulary + one adapter kind that covers ACP-native harnesses" is validated by grok dogfooding it at every scale. Two caveats the agents verified:

1. The interesting control surfaces (prompt queue, interjections, hunk review, task kill) are **`x.ai/*` extensions, not core ACP** — a universal adapter still won't erase per-harness divergence for the advanced verbs.
2. grok's own tool layer ships a **canonical tool-identity envelope** (`crates/codegen/xai-grok-tools/src/tool_taxonomy.rs`): every tool-call event gets `_meta["x.ai/tool"] = {version, name, kind (open-set), namespace, label, read_only, input}` where `label` is the cross-harness join key (grok `read_file`, codex `read_file`, opencode `read` → all `"Read"`), `input` is a canonical projection of only cross-harness keys, and a companion `claude_alias.rs` maps Claude Code tool names onto the same vocabulary ("two tables drifted apart" is cited as the reason it exists). **This is exactly the missing model for `liveContext/payload.ts`** — a small canonical envelope computed in the Rust adapters would let `rows.tsx`/`toolRows.tsx` render one vocabulary, and adding a third harness becomes a table entry, not a new adapter pair.

Supporting evidence for the consolidation thesis from their own codebase: grok hand-duplicated its permission types between two crates "temporarily" and they have already silently diverged (`ToolFilter::WebSearch` exists in one copy only); same for Sentry's private copies of path-scrub helpers. Even disciplined teams can't keep hand-synced duplicates aligned — which is #116's whole argument.

### Phase model: split what harnessActivity conflates

grok has *no single phase enum*. It decomposes what `app/src/harnessActivity.ts` treats as one state machine into planes:

1. **Authoritative busy/idle** — derived purely from host-owned state: `running turn OR non-empty prompt queue` (two tiny predicates, one shared definition of "idle" so consumers can't drift).
2. **Derived activity labels** — `Thinking / Responding / ToolRunning{title} / AutoCompacting / Retrying{n/max} / Waiting(Model|Subagent|TaskOutput|Sleep)` computed client-side from the event stream — presentation, not state.
3. **Turn-event log** — persisted, outcome-tagged: `TurnStarted / PhaseChanged / ToolStarted / ToolCompleted{outcome,duration} / PermissionRequested/Resolved{decision,wait_ms} / TurnEnded{Completed|Cancelled|Error} / Interjected` (a per-session `events.jsonl`, schema-versioned).
4. **Turn origin as first-class data** — `PromptOrigin` parsed from prompt-id prefixes (`task-completed-*`, `scheduler-fired-*`…) distinguishing synthetic auto-wake turns from user turns.

They added a durable `TurnCompleted{stop_reason}` event to the persisted stream **specifically because** a viewer attaching mid-turn otherwise hung on "Waiting…" — the exact gap Skein's L2a idle heuristic papers over. When #116 defines the harness-agnostic vocabulary: make turn-start/turn-end first-class persisted events, add outcome + duration + synthetic-origin, and keep "waiting for permission" (`permission_prompt`) as a first-class phase. Every enum in their wire types carries a `#[serde(other)] Unknown` sink with tests asserting unknown values deserialize instead of erroring — the tolerance contract `harnessEvents.ts` translators should adopt.

### A third telemetry channel exists: hooks (push, not pull)

grok fires a 14-event hook vocabulary (SessionStart/End, Stop-with-reason at **every** turn end, Pre/PostToolUse with full payloads, `Notification("permission_prompt")` at the exact moment a prompt appears, Subagent/Compact events) from the core SessionActor — so hooks fire even when grok runs as a plain TUI inside a Skein PTY. Claude Code has an equivalent native hooks mechanism, and grok even *reads Claude's hook files* with tool-name aliasing. So Skein could standardize on one push pattern — drop a hook config per room that POSTs the JSON envelope to a local Skein listener — covering Claude Code and grok with the same code, giving authoritative phase edges with zero polling. Verified constraints: grok's `http` hook type is HTTPS-only (SSRF policy — `http://127.0.0.1` is rejected), so the push must be a `command` hook (`curl … "http://127.0.0.1:${SKEIN_HOOK_PORT:-0}/…"`) with the port injected via PTY child env; project-scoped hooks need folder trust (`--trust` at first spawn); PreToolUse deny is **fail-open by design** — usable for per-room guardrails, but advisory, not a security boundary.

---

## 2. Adding grok as a Skein harness kind — the recipe

grok is arguably the **easiest possible third harness**: it follows Claude Code's exact pattern (pre-allocated session id + probeable store + JSONL tail), needs no port pinning (unlike opencode), and its docs explicitly say `--session-id` "matches Claude Code's anti-overwrite model".

| Concern | Value |
|---|---|
| Binary | `grok` (artifact name `xai-grok-pager`); install `curl -fsSL https://x.ai/cli/install.sh \| bash` |
| Spawn (new) | `grok --session-id <uuid>` — Skein pre-allocates; must be a valid UUID; **errors if the id already exists** (create-only — unlike Claude, cannot be reused to resume) |
| Resume (boot rewrite / reopen) | `grok --resume <sid>`; `--continue` = latest for cwd; `--fork-session` to fork |
| Store to probe (resume.rs) | `$GROK_HOME` (default `~/.grok`) `/sessions/<encoded-cwd>/<sid>/summary.json` exists ⇒ session exists (grok's own probe, `xai-grok-shell/src/session/persistence.rs`) |
| cwd encoding | `urlencoding::encode(abs_cwd)` — **not** Claude's dash-encoding; >255 bytes falls back to `<slug40>-<blake3-hex16>` with the real path in a `.cwd` file (`xai-grok-config/src/paths.rs`) |
| Telemetry tail (L2c) | `updates.jsonl` in the session dir — append-only NDJSON, `{"timestamp":…,"method":"session/update"\|"_x.ai/session/update","params":<ACP notification>}`. Richer than Claude's JSONL: typed tool-call **diffs**, `plan` updates, `turn_completed{stop_reason}`, `pending_interaction`/`interaction_resolved` (permission waits!), subagent/task/compaction events |
| Phase mapping | `user_message_chunk` → running; `pending_interaction` → waiting; `turn_completed` → idle; `session_end` → exited |
| Plan card | `plan.json` in the session dir is the todo state, already in ACP `PlanEntry{content, priority, status}` shape — maps 1:1 onto `plan.ts` |
| Extra goodies | `signals.json` (per-turn tool/error/latency/token counters), `active_sessions.json` (PID-liveness registry — detects crashed sessions), `events.jsonl` (lifecycle telemetry), `hunk_records.jsonl` (LOC attribution) |

**Tail-safety (verified):** `updates.jsonl` is never rewritten — rewind appends a `rewind_marker` line (timeline branch), compaction appends a `compaction_checkpoint` marker with bulk state in a separate file. Appends heal torn tails (a partial record costs exactly one line); the reader must skip unparseable lines and treat unknown `sessionUpdate` kinds as opaque. Do **not** tail `chat_history.jsonl` (atomically rewritten by compaction). Chunks are coalesced before write, so the tail sees message-level granularity.

**Spawn flags that matter:**
- **Always pass `--no-auto-update`** (or set `GROK_DISABLE_AUTOUPDATER=1` in the PTY env). Otherwise grok prints update banners into the PTY, may show an interactive `Update now? [Y/n/d]` prompt, spawns a detached self-updater that swaps `~/.grok/bin/grok` mid-session, and — with a configured minimum-version floor — can **exit 0 immediately** after force-installing ("Update installed. Run `grok` to start."), which a naive adapter would read as a clean instant exit.
- **Never pass `--worktree`** — grok would create its own worktree inside the room's worktree; rooms own the worktree.
- **Never pass `--sandbox` on resume** — the profile is pinned per session and a differing value is refused with an error.
- Consider `--trust` on first spawn per room if hooks/repo-local config should work without a one-time y/N trust prompt in the PTY (that prompt is a "waiting" state the current L2b patterns wouldn't recognize).
- Auth: `XAI_API_KEY` env for non-interactive, else browser OAuth cached at `~/.grok/auth.json`.

**Later upgrade path:** `grok agent stdio` speaks full ACP over stdin/stdout (`session/load` replays history with `_meta.isReplay` + cursor-based delta — Skein's "backfill 500 + live tail" pattern as a protocol), and the leader socket allows a Skein observer to attach to the *same* session the PTY-embedded TUI is showing. Dual-mode (PTY for display, protocol for telemetry) is grok's native architecture, not a hack.

---

## 3. Review surface (#52/#106): xai-hunk-tracker is the reference design

A dedicated crate (`crates/codegen/xai-hunk-tracker`, ~13k LOC of which ~6.6k tests) that is, in effect, **the backend of an IDE review surface, exposed over ACP extension methods** (`x.ai/hunk-tracker/get-hunks|get-files|get-summary|hunk-action|file-action|turn-action|all-action`) — the TUI itself never renders it; it exists for IDE clients. Exactly the component Skein needs. Key design decisions, all verified:

- **Two-layer model.** Git diff answers "what will be committed" (Skein's DiffCard already does this). The review unit is the *pending hunk* against a **session baseline** (HEAD blob at first touch, or the tool-reported `previous_content` for new files) — a second layer, not a decoration of the first.
- **Three-way attribution** on every hunk: `AgentEdit{prompt_index}` / `ExternalEditOnAgentFile` / `External`. Fed by (a) a normalized `FileWritten{path, content, previous_content, is_new_file}` event every mutating tool emits, and (b) fs-notify for external edits. Nothing grok-specific — Skein's Claude adapter already sees Edit/Write calls and opencode's SSE sees file events; normalizing both to one `FileWritten` shape is the #116-consolidation move that makes the review surface adapter-independent. Skein gets one extra dimension: *which harness* (multiple harnesses share a room worktree) — a straightforward extension of `prompt_index`.
- **Hunk-ID stability by recompute-then-match**: on every change, rediff the whole file (`similar` crate, 10s timeout, 1MiB cap), then re-attach identity — exact content match closest by line, else max overlap in *baseline* coordinates, with a claimed-ID set preventing duplicate inheritance on splits. Small, pure, covered by a 5.8k-line test file — liftable nearly verbatim into a Skein crate (per the "richer than DTO glue belongs in the crate" rule).
- **Asymmetric accept/reject**: accept patches the *baseline* forward (disk untouched — change stops being "pending"); reject patches the *file on disk* back. Granularities: hunk / file / **turn** / all, batched bottom-up so line numbers never shift. "Revert hunk" is a plain file write — **shippable before #182**.
- **Edge cases as an enum, not `Option<String>`**: `Missing | Binary{len} | TooLarge{len} | LfsPointer{len} | Symlink | Full(content)` with documented classification order (size cap before allocation; LFS pointer check because HEAD-blob-vs-smudged-copy produces phantom diffs; NUL sniff; lstat for symlinks). Renames are not detected (per-path model).
- **Review state survives restarts**: full snapshot + per-turn deltas persisted; the doc comment names the failure prevented — after reload an empty tracker makes pending hunks "silently disappear — the user sees their changes auto-applied". Skein analog: snapshot into the Room blob (`#[serde(default)]`!) or a sibling table.
- **`SessionSummary`** — pending hunks grouped per turn with files and +/- counts in one call — is a ready-made per-room stat for #76 cards ("3 files, +120/−14 pending, 2 turns unreviewed").
- TUI diff rendering tricks worth stealing for the activity feed: stitch consecutive same-file edits, **bail to separate hunks whenever shared coordinates can't be truthful** ("never render wrong content"); progressive two-phase syntax highlighting (instant per-hunk, background full-file upgrade, 2MiB/50k-line caps).
- Their gix-over-libgit2 rationale is documented: libgit2's global libiconv lock contends on macOS when parallel sessions run git ops — a watch-item for skein-git once #76 polls many rooms.

Also relevant: grok's plan-mode approval UI uses `PlanComment{id, line_range, text}` — a proven minimal review-comment data model reusable for hunk comments.

---

## 4. Cross-room view (#76): the Agent Dashboard is a direct blueprint

`grok dashboard` / Ctrl+\ (`crates/codegen/xai-grok-pager/src/views/dashboard/`, docs/user-guide/23-dashboard.md — docs verified against code):

- **Attention-first state groups with strict precedence**: `NeedsInput > Working > Idle > Inactive > Completed > Failed`, where needs-input **beats a running turn** (a session awaiting permission mid-turn is needs-input) and "Working" persists while background tasks/monitors are live even after the turn ends. Rows are 2 lines: state icon + title + git branch/worktree + age, then a dim secondary line (`Pending: <permission title>` / live activity / last-message preview). The Idle group folds to the 8 freshest + last-hour-active with an "N more" row; header shows `N agents · M working · K awaiting`.
- **Peek-and-reply**: selecting a row shows the last response and a live reply input; **pending permission prompts and questions are answerable from the peek with keys 1–9** without attaching. This is the killer multitasking feature — triage a blocked room from the overview.
- **Interaction-design pitfalls they hit so Skein doesn't have to**: the dispatch box always spawns a *new* session (pre-seeding reply-mode from the selection got users stuck replying to the wrong agent — recorded in code comments); filtering is an explicit mode (Ctrl+/) so typed prompts are never misread as filters; pins persist keyed by durable session id, never slot index; `last_change_at` is wall-clock `SystemTime`, not `Instant`, because rows from other processes predate this process's boot.
- **Resident + dormant merge**: the roster merges live sessions with the 200 most-recently-touched on-disk summaries (mtime-stat fast path: 12k sessions, 3s → 200ms), polled at 1s *only while the dashboard is open*. Skein's analog: live rooms merged with archived rooms from sqlite.
- **Notification discipline** (all verified in code): notify only when unfocused; suppress TurnComplete while the prompt queue is non-empty (only the last queued turn notifies); one ApprovalRequired per batch; errors with a dedicated modal skip the generic notification; the notification *title* is the session name so users know which task completed. Skein has badges/toasts/OS notifications but none of these suppression rules — adopting them directly reduces fatigue in multi-room use.
- A sqlite **worktree registry** (`xai-fast-worktree/src/db/schema.rs`: path, source_repo, kind, session_id, head_commit, creator_pid, status, + rebuild-from-disk discovery and GC) is what makes a trustworthy dashboard and orphan cleanup possible — Skein's rooms-as-JSON-blobs can't answer "what worktrees exist for this repo" without opening every blob.
- Usage: per-session context% (`ContextInfo`) is plumbed into dashboard rows (unrendered v1); `signals.json` keeps durable per-session health counters (consecutive cancellations = user fighting the agent). Cheapest Skein win: parse token usage from the Claude JSONL it already tails and surface a per-room context gauge.

---

## 5. Files pillar (#49) and editing

- **Ship-blocker-grade find**: grok's `clippy.toml` **bans `std::fs::canonicalize` / `Path::canonicalize` / `tokio::fs::canonicalize`** (disallowed-methods) because on Windows they return verbatim `\\?\C:\` paths that break external tools and poison path-equality keys; `dunce::canonicalize` is the blessed replacement. **Skein's `app/src-tauri/src/fs.rs` uses raw `std::fs::canonicalize` (lines 60/62) for its path-containment check** — the exact module #49/#174 is about to put on the hot path, on a Windows daily-driver. Adopt the ban + dunce.
- **Detection over locking** for concurrent agent+human edits: grok ships a well-built per-path file-lock manager with *zero call sites*. What's actually wired: `FileWritten` notifications with `previous_content`, per-prompt before/after snapshots (external-modification test = `current != after_snapshot`), fs-notify-attributed external hunks, and model-facing staleness hints ("the user may have changed the file since you last read it") instead of mtime checks. Lesson for the CodeMirror surface: build detection + attribution + cheap recovery first; locks are speculative infrastructure.
- **read_file practicalities** for fs.rs preview revival: hard caps (1000 lines / 25k tokens with retry guidance), magic-byte MIME sniffing before preview, image downscaling budgets, base64-data-URI lifting for notebook-ish files.
- **Config-file checklist**: grok reads the whole AGENTS.md family — `AGENTS.md`, `CLAUDE.md`, `CLAUDE.local.md`, `.claude/rules/`, `.cursor/rules/` — with a vendor×surface compat matrix. Practical upshot: **one CLAUDE.md per room already serves both a Claude harness and a grok harness**, and these are the files a room's file surface should pin. `grok inspect --json` (every loaded config file with token estimates) is a ready-made "what does this harness see" panel idea for Live Context.
- **Push-based file index**: the agent pushes `x.ai/fs/index` (full) + `x.ai/fs/index/delta` (incremental) file lists to clients — the target shape for Skein's file tree vs. re-listing on every watcher tick.
- Edit application: exact-match with rich self-recovery errors (nearest-match hint, Unicode-confusable diagnostics with roundtrip validation, TOCTOU-safe `file_snapshot_at_edit`); fuzziness quarantined to the codex port's 4-pass `seek_sequence` ladder (~120 pure lines — the portable algorithm if Skein ever re-applies harness patches from `diff.ts`). The experimental "hashline" toolset (reads hand out per-line `LINE:HASH` anchors; edits address by anchor; stale anchors detected at edit time) is a preview of where edit tools are going.

---

## 6. Git layer (#182, #171–179) and watcher

- **grok links gix AND git2 yet does every mutation via the git CLI** — commit, stash, checkout, reset, worktree add/remove, fetch — with an auth-suppression env set (`GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=""`, `GIT_LFS_SKIP_SMUDGE=1`, `ssh -o BatchMode=yes`, `--no-optional-locks`) so nothing ever blocks on a prompt. Their code comment: **"libgit2's status is 5-10x slower than the native git binary on large repos due to inefficient index refresh."** For #182: implement commit/stage/push as spawned git with those envs rather than growing libgit2 write support; hooks, LFS, credential helpers behave for free. For #171–179: skein-git's sync `status()`/`diff_workdir()` in Tauri commands is the indicted pattern; every grok blocking git API is documented "call from spawn_blocking" and actually is.
- **The single most portable piece found**: xai-fsnotify's **git-operation lock state machine** (`state.rs`, ~120 dependency-free lines) — watches `index.lock`/`gc.pid`, drives `Idle → Locked → Settling → Cooldown` (500ms settle merges rebase per-pick lock cycles into ONE operation), emits one `GitOperationStarted`/`GitOperationCompleted{head_changed}` pair per merged op. Skein's watcher leaves `.git` unfiltered and re-diffs on every 200ms tick *including mid-rebase transient states*; porting the state machine + suppressing diff refetch during ops fixes diff-card flicker and wasted work. Their watcher also: gitignore-aware filtering (mtime-keyed cache), surgical `.git` watching (refs/heads+tags only; objects/ never), per-path event coalescing, Linux inotify watch budgets, and a process-wide shared-watcher registry.
- Worktree creation: `xai-fast-worktree` makes worktrees disposable — `git worktree add --no-checkout` + parallel reflink/CoW copy (APFS/btrfs), optionally **preserving dirty+untracked state** (fork a room including uncommitted work), removal via `rm -rf` + deregistration (~10x faster). Relevant if room creation ever feels slow on big repos.
- **Worktree-to-git-ref snapshots** (backlog gem): stage the whole working state into a scratch `GIT_INDEX_FILE` (`read-tree HEAD` + `add -A` + `write-tree` + `commit-tree -p HEAD` + `update-ref`) — never touches the user's HEAD/index/tree, durable, diffable, O(changed blobs). Skein rooms are already worktrees; snapshotting at harness idle boundaries to `refs/skein/rooms/<id>/<n>` gives per-room checkpoints/rollback nearly free, and archiving a room could snapshot before removing the worktree (making close=archive fully reversible). Their `/rewind` protocol details are also instructive: `reset --soft` never `--hard`, stash-or-abort, persist-ref-then-delete ordering.

---

## 7. Persistence, crash handling, failure surfacing (#167 follow-ons, #176)

- **WAL is unsafe on network filesystems** (mmap'd `-shm` SIGBUS): `xai-sqlite-journal` statfs-detects network mounts (incl. FUSE) and downgrades to TRUNCATE journal + per-host DB filename. Skein applies WAL unconditionally; Windows `%APPDATA%` can be an SMB-redirected folder. A ~50-line probe at db open removes a real corruption class.
- **One-way schema-version ratchet**: wipe only when stored < current, tolerate newer, never regress — fixes a stable/alpha ping-pong that left an index permanently empty. Matters for Skein because `scripts/local-bundle.sh` shares the release identifier (two generations can open one skein.db).
- **Torn-tail healing + lenient readers** for any JSONL: check last byte before append, prepend `\n` if torn (damage = exactly one record); readers skip bad lines, quarantine as `.corrupt`; full rewrites tmp+rename. Also required behavior for Skein's grok tailer.
- **#176 toolkit**: PID-liveness registry (`active_sessions.json` — "Skein crashed with N rooms live" at next boot instead of silent resume); one pid/version-stamped `unified.jsonl` log that UI clients forward into over the protocol (Skein's React errors currently never reach the Rust log — a small Tauri log-forwarding command is the same pattern); boot-time "previous run crashed" report; error-context promotion (`OUT_OF_DISK_CONTEXT` — promote the root cause to the flattened message as a named constant the UI matches on, because Tauri-style boundaries flatten error chains to `Display`); hooks' failure taxonomy distinguishing "proceeded" from "proceeded because the check broke" with distinct labels, all surfaced, never blocking.
- **Redact at export, never at capture**: a 320-line liftable sanitizer crate (`xai-grok-secrets`) applied *only* at network boundaries; local sqlite/JSONL keeps raw fidelity. Skein stores payloads verbatim (correct!) — the module matters the day any export path exists (bug-report bundles, log sharing).
- Config errors cite line/column but **never echo the offending line** (it may contain a secret) — one helper serving logs, UI, and traces.

---

## 8. App architecture (#19) and testing (#168/#169)

- **#19 blueprint**: the TUI is Elm-style — three enums (`Action` = sync intent, `Effect` = async work *described not executed*, `TaskResult` = completions fed back) with a dispatch tree split into ~25 domain modules behind a router; invariants documented: "never touches terminal/network/fs, all mutations sync + deterministic, fully testable without tokio or a terminal". **Honest counterpoint: their root view file is still 10,366 lines.** The pattern buys testability and domain-local logic, not small files — for App.tsx, extract a pure reducer + effects-as-data (keydown dispatch, notifications, palette, DnD as domain modules), migrate with extract-and-re-export, and accept the root component stays big.
- **Types-crate discipline**: every boundary gets a pure-data `-types` crate (serde only, no I/O — "cheap to depend on from anywhere"), conversion at the producers. The #116 event model should be exactly this shape. Composition-root `-bin` crate + fn-pointer IoC seam breaks dependency cycles — same trick works in TS for injecting the files/review surfaces into App.tsx through a narrow interface.
- **#169 blueprint — the highest-leverage transferable asset**: they test the entire TUI by spawning the real binary in a PTY and asserting on **rendered screen text** via a ~350-line `alacritty_terminal` wrapper (`ptyctl`), with event-driven wait conditions whose timeouts embed the final screen + cursor + raw tail as diagnostics, a **mock inference server** (hold/release turns, chunk pacing, per-turn FIFO responses) and hermetic `$HOME` sandboxes, 140 Rust e2e tests + 45 declarative YAML scenarios, per-platform p99 frame-time baselines gated in CI, and exactly **9** insta snapshots (all diff rendering — snapshot only where output is visual). Skein equivalents: (a) real integration tests for the untested `pty.rs` today (spawn `/bin/sh`, assert `stty size` after resize, exit-event delivery, the issue-#23 UTF-8 carry logic); (b) a **stub harness binary emitting scripted Claude-JSONL/opencode-SSE** for deterministic adapter→phase-machine→store pipeline tests (#116); (c) "playground" dev binaries mounting one widget with fixture data — a Storybook substitute needing no test framework.

### PTY specifics

- **Skein's 2-thread design is validated** — grok's ConPTY notes corroborate every claim in pty.rs's comments; they *poll* `try_wait` where Skein's blocking waiter thread is lower-latency and more correct. Keep it.
- **Real gap found: process-tree teardown.** `PtyManager::kill` kills only the direct child; a harness's grandchildren (node servers, MCP subprocesses, opencode's embedded server) can leak on room close — worst on Windows. grok's `xai-tty-utils` uses **Windows Job Objects** (`KILL_ON_JOB_CLOSE`) and validated `killpg` on Unix, with a test proving grandchildren are reaped.
- **16ms output batching + 256KiB ring buffer with byte offsets**: bounds notification rate to ~60/s during `cargo build`-style floods and enables reconnect/replay (and #76 terminal previews from a backend ring instead of N hidden xterm instances). Skein currently sends one Channel event per `read()` at unbounded rate — a cheap win in the reader thread.
- **Env hygiene**: strip ~30 host-terminal identity markers (TERM_PROGRAM, TMUX, ZELLIJ, NVIM…) before spawning; Skein forwards the full user env, so Skein-inside-tmux leaks the tmux profile into harness terminal detection.
- `tcgetpgrp`-based busy detection (foreground pgid ≠ shell pid, sampled 500ms, transition events only) — an OS-authoritative "running a command" signal for the shell harness kind, no output parsing (Unix only).
- Sleep/wake: `xai-system-power` delivers WillSleep/DidWake events. Skein's analog problem is telemetry: laptop sleep kills opencode SSE and stales JSONL tails; a DidWake hook forcing SSE reconnect + resume-probe + watcher tick cheaply fixes "telemetry died overnight".

---

## 9. Backlog candidates (parked ideas surfaced by this recon)

- **Prompt queue + interjection UX**: three verbs — plain Enter queues for next turn; send-now cancels-and-sends; true interject merges into the running turn at safe drain points, never lost (idle fallback + stranded flush). Skein has no way to hand text to a busy harness; minimum viable = type into the PTY when idle, queue when busy.
- **Per-room checkpoint/rollback** via git-ref snapshots (§6) tied to turn boundaries from the phase machine.
- **Notification hooks**: user shell commands per event with env vars (ROOM/HARNESS/EVENT) — outsources Slack/webhook integrations.
- **Remote release policy + min-version floor**: fetch a small policy JSON at boot, check **before opening skein.db** — the #167 boot-wipe is the textbook incident where blocking old clients beats any local defense. Channel *pointer files* (not tags) support deliberate rollback; tauri-updater's no-downgrade default is the gap. In-app announcements ("a data-migrating release is coming") are a ~200-line lift.
- **Usage/context% per room** (from the JSONL Skein already tails) + `signals.json`-style room-health counters.
- **Room guardrail presets**: everything in grok's permission pipeline is drivable from flags/files, and grok reads *Claude Code's* `.claude/settings.json` — one file written per worktree configures two harnesses. A "sandboxed room" (kernel confinement instead of prompts — their `[sandbox] auto_allow_bash` duality) is the long-range idea; their dangerous-list invariants (git push never auto-approvable) apply to #182's own commands.
- **Per-repo (not per-room) agent memory**: grok keys memory by origin-remote org/repo so all worktrees share it — the right default for rooms.
- **Fork room including uncommitted work** (`PreserveWorkingTree` cloning) and session-fork-as-directory-copy with parent pointers.

## 10. Suggested order of attack (recon opinion)

1. **Quick fixes, this week**: `dunce::canonicalize` in fs.rs + clippy disallowed-methods ban; process-tree kill (Job Objects/killpg) in pty.rs; env strip-list before harness spawn; 16ms output coalescing.
2. **#116 design input**: adopt the ACP `SessionUpdate`/`PlanEntry`/tool-taxonomy vocabulary as the normalized internal model; split phase into busy/idle vs activity-label vs turn-event-log; `#[serde(other)]` tolerance everywhere. grok as the third harness is both cheap and the live testbed for the design.
3. **Watcher upgrade** feeding Live Context: git-op state machine + gitignore filtering (small, pure, portable — directly improves today's DiffCard).
4. **#52/#106**: lift the hunk-tracker design (baseline model, 3-way attribution, recompute-then-match identity, accept/reject verbs, snapshot persistence) into a Skein crate; DiffCard grows per-hunk chips + per-turn grouping; "revert hunk" ships before #182.
5. **#76**: dashboard IA (state precedence with needs-input-first, 2-line rows, peek-and-reply, idle folding) + notification suppression rules + resident/dormant merge.
6. **#182**: git writes as CLI subprocess with the auth-suppression env set.
7. **#169**: alacritty-based screen assertions for pty.rs + stub-harness fixtures for adapter tests.

---

*Produced by a 16-agent research workflow over a shallow clone (scratchpad, ephemeral); ~2.4M tokens of code reading. Full per-dimension reports (with more file-level citations) were generated in the session scratchpad; this doc is the durable synthesis. Where docs and code disagreed in grok-build, code won and the divergence is noted above.*
