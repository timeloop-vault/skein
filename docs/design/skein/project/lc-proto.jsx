// Live Context — interactive prototype.
// Plays a scripted event tape into the 3-card stack: rows slide in, the diff
// card auto-focuses, the plan card grows, burst rows expand on click, an
// agent row opens the inspector. Drag dividers persist to localStorage.

const { useState, useEffect, useRef, useCallback } = React;

const TWEAK_DEFAULTS = /*EDITMODE-BEGIN*/{
  "theme": "dark",
  "density": "regular",
  "speed": 1,
  "showCost": true,
  "showActivity": true
}/*EDITMODE-END*/;

// ─── Diff content blocks (small bits of syntax-coloured code) ──────────
function Line({ ctx, add, del, n1, n2, src }) {
  const kind = add ? 'add' : del ? 'del' : 'ctx';
  return (
    <div className={`lc-line ${kind}`}>
      <div className="gutter">
        <span className="ln">{n1 ?? ''}</span>
        <span className="ln">{n2 ?? ''}</span>
      </div>
      <span className="marker">{add ? '+' : del ? '−' : ''}</span>
      <span className="src">{src}</span>
    </div>
  );
}

const DIFF_CARGO = (
  <>
    <Line ctx n1={55} n2={55} src={<span>[<span className="tk-fn">target</span>.<span className="tk-fn">'cfg(target_os="macos")'</span>.<span className="tk-fn">dependencies</span>]</span>} />
    <Line ctx n1={56} n2={56} src={<span>core-foundation = <span className="tk-str">"0.10"</span></span>} />
    <Line ctx n1={57} n2={57} src={<span></span>} />
    <Line add n2={58} src={<span>[<span className="tk-fn">dev-dependencies</span>]</span>} />
    <Line add n2={59} src={<span>tempfile = <span className="tk-str">"3"</span></span>} />
    <Line add n2={60} src={<span>insta = {'{'} version = <span className="tk-str">"1.41"</span>, features = [<span className="tk-str">"json"</span>] {'}'}</span>} />
    <Line ctx n1={58} n2={61} src={<span></span>} />
    <Line ctx n1={59} n2={62} src={<span>[<span className="tk-fn">workspace</span>]</span>} />
  </>
);

const DIFF_ADAPTER = (
  <>
    <Line ctx n1={1} n2={1} src={<span><span className="tk-com">// Reads Claude JSONL session files and replays them into harness_actions.</span></span>} />
    <Line ctx n1={2} n2={2} src={<span><span className="tk-key">use</span> serde_json::Value;</span>} />
    <Line ctx n1={3} n2={3} src={<span></span>} />
    <Line add n2={4} src={<span><span className="tk-key">pub struct</span> <span className="tk-fn">ClaudeAdapter</span> {'{'}</span>} />
    <Line add n2={5} src={<span>    pool: <span className="tk-key">SqlitePool</span>,</span>} />
    <Line add n2={6} src={<span>    room_id: <span className="tk-key">RoomId</span>,</span>} />
    <Line add n2={7} src={<span>{'}'}</span>} />
    <Line ctx n1={4} n2={8} src={<span></span>} />
    <Line add n2={9} src={<span><span className="tk-key">impl</span> <span className="tk-fn">ClaudeAdapter</span> {'{'}</span>} />
    <Line add n2={10} src={<span>    <span className="tk-key">pub async fn</span> <span className="tk-fn">backfill</span>(&<span className="tk-key">self</span>, jsonl: &<span className="tk-key">Path</span>) {'->'} <span className="tk-key">Result</span>{'<()>'} {'{'}</span>} />
    <Line add n2={11} src={<span>        <span className="tk-com">// stream-parse the JSONL, classify each row by kind, persist.</span></span>} />
    <Line add n2={12} src={<span>    {'}'}</span>} />
    <Line add n2={13} src={<span>{'}'}</span>} />
  </>
);

// ─── Plan groups used during the demo ──────────────────────────────────
const PLAN_CLAUDE = [
  { status: 'done', text: 'Migration 0014 — harness_actions table' },
  { status: 'done', text: 'Build harness_events_claude.rs adapter + tests' },
  { status: 'now',  text: 'Wire backfill on attach' },
  { status: 'next', text: 'Tail-forward subscription channel' },
  { status: 'next', text: 'Drop deprecated activity_feed code' },
];
const PLAN_OPENCODE = [
  { status: 'done', text: 'Read existing schema migrations',  priority: 'high' },
  { status: 'now',  text: 'Spec the opencode adapter shape',  priority: 'high', inferred: true },
  { status: 'next', text: 'Add per-step snapshot hash plumbing', priority: 'med' },
];

// ─── The Tape ──────────────────────────────────────────────────────────
// Each event has a time-offset (ms from start). The reducer applies it.
// We intentionally keep this as data, not pre-built JSX — renderRow turns
// it into the right component at draw time.

const TAPE = [
  // arrival ── subtitle, then Claude's done items
  { t: 0,    type: 'subtitle', text: "Implementing #80 Live Context — backend persists harness_actions; this PR adds the SQLite migration + Claude adapter.", age: 'just now' },
  { t: 100,  type: 'row', row: { kind: 'bridge', harness: 'claude',   time: '14:01:50', status: 'connected', detail: 'ws/online' } },
  { t: 100,  type: 'row', row: { kind: 'bridge', harness: 'opencode', time: '14:01:51', status: 'connected', detail: 'http/8086' } },
  { t: 300,  type: 'plan_set', groups: [{ harness: 'claude', items: PLAN_CLAUDE.slice(0, 2).concat([{ ...PLAN_CLAUDE[2], status: 'next' }]) }] },

  // turn 1 ── Claude takes the room
  { t: 700,  type: 'turn_sep', label: 'turn · 14:01:54' },
  { t: 800,  type: 'row', row: { kind: 'user_prompt', time: '14:01:54', harness: 'claude',
                                  text: "Add the harness_actions migration and the Claude adapter. Cover backfill with tests." } },
  { t: 950,  type: 'row', row: { kind: 'ai_title', harness: 'claude', time: '14:01:56', title: 'wiring the actions table + Claude backfill' } },
  { t: 1100, type: 'row', row: { kind: 'edit', harness: 'claude', time: '14:02:18', file: 'Cargo.toml', adds: 3 } },
  // turn 1 line indexes shifted by ~100ms; fix tab_set + diff_body to match new row time
  { t: 1100, type: 'tab_set', file: 'Cargo.toml', harness: 'claude', adds: 3, active: true, refocus: true },
  { t: 1100, type: 'diff_body', body: 'cargo' },

  { t: 1900, type: 'row', row: { kind: 'read', harness: 'claude', time: '14:02:22', file: 'docs/live-context-recon.md', lines: 612 } },

  { t: 2600, type: 'row', row: { kind: 'edit', harness: 'claude', time: '14:02:24', file: 'crates/skein-core/src/harness_events_claude.rs', adds: 142, dels: 4 } },
  { t: 2600, type: 'tab_set', file: 'harness_events_claude.rs', harness: 'claude', adds: 142, dels: 4, active: true, refocus: true },
  { t: 2600, type: 'diff_body', body: 'adapter' },

  { t: 3500, type: 'row', row: { kind: 'task', harness: 'claude', time: '14:02:30', op: 'update', text: 'Wire backfill on attach', from: 'pending', to: 'in_progress' } },
  { t: 3500, type: 'plan_promote', harness: 'claude', text: 'Wire backfill on attach', to: 'now' },

  { t: 4200, type: 'row', row: { kind: 'bash', harness: 'claude', time: '14:02:31', title: 'View issue #50 epic details', command: 'gh issue view 50', ms: 812, expanded: true,
    output: "title: Live Context — top-of-mind 'what's the agent doing' on the right pane\nstate: OPEN\nlabels: ui, epic\nbody: …" } },

  { t: 5000, type: 'turn_cost', tokens: '4,218', usd: '0.18', ms: '37.4s' },
  { t: 5000, type: 'cost_inc',  tokensInc: 4218,  usdInc: 0.18 },

  // turn 2 ── opencode comes in
  { t: 5500, type: 'turn_sep', label: 'turn · 14:03:04' },
  { t: 5700, type: 'row', row: { kind: 'user_prompt', time: '14:03:04', harness: 'opencode',
                                  text: "Same room — pair on the opencode adapter. Read what Claude just wrote." } },
  { t: 5900, type: 'row', row: { kind: 'read', harness: 'opencode', time: '14:03:04', file: 'crates/skein-core/src/harness_events_claude.rs', lines: 146 } },

  { t: 6700, type: 'row', row: { kind: 'todowrite', harness: 'opencode', time: '14:03:11', count: 8 } },
  { t: 6700, type: 'plan_add_group', group: { harness: 'opencode', items: PLAN_OPENCODE } },

  { t: 7500, type: 'row', row: { kind: 'grep', harness: 'opencode', time: '14:03:18', pattern: 'harness_action', matches: 23 } },

  { t: 8400, type: 'row', row: { kind: 'edit', harness: 'opencode', time: '14:03:29', file: 'crates/skein-core/src/lib.rs', adds: 1 } },
  { t: 8400, type: 'tab_set', file: 'lib.rs', harness: 'opencode', adds: 1, active: false, flicker: true },

  { t: 9200, type: 'row', row: { kind: 'bash', harness: 'opencode', time: '14:03:42', title: 'cargo check -p skein-core', ms: 3920, expanded: true,
    output: "    Checking skein-core v0.0.1 (/Users/.../skein-core)\n    Finished `dev` profile in 3.91s" } },

  { t: 10000, type: 'turn_cost', tokens: '2,910', usd: '0.11', ms: '54.0s' },
  { t: 10000, type: 'cost_inc',  tokensInc: 2910,  usdInc: 0.11 },

  // turn 3 ── question, then permission as a passive row
  { t: 10500, type: 'turn_sep', label: 'turn · 14:04:12' },
  { t: 10700, type: 'row', row: { kind: 'perm_mode', harness: 'claude', time: '14:04:10', from: 'ask', to: 'always_for_session' } },
  { t: 11000, type: 'row', row: { kind: 'ask', harness: 'claude', time: '14:04:18', question: 'Release version?', chosen: 'v0.1.7' } },

  { t: 11700, type: 'row', row: { kind: 'permission', harness: 'claude', time: '14:04:34', command: 'cargo test -p skein-core harness_events_claude' } },

  // the user "approved in the harness terminal" — the row stays as a log entry,
  // but the test command now runs and reports back
  { t: 13500, type: 'row', row: { kind: 'bash', harness: 'claude', time: '14:04:42', title: 'cargo test -p skein-core harness_events_claude', ms: 4100, expanded: true,
    output: "running 12 tests\ntest harness_events_claude::backfill_roundtrip ... ok\n... (10 more)\ntest harness_events_claude::tail_emits_actions ... ok\n\ntest result: ok. 12 passed; 0 failed" } },

  // sub-agent
  { t: 14800, type: 'row', row: { kind: 'agent', harness: 'claude', time: '14:04:50', title: 'general-purpose: Research xterm.js bug', ms: '49.7s', tokens: '1,234 tok', toolCount: 8, inspectorId: 'agent-xterm' } },

  // api error
  { t: 16500, type: 'row', row: { kind: 'bridge', harness: 'claude', time: '14:05:16', status: 'reconnecting', detail: 'anthropic capacity blip' } },
  { t: 16700, type: 'row', row: { kind: 'api_error', harness: 'claude', time: '14:05:18', status: 'HTTP 529', attempt: 1, retryIn: 8, message: 'overloaded — anthropic capacity' } },

  { t: 18000, type: 'row', row: { kind: 'api_error', harness: 'claude', time: '14:05:26', status: 'HTTP 529', attempt: 2, retryIn: 16, message: 'overloaded — anthropic capacity' } },
  { t: 18200, type: 'row', row: { kind: 'bridge', harness: 'claude', time: '14:05:30', status: 'connected', detail: 'back online' } },

  // burst storm in turn 4
  { t: 19500, type: 'turn_sep', label: 'turn · 14:21:02' },
  { t: 19800, type: 'row', row: { kind: 'read', harness: 'claude', time: '14:21:04', file: 'rg.json', lines: 612 } },

  { t: 20500, type: 'row', row: { kind: 'burst', burstId: 'b1', harness: 'claude', time: '14:21:08', count: 12, tool: 'edit', scope: 'crates/skein-core/src/**', adds: 142, dels: 37, live: true,
    children: [
      { kind: 'edit', harness: 'claude', time: '14:21:08', file: 'crates/skein-core/src/lib.rs',                     adds: 14, dels: 14 },
      { kind: 'edit', harness: 'claude', time: '14:21:09', file: 'crates/skein-core/src/adapter.rs',                 adds: 8,  dels: 8  },
      { kind: 'edit', harness: 'claude', time: '14:21:10', file: 'crates/skein-core/src/migrations.rs',              adds: 12, dels: 12 },
      { kind: 'edit', harness: 'claude', time: '14:21:11', file: 'crates/skein-core/src/harness_events_claude.rs',   adds: 28, dels: 0  },
      { kind: 'edit', harness: 'claude', time: '14:21:12', file: 'crates/skein-core/src/harness_events_opencode.rs', adds: 24, dels: 0  },
      { kind: 'edit', harness: 'claude', time: '14:21:13', file: 'crates/skein-core/src/db.rs',                      adds: 6,  dels: 1  },
      { kind: 'edit', harness: 'claude', time: '14:21:14', file: 'crates/skein-core/src/lib.rs',                     adds: 18, dels: 0  },
      { kind: 'edit', harness: 'claude', time: '14:21:15', file: 'crates/skein-core/src/room.rs',                    adds: 9,  dels: 1  },
      { kind: 'edit', harness: 'claude', time: '14:21:16', file: 'crates/skein-core/src/session.rs',                 adds: 7,  dels: 0  },
      { kind: 'edit', harness: 'claude', time: '14:21:17', file: 'crates/skein-core/src/tests/mod.rs',               adds: 8,  dels: 1  },
      { kind: 'edit', harness: 'claude', time: '14:21:18', file: 'crates/skein-core/src/util.rs',                    adds: 4,  dels: 0  },
      { kind: 'edit', harness: 'claude', time: '14:21:19', file: 'crates/skein-core/src/error.rs',                   adds: 4,  dels: 0  },
    ],
  } },

  { t: 22500, type: 'row', row: { kind: 'burst', burstId: 'b2', harness: 'claude', time: '14:21:36', count: 8, tool: 'edit', scope: 'crates/skein-app/src/**', adds: 68, dels: 22, live: true,
    children: [
      { kind: 'edit', harness: 'claude', time: '14:21:36', file: 'crates/skein-app/src/main.rs',     adds: 12, dels: 4 },
      { kind: 'edit', harness: 'claude', time: '14:21:37', file: 'crates/skein-app/src/ui/room.rs',  adds: 10, dels: 4 },
      { kind: 'edit', harness: 'claude', time: '14:21:38', file: 'crates/skein-app/src/ui/right_pane.rs', adds: 14, dels: 6 },
      { kind: 'edit', harness: 'claude', time: '14:21:39', file: 'crates/skein-app/src/state.rs',   adds: 8,  dels: 2 },
      { kind: 'edit', harness: 'claude', time: '14:21:40', file: 'crates/skein-app/src/sub.rs',     adds: 8,  dels: 2 },
      { kind: 'edit', harness: 'claude', time: '14:21:41', file: 'crates/skein-app/src/store.rs',   adds: 6,  dels: 2 },
      { kind: 'edit', harness: 'claude', time: '14:21:42', file: 'crates/skein-app/src/util.rs',    adds: 6,  dels: 2 },
      { kind: 'edit', harness: 'claude', time: '14:21:43', file: 'crates/skein-app/src/lib.rs',     adds: 4,  dels: 0 },
    ],
  } },

  { t: 24500, type: 'row', row: { kind: 'grep', harness: 'claude', time: '14:21:54', pattern: 'HarnessEvent', matches: 0 } },
  { t: 25800, type: 'row', row: { kind: 'bash', harness: 'claude', time: '14:21:58', title: 'cargo build', ms: 11410, expanded: true,
    output: "    Compiling skein-core v0.0.1\n    Compiling skein-app v0.0.1\n    Finished `dev` profile in 11.41s" } },

  { t: 27000, type: 'turn_cost', tokens: '6,140', usd: '0.32', ms: '7.5s' },
  { t: 27000, type: 'cost_inc',  tokensInc: 6140,  usdInc: 0.32 },
];

// Backfill — what gets snapped in instantly when a room is re-attached.
// In real Skein this is "we re-scan the local Claude JSONL and opencode
// parts and replay everything we'd missed since you last had this room
// open." For the prototype we just inject 12 plausible rows with a banner
// so the user sees "this is history, not live."
const BACKFILL = [
  { kind: 'bridge', harness: 'claude',   time: '09:14:02', status: 'connected', detail: 'session resumed from disk' },
  { kind: 'bridge', harness: 'opencode', time: '09:14:03', status: 'connected', detail: 'session resumed from disk' },
  { kind: 'user_prompt', harness: 'claude', time: '09:14:08', text: "Pick up where we left off — finish the L2c-1 dogfooding pass." },
  { kind: 'ai_title', harness: 'claude', time: '09:14:09', title: 'L2c-1 dogfooding & polish' },
  { kind: 'read', harness: 'claude',   time: '09:14:11', file: 'docs/live-context-recon.md', lines: 612 },
  { kind: 'edit', harness: 'claude',   time: '09:14:23', file: 'crates/skein-core/src/lib.rs', adds: 18 },
  { kind: 'edit', harness: 'claude',   time: '09:14:31', file: 'crates/skein-core/src/migrations.rs', adds: 64 },
  { kind: 'bash', harness: 'claude',   time: '09:14:48', title: 'cargo check -p skein-core', ms: 4204 },
  { kind: 'task', harness: 'claude',   time: '09:14:52', op: 'update', text: 'Migration 0014 — harness_actions table', from: 'in_progress', to: 'completed' },
  { kind: 'pr',   harness: 'claude',   time: '09:15:08', number: 80, repo: 'skein', title: 'Live Context — backend + cards' },
  { kind: 'compact', harness: 'opencode', time: '11:32:14', before: '40k', after: '12k' },
  { kind: 'userfile', harness: 'claude', time: '13:51:22', file: 'README.md' },
];
const INSPECTORS = {
  'agent-xterm': {
    title: 'general-purpose: Research xterm.js bug',
    harness: 'claude',
    duration: '49.7s',
    tokens: '1,234 tok',
    toolCount: 8,
    status: 'completed',
    prompt: "Research the xterm.js issue where output is dropped when the buffer fills mid-write. Find authoritative source code references, summarise the proposed fix, and propose two patch strategies.",
    calls: [
      { kind: 'read',   harness: 'claude', time: '0.2s',  file: 'node_modules/xterm/src/InputHandler.ts', lines: 1842 },
      { kind: 'grep',   harness: 'claude', time: '2.4s',  pattern: 'enqueueData|writeBuffer', matches: 6 },
      { kind: 'read',   harness: 'claude', time: '3.1s',  file: 'node_modules/xterm/src/parser/EscapeSequenceParser.ts', lines: 612 },
      { kind: 'bash',   harness: 'claude', time: '6.8s',  title: 'check xterm changelog for buffer-overflow fixes', command: 'gh issue list', ms: 912 },
      { kind: 'read',   harness: 'claude', time: '14.2s', file: 'https://github.com/xtermjs/xterm.js/issues/4892', lines: 31 },
      { kind: 'read',   harness: 'claude', time: '18.0s', file: 'https://github.com/xtermjs/xterm.js/pull/4901',   lines: 84 },
      { kind: 'read',   harness: 'claude', time: '24.4s', file: 'docs/research-notes.md', lines: 0 },
      { kind: 'edit',   harness: 'claude', time: '28.1s', file: 'docs/research-notes.md', adds: 84 },
    ],
    report: (
      <>
        The drop happens in <code style={{ fontFamily: 'var(--sk-mono)' }}>InputHandler.parse()</code> when a partial escape sequence
        spans a buffer boundary. PR #4901 fixes this by buffering parser state across calls.
        <br /><br />
        <strong style={{ color: 'var(--fg-0)' }}>A · Vendor PR #4901's commit.</strong> Minimal local diff; needs a rebase if upstream changes.
        <br />
        <strong style={{ color: 'var(--fg-0)' }}>B · Wrap our adapter.</strong> Coalesce writes &lt; 4KB at our boundary so xterm never sees a partial. More work but no upstream dependency.
        <br /><br />
        Recommend B for the v1 release, vendor A in a follow-up if upstream drags.
      </>
    ),
  },
};

// ─── Row renderer ──────────────────────────────────────────────────────
function renderRow(r, handlers) {
  switch (r.kind) {
    case 'edit':       return <EditRow {...r} />;
    case 'write':      return <EditRow {...r} kind="write" />;
    case 'read':       return <ReadRow {...r} />;
    case 'grep':       return <SearchRow {...r} />;
    case 'glob':       return <SearchRow {...r} kind="glob" />;
    case 'bash':       return <BashRow {...r} />;
    case 'task':       return <TaskRow {...r} />;
    case 'todowrite':  return <TodoWriteRow {...r} />;
    case 'ask':        return <AskRow {...r} />;
    case 'agent':      return <AgentRow {...r} onOpen={() => handlers.openInspector(r.inspectorId)} />;
    case 'pr':         return <PrRow {...r} />;
    case 'queue':      return <QueueRow {...r} />;
    case 'userfile':   return <UserFileRow {...r} />;
    case 'slash':      return <SlashRow {...r} />;
    case 'compact':    return <CompactRow {...r} />;
    case 'api_error':  return <ApiErrorRow {...r} />;
    case 'tool_error': return <ToolErrorRow {...r} />;
    case 'permission': return <PermissionRow {...r} onJump={() => handlers.jumpToHarness(r.harness)} />;
    case 'user_prompt':  return <UserPromptRow {...r} />;
    case 'perm_mode':    return <PermissionModeRow {...r} />;
    case 'ai_title':     return <AiTitleRow {...r} />;
    case 'bridge':       return <BridgeStatusRow {...r} />;
    case 'burst':      return <BurstRow {...r} onExpand={() => handlers.toggleBurst(r.burstId)} />;
    default:           return null;
  }
}

// ─── Drag-resize hook ──────────────────────────────────────────────────
function useDragSplit(initial, key) {
  const STORAGE_KEY = `lc.split.${key}`;
  const [sizes, setSizes] = useState(() => {
    try {
      const saved = JSON.parse(localStorage.getItem(STORAGE_KEY));
      if (Array.isArray(saved) && saved.length === initial.length) return saved;
    } catch {}
    return initial;
  });
  useEffect(() => {
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(sizes)); } catch {}
  }, [sizes]);

  const stackRef = useRef(null);
  const dragRef = useRef(null);

  const onMouseDown = (i) => (e) => {
    e.preventDefault();
    const rect = stackRef.current?.getBoundingClientRect();
    if (!rect) return;
    dragRef.current = { i, startY: e.clientY, startSizes: sizes.slice(), rectHeight: rect.height };
    document.body.style.cursor = 'row-resize';
    document.body.style.userSelect = 'none';
  };

  useEffect(() => {
    const onMove = (e) => {
      const d = dragRef.current;
      if (!d) return;
      const dyPx = e.clientY - d.startY;
      const total = d.startSizes.reduce((a, b) => a + b, 0);
      // Convert px delta to flex delta proportional to total flex.
      const dyFlex = (dyPx / d.rectHeight) * total;
      const next = d.startSizes.slice();
      const min = total * 0.08;
      next[d.i]     = Math.max(min, d.startSizes[d.i]     + dyFlex);
      next[d.i + 1] = Math.max(min, d.startSizes[d.i + 1] - dyFlex);
      setSizes(next);
    };
    const onUp = () => {
      dragRef.current = null;
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
    return () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
  }, []);

  return { sizes, onMouseDown, stackRef };
}

// ─── App ───────────────────────────────────────────────────────────────
function App() {
  const [tw, setTweak] = useTweaks(TWEAK_DEFAULTS);
  const [playing, setPlaying] = useState(true);
  const [virtTime, setVirtTime] = useState(0);

  // World state mutated by the tape
  const [subtitle, setSubtitle] = useState({ text: 'Starting…', age: '', empty: true });
  const [tabs, setTabs] = useState([]);          // [{ file, adds, dels, harness, active, flicker }]
  const [diffBody, setDiffBody] = useState(null);
  const [diffRefocus, setDiffRefocus] = useState(0);
  const [planGroups, setPlanGroups] = useState([]);
  const [items, setItems] = useState([]);        // ordered list of activity items
  const [totals, setTotals] = useState({ events: 0, cost: 0, tokens: 0 });
  const [inspector, setInspector] = useState(null);
  const [expandedBursts, setExpandedBursts] = useState(new Set());
  const [flashToast, setFlashToast] = useState(null);
  const [diffCollapsed, setDiffCollapsed] = useState(false);
  const [planCollapsed, setPlanCollapsed] = useState(false);
  const [activityCollapsed, setActivityCollapsed] = useState(false);

  const { sizes, onMouseDown, stackRef } = useDragSplit([1, 1, 1.2], 'main');
  const activityRef = useRef(null);
  const [autoTail, setAutoTail] = useState(true);
  const [newSince, setNewSince] = useState(0);

  // Apply an event to the world
  const apply = useCallback((ev) => {
    switch (ev.type) {
      case 'subtitle':
        setSubtitle({ text: ev.text, age: ev.age, empty: false });
        break;
      case 'turn_sep':
        setItems(prev => [...prev, { id: 'sep-' + prev.length, type: 'sep', label: ev.label }]);
        break;
      case 'turn_cost':
        setItems(prev => [...prev, { id: 'cost-' + prev.length, type: 'cost', tokens: ev.tokens, usd: ev.usd, ms: ev.ms }]);
        break;
      case 'cost_inc':
        setTotals(t => ({ events: t.events, tokens: t.tokens + ev.tokensInc, cost: t.cost + ev.usdInc }));
        break;
      case 'row':
        setItems(prev => [...prev, { id: 'row-' + prev.length, type: 'row', row: ev.row }]);
        setTotals(t => ({ ...t, events: t.events + 1 }));
        break;
      case 'tab_set': {
        setTabs(prev => {
          const existing = prev.findIndex(x => x.file.endsWith(ev.file) || x.file === ev.file);
          const fresh = { file: ev.file.split('/').pop(), fullPath: ev.file, adds: ev.adds, dels: ev.dels, harness: ev.harness, active: ev.active, flicker: ev.flicker };
          let next;
          if (existing >= 0) {
            next = prev.map((x, i) => i === existing ? { ...x, ...fresh, active: ev.active ?? x.active } : (ev.active ? { ...x, active: false } : x));
          } else {
            next = ev.active
              ? [...prev.map(x => ({ ...x, active: false })), fresh]
              : [...prev, fresh];
          }
          return next;
        });
        if (ev.refocus) setDiffRefocus(c => c + 1);
        break;
      }
      case 'diff_body':
        setDiffBody(ev.body === 'cargo' ? DIFF_CARGO : ev.body === 'adapter' ? DIFF_ADAPTER : null);
        break;
      case 'plan_set':
        setPlanGroups(ev.groups);
        break;
      case 'plan_add_group':
        setPlanGroups(prev => prev.some(g => g.harness === ev.group.harness)
          ? prev.map(g => g.harness === ev.group.harness ? ev.group : g)
          : [...prev, ev.group]);
        break;
      case 'plan_promote':
        setPlanGroups(prev => prev.map(g => g.harness === ev.harness
          ? { ...g, items: g.items.map(it => it.text === ev.text ? { ...it, status: ev.to } : it) }
          : g));
        break;
    }
  }, []);

  // Tape player.
  // We use setInterval (not rAF) because rAF is paused when the iframe is
  // offscreen — that froze the whole demo for verifiers / alt-tabbed users.
  // setInterval still fires (throttled, but it fires), so the tape keeps
  // moving forward. On the next paint everything catches up.
  //
  // We also rebase startStamp on visibility change so we don't "warp
  // forward" by however many seconds the tab was hidden — visible time
  // is what counts.
  const tapeIdxRef = useRef(0);
  const startStampRef = useRef(null);
  const virtTimeRef = useRef(0);
  useEffect(() => {
    virtTimeRef.current = virtTime;
  }, [virtTime]);

  useEffect(() => {
    if (!playing) return;
    const TICK_MS = 100;
    const speed = tw.speed || 1;
    if (startStampRef.current == null) {
      startStampRef.current = performance.now() - virtTimeRef.current / speed;
    }
    const id = setInterval(() => {
      const elapsed = (performance.now() - startStampRef.current) * speed;
      setVirtTime(elapsed);
      while (tapeIdxRef.current < TAPE.length && TAPE[tapeIdxRef.current].t <= elapsed) {
        apply(TAPE[tapeIdxRef.current]);
        tapeIdxRef.current++;
      }
      if (tapeIdxRef.current >= TAPE.length) {
        clearInterval(id);
      }
    }, TICK_MS);
    return () => clearInterval(id);
  }, [playing, tw.speed, apply]);

  // Visibility rebase: when the page returns to visible, advance
  // startStamp so virtTime continues from where it was, not from
  // realtime-since-start.
  useEffect(() => {
    const onVis = () => {
      if (document.visibilityState === 'visible') {
        const speed = tw.speed || 1;
        startStampRef.current = performance.now() - virtTimeRef.current / speed;
      }
    };
    document.addEventListener('visibilitychange', onVis);
    return () => document.removeEventListener('visibilitychange', onVis);
  }, [tw.speed]);

  // Restart / reset
  const reset = () => {
    setPlaying(false);
    tapeIdxRef.current = 0;
    startStampRef.current = null;
    setSubtitle({ text: 'Starting…', age: '', empty: true });
    setTabs([]); setDiffBody(null); setDiffRefocus(0);
    setPlanGroups([]); setItems([]); setTotals({ events: 0, cost: 0, tokens: 0 });
    setInspector(null); setExpandedBursts(new Set()); setFlashToast(null);
    setNewSince(0);
    setTimeout(() => setPlaying(true), 50);
  };

  // Replay from history — simulate the "backfill on attach" behaviour:
  // the BACKFILL rows snap in instantly (no slide-in animation, marked
  // with a "backfilled" banner), and then live tailing begins from t=0.
  const replayFromHistory = () => {
    setPlaying(false);
    tapeIdxRef.current = 0;
    startStampRef.current = null;
    setSubtitle({ text: "Resumed — Claude finished L2c-1 dogfooding; opencode compacted context at 11:32.", age: '4h 26m ago' });
    setTabs([]); setDiffBody(null); setDiffRefocus(0);
    setPlanGroups([]);
    setTotals({ events: BACKFILL.length, cost: 0, tokens: 0 });
    // Inject the backfill items in one go.
    const backfillItems = [
      { id: 'bf-banner', type: 'backfill', count: BACKFILL.length, range: '09:14 – 13:51' },
      ...BACKFILL.map((row, i) => ({ id: 'bf-' + i, type: 'row', row, backfilled: true })),
      { id: 'bf-tail',  type: 'backfill_end' },
    ];
    setItems(backfillItems);
    setInspector(null); setExpandedBursts(new Set()); setFlashToast(null);
    setNewSince(0);
    setTimeout(() => setPlaying(true), 50);
  };

  // Auto-scroll the activity card when tailing
  useEffect(() => {
    if (!autoTail || !activityRef.current) return;
    const el = activityRef.current;
    el.scrollTop = el.scrollHeight;
    setNewSince(0);
  }, [items.length, autoTail]);

  // Detect manual scroll
  const onActivityScroll = (e) => {
    const el = e.target;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 30;
    if (atBottom !== autoTail) {
      setAutoTail(atBottom);
      if (atBottom) setNewSince(0);
    }
  };

  // When new rows arrive while scrolled up, count them
  const prevCount = useRef(items.length);
  useEffect(() => {
    if (!autoTail && items.length > prevCount.current) {
      setNewSince(n => n + (items.length - prevCount.current));
    }
    prevCount.current = items.length;
  }, [items.length, autoTail]);

  const handlers = {
    openInspector: (id) => setInspector(id),
    jumpToHarness: (harness) => {
      setFlashToast(`Jumped to the ${harness === 'claude' ? 'Claude Code' : harness} terminal — resolve there.`);
      setTimeout(() => setFlashToast(null), 1900);
    },
    toggleBurst: (burstId) => setExpandedBursts(prev => {
      const next = new Set(prev);
      if (next.has(burstId)) next.delete(burstId); else next.add(burstId);
      return next;
    }),
  };

  // Flatten items into rendered nodes, expanding bursts as needed
  const renderedItems = [];
  for (const it of items) {
    if (it.type === 'sep') {
      renderedItems.push(<TurnSep key={it.id} label={it.label} />);
    } else if (it.type === 'cost') {
      if (tw.showCost) renderedItems.push(<TurnCost key={it.id} tokens={it.tokens} usd={it.usd} ms={it.ms} />);
    } else if (it.type === 'backfill') {
      renderedItems.push(
        <div key={it.id} className="lc-backfill-banner">
          <span className="glyph">↩</span>
          <span className="text">backfilled from disk · <b>{it.count}</b> events · <span className="dim">{it.range}</span></span>
        </div>
      );
    } else if (it.type === 'backfill_end') {
      renderedItems.push(
        <div key={it.id} className="lc-backfill-end">
          <span className="line" />
          <span className="text">resume tailing — live below</span>
          <span className="line" />
        </div>
      );
    } else if (it.type === 'row') {
      const r = it.row;
      const animClass = it.backfilled ? '' : 'row-slide-in';
      if (r.kind === 'burst' && expandedBursts.has(r.burstId)) {
        renderedItems.push(
          <div key={it.id + '-hdr'} className={animClass} style={{ fontFamily: 'var(--sk-mono)', fontSize: 10, color: 'var(--fg-3)', padding: '6px 12px 2px', display: 'flex', gap: 8 }}>
            <span style={{ flex: 1 }}>burst expanded · {r.count} {r.tool}s in {r.scope}</span>
            <span style={{ cursor: 'pointer', color: 'var(--accent)' }} onClick={() => handlers.toggleBurst(r.burstId)}>collapse</span>
          </div>
        );
        for (let i = 0; i < r.children.length; i++) {
          renderedItems.push(<div key={it.id + '-c' + i} className={animClass}>{renderRow(r.children[i], handlers)}</div>);
        }
      } else {
        renderedItems.push(<div key={it.id} className={animClass}>{renderRow(r, handlers)}</div>);
      }
    }
  }

  return (
    <div className={`lc-app lc ${tw.theme === 'light' ? 'light' : ''} density-${tw.density}`}>
      {/* Thin room-context bar so the prototype reads as a real Skein view */}
      <div className="lc-room-bar">
        <span className="dot">●</span>
        <span className="name">skein</span>
        <span className="sep">·</span>
        <span>kit · skein</span>
        <span className="sep">·</span>
        <span>feat/live-context</span>
        <span className="meta">
          <span>{(virtTime / 1000).toFixed(1)}s</span>
          <button onClick={() => setPlaying(p => !p)}>{playing ? '⏸' : '▶'} {playing ? 'pause' : 'play'}</button>
          <button onClick={reset}>↺ restart</button>
          <button onClick={replayFromHistory}>↩ from history</button>
        </span>
      </div>

      <div className="lc lc-pane">
        <div className={`lc-subtitle ${subtitle.empty ? 'is-empty' : ''}`}>
          <span className="glyph">{subtitle.empty ? 'IDLE' : 'AT'}</span>
          <span className="text">{subtitle.text}</span>
          <span className="meta">{subtitle.age}</span>
        </div>

        <div className="lc-stack" ref={stackRef}>
          {/* Diff card */}
          <div className={`lc-card ${diffCollapsed ? 'collapsed' : ''}`} style={{ flex: sizes[0] }}>
            <div className="lc-card-head" onClick={() => setDiffCollapsed(v => !v)}>
              <span className="chev">▾</span>
              <span className="label">Diff</span>
              <span className="meta">
                <span><span className="pulse" /> auto-follow</span>
                {tabs.find(t => t.active) && <span style={{ color: 'var(--fg-3)' }}>· focused: {tabs.find(t => t.active).harness}</span>}
              </span>
            </div>
            <div className="lc-card-body">
              <div className="lc-diff">
                {tabs.length > 0 && (
                  <div className="lc-diff-tabs">
                    {tabs.map((t, i) => (
                      <div key={i} className={`lc-diff-tab ${t.active ? 'active' : ''} ${t.flicker ? 'flicker' : ''}`}
                        onClick={() => setTabs(prev => prev.map(x => ({ ...x, active: x === t, flicker: x === t ? false : x.flicker })))}>
                        <Chip kind={t.harness} size={9} />
                        <span>{t.file}</span>
                        {t.adds != null && <span className="delta-add">+{t.adds}</span>}
                        {t.dels != null && <span className="delta-del">−{t.dels}</span>}
                      </div>
                    ))}
                  </div>
                )}
                <div key={diffRefocus} className="lc-diff-body refocus">
                  {diffBody ?? (
                    <div className="lc-empty"><div className="lc-empty-inner">
                      <div className="big">◌</div>
                      when an agent edits a file in this room, it appears here
                    </div></div>
                  )}
                </div>
              </div>
            </div>
          </div>

          <div className="lc-divider" onMouseDown={onMouseDown(0)} />

          {/* Plan card */}
          <div className={`lc-card ${planCollapsed ? 'collapsed' : ''}`} style={{ flex: sizes[1] }}>
            <div className="lc-card-head" onClick={() => setPlanCollapsed(v => !v)}>
              <span className="chev">▾</span>
              <span className="label">Plan</span>
              <span className="meta">
                {planGroups.reduce((n, g) => n + g.items.filter(i => i.status === 'now').length, 0)} now
                <span style={{ color: 'var(--fg-3)' }}>
                  {' · '}{planGroups.reduce((n, g) => n + g.items.filter(i => i.status === 'done').length, 0)}/
                  {planGroups.reduce((n, g) => n + g.items.length, 0)}
                </span>
              </span>
            </div>
            <div className="lc-card-body">
              <div className="lc-plan">
                {planGroups.length === 0 && (
                  <div className="lc-empty"><div className="lc-empty-inner">
                    <div className="big">·</div>
                    no plan items yet — agents will populate this as they work
                  </div></div>
                )}
                {planGroups.map((g, gi) => (
                  <div key={gi} className="lc-plan-group">
                    <div className="lc-plan-grouphead">
                      <Chip kind={g.harness} size={10} />
                      <span>{HARNESS_META[g.harness].name}</span>
                      <span className="count">
                        {g.items.filter(i => i.status === 'done').length}/{g.items.length}
                      </span>
                    </div>
                    {g.items.map((it, i) => (
                      <div key={i} className={`lc-plan-row ${it.status} ${it.inferred ? 'inferred' : ''}`}>
                        <span className="box">{it.status === 'done' ? '✓' : ''}</span>
                        <span className="text">{it.text}</span>
                        {it.priority && <span className={`pri ${it.priority === 'high' ? 'high' : 'med'}`}>{it.priority}</span>}
                      </div>
                    ))}
                  </div>
                ))}
              </div>
            </div>
          </div>

          <div className="lc-divider" onMouseDown={onMouseDown(1)} />

          {/* Activity card */}
          <div className={`lc-card ${activityCollapsed ? 'collapsed' : ''}`} style={{ flex: sizes[2] }}>
            <div className="lc-card-head" onClick={() => setActivityCollapsed(v => !v)}>
              <span className="chev">▾</span>
              <span className="label">Activity</span>
              <span className="meta">
                <span>{totals.events} events</span>
                <span style={{ color: 'var(--fg-3)' }}>
                  · ${totals.cost.toFixed(2)} · {(totals.tokens / 1000).toFixed(1)}k
                </span>
                <span className={`pulse ${playing ? '' : 'idle'}`} />
              </span>
            </div>
            <div className="lc-card-body" ref={activityRef} onScroll={onActivityScroll}>
              <div className="lc-activity">
                {renderedItems.length === 0 && (
                  <div className="lc-empty"><div className="lc-empty-inner">
                    <div className="big">·</div>
                    activity will tail here
                  </div></div>
                )}
                {renderedItems}
                <ActivityTail idle={!playing} />
              </div>
              {!autoTail && newSince > 0 && (
                <div className="lc-newbelow" onClick={() => {
                  if (activityRef.current) activityRef.current.scrollTop = activityRef.current.scrollHeight;
                  setAutoTail(true); setNewSince(0);
                }}>
                  ▼ {newSince} new
                </div>
              )}
            </div>
          </div>

          {/* Sub-agent inspector — overlays the stack */}
          {inspector && (() => {
            const data = INSPECTORS[inspector];
            return (
              <div className="lc-inspector" onClick={() => setInspector(null)}>
                <div className="lc-inspector-panel" onClick={e => e.stopPropagation()}>
                  <div className="lc-inspector-head">
                    <div className="title">
                      <Chip kind={data.harness} size={12} />
                      <span>{data.title}</span>
                      <span className="kind">sub-agent</span>
                    </div>
                    <div className="meta">
                      <span><span className="v">{data.duration}</span></span>
                      <span><span className="v">{data.tokens}</span></span>
                      <span><span className="v">{data.toolCount} tool calls</span></span>
                      <span><span className="v">{data.status}</span></span>
                    </div>
                    <button className="lc-inspector-close" onClick={() => setInspector(null)}>×</button>
                  </div>
                  <div className="lc-inspector-body">
                    <div className="lc-inspector-section-head">prompt</div>
                    <div className="lc-inspector-prompt">{data.prompt}</div>

                    <div className="lc-inspector-section-head">tool calls ({data.calls.length})</div>
                    {data.calls.map((c, i) => <div key={i}>{renderRow(c, handlers)}</div>)}

                    <div className="lc-inspector-section-head">final report</div>
                    <div className="lc-inspector-finalreport">{data.report}</div>
                  </div>
                </div>
              </div>
            );
          })()}
        </div>
      </div>

      {flashToast && (
        <div className="lc-flash-toast">
          <span className="accent">↗</span>
          <span>{flashToast}</span>
        </div>
      )}

      <TweaksPanel>
        <TweakSection label="Theme" />
        <TweakRadio label="Mode" value={tw.theme} options={['dark', 'light']} onChange={(v) => setTweak('theme', v)} />
        <TweakRadio label="Density" value={tw.density} options={['compact', 'regular', 'comfy']} onChange={(v) => setTweak('density', v)} />
        <TweakSection label="Playback" />
        <TweakSlider label="Speed" value={tw.speed} min={0.5} max={4} step={0.5} unit="×"
                     onChange={(v) => setTweak('speed', v)} />
        <TweakButton onClick={reset}>↺ Restart tape</TweakButton>
        <TweakButton onClick={replayFromHistory}>↩ Replay from history</TweakButton>
        <TweakSection label="Activity" />
        <TweakToggle label="Show per-turn cost rows" value={tw.showCost} onChange={(v) => setTweak('showCost', v)} />
      </TweaksPanel>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById('root')).render(<App />);
