// Skein interactive prototype — single React tree.

const { useState, useEffect, useRef, useMemo } = React;

const TWEAK_DEFAULTS = /*EDITMODE-BEGIN*/{
  "theme": "dark",
  "density": "regular",
  "harnessTabStyle": "rounded",
  "rightPaneMode": "stack",
  "showActivityFeed": true
}/*EDITMODE-END*/;

// ── Data ────────────────────────────────────────────────────────

const HARNESS_KINDS = {
  claude:   { id: 'claude',   label: 'CC',  name: 'Claude Code',   chip: 'h-claude',   desc: 'Anthropic. Direct API.' },
  opencode: { id: 'opencode', label: 'oc',  name: 'opencode',      chip: 'h-opencode', desc: 'Local server, OSS.' },
  copilot:  { id: 'copilot',  label: 'gh',  name: 'Copilot CLI',   chip: 'h-copilot',  desc: 'GitHub entitlement.' },
  byoh:     { id: 'byoh',     label: 'sk',  name: 'Skein BYOH',    chip: 'h-byoh',     desc: 'Built-in agent loop.' },
};

// Two-letter mono chip (used everywhere a harness needs identity).
const HChip = ({ kind, size = 14 }) => {
  const k = HARNESS_KINDS[kind];
  return (
    <span className={`h-chip ${k.chip}`}
      style={{ width: size, height: size, fontSize: Math.round(size * 0.62), borderRadius: Math.max(2, size / 4) }}
      title={k.name}>{k.label}</span>
  );
};
const StatusDot = ({ status, size = 6 }) => (
  <span className={`tab-status st-${status}`} style={{ width: size, height: size }} />
);

// Sample workspaces (sessions). Each workspace owns N harnesses.
const INITIAL_SESSIONS = [
  {
    id: 's1', name: 'kit · skein-tauri-shell', branch: 'feat/window-chrome',
    repo: 'skein', task: 'Build the Tauri window chrome',
    status: 'running', badge: 0,
    harnesses: [
      { id: 'h1a', kind: 'claude', name: 'shell + tabs',       status: 'running', model: 'sonnet-4.5', tokens: '14.2k' },
      { id: 'h1b', kind: 'opencode', name: 'design check',     status: 'idle',    model: 'sonnet-4.5', tokens: '2.1k' },
    ],
    activeHarnessId: 'h1a',
  },
  {
    id: 's2', name: 'kit · agent-loop-v0', branch: 'main',
    repo: 'skein', task: 'Stand up the BYOH loop',
    status: 'waiting', badge: 1,
    harnesses: [
      { id: 'h2a', kind: 'byoh', name: 'main',                 status: 'waiting', model: 'sonnet-4.5', tokens: '8.1k' },
    ],
    activeHarnessId: 'h2a',
  },
  {
    id: 's3', name: 'work · example-pim-search', branch: 'fix/index-rebuild',
    repo: 'pim', task: 'Rebuild ES index without downtime',
    status: 'running', badge: 0,
    harnesses: [
      { id: 'h3a', kind: 'copilot', name: 'main',              status: 'running', model: 'gpt-5',     tokens: '22.7k' },
      { id: 'h3b', kind: 'copilot', name: 'review pass',       status: 'idle',    model: 'gpt-5',     tokens: '4.4k' },
    ],
    activeHarnessId: 'h3a',
  },
  {
    id: 's4', name: 'kit · sqlite-migrations', branch: 'feat/sessions-table',
    repo: 'skein', task: 'Sessions + messages tables',
    status: 'idle', badge: 0,
    harnesses: [
      { id: 'h4a', kind: 'claude', name: 'main',               status: 'idle',    model: 'sonnet-4.5', tokens: '3.4k' },
    ],
    activeHarnessId: 'h4a',
  },
  {
    id: 's5', name: 'work · example-pim-search', branch: 'spike/embeddings',
    repo: 'pim', task: 'Try pgvector for product search',
    status: 'error', badge: 1,
    harnesses: [
      { id: 'h5a', kind: 'copilot', name: 'main',              status: 'error',   model: 'gpt-5',     tokens: '17.9k' },
    ],
    activeHarnessId: 'h5a',
  },
];

// Each harness pane is a faithful TUI render — it should look and feel
// like running `claude` / `opencode` / `gh copilot` in your real terminal.
// We're emulating, not embedding, but the typographic fingerprint matters.

const ClaudePanel = ({ harnessId }) => (
  <div className="sk-term term-claude">
    <div className="sk-shell-prompt"><span className="ps1-host">user@skein</span> <span className="ps1-path">~/code/skein</span> <span className="ps1-arrow">❯</span> <span className="ps1-cmd">claude --resume {harnessId}</span></div>
    <div className="header">
      <span className="name">● Claude Code</span> <span className="dim">v1.18.2 · sonnet-4.5</span>
      <br />
      <span className="dim">cwd: /home/stefan/code/skein · model: sonnet-4.5</span>
    </div>
    <span className="line dim">─ resumed conversation ─</span>
    <br /><br />
    <span className="line"><span className="accent">&gt;</span> <span className="em">Wire up the title bar with traffic-light buttons on macOS and a custom drag region. Keep it 30px tall.</span></span>
    <br /><br />
    <span className="line"><span className="bullet">●</span> Reading the existing window setup.</span>
    <br /><br />
    <span className="line muted">⚡ Read(src-tauri/tauri.conf.json)</span>
    <span className="line dim">  ⤷ 22 lines</span>
    <span className="line muted">⚡ Read(src/components/Titlebar.tsx)</span>
    <span className="line dim">  ⤷ 22 lines</span>
    <br />
    <span className="line"><span className="bullet">●</span> Setting <span className="em">titleBarStyle: "Overlay"</span> on macOS, <span className="em">decorations: false</span> elsewhere, and adding a drag region to the new <span className="em">Titlebar</span> component.</span>
    <br /><br />
    <span className="line muted">⚡ Edit(src-tauri/tauri.conf.json)</span>
    <span className="line ok">  ⤷ +3  −1</span>
    <span className="line muted">⚡ Write(src/components/Titlebar.tsx)</span>
    <span className="line ok">  ⤷ 38 lines</span>
    <br />
    <span className="line"><span className="bullet">●</span> Done. The traffic lights are inset 10px from the left, the app name centers, and the bar is draggable except over interactive elements.</span>
    <br /><br />
    <span className="line">  Want me to run <span className="accent">pnpm tauri dev</span> to verify?</span>
    <br /><br />
    <span className="line dim">┍─ chat ─────────────────────────────────────────────</span>
    <span className="line"><span className="accent">&gt; </span><span className="blink"></span></span>
    <span className="line dim">┕── ⏎ send · ⇧⏎ newline · ! bash · / commands</span>
  </div>
);

const OpenCodePanel = ({ harnessId }) => (
  <div className="sk-term term-opencode">
    <div className="sk-shell-prompt"><span className="ps1-host">user@skein</span> <span className="ps1-path">~/code/skein</span> <span className="ps1-arrow">❯</span> <span className="ps1-cmd">opencode --session {harnessId}</span></div>
    <div className="header"><span className="name">opencode</span> <span className="dim">0.4.1 · sonnet-4.5</span></div>
    <span className="line dim">session restored. 2 messages.</span>
    <br /><br />
    <span className="line"><span className="tag">[user]</span> <span className="em">Look at what the other harness just did to Titlebar.tsx and tell me if the spacing matches the design system.</span></span>
    <br /><br />
    <span className="line"><span className="tag">[assistant]</span> Reading the diff in the worktree…</span>
    <br /><br />
    <span className="line muted">  ┌ read_file</span>
    <span className="line muted">  │   path: src/components/Titlebar.tsx</span>
    <span className="line ok">  └ ok · 38 lines</span>
    <br />
    <span className="line">The 30px height is fine but the inner gap is <span className="em">10px</span> where</span>
    <span className="line">the design tokens specify <span className="em">--space-3</span> (12px). Two-pixel</span>
    <span className="line">inconsistency — small but it'll compound elsewhere.</span>
    <br /><br />
    <span className="line dim">───────────────────────────────────────────────────</span>
    <span className="line"><span className="tag">&gt;</span> <span className="blink"></span></span>
  </div>
);

const ByohPanel = ({ harnessId, onApprove, onDeny }) => (
  <div className="sk-term term-byoh">
    <div className="sk-shell-prompt"><span className="ps1-host">user@skein</span> <span className="ps1-path">~/code/skein</span> <span className="ps1-arrow">❯</span> <span className="ps1-cmd">skein agent --session {harnessId}</span></div>
    <div className="header"><span className="name">skein-byoh</span> <span className="dim">v0 · sonnet-4.5</span></div>
    <span className="line dim">connecting to anthropic… ok. 6 tools registered.</span>
    <br /><br />
    <span className="line you">user&gt; <span className="em">Add a debounce around the fs watcher so the diff pane doesn't flicker on every keystroke. 80ms feels right.</span></span>
    <br /><br />
    <span className="line">agent&gt; Looking at <span className="em">src/fs/watcher.rs</span> — the watcher pipes raw <span className="em">notify</span></span>
    <span className="line">       events straight to the channel. I'll wrap that in a debounce.</span>
    <br /><br />
    <span className="line dim">[tool] read_file <span className="accent">src/fs/watcher.rs</span></span>
    <span className="line dim">       → ok (38 lines)</span>
    <span className="line dim">[tool] grep <span className="accent">"recommended_watcher"</span></span>
    <span className="line dim">       → ok (1 match)</span>
    <span className="line dim">[tool] str_replace <span className="accent">src/fs/watcher.rs</span></span>
    <span className="line dim">       → ok (+5 −1)</span>
    <br />
    <span className="line">agent&gt; Added a 80ms debounce and an extension filter (rs, toml, md).</span>
    <span className="line">       Want me to run the test suite to confirm nothing regressed?</span>
    <br /><br />
    <span className="line">       <span className="info">[tool] bash</span> <span className="accent">cargo test --package skein-core fs::watcher</span></span>
    <div className="box warn">
      <span className="em">⚡ Permission required · bash</span>
      <br />
      <span className="dim">  $ cargo test --package skein-core fs::watcher</span>
    </div>
    <span className="line dim">  &nbsp;&nbsp;<span className="key">y</span> approve once  <span className="key">a</span> always for this session  <span className="key">n</span> deny  <span className="key">e</span> edit</span>
    <br />
    <span className="line">user&gt; <span style={{ color: 'var(--accent)', cursor: 'pointer' }} onClick={onApprove}>y</span><span className="blink"></span></span>
  </div>
);

const ByohResolvedPanel = ({ harnessId }) => (
  <div className="sk-term term-byoh">
    <div className="sk-shell-prompt"><span className="ps1-host">user@skein</span> <span className="ps1-path">~/code/skein</span> <span className="ps1-arrow">❯</span> <span className="ps1-cmd">skein agent --session {harnessId}</span></div>
    <div className="header"><span className="name">skein-byoh</span> <span className="dim">v0 · sonnet-4.5</span></div>
    <span className="line dim">[tool] str_replace <span className="accent">src/fs/watcher.rs</span> → ok</span>
    <br />
    <span className="line">agent&gt; Want me to run the test suite?</span>
    <span className="line ok">user&gt; y</span>
    <span className="line dim">[tool] bash <span className="accent">cargo test fs::watcher</span></span>
    <span className="line ok">       → ok (12 passed, 0 failed) <span className="dim">in 4.1s</span></span>
    <br />
    <span className="line">agent&gt; All passed. The debounce is in. Want me to commit it as</span>
    <span className="line">       <span className="em">fs: debounce watcher events to 80ms</span>?</span>
    <br />
    <span className="line">user&gt; <span className="blink"></span></span>
  </div>
);

const CopilotPanel = ({ harnessId }) => (
  <div className="sk-term term-copilot">
    <div className="sk-shell-prompt"><span className="ps1-host">user@laptop</span> <span className="ps1-path">~/repos/pim</span> <span className="ps1-arrow">❯</span> <span className="ps1-cmd">gh copilot suggest --session {harnessId}</span></div>
    <div className="header"><span className="name">GitHub Copilot CLI</span> <span className="dim">· gpt-5 (entitlement)</span></div>
    <span className="line dim">authenticated as stefan-i · example enterprise plan</span>
    <br /><br />
    <span className="line"><span className="tag">?</span> <span className="em">The reindex is bringing the search API to its knees. Look at how we're reading from the canonical store.</span></span>
    <br /><br />
    <span className="line muted">⊣ Inspecting the indexer entry point.</span>
    <br />
    <span className="line muted">┌─ file ops</span>
    <span className="line muted">│  read  src/indexer/main.rs</span>
    <span className="line muted">│  grep  "BATCH_SIZE"  4 matches</span>
    <span className="line muted">└─</span>
    <br />
    <span className="line">The reindexer's pulling <span className="em">50k rows</span> per batch and we're seeing</span>
    <span className="line">memory pressure spikes. I'd drop that to <span className="em">5k</span> and add a 50ms</span>
    <span className="line">yield between batches. Going to draft the change.</span>
    <br /><br />
    <span className="line dim">copilot ⤷</span>
    <span className="line"><span className="tag">?</span> <span className="blink"></span></span>
  </div>
);

// The errored variant — copilot ran into a 401 mid-stream.
// Same TUI fingerprint, but instead of a prompt the pane ends in
// an error block + retry/reauth affordance.
const CopilotErroredPanel = ({ harnessId, onRetry, onReauth }) => (
  <div className="sk-term term-copilot">
    <div className="sk-shell-prompt"><span className="ps1-host">user@laptop</span> <span className="ps1-path">~/repos/pim</span> <span className="ps1-arrow">❯</span> <span className="ps1-cmd">gh copilot suggest --session {harnessId}</span></div>
    <div className="header"><span className="name">GitHub Copilot CLI</span> <span className="dim">· gpt-5 (entitlement)</span></div>
    <span className="line dim">authenticated as stefan-i · example enterprise plan</span>
    <br /><br />
    <span className="line"><span className="tag">?</span> <span className="em">Add a similarity_search endpoint that uses the pgvector index.</span></span>
    <br /><br />
    <span className="line muted">┌─ file ops</span>
    <span className="line muted">│  read  src/embeddings/pgvector.rs</span>
    <span className="line muted">│  edit  src/embeddings/pgvector.rs  (+24 −2)</span>
    <span className="line muted">└─</span>
    <br />
    <span className="line">Hooking up an HNSW index on the embeddings column. The query</span>
    <span className="line">side is straightforward — the harder part is keeping the</span>
    <span className="line">migration online. I'll spell it out…</span>
    <br /><br />
    <span className="line dim">[stream]</span>
    <div className="box err-box">
      <span className="em err">✕ Stream interrupted · 401 Unauthorized</span>
      <br />
      <span className="dim">  github copilot subscription token expired</span>
      <br />
      <span className="dim">  request_id: rq_8d2f1c · 12:15:08</span>
      <br /><br />
      <span className="dim">  Skein paused this harness. Your worktree changes are safe;</span>
      <br />
      <span className="dim">  the partial reply above stays for context. Re-auth and resume,</span>
      <br />
      <span className="dim">  or retry on the same context.</span>
    </div>
    <br />
    <span className="line">  <span style={{ color: 'var(--accent)', cursor: 'pointer', textDecoration: 'underline' }} onClick={onReauth}>[ Re-authenticate Copilot ]</span>{'   '}<span style={{ cursor: 'pointer', textDecoration: 'underline' }} onClick={onRetry}>[ Retry on same context ]</span>{'   '}<span className="dim">[ Switch harness ]</span></span>
  </div>
);

const HARNESS_PANEL = {
  claude: ClaudePanel,
  opencode: OpenCodePanel,
  copilot: CopilotPanel,
  byoh: ByohPanel,
};

// Per-session right-pane data.
// NOTE: The old shape (tree/activeFile/diff/plan/activity) below is retained
// only because some legacy code references it. The Live Context card stack
// reads SESSION_LC defined immediately after.
const SESSION_DATA = {
  s1: {
    tree: [
      { name: 'src',                   kind: 'dir', depth: 0, open: true },
      { name: 'components',            kind: 'dir', depth: 1, open: true },
      { name: 'Titlebar.tsx',          kind: 'file', depth: 2, touched: '+38', active: true },
      { name: 'TabStrip.tsx',          kind: 'file', depth: 2 },
      { name: 'src-tauri',             kind: 'dir', depth: 0, open: true },
      { name: 'tauri.conf.json',       kind: 'file', depth: 1, touched: '+3 −1' },
    ],
    activeFile: { path: 'src/components/Titlebar.tsx', adds: 38, dels: 0 },
    diff: [
      { kind: 'add', n1: '', n2: 1,  src: <span><span className="tk-key">import</span> {'{'} useEffect {'}'} <span className="tk-key">from</span> <span className="tk-str">"react"</span>;</span> },
      { kind: 'add', n1: '', n2: 2,  src: <span><span className="tk-key">import</span> {'{'} getCurrentWindow {'}'} <span className="tk-key">from</span> <span className="tk-str">"@tauri-apps/api/window"</span>;</span> },
      { kind: 'add', n1: '', n2: 3,  src: <span></span> },
      { kind: 'add', n1: '', n2: 4,  src: <span><span className="tk-key">export function</span> <span className="tk-fn">Titlebar</span>() {'{'}</span> },
      { kind: 'add', n1: '', n2: 5,  src: <span>  <span className="tk-key">return</span> (</span> },
      { kind: 'add', n1: '', n2: 6,  src: <span>    {'<'}<span className="tk-fn">div</span> className=<span className="tk-str">"sk-titlebar"</span> data-tauri-drag-region{'>'}</span> },
      { kind: 'add', n1: '', n2: 7,  src: <span>      {'<'}<span className="tk-fn">TrafficLights</span> /{'>'}</span> },
      { kind: 'add', n1: '', n2: 8,  src: <span>      {'<'}<span className="tk-fn">span</span> className=<span className="tk-str">"sk-app-name"</span>{'>'}skein{'<'}/<span className="tk-fn">span</span>{'>'}</span> },
      { kind: 'add', n1: '', n2: 9,  src: <span>    {'<'}/<span className="tk-fn">div</span>{'>'}</span> },
      { kind: 'add', n1: '', n2: 10, src: <span>  );</span> },
      { kind: 'add', n1: '', n2: 11, src: <span>{'}'}</span> },
    ],
    plan: [
      { state: 'done', text: 'wire titleBarStyle Overlay on macOS', by: 'h1a' },
      { state: 'done', text: 'add drag region to titlebar', by: 'h1a' },
      { state: 'now',  text: 'fix space-3 token usage (h1b flagged)', by: 'h1a' },
      { state: 'next', text: 'tabstrip overflow scroll', by: 'h1a' },
    ],
    activity: [
      { time: '14:02:18', by: 'h1a', kind: 'claude',   msg: <span>str_replace <span className="arg">tauri.conf.json</span> <span className="ok">✓</span></span> },
      { time: '14:02:31', by: 'h1a', kind: 'claude',   msg: <span>write_file <span className="arg">Titlebar.tsx</span> <span className="ok">✓</span></span> },
      { time: '14:03:04', by: 'h1b', kind: 'opencode', msg: <span>read_file <span className="arg">Titlebar.tsx</span> <span className="ok">✓</span></span> },
      { time: '14:03:11', by: 'h1b', kind: 'opencode', msg: <span>flagged: <span className="arg">space-3 token mismatch</span></span> },
    ],
  },
  s2: {
    tree: [
      { name: 'src',          kind: 'dir', depth: 0, open: true },
      { name: 'agent',        kind: 'dir', depth: 1, open: true },
      { name: 'loop.rs',      kind: 'file', depth: 2 },
      { name: 'tools.rs',     kind: 'file', depth: 2, touched: '+12' },
      { name: 'fs',           kind: 'dir', depth: 1, open: true },
      { name: 'watcher.rs',   kind: 'file', depth: 2, touched: '+5 −1', active: true },
      { name: 'worktree.rs',  kind: 'file', depth: 2 },
      { name: 'Cargo.toml',   kind: 'file', depth: 0, touched: '+2' },
    ],
    activeFile: { path: 'src/fs/watcher.rs', adds: 5, dels: 1 },
    diff: [
      { kind: 'ctx', n1: 24, n2: 24, src: <span><span className="tk-com">// poll the worktree for fs changes; debounce 80ms</span></span> },
      { kind: 'ctx', n1: 25, n2: 25, src: <span><span className="tk-key">pub async fn</span> <span className="tk-fn">watch_worktree</span>(path: <span className="tk-key">&Path</span>) {'->'} <span className="tk-key">Result</span>{'<'}<span className="tk-key">Watcher</span>{'>'} {'{'}</span> },
      { kind: 'ctx', n1: 26, n2: 26, src: <span>    <span className="tk-key">let</span> (tx, rx) = mpsc::<span className="tk-fn">channel</span>(<span className="tk-num">64</span>);</span> },
      { kind: 'del', n1: 27, n2: '', src: <span>    <span className="tk-key">let mut</span> watcher = notify::<span className="tk-fn">recommended_watcher</span>(tx)?;</span> },
      { kind: 'add', n1: '', n2: 27, src: <span>    <span className="tk-key">let mut</span> watcher = notify::<span className="tk-fn">recommended_watcher</span>(<span className="tk-fn">debounce</span>(tx, <span className="tk-num">80</span>))?;</span> },
      { kind: 'ctx', n1: 28, n2: 28, src: <span>    watcher.<span className="tk-fn">watch</span>(path, RecursiveMode::<span className="tk-fn">Recursive</span>)?;</span> },
      { kind: 'add', n1: '', n2: 30, src: <span>    <span className="tk-com">// emit only on .rs / .toml / .md to keep diff pane quiet</span></span> },
      { kind: 'add', n1: '', n2: 31, src: <span>    <span className="tk-key">let</span> filtered = rx.<span className="tk-fn">filter_map</span>(<span className="tk-fn">interesting_path</span>);</span> },
      { kind: 'ctx', n1: 30, n2: 32, src: <span>    <span className="tk-key">Ok</span>(<span className="tk-key">Watcher</span> {'{'} watcher, rx: filtered {'}'})</span> },
      { kind: 'ctx', n1: 31, n2: 33, src: <span>{'}'}</span> },
    ],
    plan: [
      { state: 'done', text: 'read src/fs/watcher.rs', by: 'h2a' },
      { state: 'done', text: 'add debounce wrapper around notify channel', by: 'h2a' },
      { state: 'done', text: 'filter to .rs / .toml / .md', by: 'h2a' },
      { state: 'now',  text: 'run cargo test for fs::watcher', by: 'h2a' },
      { state: 'next', text: 'update CHANGELOG with debounce default', by: 'h2a' },
    ],
    activity: [
      { time: '13:51:02', by: 'h2a', kind: 'byoh', msg: <span>read_file <span className="arg">watcher.rs</span> <span className="ok">✓</span></span> },
      { time: '13:51:18', by: 'h2a', kind: 'byoh', msg: <span>str_replace <span className="arg">watcher.rs</span> <span className="ok">+5 −1</span></span> },
      { time: '13:52:01', by: 'h2a', kind: 'byoh', msg: <span>requested permission for <span className="arg">cargo test</span></span> },
    ],
  },
  s3: {
    tree: [
      { name: 'src',           kind: 'dir', depth: 0, open: true },
      { name: 'indexer',       kind: 'dir', depth: 1, open: true },
      { name: 'main.rs',       kind: 'file', depth: 2, active: true },
      { name: 'batch.rs',      kind: 'file', depth: 2 },
      { name: 'api',           kind: 'dir', depth: 1 },
    ],
    activeFile: { path: 'src/indexer/main.rs', adds: 0, dels: 0 },
    diff: [
      { kind: 'ctx', n1: 102, n2: 102, src: <span><span className="tk-key">const</span> <span className="tk-fn">BATCH_SIZE</span>: <span className="tk-key">usize</span> = <span className="tk-num">50_000</span>;</span> },
      { kind: 'ctx', n1: 103, n2: 103, src: <span></span> },
      { kind: 'ctx', n1: 104, n2: 104, src: <span><span className="tk-key">async fn</span> <span className="tk-fn">reindex</span>() {'{'}</span> },
      { kind: 'ctx', n1: 105, n2: 105, src: <span>    <span className="tk-key">while let Some</span>(batch) = <span className="tk-fn">next_batch</span>().<span className="tk-fn">await</span> {'{'}</span> },
      { kind: 'ctx', n1: 106, n2: 106, src: <span>        es.<span className="tk-fn">bulk_index</span>(batch).<span className="tk-fn">await</span>?;</span> },
      { kind: 'ctx', n1: 107, n2: 107, src: <span>    {'}'}</span> },
      { kind: 'ctx', n1: 108, n2: 108, src: <span>{'}'}</span> },
    ],
    plan: [
      { state: 'now',  text: 'measure memory under current batch size', by: 'h3a' },
      { state: 'next', text: 'reduce BATCH_SIZE to 5_000', by: 'h3a' },
      { state: 'next', text: 'add tokio::time::yield between batches', by: 'h3a' },
    ],
    activity: [
      { time: '13:48:11', by: 'h3a', kind: 'copilot', msg: <span>read_file <span className="arg">main.rs</span> <span className="ok">✓</span></span> },
      { time: '13:48:42', by: 'h3a', kind: 'copilot', msg: <span>grep <span className="arg">"BATCH_SIZE"</span> <span className="ok">4 matches</span></span> },
    ],
  },
  s4: {
    tree: [
      { name: 'src',         kind: 'dir', depth: 0, open: true },
      { name: 'db',          kind: 'dir', depth: 1, open: true },
      { name: 'schema.sql',  kind: 'file', depth: 2, active: true },
      { name: 'migrations',  kind: 'dir', depth: 2 },
    ],
    activeFile: { path: 'src/db/schema.sql', adds: 0, dels: 0 },
    diff: [
      { kind: 'ctx', n1: 1, n2: 1, src: <span><span className="tk-com">-- sessions, messages, tool_calls</span></span> },
      { kind: 'ctx', n1: 2, n2: 2, src: <span><span className="tk-key">CREATE TABLE</span> sessions (</span> },
      { kind: 'ctx', n1: 3, n2: 3, src: <span>    id <span className="tk-key">TEXT PRIMARY KEY</span>,</span> },
      { kind: 'ctx', n1: 4, n2: 4, src: <span>    name <span className="tk-key">TEXT NOT NULL</span></span> },
      { kind: 'ctx', n1: 5, n2: 5, src: <span>);</span> },
    ],
    plan: [
      { state: 'next', text: 'finalize sessions schema', by: 'h4a' },
    ],
    activity: [],
  },
  s5: {
    tree: [
      { name: 'src',          kind: 'dir', depth: 0, open: true },
      { name: 'embeddings',   kind: 'dir', depth: 1, open: true },
      { name: 'pgvector.rs',  kind: 'file', depth: 2, touched: '+24 −2', active: true },
      { name: 'migrate.sql',  kind: 'file', depth: 2, touched: '+8' },
      { name: 'api',          kind: 'dir', depth: 1 },
    ],
    activeFile: { path: 'src/embeddings/pgvector.rs', adds: 24, dels: 2 },
    diff: [
      { kind: 'ctx', n1: 1, n2: 1, src: <span><span className="tk-key">use</span> sqlx::PgPool;</span> },
      { kind: 'add', n1: '', n2: 2, src: <span><span className="tk-key">use</span> pgvector::Vector;</span> },
      { kind: 'ctx', n1: 2, n2: 3, src: <span></span> },
      { kind: 'ctx', n1: 3, n2: 4, src: <span><span className="tk-key">pub async fn</span> <span className="tk-fn">upsert_embedding</span>(pool: <span className="tk-key">&PgPool</span>, id: <span className="tk-key">&str</span>, v: <span className="tk-key">&[f32]</span>) {'->'} <span className="tk-key">Result</span>{'<'}<span className="tk-key">()</span>{'>'} {'{'}</span> },
      { kind: 'add', n1: '', n2: 5, src: <span>    <span className="tk-key">let</span> v = <span className="tk-key">Vector</span>::<span className="tk-fn">from</span>(v.<span className="tk-fn">to_vec</span>());</span> },
      { kind: 'ctx', n1: 4, n2: 6, src: <span>    sqlx::<span className="tk-fn">query</span>(<span className="tk-str">"INSERT INTO embeddings (id, v) VALUES ($1, $2)"</span>)</span> },
      { kind: 'ctx', n1: 5, n2: 7, src: <span>{'}'}</span> },
    ],
    plan: [
      { state: 'done', text: 'add pgvector dep + migrations', by: 'h5a' },
      { state: 'done', text: 'upsert_embedding helper',       by: 'h5a' },
      { state: 'now',  text: 'wire similarity_search endpoint', by: 'h5a' },
      { state: 'next', text: 'benchmark vs sqlite-vss',         by: 'h5a' },
    ],
    activity: [
      { time: '12:14:01', by: 'h5a', kind: 'copilot', msg: <span>read_file <span className="arg">pgvector.rs</span> <span className="ok">✓</span></span> },
      { time: '12:14:33', by: 'h5a', kind: 'copilot', msg: <span>str_replace <span className="arg">pgvector.rs</span> <span className="ok">+24 −2</span></span> },
      { time: '12:15:08', by: 'h5a', kind: 'copilot', msg: <span>stream <span className="err">interrupted · 401 unauthorized</span></span> },
    ],
  },
};

// ── Live Context data — what each room shows in the right-pane card stack.
// Shape:
//   subtitle: { text, age, empty? }
//   diff:     { tabs: [{file, adds, dels, harness, active, flicker?}], body: JSX }
//   plan:     [{ harness, items: [{status, text, priority?, inferred?}] }]
//   activity: { totals: {events, cost, tokens, idleFor?}, items: [{type, ...}] }

const LcLine = ({ ctx, add, del, n1, n2, src }) => {
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
};

const DIFF_TITLEBAR = (
  <>
    <LcLine add n2={1}  src={<span><span className="tk-key">import</span> {'{'} useEffect {'}'} <span className="tk-key">from</span> <span className="tk-str">"react"</span>;</span>} />
    <LcLine add n2={2}  src={<span><span className="tk-key">import</span> {'{'} getCurrentWindow {'}'} <span className="tk-key">from</span> <span className="tk-str">"@tauri-apps/api/window"</span>;</span>} />
    <LcLine add n2={3}  src={<span></span>} />
    <LcLine add n2={4}  src={<span><span className="tk-key">export function</span> <span className="tk-fn">Titlebar</span>() {'{'}</span>} />
    <LcLine add n2={5}  src={<span>  <span className="tk-key">return</span> (</span>} />
    <LcLine add n2={6}  src={<span>    {'<'}<span className="tk-fn">div</span> className=<span className="tk-str">"sk-titlebar"</span> data-tauri-drag-region{'>'}</span>} />
    <LcLine add n2={7}  src={<span>      {'<'}<span className="tk-fn">TrafficLights</span> /{'>'}</span>} />
    <LcLine add n2={8}  src={<span>      {'<'}<span className="tk-fn">span</span> className=<span className="tk-str">"sk-app-name"</span>{'>'}skein{'<'}/<span className="tk-fn">span</span>{'>'}</span>} />
    <LcLine add n2={9}  src={<span>    {'<'}/<span className="tk-fn">div</span>{'>'}</span>} />
    <LcLine add n2={10} src={<span>  );</span>} />
    <LcLine add n2={11} src={<span>{'}'}</span>} />
  </>
);

const DIFF_WATCHER = (
  <>
    <LcLine ctx n1={24} n2={24} src={<span><span className="tk-com">// poll the worktree for fs changes; debounce 80ms</span></span>} />
    <LcLine ctx n1={25} n2={25} src={<span><span className="tk-key">pub async fn</span> <span className="tk-fn">watch_worktree</span>(path: <span className="tk-key">&Path</span>) {'->'} <span className="tk-key">Result</span>{'<'}<span className="tk-key">Watcher</span>{'>'} {'{'}</span>} />
    <LcLine ctx n1={26} n2={26} src={<span>    <span className="tk-key">let</span> (tx, rx) = mpsc::<span className="tk-fn">channel</span>(<span className="tk-num">64</span>);</span>} />
    <LcLine del n1={27}         src={<span>    <span className="tk-key">let mut</span> watcher = notify::<span className="tk-fn">recommended_watcher</span>(tx)?;</span>} />
    <LcLine add n2={27}         src={<span>    <span className="tk-key">let mut</span> watcher = notify::<span className="tk-fn">recommended_watcher</span>(<span className="tk-fn">debounce</span>(tx, <span className="tk-num">80</span>))?;</span>} />
    <LcLine ctx n1={28} n2={28} src={<span>    watcher.<span className="tk-fn">watch</span>(path, RecursiveMode::<span className="tk-fn">Recursive</span>)?;</span>} />
    <LcLine add n2={30}         src={<span>    <span className="tk-com">// emit only on .rs / .toml / .md to keep the diff pane quiet</span></span>} />
    <LcLine add n2={31}         src={<span>    <span className="tk-key">let</span> filtered = rx.<span className="tk-fn">filter_map</span>(<span className="tk-fn">interesting_path</span>);</span>} />
    <LcLine ctx n1={30} n2={32} src={<span>    <span className="tk-key">Ok</span>(<span className="tk-key">Watcher</span> {'{'} watcher, rx: filtered {'}'})</span>} />
    <LcLine ctx n1={31} n2={33} src={<span>{'}'}</span>} />
  </>
);

const DIFF_REINDEX = (
  <>
    <LcLine ctx n1={102} n2={102} src={<span><span className="tk-key">const</span> <span className="tk-fn">BATCH_SIZE</span>: <span className="tk-key">usize</span> = <span className="tk-num">50_000</span>;</span>} />
    <LcLine ctx n1={103} n2={103} src={<span></span>} />
    <LcLine ctx n1={104} n2={104} src={<span><span className="tk-key">async fn</span> <span className="tk-fn">reindex</span>() {'{'}</span>} />
    <LcLine ctx n1={105} n2={105} src={<span>    <span className="tk-key">while let Some</span>(batch) = <span className="tk-fn">next_batch</span>().<span className="tk-fn">await</span> {'{'}</span>} />
    <LcLine ctx n1={106} n2={106} src={<span>        es.<span className="tk-fn">bulk_index</span>(batch).<span className="tk-fn">await</span>?;</span>} />
    <LcLine ctx n1={107} n2={107} src={<span>    {'}'}</span>} />
    <LcLine ctx n1={108} n2={108} src={<span>{'}'}</span>} />
  </>
);

const DIFF_PGVECTOR = (
  <>
    <LcLine ctx n1={1} n2={1} src={<span><span className="tk-key">use</span> sqlx::PgPool;</span>} />
    <LcLine add n2={2}        src={<span><span className="tk-key">use</span> pgvector::Vector;</span>} />
    <LcLine ctx n1={2} n2={3} src={<span></span>} />
    <LcLine ctx n1={3} n2={4} src={<span><span className="tk-key">pub async fn</span> <span className="tk-fn">upsert_embedding</span>(pool: <span className="tk-key">&PgPool</span>, id: <span className="tk-key">&str</span>, v: <span className="tk-key">&[f32]</span>) {'->'} <span className="tk-key">Result</span>{'<'}<span className="tk-key">()</span>{'>'} {'{'}</span>} />
    <LcLine add n2={5}        src={<span>    <span className="tk-key">let</span> v = <span className="tk-key">Vector</span>::<span className="tk-fn">from</span>(v.<span className="tk-fn">to_vec</span>());</span>} />
    <LcLine ctx n1={4} n2={6} src={<span>    sqlx::<span className="tk-fn">query</span>(<span className="tk-str">"INSERT INTO embeddings (id, v) VALUES ($1, $2)"</span>)</span>} />
    <LcLine ctx n1={5} n2={7} src={<span>{'}'}</span>} />
  </>
);

const SESSION_LC = {
  s1: {
    subtitle: { text: "Wiring the Tauri window chrome — Claude shipped the titlebar; opencode flagged a token mismatch.", age: '2m ago · Claude' },
    diff: {
      tabs: [
        { file: 'Titlebar.tsx',     adds: 38, harness: 'claude',   active: true },
        { file: 'tauri.conf.json',  adds: 3, dels: 1, harness: 'claude' },
        { file: 'lib.rs',           adds: 1, harness: 'opencode', flicker: true },
      ],
      body: DIFF_TITLEBAR,
    },
    plan: [
      { harness: 'claude', items: [
        { status: 'done', text: 'wire titleBarStyle Overlay on macOS' },
        { status: 'done', text: 'add drag region to titlebar' },
        { status: 'now',  text: 'fix space-3 token usage (h1b flagged)' },
        { status: 'next', text: 'tabstrip overflow scroll' },
      ]},
      { harness: 'opencode', items: [
        { status: 'done', text: 'review titlebar against design tokens', priority: 'high' },
        { status: 'now',  text: 'flag remaining space-N usage drift',    priority: 'med', inferred: true },
      ]},
    ],
    activity: {
      totals: { events: 18, cost: '0.31', tokens: '6.2k' },
      items: [
        { type: 'sep', label: 'turn · 14:01:54' },
        { type: 'row', row: { kind: 'user_prompt', time: '14:01:54', harness: 'claude', text: 'Wire up the title bar with traffic-light buttons and a custom drag region.' } },
        { type: 'row', row: { kind: 'ai_title', harness: 'claude', time: '14:01:55', title: 'titlebar + drag region' } },
        { type: 'row', row: { kind: 'read',  harness: 'claude', time: '14:02:14', file: 'src-tauri/tauri.conf.json', lines: 22 } },
        { type: 'row', row: { kind: 'edit',  harness: 'claude', time: '14:02:18', file: 'src-tauri/tauri.conf.json', adds: 3, dels: 1 } },
        { type: 'row', row: { kind: 'write', harness: 'claude', time: '14:02:31', file: 'src/components/Titlebar.tsx', adds: 38 } },
        { type: 'cost', tokens: '4,218', usd: '0.18', ms: '37s' },
        { type: 'sep', label: 'turn · 14:03:04' },
        { type: 'row', row: { kind: 'user_prompt', time: '14:03:04', harness: 'opencode', text: 'Read what Claude just shipped and check it against the design tokens.' } },
        { type: 'row', row: { kind: 'read', harness: 'opencode', time: '14:03:04', file: 'src/components/Titlebar.tsx', lines: 38 } },
        { type: 'row', row: { kind: 'edit', harness: 'opencode', time: '14:03:18', file: 'src/components/Titlebar.tsx', adds: 1, dels: 1 } },
      ],
    },
  },

  s2: {
    subtitle: { text: "BYOH paused for cargo test — waiting on you to approve.", age: 'just now' },
    diff: {
      tabs: [{ file: 'src/fs/watcher.rs', adds: 5, dels: 1, harness: 'byoh', active: true }],
      body: DIFF_WATCHER,
    },
    plan: [
      { harness: 'byoh', items: [
        { status: 'done', text: 'read src/fs/watcher.rs' },
        { status: 'done', text: 'add debounce wrapper around notify channel' },
        { status: 'done', text: 'filter to .rs / .toml / .md' },
        { status: 'now',  text: 'run cargo test for fs::watcher' },
        { status: 'next', text: 'update CHANGELOG with debounce default' },
      ]},
    ],
    activity: {
      totals: { events: 8 },
      items: [
        { type: 'sep', label: 'turn · 13:51:00' },
        { type: 'row', row: { kind: 'user_prompt', time: '13:51:00', harness: 'byoh', text: "Debounce the fs watcher so the diff pane doesn't flicker on every keystroke. 80ms feels right." } },
        { type: 'row', row: { kind: 'read', harness: 'byoh', time: '13:51:02', file: 'src/fs/watcher.rs', lines: 38 } },
        { type: 'row', row: { kind: 'grep', harness: 'byoh', time: '13:51:14', pattern: 'recommended_watcher', matches: 1 } },
        { type: 'row', row: { kind: 'edit', harness: 'byoh', time: '13:51:18', file: 'src/fs/watcher.rs', adds: 5, dels: 1 } },
        { type: 'row', row: { kind: 'permission', harness: 'byoh', time: '13:52:01', command: 'cargo test --package skein-core fs::watcher' } },
      ],
    },
  },

  s3: {
    subtitle: { text: "Reducing memory pressure in the reindexer — Copilot is drafting the BATCH_SIZE change.", age: '14s ago · Copilot' },
    diff: {
      tabs: [{ file: 'src/indexer/main.rs', harness: 'copilot', active: true }],
      body: DIFF_REINDEX,
    },
    plan: [
      { harness: 'copilot', items: [
        { status: 'now',  text: 'measure memory under current batch size' },
        { status: 'next', text: 'reduce BATCH_SIZE to 5_000' },
        { status: 'next', text: 'add tokio::time::yield between batches' },
      ]},
    ],
    activity: {
      totals: { events: 12, cost: '0.42', tokens: '22.7k' },
      items: [
        { type: 'sep', label: 'turn · 13:48:00' },
        { type: 'row', row: { kind: 'user_prompt', time: '13:48:00', harness: 'copilot', text: 'The reindex is bringing the search API to its knees. Look at how we read from the canonical store.' } },
        { type: 'row', row: { kind: 'read', harness: 'copilot', time: '13:48:11', file: 'src/indexer/main.rs', lines: 142 } },
        { type: 'row', row: { kind: 'grep', harness: 'copilot', time: '13:48:42', pattern: 'BATCH_SIZE', matches: 4 } },
      ],
    },
  },

  s4: {
    subtitle: { text: "Paused — sessions schema work resumes when you're ready.", age: '2h 14m ago' },
    diff: {
      tabs: [{ file: 'src/db/schema.sql', harness: 'claude', active: true }],
      body: (
        <>
          <LcLine ctx n1={1} n2={1} src={<span><span className="tk-com">-- sessions, messages, tool_calls</span></span>} />
          <LcLine ctx n1={2} n2={2} src={<span><span className="tk-key">CREATE TABLE</span> sessions (</span>} />
          <LcLine ctx n1={3} n2={3} src={<span>    id <span className="tk-key">TEXT PRIMARY KEY</span>,</span>} />
          <LcLine ctx n1={4} n2={4} src={<span>    name <span className="tk-key">TEXT NOT NULL</span></span>} />
          <LcLine ctx n1={5} n2={5} src={<span>);</span>} />
        </>
      ),
    },
    plan: [{ harness: 'claude', items: [{ status: 'next', text: 'finalize sessions schema' }] }],
    activity: { totals: { events: 4, idleFor: '2h 14m' }, items: [] },
  },

  s5: {
    subtitle: { text: "Copilot stream interrupted — 401 Unauthorized. Worktree is safe; re-auth and resume.", age: 'happening now' },
    diff: {
      tabs: [{ file: 'src/embeddings/pgvector.rs', adds: 24, dels: 2, harness: 'copilot', active: true }],
      body: DIFF_PGVECTOR,
    },
    plan: [
      { harness: 'copilot', items: [
        { status: 'done', text: 'add pgvector dep + migrations' },
        { status: 'done', text: 'upsert_embedding helper' },
        { status: 'now',  text: 'wire similarity_search endpoint' },
        { status: 'next', text: 'benchmark vs sqlite-vss' },
      ]},
    ],
    activity: {
      totals: { events: 6 },
      items: [
        { type: 'sep', label: 'turn · 12:14:00' },
        { type: 'row', row: { kind: 'user_prompt', time: '12:14:00', harness: 'copilot', text: 'Try pgvector for product search. Add the migration and upsert helper.' } },
        { type: 'row', row: { kind: 'bridge', harness: 'copilot', time: '12:14:01', status: 'connected', detail: 'gh entitlement · ok' } },
        { type: 'row', row: { kind: 'read',   harness: 'copilot', time: '12:14:01', file: 'src/embeddings/pgvector.rs', lines: 18 } },
        { type: 'row', row: { kind: 'edit',   harness: 'copilot', time: '12:14:33', file: 'src/embeddings/pgvector.rs', adds: 24, dels: 2 } },
        { type: 'row', row: { kind: 'bridge', harness: 'copilot', time: '12:15:07', status: 'reconnecting', detail: 'attempt 1 of 5' } },
        { type: 'row', row: { kind: 'api_error', harness: 'copilot', time: '12:15:08', status: 'HTTP 401', attempt: 1, retryIn: 12, message: 'subscription token expired' } },
      ],
    },
  },
};

// ── UI ──────────────────────────────────────────────────────────

const Titlebar = ({ onTour }) => (
  <div className="sk-titlebar">
    <div className="sk-traffic">
      <span className="sk-traffic-light close" />
      <span className="sk-traffic-light min" />
      <span className="sk-traffic-light max" />
    </div>
    <span className="sk-app-name"><span className="dot">●</span> skein</span>
    {onTour && <button className="sk-tour-launch" onClick={onTour}>▶ Take the tour</button>}
  </div>
);

const SessionTab = ({ s, active, onClick }) => (
  <div className={`sk-tab ${active ? 'active' : ''}`} onClick={onClick} title={s.task}>
    <div className="row-1">
      <StatusDot status={s.status} />
      <span className="name">{s.name}</span>
      {s.badge > 0 && <span className="tab-badge">{s.badge}</span>}
    </div>
    <div className="row-2">
      <span>{s.branch}</span>
      <span>·</span>
      <span style={{ display: 'flex', gap: 2 }}>
        {s.harnesses.map(h => <HChip key={h.id} kind={h.kind} size={9} />)}
      </span>
    </div>
  </div>
);

const SessionTabStrip = ({ sessions, activeId, onSelect, onNew }) => (
  <div className="sk-tabstrip">
    {sessions.map(s => (
      <SessionTab key={s.id} s={s} active={s.id === activeId} onClick={() => onSelect(s.id)} />
    ))}
    <div className="sk-tab-newbtn" onClick={onNew} title="New session">+</div>
  </div>
);

const HarnessTab = ({ h, active, onClick, onClose }) => (
  <div className={`sk-harness-tab ${active ? 'active' : ''}`} onClick={onClick}>
    <StatusDot status={h.status} size={5} />
    <HChip kind={h.kind} size={11} />
    <span className="ht-name">{h.name}</span>
    <span className="ht-x" onClick={(e) => { e.stopPropagation(); onClose(); }}>×</span>
  </div>
);

const HarnessTabBar = ({ session, onSelect, onAdd, onClose }) => (
  <div className="sk-harness-tabs">
    {session.harnesses.map(h => (
      <HarnessTab key={h.id} h={h} active={h.id === session.activeHarnessId}
        onClick={() => onSelect(h.id)}
        onClose={() => onClose(h.id)} />
    ))}
    <div className="sk-harness-add" onClick={onAdd}>+ harness</div>
    <div className="sk-harness-meta">
      <span>{session.repo} · {session.branch}</span>
    </div>
  </div>
);

const PermissionCard = ({ onApprove, onDeny }) => (
  <div className="sk-permission">
    <div className="head"><span className="icon">⏵</span><span>Permission needed · bash</span></div>
    <div className="cmd">cargo test --package skein-core fs::watcher</div>
    <div className="actions">
      <button className="sk-btn primary" onClick={onApprove}>Approve once</button>
      <button className="sk-btn">Approve always for this session</button>
      <button className="sk-btn ghost" onClick={onDeny}>Deny</button>
    </div>
  </div>
);

const Composer = ({ harnessName }) => {
  const [v, setV] = useState('');
  return (
    <div className="sk-composer">
      <div className="input-row">
        <span style={{ color: 'var(--accent)', fontFamily: 'var(--sk-mono)', fontSize: 11 }}>›</span>
        <input
          placeholder={`reply to ${harnessName}…`}
          value={v} onChange={e => setV(e.target.value)} />
      </div>
      <div className="hint">
        <span><span className="key">⏎</span> send</span>
        <span><span className="key">⇧⏎</span> newline</span>
        <span><span className="key">@</span> file</span>
        <span><span className="key">⌘K</span> sessions</span>
      </div>
    </div>
  );
};

const HarnessPicker = ({ onPick }) => (
  <div className="sk-empty-harness">
    <h3>Add a harness</h3>
    <p>Pick an agent for this workspace. All harnesses see the same worktree.</p>
    <div className="sk-harness-grid">
      {Object.values(HARNESS_KINDS).map(k => (
        <div key={k.id} className="sk-harness-card" onClick={() => onPick(k.id)}>
          <div className="head"><HChip kind={k.id} size={16} /> <span className="h-name">{k.name}</span></div>
          <div className="h-desc">{k.desc}</div>
        </div>
      ))}
    </div>
  </div>
);

// Idle state: harness tab open but nothing running yet — the user gets a
// real shell prompt and quickstart buttons that "type" a command for them.
const IdleHarnessTerminal = ({ onStart }) => (
  <div className="sk-term-idle">
    <div className="sk-shell-prompt">
      <span className="ps1-host">user@skein</span>{' '}
      <span className="ps1-path">~/code/skein</span>{' '}
      <span className="ps1-arrow">❯</span>{' '}
      <span style={{ color: 'var(--accent)' }} className="blink"></span>
    </div>
    <div style={{ marginTop: 12, color: 'var(--fg-2)' }}>
      Empty shell. Run any harness manually, or pick a quickstart:
    </div>
    <div className="quick">
      <button onClick={() => onStart('claude')}>
        <HChip kind="claude" size={12} /> <span>Claude Code</span>
        <span className="cmd">claude</span>
      </button>
      <button onClick={() => onStart('opencode')}>
        <HChip kind="opencode" size={12} /> <span>opencode</span>
        <span className="cmd">opencode</span>
      </button>
      <button onClick={() => onStart('copilot')}>
        <HChip kind="copilot" size={12} /> <span>Copilot CLI</span>
        <span className="cmd">gh copilot suggest</span>
      </button>
      <button onClick={() => onStart('byoh')}>
        <HChip kind="byoh" size={12} /> <span>Skein BYOH</span>
        <span className="cmd">skein agent</span>
      </button>
    </div>
    <div style={{ marginTop: 16, fontSize: 10.5, color: 'var(--fg-3)' }}>
      Tip: this is a real PTY. Anything you'd type in fish or pwsh works here.
    </div>
  </div>
);

const HarnessBody = ({ harness, resolved, onApprove, onDeny, onRetry, onReauth }) => {
  // BYOH waiting -> show permission inline; resolved -> show success follow-up
  if (harness.kind === 'byoh' && harness.status === 'waiting' && !resolved) {
    return <ByohPanel harnessId={harness.id} onApprove={onApprove} onDeny={onDeny} />;
  }
  if (harness.kind === 'byoh' && harness.status === 'waiting' && resolved) {
    return <ByohResolvedPanel harnessId={harness.id} />;
  }
  if (harness.status === 'error') {
    return <CopilotErroredPanel harnessId={harness.id} onRetry={onRetry} onReauth={onReauth} />;
  }
  if (harness.status === 'idle' && harness.empty) {
    return <IdleHarnessTerminal onStart={() => {}} />;
  }
  const Panel = HARNESS_PANEL[harness.kind] ?? ByohPanel;
  return <Panel harnessId={harness.id} />;
};

// (old card components — FileTree, DiffEditor, PlanCard, ActivityFeed,
// FullPaneHead, FilesFullPane, DiffFullPane, PlanFullPane, ContextStack —
// removed in the Live Context migration. The new card stack lives in
// live-context-rows.jsx / live-context-cards.jsx and is used directly
// from App via the SESSION_LC data above.)

// ── New session dialog ──────────────────────────────────────────
// Most decisions are pre-filled with sensible defaults; the user
// only commits to two things: which repo, and a one-line task.
// "Branch" defaults to a new worktree branch derived from the task.

const REPO_PRESETS = [
  { id: 'skein',  label: 'skein',           path: '~/code/skein',       lastBranch: 'main' },
  { id: 'pim',    label: 'example-pim-search', path: '~/work/pim',        lastBranch: 'main' },
  { id: 'design', label: 'skein-design',    path: '~/code/skein-design', lastBranch: 'main' },
];

const NewSessionDialog = ({ onCommit, onCancel }) => {
  const [repoId, setRepoId] = useState('skein');
  const [task, setTask] = useState('');
  const [harness, setHarness] = useState('claude');
  const [branchMode, setBranchMode] = useState('worktree'); // 'worktree' | 'current'
  const repo = REPO_PRESETS.find(r => r.id === repoId);
  const slug = task.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 28) || 'task';
  const proposedBranch = branchMode === 'worktree' ? `skein/${slug}` : repo.lastBranch;
  const canCreate = task.trim().length > 0;

  const submit = () => { if (canCreate) onCommit({ repo, task: task.trim(), harness, branch: proposedBranch, branchMode }); };

  return (
    <div className="sk-modal-bg" onClick={onCancel}>
      <div className="sk-modal" onClick={(e) => e.stopPropagation()}>
        <div className="sk-modal-head">
          <h2>New session</h2>
          <div className="sub">A session is a repo + task. You can add more harnesses inside.</div>
        </div>
        <div className="sk-modal-body">

          <div className="sk-field">
            <label>Task</label>
            <input
              className="sk-input"
              autoFocus
              placeholder="e.g. Wire up the migration runner"
              value={task}
              onChange={(e) => setTask(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') submit(); if (e.key === 'Escape') onCancel(); }} />
          </div>

          <div className="sk-field">
            <label>Repo</label>
            <select className="sk-select" value={repoId} onChange={(e) => setRepoId(e.target.value)}>
              {REPO_PRESETS.map(r => <option key={r.id} value={r.id}>{r.label}  ·  {r.path}</option>)}
            </select>
          </div>

          <div className="sk-field">
            <label>Branch</label>
            <div className="sk-radio-row">
              <div className={`sk-radio-card ${branchMode === 'worktree' ? 'selected' : ''}`} onClick={() => setBranchMode('worktree')}>
                <div className="top">New worktree</div>
                <div className="desc">{proposedBranch}</div>
              </div>
              <div className={`sk-radio-card ${branchMode === 'current' ? 'selected' : ''}`} onClick={() => setBranchMode('current')}>
                <div className="top">Current branch</div>
                <div className="desc">{repo.lastBranch} · in place</div>
              </div>
            </div>
          </div>

          <div className="sk-field">
            <label>Starting harness</label>
            <div className="sk-radio-row">
              {Object.values(HARNESS_KINDS).map(k => (
                <div key={k.id}
                  className={`sk-radio-card ${harness === k.id ? 'selected' : ''}`}
                  onClick={() => setHarness(k.id)}>
                  <div className="top"><HChip kind={k.id} size={14} /> {k.name}</div>
                  <div className="desc">{k.desc}</div>
                </div>
              ))}
            </div>
          </div>

        </div>
        <div className="sk-modal-foot">
          <button className="sk-btn" onClick={onCancel}>Cancel</button>
          <button
            className="sk-btn primary"
            disabled={!canCreate}
            style={{ opacity: canCreate ? 1 : 0.5, cursor: canCreate ? 'pointer' : 'not-allowed' }}
            onClick={submit}>
            Create session
          </button>
        </div>
      </div>
    </div>
  );
};

// ── Empty state (first run, no sessions) ────────────────────────

const EmptyState = ({ onNew }) => (
  <div className="sk-empty">
    <div className="glyph">⊜</div>
    <h1>No sessions yet</h1>
    <div className="lede">
      A session pins a repo and a task. Open as many harnesses inside as you want —
      Claude Code and opencode on the same worktree, two Copilot runs on a fix,
      whatever shape the work takes.
    </div>
    <button className="start-btn" onClick={onNew}>Create your first session</button>
    <div className="hint-list">
      <div className="row"><span className="kbd">⌘ N</span><span>New session</span></div>
      <div className="row"><span className="kbd">⌘ ⇧ H</span><span>Add harness to current session</span></div>
      <div className="row"><span className="kbd">⌘ K</span><span>Switch session / harness</span></div>
    </div>
  </div>
);

// ── Scripted tour ───────────────────────────────────────────────
// Drives the prototype through the demo story so anyone you send
// the file to can watch it explain itself. Each step has:
//   target   — selector to spotlight (or null for centred callout)
//   place    — 'top' | 'bottom' | 'left' | 'right' | 'center'
//   title, body — what to say
//   action   — optional fn(actions) to run when the step *enters*
//   advance  — 'auto' (autoplay after delay) or 'manual'
//
// `actions` is the App's exposed surface so the tour can drive it
// programmatically (no fragile DOM-clicking).

const TOUR_STEPS = [
  {
    target: '.sk-tabstrip',
    place: 'bottom',
    title: 'Sessions, not chats',
    body: <>Each tab is a <em>workspace</em> — one repo, one task. The status dot tells you whether it's running, waiting, idle, or errored.</>,
    advance: 'auto', delay: 3800,
  },
  {
    target: '.sk-tab-newbtn',
    place: 'bottom',
    title: 'Create a new session',
    body: <>Pick a repo, name the task, and Skein puts you on a fresh worktree branch.</>,
    action: (a) => a.setShowNewSession(true),
    advance: 'auto', delay: 3000,
  },
  {
    target: '.sk-modal',
    place: 'right',
    title: 'Two real decisions',
    body: <>Repo and task. Branch defaults to a new worktree (you can put a Claude harness on <code>feat/x</code> while a Copilot harness fixes <code>main</code>).</>,
    advance: 'auto', delay: 4200,
  },
  {
    target: null, place: 'center',
    title: 'Skipping the form for now',
    body: <>We'll close this and look at a workspace that already has work in flight.</>,
    action: (a) => { a.setShowNewSession(false); a.setActiveSessionId('s1'); },
    advance: 'auto', delay: 2400,
  },
  {
    target: '.sk-harness-tabs',
    place: 'bottom',
    title: 'Harnesses live inside a session',
    body: <>This session has Claude Code <em>and</em> opencode running on the same worktree. They see each other's edits.</>,
    advance: 'auto', delay: 3800,
  },
  {
    target: '.sk-harness-col',
    place: 'right',
    title: 'Each harness is a real TUI',
    body: <>We're emulating <code>claude</code>, <code>opencode</code>, <code>gh copilot</code>, and a built-in agent — same fingerprints you'd see in your terminal.</>,
    action: (a) => a.switchHarnessInSession('s1', 'h1b'),
    advance: 'auto', delay: 4400,
  },
  {
    target: '.sk-right',
    place: 'left',
    title: 'The worktree is shared',
    body: <>Switch harnesses, the diff and plan stay put. They're a property of the <em>workspace</em>, not the agent. opencode just flagged a token mismatch in Claude's diff — that's the cross-harness conversation.</>,
    advance: 'auto', delay: 4800,
  },
  {
    target: '.sk-toast, .sk-statusbar .urgent',
    place: 'top',
    title: 'Ambient signal',
    body: <>Another session needs you. The toast and status-bar segment both deep-link there.</>,
    action: (a) => { a.setActiveSessionId('s2'); a.setToastDismissed(false); },
    advance: 'auto', delay: 3600,
  },
  {
    target: '.sk-harness-col',
    place: 'right',
    title: 'Permission, inline',
    body: <>The BYOH agent paused for <code>cargo test</code>. You approve in-place — no modal, no context switch.</>,
    advance: 'auto', delay: 3400,
  },
  {
    target: '.sk-harness-col',
    place: 'right',
    title: 'Approving…',
    body: <>Watch the agent continue, the tests pass, and the status flip green.</>,
    action: (a) => a.approve('h2a'),
    advance: 'auto', delay: 3600,
  },
  {
    target: '.sk-tabstrip',
    place: 'bottom',
    title: 'Errors are scoped',
    body: <>Now jump to the spike — Copilot's token expired mid-stream. Worktree is safe; only the harness died.</>,
    action: (a) => a.setActiveSessionId('s5'),
    advance: 'auto', delay: 3800,
  },
  {
    target: '.sk-harness-col',
    place: 'right',
    title: 'Recover in place',
    body: <>Re-auth, retry, or hand the work to a different harness. Because session ≠ harness, none of this loses your context.</>,
    advance: 'auto', delay: 3800,
  },
  {
    target: null, place: 'center',
    title: "That's Skein.",
    body: <>Sessions you can leave and come back to. Multiple agents on one worktree. Failures that don't take the room down with them. Hit Restart any time.</>,
    advance: 'manual',
  },
];

// Spotlight + callout. Reads the bounding rect of `target` to position
// itself; falls back to a centred card if no target.
const TourOverlay = ({ step, idx, total, onNext, onPrev, onSkip, onRestart }) => {
  const [rect, setRect] = useState(null);
  useEffect(() => {
    const measure = () => {
      if (!step.target) { setRect(null); return; }
      // Try each selector in a comma-list; take first match.
      const sel = step.target.split(',').map(s => s.trim());
      let el = null;
      for (const s of sel) { el = document.querySelector(s); if (el) break; }
      if (el) {
        const r = el.getBoundingClientRect();
        setRect({ top: r.top, left: r.left, width: r.width, height: r.height });
      } else {
        setRect(null);
      }
    };
    measure();
    const t = setTimeout(measure, 60); // re-measure once layout settles
    window.addEventListener('resize', measure);
    return () => { clearTimeout(t); window.removeEventListener('resize', measure); };
  }, [step]);

  // Auto-advance
  useEffect(() => {
    if (step.advance !== 'auto') return;
    const t = setTimeout(onNext, step.delay ?? 3500);
    return () => clearTimeout(t);
  }, [step, onNext]);

  // Place callout based on rect + step.place
  const calloutStyle = (() => {
    const PAD = 14;
    const W = 360;
    if (!rect || step.place === 'center') {
      return { left: '50%', top: '50%', transform: 'translate(-50%, -50%)', width: W };
    }
    if (step.place === 'bottom') return { left: Math.max(20, Math.min(window.innerWidth - W - 20, rect.left + rect.width / 2 - W / 2)), top: rect.top + rect.height + PAD, width: W };
    if (step.place === 'top')    return { left: Math.max(20, Math.min(window.innerWidth - W - 20, rect.left + rect.width / 2 - W / 2)), top: rect.top - PAD, transform: 'translateY(-100%)', width: W };
    if (step.place === 'right')  return { left: rect.left + rect.width + PAD, top: Math.max(20, rect.top + 40), width: W };
    if (step.place === 'left')   return { left: rect.left - PAD, transform: 'translateX(-100%)', top: Math.max(20, rect.top + 40), width: W };
    return { left: '50%', top: '50%', transform: 'translate(-50%, -50%)', width: W };
  })();

  return (
    <div className="sk-tour-overlay">
      {rect ? (
        <div className="sk-tour-spotlight" style={{
          top: rect.top - 6, left: rect.left - 6,
          width: rect.width + 12, height: rect.height + 12,
        }} />
      ) : (
        <div className="sk-tour-scrim" />
      )}
      <div className="sk-tour-callout" style={calloutStyle}>
        <div className="head">
          <span className="step">{idx + 1}/{total}</span>
          <span className="dot-row">{TOUR_STEPS.map((_, i) => (
            <span key={i} className={`d ${i === idx ? 'on' : i < idx ? 'past' : ''}`} />
          ))}</span>
        </div>
        <div className="title">{step.title}</div>
        <div className="body">{step.body}</div>
        <div className="foot">
          <button className="ghost" onClick={onSkip}>Skip tour</button>
          <span className="spacer" />
          {idx > 0 && <button className="ghost" onClick={onPrev}>← Back</button>}
          {step.advance === 'manual'
            ? <button className="primary" onClick={onRestart}>Restart</button>
            : <button className="primary" onClick={onNext}>{idx === total - 1 ? 'Done' : 'Next →'}</button>}
        </div>
      </div>
    </div>
  );
};

// ── App ─────────────────────────────────────────────────────────

function App() {
  const [t, setTweak] = useTweaks(TWEAK_DEFAULTS);
  const [sessions, setSessions] = useState(INITIAL_SESSIONS);
  const [activeSessionId, setActiveSessionId] = useState('s2'); // start on the waiting one
  const [permissionResolved, setPermissionResolved] = useState({}); // by harness id
  const [showPicker, setShowPicker] = useState(null); // sessionId waiting for harness pick
  const [toastDismissed, setToastDismissed] = useState(false);
  const [showNewSession, setShowNewSession] = useState(false);
  const [tourIdx, setTourIdx] = useState(null); // null = off, otherwise step index

  const session = sessions.find(s => s.id === activeSessionId);
  const activeHarness = session?.harnesses.find(h => h.id === session.activeHarnessId);
  const data = SESSION_DATA[session?.id];

  const switchSession = (id) => { setActiveSessionId(id); setToastDismissed(true); };

  const switchHarnessInSession = (sessionId, harnessId) => {
    setSessions(prev => prev.map(s => s.id === sessionId ? { ...s, activeHarnessId: harnessId } : s));
  };
  const closeHarness = (sessionId, harnessId) => {
    setSessions(prev => prev.map(s => {
      if (s.id !== sessionId) return s;
      const remaining = s.harnesses.filter(h => h.id !== harnessId);
      if (remaining.length === 0) return s; // don't remove last one
      return { ...s, harnesses: remaining, activeHarnessId: remaining[0].id };
    }));
  };
  const addHarness = (sessionId) => setShowPicker(sessionId);
  const pickHarness = (kind) => {
    const newId = 'h' + Math.random().toString(36).slice(2, 7);
    setSessions(prev => prev.map(s => {
      if (s.id !== showPicker) return s;
      const newH = { id: newId, kind, name: `${HARNESS_KINDS[kind].label}-${s.harnesses.length + 1}`, status: 'idle', model: kind === 'copilot' ? 'gpt-5' : 'sonnet-4.5', tokens: '0' };
      return { ...s, harnesses: [...s.harnesses, newH], activeHarnessId: newId };
    }));
    setShowPicker(null);
  };

  const approve = (harnessId) => {
    setPermissionResolved(prev => ({ ...prev, [harnessId]: true }));
    setSessions(prev => prev.map(s => {
      if (!s.harnesses.find(h => h.id === harnessId)) return s;
      return {
        ...s,
        status: 'running', badge: 0,
        harnesses: s.harnesses.map(h => h.id === harnessId ? { ...h, status: 'running' } : h),
      };
    }));
  };
  const deny = (harnessId) => approve(harnessId); // same UX path for now

  const recoverError = (harnessId) => {
    setSessions(prev => prev.map(s => {
      if (!s.harnesses.find(h => h.id === harnessId)) return s;
      return {
        ...s,
        status: 'running', badge: 0,
        harnesses: s.harnesses.map(h => h.id === harnessId ? { ...h, status: 'running' } : h),
      };
    }));
  };

  // Tour state machine — drives App by calling exposed setters.
  const tourActions = {
    setShowNewSession,
    setActiveSessionId,
    setToastDismissed,
    switchHarnessInSession,
    approve,
  };
  const startTour = () => {
    // Reset to the demo's known starting state, then enter step 0.
    setSessions(INITIAL_SESSIONS);
    setActiveSessionId('s2');
    setPermissionResolved({});
    setToastDismissed(false);
    setShowNewSession(false);
    setTourIdx(0);
  };
  const nextStep = () => {
    setTourIdx(i => {
      if (i === null) return null;
      if (i >= TOUR_STEPS.length - 1) return null; // exit at end
      return i + 1;
    });
  };
  const prevStep = () => setTourIdx(i => (i === null || i <= 0) ? i : i - 1);
  const skipTour = () => { setTourIdx(null); setShowNewSession(false); };

  // Run a step's `action` when entering it.
  useEffect(() => {
    if (tourIdx === null) return;
    const step = TOUR_STEPS[tourIdx];
    if (step?.action) step.action(tourActions);
  }, [tourIdx]);

  const urgent = sessions.find(s => s.id !== activeSessionId && s.status === 'waiting');

  const createSession = ({ repo, task, harness, branch }) => {
    const sid = 's' + Math.random().toString(36).slice(2, 6);
    const hid = 'h' + Math.random().toString(36).slice(2, 6);
    const newSession = {
      id: sid,
      name: `${repo.id === 'pim' ? 'work' : 'kit'} · ${repo.label}`,
      branch, repo: repo.id, task,
      status: 'running', badge: 0,
      harnesses: [{ id: hid, kind: harness, name: 'main', status: 'running', model: harness === 'copilot' ? 'gpt-5' : 'sonnet-4.5', tokens: '0' }],
      activeHarnessId: hid,
    };
    setSessions(prev => [...prev, newSession]);
    setActiveSessionId(sid);
    setShowNewSession(false);
  };

  // Empty state: no sessions at all.
  if (sessions.length === 0) {
    return (
      <div className={`sk-app sk-${t.theme} density-${t.density}`}>
        <Titlebar onTour={startTour} />
        <EmptyState onNew={() => setShowNewSession(true)} />
        {showNewSession && <NewSessionDialog onCommit={createSession} onCancel={() => setShowNewSession(false)} />}
        <TweaksPanel>
          <TweakSection label="Theme" />
          <TweakRadio label="Mode" value={t.theme} options={['dark', 'light']} onChange={(v) => setTweak('theme', v)} />
          <TweakButton label="Reset sessions" onClick={() => setSessions(INITIAL_SESSIONS)}>Restore samples</TweakButton>
        </TweaksPanel>
      </div>
    );
  }

  return (
    <div className={`sk-app sk-${t.theme} density-${t.density}`}>
      <Titlebar onTour={startTour} />

      <SessionTabStrip
        sessions={sessions}
        activeId={activeSessionId}
        onSelect={switchSession}
        onNew={() => setShowNewSession(true)} />

      <div className="sk-workspace">
        <div className="sk-harness-col">
          <HarnessTabBar
            session={session}
            onSelect={(id) => switchHarnessInSession(session.id, id)}
            onAdd={() => addHarness(session.id)}
            onClose={(id) => closeHarness(session.id, id)} />

          {showPicker === session.id ? (
            <HarnessPicker onPick={pickHarness} />
          ) : (
            <HarnessBody
              harness={activeHarness}
              resolved={permissionResolved[activeHarness.id]}
              onApprove={() => approve(activeHarness.id)}
              onDeny={() => deny(activeHarness.id)}
              onRetry={() => recoverError(activeHarness.id)}
              onReauth={() => recoverError(activeHarness.id)} />
          )}
        </div>

        <div className="sk-right">
          <div className="lc">
            {(() => {
              const lc = SESSION_LC[session.id];
              if (!lc) return <div className="lc-empty"><div className="lc-empty-inner"><div className="big">·</div>no live context for this room yet</div></div>;
              return (
                <>
                  <div className={`lc-subtitle ${lc.subtitle.empty ? 'is-empty' : ''}`}>
                    <span className="glyph">{lc.subtitle.empty ? 'IDLE' : 'AT'}</span>
                    <span className="text">{lc.subtitle.text}</span>
                    <span className="meta">{lc.subtitle.age}</span>
                  </div>
                  <div className="lc-stack">
                    <DiffCard flex={1.2} tabs={lc.diff.tabs} body={lc.diff.body} />
                    <div className="lc-divider" />
                    <PlanCard flex={1} groups={lc.plan} />
                    <div className="lc-divider" />
                    <ActivityCard flex={1.2} totals={lc.activity.totals}>
                      {lc.activity.items.map((it, i) => {
                        if (it.type === 'sep')  return <TurnSep key={i} label={it.label} />;
                        if (it.type === 'cost') return t.showActivityFeed ? <TurnCost key={i} tokens={it.tokens} usd={it.usd} ms={it.ms} /> : null;
                        if (it.type === 'row') {
                          const r = it.row;
                          switch (r.kind) {
                            case 'edit':        return <EditRow key={i} {...r} />;
                            case 'write':       return <EditRow key={i} {...r} kind="write" />;
                            case 'read':        return <ReadRow key={i} {...r} />;
                            case 'grep':        return <SearchRow key={i} {...r} />;
                            case 'glob':        return <SearchRow key={i} {...r} kind="glob" />;
                            case 'bash':        return <BashRow key={i} {...r} />;
                            case 'task':        return <TaskRow key={i} {...r} />;
                            case 'todowrite':   return <TodoWriteRow key={i} {...r} />;
                            case 'ask':         return <AskRow key={i} {...r} />;
                            case 'agent':       return <AgentRow key={i} {...r} />;
                            case 'pr':          return <PrRow key={i} {...r} />;
                            case 'queue':       return <QueueRow key={i} {...r} />;
                            case 'userfile':    return <UserFileRow key={i} {...r} />;
                            case 'slash':       return <SlashRow key={i} {...r} />;
                            case 'compact':     return <CompactRow key={i} {...r} />;
                            case 'api_error':   return <ApiErrorRow key={i} {...r} />;
                            case 'tool_error':  return <ToolErrorRow key={i} {...r} />;
                            case 'permission':  return <PermissionRow key={i} {...r} />;
                            case 'user_prompt': return <UserPromptRow key={i} {...r} />;
                            case 'perm_mode':   return <PermissionModeRow key={i} {...r} />;
                            case 'ai_title':    return <AiTitleRow key={i} {...r} />;
                            case 'bridge':      return <BridgeStatusRow key={i} {...r} />;
                            case 'burst':       return <BurstRow key={i} {...r} />;
                            default:            return null;
                          }
                        }
                        return null;
                      })}
                      <ActivityTail idle={!!lc.activity.totals?.idleFor} />
                    </ActivityCard>
                  </div>
                </>
              );
            })()}
          </div>
        </div>
      </div>

      {/* Status bar */}
      <div className="sk-statusbar" style={{ position: 'relative' }}>
        <span className="seg">
          <HChip kind={activeHarness.kind} size={10} />
          <span>{HARNESS_KINDS[activeHarness.kind].name}</span>
        </span>
        <span className="seg"><span className={`dot-tiny st-${activeHarness.status}`} />{activeHarness.status}</span>
        <span className="seg">{session.branch}</span>
        <span className="seg">{activeHarness.model}</span>
        <span className="seg">{activeHarness.tokens} tok</span>
        <span className="spacer" />
        {urgent && (
          <span className="seg urgent" onClick={() => switchSession(urgent.id)}>
            <span className="dot-tiny st-waiting" />
            {urgent.name.split(' · ').pop()} needs you →
          </span>
        )}
        <span className="seg">utf-8 · LF</span>

        {urgent && !toastDismissed && (
          <div className="sk-toast" onClick={() => switchSession(urgent.id)}>
            <HChip kind={urgent.harnesses[0].kind} size={14} />
            <div>
              <div style={{ color: 'var(--fg-0)' }}>{urgent.name.split(' · ').pop()} needs permission</div>
              <div style={{ color: 'var(--fg-2)', fontFamily: 'var(--sk-mono)', fontSize: 10, marginTop: 2 }}>cargo test fs::watcher</div>
            </div>
            <span style={{ color: 'var(--fg-3)', marginLeft: 8 }} onClick={(e) => { e.stopPropagation(); setToastDismissed(true); }}>×</span>
          </div>
        )}
      </div>

      <TweaksPanel>
        <TweakSection label="Theme" />
        <TweakRadio label="Mode" value={t.theme} options={['dark', 'light']} onChange={(v) => setTweak('theme', v)} />
        <TweakRadio label="Density" value={t.density} options={['compact', 'regular', 'comfy']} onChange={(v) => setTweak('density', v)} />
        <TweakSection label="Right pane" />
        <TweakToggle label="Activity feed (cross-harness)" value={t.showActivityFeed} onChange={(v) => setTweak('showActivityFeed', v)} />
        <TweakSection label="Demo state" />
        <TweakButton onClick={() => setSessions([])}>Reset to empty state</TweakButton>
        <TweakButton onClick={() => { setSessions(INITIAL_SESSIONS); setActiveSessionId('s2'); setPermissionResolved({}); setToastDismissed(false); }}>Restore samples</TweakButton>
      </TweaksPanel>

      {showNewSession && <NewSessionDialog onCommit={createSession} onCancel={() => setShowNewSession(false)} />}

      {tourIdx !== null && (
        <TourOverlay
          step={TOUR_STEPS[tourIdx]}
          idx={tourIdx}
          total={TOUR_STEPS.length}
          onNext={nextStep}
          onPrev={prevStep}
          onSkip={skipTour}
          onRestart={startTour} />
      )}
    </div>
  );
}

ReactDOM.createRoot(document.getElementById('root')).render(<App />);
