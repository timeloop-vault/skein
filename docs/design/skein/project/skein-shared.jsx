// Shared bits: harness chips, status dots, sample sessions, code snippets.

const HARNESS = {
  claude:   { id: 'claude',   label: 'CC',  name: 'Claude Code',  bg: 'h-bg-claude'   },
  opencode: { id: 'opencode', label: 'oc',  name: 'opencode',     bg: 'h-bg-opencode' },
  copilot:  { id: 'copilot',  label: 'gh',  name: 'Copilot CLI',  bg: 'h-bg-copilot'  },
  byoh:     { id: 'byoh',     label: 'sk',  name: 'Skein BYOH',   bg: 'h-bg-byoh'     },
};

const HarnessChip = ({ harness, size = 12 }) => {
  const h = HARNESS[harness];
  return (
    <span
      className={`sk-tab-harness ${h.bg}`}
      style={{ width: size, height: size, fontSize: Math.round(size * 0.66), borderRadius: Math.max(2, size / 4) }}
      title={h.name}
    >{h.label}</span>
  );
};

const StatusDot = ({ status, size = 6 }) => (
  <span className={`tab-status st-${status}`} style={{ width: size, height: size }} />
);

// The canonical scenario from spec §11: 3 active, 1 waiting, 1 idle (we add a 6th idle and a 7th errored to round it out).
const SESSIONS = [
  { id: 's1', name: 'kit · skein-tauri-shell',     branch: 'feat/window-chrome',        harness: 'claude',   status: 'running', repo: 'skein',         tokens: '14.2k', model: 'sonnet-4.5', active: true,  badge: 0 },
  { id: 's2', name: 'kit · agent-loop-v0',          branch: 'main',                      harness: 'byoh',     status: 'waiting', repo: 'skein',         tokens: '8.1k',  model: 'sonnet-4.5', badge: 1 },
  { id: 's3', name: 'work · example-pim-search',      branch: 'fix/index-rebuild',         harness: 'copilot',  status: 'running', repo: 'pim',           tokens: '22.7k', model: 'gpt-5',      badge: 0 },
  { id: 's4', name: 'kit · sqlite-migrations',      branch: 'feat/sessions-table',       harness: 'claude',   status: 'idle',    repo: 'skein',         tokens: '3.4k',  model: 'sonnet-4.5', badge: 0 },
  { id: 's5', name: 'play · monaco-diff-spike',     branch: 'spike/diff-react',          harness: 'opencode', status: 'idle',    repo: 'spikes',        tokens: '1.0k',  model: 'sonnet-4.5', badge: 0 },
];

// A snippet of "real-looking" code with a diff
const DIFF_LINES = [
  { kind: 'ctx', n1: 24,  n2: 24,  src: <span><span className="tk-com">// poll the worktree for fs changes; debounce 80ms</span></span> },
  { kind: 'ctx', n1: 25,  n2: 25,  src: <span><span className="tk-key">pub async fn</span> <span className="tk-fn">watch_worktree</span>(path: <span className="tk-key">&Path</span>) {'->'} <span className="tk-key">Result</span>{'<'}<span className="tk-key">Watcher</span>{'>'} {'{'}</span> },
  { kind: 'ctx', n1: 26,  n2: 26,  src: <span>    <span className="tk-key">let</span> (tx, rx) = mpsc::<span className="tk-fn">channel</span>(<span className="tk-num">64</span>);</span> },
  { kind: 'del', n1: 27,  n2: '',  src: <span>    <span className="tk-key">let mut</span> watcher = notify::<span className="tk-fn">recommended_watcher</span>(tx)?;</span> },
  { kind: 'add', n1: '',  n2: 27,  src: <span>    <span className="tk-key">let mut</span> watcher = notify::<span className="tk-fn">recommended_watcher</span>(<span className="tk-fn">debounce</span>(tx, <span className="tk-num">80</span>))?;</span> },
  { kind: 'ctx', n1: 28,  n2: 28,  src: <span>    watcher.<span className="tk-fn">watch</span>(path, RecursiveMode::<span className="tk-fn">Recursive</span>)?;</span> },
  { kind: 'ctx', n1: 29,  n2: 29,  src: <span>    </span> },
  { kind: 'add', n1: '',  n2: 30,  src: <span>    <span className="tk-com">// emit only on .rs / .toml / .md to keep diff pane quiet</span></span> },
  { kind: 'add', n1: '',  n2: 31,  src: <span>    <span className="tk-key">let</span> filtered = rx.<span className="tk-fn">filter_map</span>(<span className="tk-fn">interesting_path</span>);</span> },
  { kind: 'ctx', n1: 30,  n2: 32,  src: <span>    <span className="tk-key">Ok</span>(<span className="tk-key">Watcher</span> {'{'} watcher, rx: filtered {'}'})</span> },
  { kind: 'ctx', n1: 31,  n2: 33,  src: <span>{'}'}</span> },
  { kind: 'ctx', n1: 32,  n2: 34,  src: <span></span> },
  { kind: 'ctx', n1: 33,  n2: 35,  src: <span><span className="tk-key">fn</span> <span className="tk-fn">interesting_path</span>(ev: <span className="tk-key">Event</span>) {'->'} <span className="tk-key">Option</span>{'<'}<span className="tk-key">Event</span>{'>'} {'{'}</span> },
  { kind: 'ctx', n1: 34,  n2: 36,  src: <span>    <span className="tk-key">let</span> ext = ev.path.<span className="tk-fn">extension</span>()?.<span className="tk-fn">to_str</span>()?;</span> },
  { kind: 'ctx', n1: 35,  n2: 37,  src: <span>    <span className="tk-key">matches!</span>(ext, <span className="tk-str">"rs"</span> | <span className="tk-str">"toml"</span> | <span className="tk-str">"md"</span>).<span className="tk-fn">then_some</span>(ev)</span> },
  { kind: 'ctx', n1: 36,  n2: 38,  src: <span>{'}'}</span> },
];

const TREE = [
  { name: 'src',                kind: 'dir', depth: 0, open: true },
  { name: 'main.rs',            kind: 'file', depth: 1 },
  { name: 'agent',              kind: 'dir', depth: 1, open: true },
  { name: 'loop.rs',            kind: 'file', depth: 2 },
  { name: 'tools.rs',           kind: 'file', depth: 2, touched: '+12' },
  { name: 'fs',                 kind: 'dir', depth: 1, open: true },
  { name: 'watcher.rs',         kind: 'file', depth: 2, touched: '+5 −1', active: true },
  { name: 'worktree.rs',        kind: 'file', depth: 2 },
  { name: 'session',            kind: 'dir', depth: 1 },
  { name: 'Cargo.toml',         kind: 'file', depth: 0, touched: '+2' },
  { name: 'README.md',          kind: 'file', depth: 0 },
];

// Chat content with a permission prompt for s2 (BYOH waiting session).
const CHAT_S2 = (
  <>
    <div className="sk-msg-user">Add a debounce around the fs watcher so the diff pane doesn't flicker on every keystroke. 80ms feels right.</div>
    <div className="sk-msg-assistant">Looking at <strong>src/fs/watcher.rs</strong> — the watcher pipes raw <code style={{fontFamily:'var(--sk-mono)',fontSize:'11px'}}>notify</code> events straight to the channel. I'll wrap that in a debounce and filter to source files only.</div>
    <div className="sk-tool"><span>▸</span><span className="name">read_file</span><span className="arg">src/fs/watcher.rs</span><span className="check">✓ 38 lines</span></div>
    <div className="sk-tool"><span>▸</span><span className="name">grep</span><span className="arg">"recommended_watcher"</span><span className="check">✓ 1 match</span></div>
    <div className="sk-tool"><span>▸</span><span className="name">str_replace</span><span className="arg">src/fs/watcher.rs</span><span className="check">✓ +5 −1</span></div>
    <div className="sk-msg-assistant">Added a 80ms debounce and an extension filter (<code style={{fontFamily:'var(--sk-mono)'}}>rs</code>, <code style={{fontFamily:'var(--sk-mono)'}}>toml</code>, <code style={{fontFamily:'var(--sk-mono)'}}>md</code>). Want me to run the test suite to confirm nothing regressed?</div>
    <div className="sk-permission">
      <div className="head"><span className="icon">⏵</span><span>Permission needed · bash</span></div>
      <div className="cmd">cargo test --package skein-core fs::watcher</div>
      <div className="actions">
        <button className="sk-btn primary">Approve once</button>
        <button className="sk-btn">Approve always for this session</button>
        <button className="sk-btn ghost">Deny</button>
      </div>
    </div>
  </>
);

const TODO_LIST = [
  { state: 'done', text: 'read src/fs/watcher.rs' },
  { state: 'done', text: 'add debounce wrapper around notify channel' },
  { state: 'done', text: 'filter to .rs / .toml / .md' },
  { state: 'now',  text: 'run cargo test for fs::watcher' },
  { state: 'next', text: 'update CHANGELOG with debounce default' },
  { state: 'next', text: 'commit and push to feat/window-chrome' },
];

// ── Reusable shell pieces ──────────────────────────────────────

const Titlebar = ({ theme = 'dark' }) => (
  <div className="sk-titlebar">
    <div className="sk-traffic">
      <span className="sk-traffic-light close" />
      <span className="sk-traffic-light min" />
      <span className="sk-traffic-light max" />
    </div>
    <span className="sk-app-name"><span className="dot">●</span> skein</span>
  </div>
);

const TabStrip = ({ sessions, activeId }) => (
  <div className="sk-tabstrip">
    {sessions.map((s) => (
      <div key={s.id} className={`sk-tab ${s.id === activeId ? 'active' : ''}`}>
        <StatusDot status={s.status} />
        <HarnessChip harness={s.harness} />
        <span className="tab-name">{s.name}</span>
        {s.badge > 0 && <span className="tab-badge">{s.badge}</span>}
      </div>
    ))}
    <div className="sk-tab-newbtn">+</div>
  </div>
);

const StatusBar = ({ session, urgent }) => (
  <div className="sk-statusbar">
    <span className="seg">
      <HarnessChip harness={session.harness} size={10} />
      <span>{HARNESS[session.harness].name}</span>
    </span>
    <span className="seg"><span className={`dot-tiny st-${session.status}`} />{session.status}</span>
    <span className="seg">{session.branch}</span>
    <span className="seg">{session.model}</span>
    <span className="seg">{session.tokens} tok</span>
    <span className="spacer" />
    {urgent && (
      <span className="seg urgent">
        <span className="dot-tiny st-waiting" />
        {urgent}
      </span>
    )}
    <span className="seg">utf-8 · LF</span>
  </div>
);

const HarnessPane = ({ session, content }) => (
  <div className="sk-harness">
    <div className="sk-harness-head">
      <HarnessChip harness={session.harness} size={14} />
      <span className="sk-harness-name"><span className="accent">{HARNESS[session.harness].name}</span></span>
      <span className="sk-harness-meta">
        <span>{session.model}</span>
        <span>{session.branch}</span>
      </span>
    </div>
    <div className="sk-harness-body">{content}</div>
    <div className="sk-composer">
      <div className="row"><span className="caret" /><span>reply to {HARNESS[session.harness].name}…</span></div>
      <div className="hint">
        <span><span className="key">⏎</span> send</span>
        <span><span className="key">⇧⏎</span> newline</span>
        <span><span className="key">@</span> file</span>
        <span><span className="key">⌘K</span> sessions</span>
      </div>
    </div>
  </div>
);

const FileTree = ({ items = TREE }) => (
  <div className="sk-tree">
    {items.map((n, i) => (
      <div key={i} className={`sk-tree-row ${n.kind} ${n.touched ? 'touched' : ''} ${n.active ? 'active' : ''}`}>
        {Array.from({ length: n.depth }).map((_, k) => <span key={k} className="indent" />)}
        <span className="icon">{n.kind === 'dir' ? (n.open ? '▾' : '▸') : '·'}</span>
        <span>{n.name}</span>
        {n.touched && <span className="badge">{n.touched}</span>}
      </div>
    ))}
  </div>
);

const DiffEditor = () => (
  <div className="sk-editor">
    <div className="sk-editor-head">
      <span className="path">src/fs/watcher.rs</span>
      <span style={{ color: 'var(--fg-3)' }}>· modified just now</span>
      <span className="delta-add">+5</span>
      <span className="delta-del">−1</span>
    </div>
    <div className="sk-code">
      {DIFF_LINES.map((l, i) => (
        <div key={i} className={`sk-line ${l.kind === 'add' ? 'add' : l.kind === 'del' ? 'del' : ''}`}>
          <div className="gutter">
            <span className="ln">{l.n1}</span>
            <span className="ln">{l.n2}</span>
          </div>
          <span className="marker">{l.kind === 'add' ? '+' : l.kind === 'del' ? '−' : ''}</span>
          <span className="src">{l.src}</span>
        </div>
      ))}
    </div>
  </div>
);

const TodoCard = () => (
  <div className="sk-todo">
    {TODO_LIST.map((t, i) => (
      <div key={i} className={`row ${t.state}`}>
        <span className="box">{t.state === 'done' ? '✓' : t.state === 'now' ? '◆' : ''}</span>
        <span>{t.text}</span>
      </div>
    ))}
  </div>
);

Object.assign(window, {
  HARNESS, HarnessChip, StatusDot, SESSIONS,
  Titlebar, TabStrip, StatusBar,
  HarnessPane, FileTree, DiffEditor, TodoCard,
  CHAT_S2, TREE, DIFF_LINES, TODO_LIST,
});
