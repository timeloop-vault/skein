// Live Context — Card components.
// Each card has identical chrome (head + body); the body is bespoke.

// ─── Room subtitle (away_summary) ────────────────────────────────
const RoomSubtitle = ({ text, age, empty = false }) => (
  <div className={`lc-subtitle ${empty ? 'is-empty' : ''}`}>
    <span className="glyph">{empty ? 'IDLE' : 'AT'}</span>
    <span className="text">{text}</span>
    <span className="meta">{age}</span>
  </div>
);

// ─── Card chrome ────────────────────────────────────────────────
const CardHead = ({ label, meta, collapsed, onToggle }) => (
  <div className="lc-card-head" onClick={onToggle}>
    <span className="chev">▾</span>
    <span className="label">{label}</span>
    <span className="meta">{meta}</span>
  </div>
);

const Card = ({ label, meta, flex = 1, collapsed = false, onToggle, children }) => (
  <div className={`lc-card ${collapsed ? 'collapsed' : ''}`} style={{ flex }}>
    <CardHead label={label} meta={meta} collapsed={collapsed} onToggle={onToggle} />
    <div className="lc-card-body">{children}</div>
  </div>
);

// ─── Diff card ──────────────────────────────────────────────────
// Auto-focus rule: latest file-touching tool call from the FOCUSED
// harness. Other harness edits on the same file produce an
// ambient "flicker" pill that the user can click to swap.

// Ambient signal lives ONLY in the tab bar (the thin pulsing accent line on a
// flicker'd tab). The previous floating pill was redundant; the tab bar shows
// the same information, spatially attached to the file it's about, and scales
// to N flickering tabs without piling up.

const DiffCard = ({
  tabs = [],         // [{ file, adds, dels, harness, active, flicker }]
  body,              // JSX for the diff body
  flex = 1,
  collapsed,
  onToggle,
}) => {
  const focused = tabs.find(t => t.active);
  return (
    <Card label="Diff" flex={flex} collapsed={collapsed} onToggle={onToggle}
      meta={(
        <>
          <span><span className="pulse" /> auto-follow</span>
          {focused && <span style={{ color: 'var(--fg-3)' }}>· focused: {focused.harness}</span>}
        </>
      )}>
      <div className="lc-diff">
        {tabs.length > 0 && (
          <div className="lc-diff-tabs">
            {tabs.map((t, i) => (
              <div key={i} className={`lc-diff-tab ${t.active ? 'active' : ''} ${t.flicker ? 'flicker' : ''}`}>
                <Chip kind={t.harness} size={9} />
                <span>{t.file}</span>
                {t.adds != null && <span className="delta-add">+{t.adds}</span>}
                {t.dels != null && <span className="delta-del">−{t.dels}</span>}
              </div>
            ))}
          </div>
        )}
        <div className="lc-diff-body">{body}</div>
      </div>
    </Card>
  );
};

// ─── Plan card ──────────────────────────────────────────────────
// Grouped by harness — sub-lists headed by the harness chip.
// opencode "now" rows are inferred (no in_progress state), marked italic.

const PlanItem = ({ status, text, priority, inferred }) => (
  <div className={`lc-plan-row ${status} ${inferred ? 'inferred' : ''}`}>
    <span className="box">{status === 'done' ? '✓' : ''}</span>
    <span className="text">{text}</span>
    {priority && <span className={`pri ${priority === 'high' ? 'high' : 'med'}`}>{priority}</span>}
  </div>
);

const PlanGroup = ({ harness, items }) => {
  const done = items.filter(i => i.status === 'done').length;
  const total = items.length;
  return (
    <div className="lc-plan-group">
      <div className="lc-plan-grouphead">
        <Chip kind={harness} size={10} />
        <span>{HARNESS_META[harness].name}</span>
        <span className="count">{done}/{total}</span>
      </div>
      {items.map((it, i) => <PlanItem key={i} {...it} />)}
    </div>
  );
};

const PlanCard = ({ groups = [], flex = 1, collapsed, onToggle }) => {
  const totalNow = groups.reduce((n, g) => n + g.items.filter(i => i.status === 'now').length, 0);
  const totalDone = groups.reduce((n, g) => n + g.items.filter(i => i.status === 'done').length, 0);
  const totalAll = groups.reduce((n, g) => n + g.items.length, 0);
  return (
    <Card label="Plan" flex={flex} collapsed={collapsed} onToggle={onToggle}
      meta={(
        <>
          <span>{totalNow} now</span>
          <span style={{ color: 'var(--fg-3)' }}>· {totalDone}/{totalAll}</span>
        </>
      )}>
      <div className="lc-plan">
        {groups.map((g, i) => <PlanGroup key={i} {...g} />)}
        {groups.length === 0 && (
          <div className="lc-empty"><div className="lc-empty-inner">
            <div className="big">·</div>
            no plan items yet — agents will populate this as they work
          </div></div>
        )}
      </div>
    </Card>
  );
};

// ─── Activity card ──────────────────────────────────────────────
// Renders an array of "items" which are either raw row elements
// (turn separators, cost rows, etc.) or row descriptors.

const ActivityCard = ({ children, flex = 1, collapsed, onToggle, totals }) => (
  <Card label="Activity" flex={flex} collapsed={collapsed} onToggle={onToggle}
    meta={(
      <>
        {totals?.events != null && <span>{totals.events} events</span>}
        {totals?.cost && <span style={{ color: 'var(--fg-3)' }}>· ${totals.cost} · {totals.tokens}</span>}
        {totals?.idleFor && <span style={{ color: 'var(--fg-3)' }}>· idle {totals.idleFor}</span>}
      </>
    )}>
    <div className="lc-activity">
      {children}
    </div>
  </Card>
);

Object.assign(window, {
  RoomSubtitle, Card, DiffCard, PlanCard, PlanGroup, PlanItem, ActivityCard,
});
