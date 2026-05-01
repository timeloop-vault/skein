# [Working title] — Agent IDE Spec

> A desktop IDE built around AI coding agents as first-class citizens. One window, multiple agent sessions in the sidebar, code and diffs on the right. Bring-your-own-harness for v1, embed-other-tools for v2.

---

## 1. Vision

Existing AI coding tools force a context switch: agents in one place, code in another, diffs in a third tab. This project consolidates them. The agent and the code it's working on live side-by-side in the same window, persistently, across many concurrent sessions.

The first version owns its agent loop end-to-end (BYOH — bring-your-own-harness). Later versions can embed Claude Code, opencode, and GitHub Copilot CLI as guest agents inside the same shell, so the user can mix harnesses per session without leaving the app.

## 2. Target user

Solo developer running multiple parallel coding sessions across personal and work projects. Comfortable with terminals, tmux, git worktrees. Not satisfied with VS Code's terminal panel for AI agent work because it's cramped, doesn't show diffs alongside the agent, and forces session switching through tabs that don't carry agent state.

Two real environments to support:

- **Personal**: Claude Code as primary driver, direct Anthropic API access.
- **Work (INGKA)**: GitHub Copilot mandated, accessed via `copilot-sdk-rust`, models routed through the Copilot entitlement.

## 3. Goals and non-goals

### Goals (v1)
- Single window, no context switching to see the agent's work.
- Multiple concurrent agent sessions, each with its own git worktree.
- Diff view always visible alongside the active session.
- Per-session persistence: kill the app, reopen, sessions are still there.
- Streaming agent output, tool-call-by-tool-call.
- Cross-platform: Windows + WSL2 first, Linux second, macOS welcome but not blocking.

### Non-goals (v1, deferred to later versions)
- Embedding Claude Code, opencode, or Copilot CLI as guests (BYOK — bring-your-own-kit).
- MCP server support.
- Slash commands and skills.
- Sub-agents.
- Cost tracking and budgeting.
- Mobile / remote access.
- Cloud sync.
- Plugin/extension system.

## 4. Core concepts

**Session** — A single conversation with an agent, scoped to a git worktree. Has a status (idle / running / waiting on input / errored), a name, a creation time, and a message history.

**Worktree** — Each session gets its own git worktree of the parent repo, so two sessions on the same project don't fight each other's edits. Created on session start, cleaned up on session delete.

**Harness** — The agent loop: send messages and tool definitions to a model API, execute returned tool calls locally, append results, repeat until the model returns a plain text response. v1 owns this loop; v2 will optionally delegate it to external CLIs.

**Tool** — A function the agent can call: `read_file`, `write_file`, `str_replace`, `list_dir`, `grep`, `bash`. v1 hardcodes these. v2 can add MCP-defined tools.

## 5. Architecture overview

### Stack

- **Shell**: Tauri 2.x. Native OS webview, Rust backend. Smaller and faster than Electron, cross-platform, mature ecosystem.
- **Frontend**: TypeScript + React (or Svelte — open for design-phase decision based on component library fit). Tailwind for styling.
- **Editor component**: Monaco (the editor from VS Code) embedded in the webview. Brings syntax highlighting, code folding, minimap, and a built-in diff editor for free.
- **Terminal component (deferred to v2 BYOK)**: xterm.js + portable-pty. Not used in v1.
- **Backend (Rust)**:
  - `tokio` async runtime
  - `reqwest` for HTTP
  - `serde` / `serde_json` for message marshalling
  - `sqlx` (SQLite) for session and message persistence
  - `git2` for worktree management
  - `similar` for text diffing where Monaco's built-in diff isn't enough
  - `misanthropy` (community Anthropic client) or hand-rolled API client for the model loop

### Data flow

```
[User input in chat pane]
    ↓ (Tauri invoke)
[Rust: agent loop]
    ↓ (HTTPS)
[Anthropic API]
    ↓ (streaming response with tool_use blocks)
[Rust: tool dispatcher]
    ↓ (filesystem / shell ops on worktree)
[Tool results]
    ↓ (loop back to Anthropic with tool_result)
... until plain text response ...
    ↓ (Tauri event stream)
[Frontend: chat pane renders tokens, diff pane re-reads worktree]
```

### Data model (SQLite)

```
sessions
  id (uuid)
  name (text)
  agent_kind (text: 'byoh' | 'claude-code' | 'opencode' | 'copilot')
  status (text)
  worktree_path (text)
  parent_repo_path (text)
  model (text)
  created_at (timestamp)
  updated_at (timestamp)

messages
  id (uuid)
  session_id (fk → sessions.id)
  role (text: 'user' | 'assistant' | 'tool_result')
  content (json)
  created_at (timestamp)

tool_calls
  id (uuid)
  message_id (fk → messages.id)
  tool_name (text)
  input (json)
  output (json)
  status (text: 'pending' | 'success' | 'error' | 'denied')
  created_at (timestamp)
```

## 6. UX layout (starting point — to iterate in Claude Design)

Three regions in a single window:

```
┌──────────────────────────────────────────────────────────────────┐
│  [top bar: app name | new session | settings]                    │
├─────────────┬────────────────────────────────────────────────────┤
│             │                          │                         │
│             │                          │                         │
│  Sidebar    │     Agent pane           │     Code / diff pane    │
│  Sessions   │     (chat + tool calls)  │     (Monaco editor or   │
│             │                          │      diff view)         │
│  - session1 │     [streaming output]   │                         │
│  - session2 │                          │                         │
│  - session3 │     [user input box]     │                         │
│             │                          │                         │
├─────────────┴────────────────────────────────────────────────────┤
│  [status bar: active model | session status | token count]       │
└──────────────────────────────────────────────────────────────────┘
```

### Sidebar (left, ~20% width)
- List of sessions, most recently active first.
- Each row: status dot + name + small agent-kind icon (different for BYOH / Claude Code / opencode / Copilot — only BYOH active in v1).
- Click to switch active session.
- Right-click for rename, archive, delete.
- "New session" button at top, opens a small dialog for: name, parent repo path, model.

### Agent pane (middle, ~40% width)
- Chat history: user messages, assistant text, tool calls (collapsible by default), tool results (collapsible).
- Streaming tokens render live.
- Permission prompts (e.g., for `bash`) appear inline as approval cards, not modals.
- User input box pinned to bottom, multi-line, supports paste + drag-drop file references.

### Code / diff pane (right, ~40% width)
- Two tabs above the pane: **Diff** and **Files**.
- **Diff** tab (default): shows current uncommitted changes in the session's worktree, file by file, using Monaco's diff editor.
- **Files** tab: file tree of the worktree, click any file to view (or edit) it in Monaco.
- Updates whenever the agent's tools touch the filesystem.

### Status bar (bottom)
- Active model, current session status, rolling token counter.

### Design questions to resolve in Claude Design phase
1. Tab metaphor vs flat list for the sidebar — which feels right with 3-15 active sessions?
2. Should the diff pane auto-scroll to the most recently changed file, or stay where the user left it?
3. How are tool calls visually distinct from assistant text — collapsed cards, indented blocks, color-coded?
4. Permission prompts: inline cards in the chat, modal, or status bar action?
5. Status indicators: dots, small text, full pill labels? What colors map to running / waiting / idle / errored?
6. Dark mode first, light mode parity, or both from day one?
7. Empty state when there are no sessions yet — what's the most welcoming first interaction?
8. How does the user know which session caused a notification (e.g., agent waiting for input) when they're focused on a different session?

## 7. v1 scope (BYOH only)

What ships in the first usable version:

- Tauri shell, single window, Windows + WSL2 + Linux builds.
- Sidebar / agent pane / diff pane layout, resizable splits.
- Sessions persist across app restarts (SQLite).
- Per-session git worktree creation and cleanup.
- Anthropic API integration (direct, no Copilot routing yet).
- Six hardcoded tools: `read_file`, `write_file`, `str_replace`, `list_dir`, `grep`, `bash`.
- Streaming token rendering.
- One permission mode flag: `ask_before_bash`.
- Diff pane auto-refresh on filesystem changes.
- Monaco editor for file viewing.

Explicitly NOT in v1:
- BYOK (Claude Code / opencode / Copilot CLI embedding).
- MCP.
- Skills, slash commands, sub-agents.
- Multi-provider routing.
- Cost tracking.
- Settings UI beyond a single config file.

## 8. v2 direction (BYOK)

Once v1 feels right, layer in guest agents:

### opencode (easiest)
opencode runs as a client-server architecture with a Hono HTTP server. Run `opencode serve` as a child process, talk to it over HTTP/SSE, render the conversation in our own UI components. No PTY needed.

### GitHub Copilot CLI (INGKA work)
Use `copilot-sdk-rust` to authenticate via the user's existing GitHub Copilot entitlement. Models accessed are governed by the user's enterprise Copilot plan. Custom UI rendering, same as opencode.

### Claude Code (personal driver)
Two viable paths, decide once v1 layout proves itself:
- **Path A (unified)**: Use a community Rust Claude Agent SDK (e.g. `claude-agent-sdk` on crates.io). Subprocess-wraps the `claude` binary, streams JSON events, renders in our chat UI components. Looks identical to BYOH and opencode panes.
- **Path B (native TUI)**: Embed xterm.js + portable-pty, run `claude` natively in a terminal pane. Preserves slash commands, native TUI feel, but visually different from other panes. One pane in the IDE looks like a terminal; the others look like chat.

Recommendation: A first, add B as an option later if users (Stefan) miss the native feel.

## 9. Constraints and context

- **Primary dev environment**: Windows 11 + WSL2 + PowerShell on the Windows side, fish shell in WSL.
- **Solo developer**: scope must be achievable in evenings + weekends.
- **Prior art**: developer has shipped Tauri + Rust desktop apps before (custom internal tools at INGKA), comfortable with `octocrab`, `git2`, the Tauri command/event model.
- **Why not Electron**: bundle size and resource usage matter; Tauri's mature webview-based approach is sufficient for the UI complexity needed here.
- **Why not pure Rust (Slint, Iced, egui, gpui)**: would require building or vendoring a code editor, a diff renderer, and (eventually) a terminal — months of yak-shaving before any agent code. Tauri lets us reuse Monaco and xterm.js, both battle-tested.
- **Why not VS Code extension**: the whole point is to escape the cramped terminal-panel layout that VS Code imposes. An extension would inherit the constraint.

## 10. Open architectural decisions

These are decisions to make during design phase, not before:

1. **React vs Svelte for the frontend** — both work with Monaco and Tauri. React has a deeper component ecosystem for things like the diff viewer; Svelte produces smaller bundles and feels lighter to write. Pick based on which has cleaner Monaco bindings as of build time.
2. **Monaco's built-in diff vs a separate diff component** — Monaco's diff editor is excellent but heavyweight. For lots of small diffs, a lighter component (`react-diff-viewer-continued`) might feel snappier. Worth prototyping both.
3. **State management on the frontend** — Tauri events as a single source of truth (with Zustand or Svelte stores as a thin layer) vs heavier Redux/Pinia setups. Lean toward minimal.
4. **Worktree placement** — sibling directory (`../<repo>-<session>/`) vs subdirectory (`<repo>/.sessions/<session>/`)? Sibling is cleaner for IDEs but requires write access to the parent dir.
5. **How permissions are configured** — per-session (set on creation) vs global config vs runtime prompts only. v1 default: runtime prompts for `bash`, allow everything else.

## 11. Suggested handover sequence

For Claude Design:
- Read sections 1-6 carefully. Treat the layout in §6 as a starting sketch, not a spec.
- Iterate on the eight design questions in §6.
- Produce: a Figma-equivalent set of mockups (or HTML/CSS prototypes) for the main window, the new-session dialog, and the empty state.
- Validate the design against scenarios: 3 active sessions, 1 waiting on permission, 1 idle.

For Claude Code (after design is settled):
- Read sections 5, 7, 9, 10.
- Scaffold a Tauri 2.x project with the chosen frontend framework.
- Implement v1 scope from §7 in roughly the order: shell + sidebar → agent loop with one tool (`read_file`) → remaining tools → worktree management → diff pane → persistence → streaming → permissions.
- Defer everything in §8 and the deferred items in §3.

## 12. Definition of done for v1

The developer can:
1. Open the app, create a new session pointed at a local repo.
2. Have a multi-turn conversation with the agent, including at least one `bash` permission prompt that they approve.
3. Watch the diff pane update in real time as the agent edits files.
4. Close the app, reopen it, find the session and its history intact.
5. Run two sessions concurrently against the same repo without their changes colliding.

If all five work end-to-end on Windows + WSL2 without manual intervention, v1 is done.
