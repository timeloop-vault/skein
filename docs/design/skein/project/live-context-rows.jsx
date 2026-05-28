// Live Context — Row taxonomy.
// One small component per event kind. Every row goes through <Row /> which
// owns the grid (time | chip | glyph | gist | meta) and the kind-class.
// Specialized rows are thin wrappers that compute the gist for that kind.
//
// Style rule of thumb:
//   - mono everywhere
//   - the GIST is the only thing that's allowed to be wide / mixed
//   - meta is always small monochrome dim
//   - everything fits one line by default; preview block is optional

const HARNESS_META = {
  claude:   { label: 'CC',  name: 'Claude Code', cls: 'h-claude' },
  opencode: { label: 'oc',  name: 'opencode',    cls: 'h-opencode' },
  copilot:  { label: 'gh',  name: 'Copilot',     cls: 'h-copilot' },
  byoh:     { label: 'sk',  name: 'BYOH',        cls: 'h-byoh' },
};

const Chip = ({ kind, size = 11 }) => {
  const k = HARNESS_META[kind] || HARNESS_META.byoh;
  return (
    <span className={`h-chip ${k.cls}`}
      style={{ width: size, height: size, fontSize: Math.round(size * 0.66), borderRadius: 2.5 }}
      title={k.name}>{k.label}</span>
  );
};

// Glyphs are one mono character per kind — recognisable at a glance,
// not chatty.
const GLYPH = {
  edit:    '✎',
  write:   '✎',
  read:    '◌',
  grep:    '⌕',
  glob:    '⌕',
  bash:    '$',
  task:    '◇',     // task_create or task_update
  todowrite:'☰',
  ask:     '?',
  agent:   '✦',
  pr:      '⤴',
  error:   '✕',
  queue:   '⏵',
  userfile:'✋',
  slash:   '/',
  compact: '⤓',
  cost:    '$',
  perm:    '⏵',
  burst:   '▸',
};

// Format a relative time for stamps. We mock these with hard-coded strings
// because the canvas is a static reference; the interactive promote will
// compute them.
const Time = ({ stamp }) => <span className="time">{stamp}</span>;

// The Row is the only thing that owns the grid layout. Other row components
// pass children INTO this so they can keep their kind-specific gist clean.
const Row = ({ kind, harness, time, children, right, onClick, extra, glyph }) => (
  <div className={`lc-row k-${kind}`} onClick={onClick} style={onClick ? { cursor: 'pointer' } : null}>
    <Time stamp={time} />
    <span className="by">{harness && <Chip kind={harness} />}</span>
    <span className="gist">
      <span className="glyph" aria-hidden style={{ display: 'inline-block', width: 12, textAlign: 'center', marginRight: 6 }}>
        {glyph ?? GLYPH[kind] ?? '·'}
      </span>
      {children}
    </span>
    <span className="right">{right}</span>
    {extra}
  </div>
);

// ─── Row variants ────────────────────────────────────────────────

// edit / write — file path + (+N / -N)
const EditRow = ({ harness, time, file, adds, dels, kind = 'edit' }) => (
  <Row kind={kind} harness={harness} time={time}
    right={<>{adds != null && <span className="delta-add">+{adds}</span>} {dels != null && <span className="delta-del">−{dels}</span>}</>}>
    <span className="tool">{kind === 'write' ? 'write' : 'edit'}</span>{' '}
    <span className="target">{file}</span>
  </Row>
);

// read — file path, no diff. Result is silent (just present).
const ReadRow = ({ harness, time, file, lines }) => (
  <Row kind="read" harness={harness} time={time}
    right={<span className="dim">{lines} ln</span>}>
    <span className="tool">read</span>{' '}
    <span className="target">{file}</span>
  </Row>
);

// grep / glob
const SearchRow = ({ harness, time, pattern, matches, kind = 'grep' }) => (
  <Row kind={kind} harness={harness} time={time}
    right={<span className="dim">{matches} {matches === 1 ? 'match' : 'matches'}</span>}>
    <span className="tool">{kind}</span>{' '}
    <span className="arg">{pattern}</span>
  </Row>
);

// bash — title (claude supplies it) or command itself if not
const BashRow = ({ harness, time, title, command, ok = true, ms, expanded = false, output }) => (
  <Row kind="bash" harness={harness} time={time}
    right={<>{ms && <span className="dim">{ms}ms</span>}</>}
    extra={expanded && output ? (
      <div className="lc-row-preview">
        <div className="head">
          <span>stdout</span>
          <span className="size">{output.length > 200 ? `${(output.length/1024).toFixed(1)} KB · truncated` : `${output.length} chars`}</span>
        </div>
        <pre style={{ margin: 0, fontFamily: 'inherit', fontSize: 'inherit', whiteSpace: 'pre-wrap', overflow: 'hidden' }}>{output}</pre>
      </div>
    ) : null}>
    <span className="tool">bash</span>{' '}
    <span className="target">{title ?? command}</span>
    {!ok && <span className="err-text"> · exit 1</span>}
    {!title && command && <span className="dim"> · {command}</span>}
  </Row>
);

// task_create — "+ <text>" with status pending
// task_update — "→ in_progress" or "✓ completed"
const TaskRow = ({ harness, time, op, text, from, to }) => {
  const verb = op === 'create' ? '+ task' : 'update';
  const transition = from && to ? `${from} → ${to}` : to;
  return (
    <Row kind="task" harness={harness} time={time}
      right={op !== 'create' && <span className="dim">{transition}</span>}>
      <span className="tool">{verb}</span>{' '}
      <span className="target">{text}</span>
      {op === 'create' && <span className="pill">pending</span>}
    </Row>
  );
};

// todowrite — single row "8 todos"; expandable to the list
const TodoWriteRow = ({ harness, time, count, items, expanded = false }) => (
  <Row kind="todowrite" harness={harness} time={time}
    right={<span className="dim">replaced plan</span>}
    extra={expanded && items ? (
      <div className="lc-row-preview tall">
        <div className="head"><span>todos</span><span className="size">{count} items</span></div>
        {items.map((it, i) => (
          <div key={i} style={{ display: 'flex', gap: 8, padding: '1px 0' }}>
            <span style={{ width: 11, color: it.status === 'completed' ? 'var(--ok)' : 'var(--fg-3)' }}>{it.status === 'completed' ? '✓' : '·'}</span>
            <span style={{ flex: 1, color: it.status === 'completed' ? 'var(--fg-3)' : 'var(--fg-1)' }}>{it.text}</span>
            {it.priority && <span className={`pill ${it.priority === 'high' ? 'high' : 'med'}`} style={{ alignSelf: 'flex-start', fontFamily: 'var(--sk-mono)', fontSize: '8.5px' }}>{it.priority}</span>}
          </div>
        ))}
      </div>
    ) : null}>
    <span className="tool">todowrite</span>{' '}
    <span className="target">{count} todos</span>
  </Row>
);

// ask_user_question — question + chosen answer
const AskRow = ({ harness, time, question, chosen }) => (
  <Row kind="ask" harness={harness} time={time}
    right={<span className="dim">user chose</span>}>
    <span className="tool">asked</span>{' '}
    <span className="target">{question}</span>{' '}
    <span className="arg">→ {chosen}</span>
  </Row>
);

// agent / task (sub-agent invocation) — clickable, opens inspector
const AgentRow = ({ harness, time, title, ms, tokens, toolCount, onOpen, status = 'completed' }) => (
  <Row kind="agent" harness={harness} time={time} onClick={onOpen}
    right={<><span className="dim">{ms}</span></>}>
    <span className="tool">sub-agent</span>{' '}
    <span className="target">{title}</span>
    <span className="pill">{toolCount} calls · {tokens}</span>
    {status === 'completed'
      ? <span className="dim"> · click to inspect</span>
      : <span className="dim"> · running…</span>}
  </Row>
);

// PR opened — pinned-ish, monochrome with subtle accent
const PrRow = ({ harness, time, number, repo, title }) => (
  <Row kind="pr" harness={harness} time={time}
    right={<span className="dim">{repo}</span>}>
    <span className="tool">opened</span>{' '}
    <span className="target">PR #{number}</span>{' '}
    <span className="dim">— {title}</span>
  </Row>
);

// queued — user typed while busy
const QueueRow = ({ harness, time, text }) => (
  <Row kind="queue" harness={harness} time={time}
    right={<span className="dim">queued</span>}>
    <span className="tool">queue</span>{' '}
    <span className="target">"{text}"</span>
  </Row>
);

// user edited file outside the harness — distinct
const UserFileRow = ({ harness, time, file, hunk }) => (
  <Row kind="userfile" harness={harness} time={time}
    right={<span className="dim">user edited</span>}
    extra={hunk ? (
      <div className="lc-row-preview">
        <div className="head"><span>diff hunk</span></div>
        <pre style={{ margin: 0, fontFamily: 'inherit', fontSize: 'inherit', whiteSpace: 'pre' }}>{hunk}</pre>
      </div>
    ) : null}>
    <span className="tool">noticed</span>{' '}
    <span className="target">{file}</span>{' '}
    <span className="dim">edited outside</span>
  </Row>
);

// slash command
const SlashRow = ({ harness, time, name, output }) => (
  <Row kind="slash" harness={harness} time={time}
    right={output && <span className="dim">{output.length}c</span>}>
    <span className="tool">slash</span>{' '}
    <span className="target">/{name}</span>
  </Row>
);

// context compaction (opencode)
const CompactRow = ({ harness, time, before, after }) => (
  <Row kind="compact" harness={harness} time={time}
    right={<span className="dim">{before} → {after}</span>}>
    <span className="tool">compacted context</span>
  </Row>
);

// turn cost — small hair-line row
const CostRow = ({ harness, time, tokens, usd, ms }) => (
  <Row kind="cost" harness={harness} time={time}
    right={<span className="dim">{ms}</span>}>
    <span className="tool">turn</span>{' '}
    <span className="dim">{tokens} tok</span>{' '}
    <span className="dim">· ${usd}</span>
  </Row>
);

// API error / retry — with countdown
const ApiErrorRow = ({ harness, time, status, retryIn, attempt, message }) => (
  <Row kind="error" harness={harness} time={time}
    right={<span className="dim">attempt {attempt}</span>}
    extra={(
      <div className="lc-row-preview api-error">
        <div className="head">
          <span>{message}</span>
          <span className="size">{status}</span>
        </div>
        <div style={{ color: 'var(--fg-2)' }}>retrying in {retryIn}s — chat is paused, no input lost</div>
      </div>
    )}>
    <span className="tool err-text">api error</span>{' '}
    <span className="target">{status}</span>
  </Row>
);

// tool error (non-API) — same shape, no countdown
const ToolErrorRow = ({ harness, time, tool, message }) => (
  <Row kind="error" harness={harness} time={time}>
    <span className="tool err-text">{tool}</span>{' '}
    <span className="target">{message}</span>
  </Row>
);

// Passive permission notice — buttons live in the harness terminal where the
// agent actually paused. This row is a clickable deep-link only; the activity
// feed stays an honest log instead of growing interaction surfaces.
const PermissionRow = ({ harness, time, command, onJump }) => (
  <Row kind="perm" harness={harness} time={time} onClick={onJump}
    right={<span className="dim">awaiting you</span>}>
    <span className="tool">permission</span>{' '}
    <span className="target">bash · {command}</span>
    <span className="dim"> · jump to harness ↗</span>
  </Row>
);

// burst — collapsed "Edit ×12 in app/src/"  with cumulative +N/−N
const BurstRow = ({ harness, time, count, tool, scope, live, adds, dels, errs, onExpand }) => (
  <Row kind="burst" harness={harness} time={time} onClick={onExpand}
    right={(
      <>
        {adds != null && <span className="delta-add">+{adds}</span>}
        {' '}
        {dels != null && <span className="delta-del">−{dels}</span>}
        {errs ? <span className="dim"> · {errs} err</span> : null}
        {' '}
        <span className="dim">{live ? 'in 28s' : 'in 4m'}</span>
      </>
    )}>
    <span className="tool">{tool} ×{count}</span>{' '}
    <span className="target">{scope}</span>
    <span className="dim"> · click to expand</span>
    {live && <span className="pill" style={{ background: 'color-mix(in srgb, var(--accent) 18%, transparent)', color: 'var(--accent)' }}>live</span>}
  </Row>
);

// user_prompt — the user's typed message to the agent. No chip (user
// isn't a harness); distinct accent treatment so the feed has voice.
const UserPromptRow = ({ time, text, harness }) => (
  <div className="lc-row k-user">
    <Time stamp={time} />
    <span className="by">{harness && <Chip kind={harness} size={11} />}</span>
    <span className="gist">
      <span className="glyph" style={{ display: 'inline-block', width: 12, textAlign: 'center', marginRight: 6, color: 'var(--accent)' }}>›</span>
      <span className="tool">user</span>{' '}
      <span className="target" style={{ color: 'var(--fg-0)', fontWeight: 500 }}>{text}</span>
    </span>
    <span className="right" />
  </div>
);

// permission_mode — config transition (ask / always_for_session / always).
const PermissionModeRow = ({ harness, time, from, to }) => (
  <Row kind="perm-mode" harness={harness} time={time}
    right={<span className="dim">{from} → {to}</span>}>
    <span className="tool">permission mode</span>
  </Row>
);

// ai_title — the agent's self-set title for what it's working on.
// Subtle, dim, italic — context, not action.
const AiTitleRow = ({ harness, time, title }) => (
  <Row kind="title" harness={harness} time={time}
    right={<span className="dim">harness titled</span>}>
    <span className="tool">title</span>{' '}
    <span className="target" style={{ fontStyle: 'italic', color: 'var(--fg-1)' }}>"{title}"</span>
  </Row>
);

// bridge_status — connection up/down. ok / warn coloring.
const BridgeStatusRow = ({ harness, time, status, detail }) => (
  <Row kind={`bridge bridge-${status}`} harness={harness} time={time}
    right={detail && <span className="dim">{detail}</span>}>
    <span className="tool">bridge</span>{' '}
    <span className={status === 'connected' ? 'ok' : status === 'reconnecting' ? 'err-text' : 'err-text'}>
      {status}
    </span>
  </Row>
);

// Turn separator — a horizontal hair-line with timestamps
const TurnSep = ({ time, endTime, label }) => (
  <div className="lc-turn-sep">
    <span className="stamp">{label ?? `turn · ${time}`}</span>
    {endTime && <span className="right-stamp">{endTime}</span>}
  </div>
);

const TurnCost = ({ tokens, usd, ms }) => (
  <div className="lc-turn-cost">
    <span><span className="v">{tokens}</span> tok</span>
    <span><span className="v">${usd}</span></span>
    <span><span className="v">{ms}</span></span>
  </div>
);

// Tail — "live" indicator at the bottom of the activity card
const ActivityTail = ({ idle = false }) => (
  <div className={`lc-tail ${idle ? 'idle' : ''}`}>
    <span className="blinker" />
    <span>{idle ? 'idle' : 'tailing — new rows slide in'}</span>
  </div>
);

Object.assign(window, {
  Chip, Row, GLYPH, HARNESS_META,
  EditRow, ReadRow, SearchRow, BashRow, TaskRow, TodoWriteRow,
  AskRow, AgentRow, PrRow, QueueRow, UserFileRow, SlashRow,
  CompactRow, CostRow, ApiErrorRow, ToolErrorRow, PermissionRow,
  BurstRow, TurnSep, TurnCost, ActivityTail,
  UserPromptRow, PermissionModeRow, AiTitleRow, BridgeStatusRow,
});
