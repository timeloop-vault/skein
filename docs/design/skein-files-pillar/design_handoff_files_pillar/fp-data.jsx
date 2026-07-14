/* ══════════════════════════════════════════════════════════════════════
   Files Pillar prototype — sample data
   A small Skein-flavoured worktree, real-ish file bodies, and the tape for
   the hero scenario: the agent edits the file you have open. The edit it
   makes is the focus-guard fix (the exact bug the brief flags), so the demo
   tells one coherent story.
   ══════════════════════════════════════════════════════════════════════ */

// ── Worktree tree (nested; dirs first, most-recently-touched first) ──────────
const FP_TREE = {
  name: "skein", type: "dir", open: true, children: [
    { name: "src", type: "dir", open: true, children: [
      { name: "liveContext", type: "dir", open: false, children: [
        { name: "DiffCard.tsx", type: "file", path: "src/liveContext/DiffCard.tsx", mtime: "14:59" },
        { name: "Activity.tsx",  type: "file", path: "src/liveContext/Activity.tsx",  mtime: "14:58" },
      ]},
      { name: "LiveTerminal.tsx",  type: "file", path: "src/LiveTerminal.tsx",  mtime: "15:46", touched: true },
      { name: "harnessEvents.ts",  type: "file", path: "src/harnessEvents.ts",  mtime: "15:45", touched: true },
      { name: "types.ts",          type: "file", path: "src/types.ts",          mtime: "15:44" },
      { name: "App.tsx",           type: "file", path: "src/App.tsx",           mtime: "12:02" },
      { name: "styles.css",        type: "file", path: "src/styles.css",        mtime: "11:20" },
    ]},
    { name: "src-tauri", type: "dir", open: false, children: [
      { name: "tauri.conf.json", type: "file", path: "src-tauri/tauri.conf.json", mtime: "09:14" },
      { name: "main.rs",         type: "file", path: "src-tauri/main.rs",         mtime: "09:02" },
    ]},
    { name: "package.json", type: "file", path: "package.json", mtime: "08:00" },
    { name: "README.md",    type: "file", path: "README.md",    mtime: "Jul 2" },
  ],
};

// ── File bodies (arrays of lines) ────────────────────────────────────────────
const L = (s) => s.replace(/\n$/, "").split("\n");

const FP_FILES = {
  "src/LiveTerminal.tsx": {
    lang: "tsx",
    lines: L(`import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { PtyStream } from "../pty";
import type { HarnessId } from "./types";

interface Props {
  harnessId: HarnessId;
  focused: boolean;
}

// One live PTY, mounted for the whole room lifetime. Switching harnesses
// hides with display:none so the conversation survives tab switches.
export function LiveTerminal({ harnessId, focused }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal>();

  useEffect(() => {
    const t = new Terminal({ fontFamily: "JetBrains Mono", fontSize: 13 });
    t.open(host.current!);
    term.current = t;

    const stream = PtyStream.attach(harnessId);
    stream.onData((d) => t.write(d));
    t.onData((d) => stream.send(d));

    return () => {
      stream.dispose();
      t.dispose();
    };
  }, [harnessId]);

  return <div className="live-terminal" ref={host} />;
}`),
  },

  "src/harnessEvents.ts": {
    lang: "ts",
    lines: L(`import type { HarnessAction, HarnessId } from "./types";

// The per-room event tape. Every tool call, prompt and status flip the
// backend writes to harness_actions lands here and fans out to the cards.
const listeners = new Map<HarnessId, Set<(a: HarnessAction) => void>>();

export function emit(action: HarnessAction) {
  const set = listeners.get(action.harness);
  if (!set) return;
  for (const fn of set) fn(action);
}

export function subscribe(id: HarnessId, fn: (a: HarnessAction) => void) {
  let set = listeners.get(id);
  if (!set) listeners.set(id, (set = new Set()));
  set.add(fn);
  return () => set!.delete(fn);
}`),
  },

  "src/types.ts": {
    lang: "ts",
    lines: L(`export type HarnessKind = "claude" | "opencode" | "copilot" | "byoh";
export type HarnessId = string & { readonly brand: unique symbol };

export type ActionKind =
  | "tool_call"
  | "pr_link"
  | "turn_cost"
  | "api_error"
  | "ai_title";

export interface HarnessAction {
  id: string;
  room: string;
  harness: HarnessKind;
  kind: ActionKind;
  ts: number;
  payload: unknown;
}`),
  },

  "src/App.tsx": {
    lang: "tsx",
    lines: L(`import { RoomTabs } from "./RoomTabs";
import { HarnessColumn } from "./HarnessColumn";
import { LiveContext } from "./liveContext/LiveContext";

export function App() {
  return (
    <div className="sk-app sk-dark">
      <RoomTabs />
      <div className="sk-workspace">
        <HarnessColumn />
        <LiveContext />
      </div>
    </div>
  );
}`),
  },

  "src/styles.css": {
    lang: "css",
    lines: L(`.sk-app {
  width: 100vw;
  height: 100vh;
  background: var(--bg-0);
  color: var(--fg-0);
  font-family: var(--sk-sans);
  display: flex;
  flex-direction: column;
}`),
  },

  "src/liveContext/DiffCard.tsx": { lang: "tsx", lines: L(`// Live worktree diff, per-file tabs. Auto-follows the focused harness.\nexport function DiffCard() {\n  return <div className="lc-card lc-diff" />;\n}`) },
  "src/liveContext/Activity.tsx": { lang: "tsx", lines: L(`// Per-tool-call feed. Tails forward as harness_actions rows land.\nexport function Activity() {\n  return <div className="lc-card lc-activity" />;\n}`) },
  "src-tauri/main.rs": { lang: "rs", lines: L(`fn main() {\n    tauri::Builder::default()\n        .plugin(pty::init())\n        .run(tauri::generate_context!())\n        .expect("error while running skein");\n}`) },
  "src-tauri/tauri.conf.json": { lang: "json", lines: L(`{\n  "productName": "Skein",\n  "version": "0.2.5",\n  "app": {\n    "windows": [{ "titleBarStyle": "Overlay", "width": 1280, "height": 800 }]\n  }\n}`) },
  "package.json": { lang: "json", lines: L(`{\n  "name": "skein",\n  "version": "0.2.5",\n  "private": true,\n  "scripts": {\n    "dev": "tauri dev",\n    "build": "tauri build"\n  },\n  "dependencies": {\n    "@xterm/xterm": "^5.5.0",\n    "react": "^18.3.1"\n  }\n}`) },
  "README.md": { lang: "md", lines: L(`# Skein\n\nAn agentic, harness-first IDE. A room is a task + a git worktree + a cwd.\nEach harness (Claude Code, opencode, shell) is a real PTY.\n\n## Running\n\n    npm run dev\n\nAll harnesses of all active rooms stay mounted; switching hides with\ndisplay:none so conversations survive tab switches.`) },
};

// ── The hero scenario: agent edits src/LiveTerminal.tsx ──────────────────────
// The agent adds a focus guard so editor keystrokes can't leak into a live
// PTY (the brief's focus-leak bug). "after" is the incoming version, tagged
// per line: 0 = context, 1 = added, 2 = changed.
const FP_AGENT_EDIT = {
  path: "src/LiveTerminal.tsx",
  harness: "claude",
  summary: "guard PTY writes behind pane focus",
  addPlus: 6,
  addMinus: 1,
  after: [
    [`import { useEffect, useRef } from "react";`, 0],
    [`import { Terminal } from "@xterm/xterm";`, 0],
    [`import { PtyStream } from "../pty";`, 0],
    [`import type { HarnessId } from "./types";`, 0],
    [``, 0],
    [`interface Props {`, 0],
    [`  harnessId: HarnessId;`, 0],
    [`  focused: boolean;`, 0],
    [`}`, 0],
    [``, 0],
    [`// One live PTY, mounted for the whole room lifetime. Switching harnesses`, 0],
    [`// hides with display:none so the conversation survives tab switches.`, 0],
    [`export function LiveTerminal({ harnessId, focused }: Props) {`, 0],
    [`  const host = useRef<HTMLDivElement>(null);`, 0],
    [`  const term = useRef<Terminal>();`, 0],
    [``, 0],
    [`  useEffect(() => {`, 0],
    [`    const t = new Terminal({ fontFamily: "JetBrains Mono", fontSize: 13 });`, 0],
    [`    t.open(host.current!);`, 0],
    [`    term.current = t;`, 0],
    [``, 0],
    [`    const stream = PtyStream.attach(harnessId);`, 0],
    [`    stream.onData((d) => t.write(d));`, 0],
    [`    // Only forward keystrokes when this pane actually owns focus, so`, 1],
    [`    // typing in the editor never leaks into a live agent's PTY (#…).`, 1],
    [`    t.onData((d) => {`, 2],
    [`      if (focused) stream.send(d);`, 1],
    [`    });`, 1],
    [`    t.textarea?.addEventListener("focus", () => stream.setActive(true));`, 1],
    [`    t.textarea?.addEventListener("blur", () => stream.setActive(false));`, 1],
    [``, 0],
    [`    return () => {`, 0],
    [`      stream.dispose();`, 0],
    [`      t.dispose();`, 0],
    [`    };`, 0],
    [`  }, [harnessId, focused]);`, 2],
    [``, 0],
    [`  return <div className="live-terminal" ref={host} />;`, 0],
    [`}`, 0],
  ],
};

// ── Left harness: Claude Code TUI, mid-task on the focus fix ──────────────────
const FP_TERM = [
  { c: "bold",  t: "## Plan" },
  { c: "",      t: "The editor-to-PTY focus leak is a real hazard — a stray keystroke in the" },
  { c: "",      t: "file view can land in a live agent. I'll guard PtyStream writes behind" },
  { c: "",      t: "pane focus in " , mono: "src/LiveTerminal.tsx", tail: "." },
  { c: "sp" },
  { c: "dim",   t: "  └ Read(src/LiveTerminal.tsx)  40 lines" },
  { c: "dim",   t: "  └ Read(src/types.ts)  17 lines" },
  { c: "sp" },
  { c: "bullet", t: "Editing " , mono: "src/LiveTerminal.tsx", tail: " — adding the focus guard now." },
  { c: "dim",   t: "⚡ Edit(src/LiveTerminal.tsx)" },
  { c: "sp" },
  { c: "cogit", t: "✻ Applying edit…" , thinking: true },
];

// ── Right pane: Live Context samples (compact) ───────────────────────────────
const FP_DIFF_TABS = [
  { file: "LiveTerminal.tsx", add: 6, del: 1, active: true, live: true },
  { file: "harnessEvents.ts", add: 21, del: 3 },
  { file: "types.ts", add: 6, del: 0 },
];

const FP_PLAN = [
  { s: "done", t: "read LiveTerminal + PtyStream wiring" },
  { s: "now",  t: "guard PTY writes behind pane focus" },
  { s: "todo", t: "add focus/blur active-stream toggle" },
  { s: "todo", t: "manual: type in editor, confirm no leak" },
];

const FP_ACTIVITY = [
  { time: "15:45:02", g: "◌", gc: "fg3", tool: "read",  tgt: "types.ts", right: "17 ln" },
  { time: "15:45:14", g: "◌", gc: "fg3", tool: "read",  tgt: "LiveTerminal.tsx", right: "40 ln" },
  { time: "15:45:41", g: "☰", gc: "accent", tool: "todowrite", tgt: "4 todos", right: "replaced plan" },
  { time: "15:46:03", g: "›", gc: "accent", tool: "user", tgt: "guard the focus leak in LiveTerminal", kuser: true },
  { time: "15:46:20", g: "✎", gc: "ok", tool: "edit", tgt: "LiveTerminal.tsx", right: "+6 −1", live: true },
];

Object.assign(window, {
  FP_TREE, FP_FILES, FP_AGENT_EDIT, FP_TERM, FP_DIFF_TABS, FP_PLAN, FP_ACTIVITY,
});
