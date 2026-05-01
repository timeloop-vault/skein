// Top-level: Design canvas with three layout variations + ambient signal explorations.

const { useState } = React;

// Sub-pane for variations B (stacked context cards in the right pane)
const ContextStack = () => (
  <div className="sk-context-stack">
    <div className="sk-context-card h-2">
      <div className="sk-context-head">
        <span className="live" />
        <span className="label">Diff · src/fs/watcher.rs</span>
        <span className="meta">+5 −1 · just now</span>
      </div>
      <div className="sk-context-body" style={{ overflow: 'hidden' }}>
        <DiffEditor />
      </div>
    </div>
    <div className="sk-context-card h-1">
      <div className="sk-context-head">
        <span className="label">Plan</span>
        <span className="meta">4 of 6 done</span>
      </div>
      <div className="sk-context-body">
        <TodoCard />
      </div>
    </div>
    <div className="sk-context-card h-1">
      <div className="sk-context-head">
        <span className="label">Worktree · skein/.sessions/agent-loop-v0</span>
        <span className="meta">2 modified</span>
      </div>
      <div className="sk-context-body">
        <FileTree />
      </div>
    </div>
  </div>
);

// ── Variation A — 2-pane: harness | (tree + editor). The baseline.
const VariationA = ({ theme }) => {
  const active = SESSIONS[1]; // the BYOH waiting session — most interesting populated state
  return (
    <div className={`sk-app sk-${theme}`}>
      <Titlebar />
      <TabStrip sessions={SESSIONS} activeId={active.id} />
      <div className="sk-workspace">
        <HarnessPane session={active} content={CHAT_S2} />
        <div className="sk-right">
          <div className="sk-right-tabs">
            <div className="sk-right-tab active">Diff <span className="num">2</span></div>
            <div className="sk-right-tab">Files</div>
            <div className="sk-right-tab">Plan</div>
            <div className="sk-right-meta">
              <span>auto-follow agent</span>
              <span style={{ color: 'var(--accent)' }}>●</span>
            </div>
          </div>
          <div className="sk-right-body">
            <FileTree />
            <DiffEditor />
          </div>
        </div>
      </div>
      <StatusBar session={active} urgent="s3 · pim-search permission needed" />
    </div>
  );
};

// ── Variation B — 2-pane, but the right pane is a stack of context cards
//                  (diff + plan + tree). Closer to "the right pane retools
//                  itself for whatever the agent is doing".
const VariationB = ({ theme }) => {
  const active = SESSIONS[1];
  return (
    <div className={`sk-app sk-${theme}`}>
      <Titlebar />
      <TabStrip sessions={SESSIONS} activeId={active.id} />
      <div className="sk-workspace">
        <HarnessPane session={active} content={CHAT_S2} />
        <div className="sk-right">
          <div className="sk-right-tabs">
            <div className="sk-right-tab active">Live context</div>
            <div className="sk-right-tab">Files</div>
            <div className="sk-right-tab">Diff</div>
            <div className="sk-right-meta">
              <span>3 panels</span>
            </div>
          </div>
          <ContextStack />
        </div>
      </div>
      <StatusBar session={active} urgent="s3 · pim-search permission needed" />
    </div>
  );
};

// ── Variation D — 3-pane fallback: sidebar list + harness + code.
//                  Shows what we're trading for tabs-on-top.
const VariationD = ({ theme }) => {
  const active = SESSIONS[1];
  return (
    <div className={`sk-app sk-${theme}`}>
      <Titlebar />
      <div className="sk-workspace">
        <div className="sk-sidebar">
          <div className="sk-sidebar-head">
            <span>Sessions · 5</span>
            <span className="add">+</span>
          </div>
          <div className="sk-sidebar-list">
            {SESSIONS.map((s) => (
              <div key={s.id} className={`sk-side-row ${s.id === active.id ? 'active' : ''}`}>
                <div style={{ display: 'flex', flexDirection: 'column', flex: 1, minWidth: 0 }}>
                  <div className="row1">
                    <StatusDot status={s.status} />
                    <HarnessChip harness={s.harness} />
                    <span className="name">{s.name}</span>
                    {s.badge > 0 && <span className="tab-badge">{s.badge}</span>}
                  </div>
                  <div className="row2">
                    <span>{s.branch}</span>
                    <span>·</span>
                    <span>{s.tokens}</span>
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
        <HarnessPane session={active} content={CHAT_S2} />
        <div className="sk-right">
          <div className="sk-right-tabs">
            <div className="sk-right-tab active">Diff <span className="num">2</span></div>
            <div className="sk-right-tab">Files</div>
            <div className="sk-right-meta"><span>auto-follow</span></div>
          </div>
          <div className="sk-right-body">
            <FileTree />
            <DiffEditor />
          </div>
        </div>
      </div>
      <StatusBar session={active} urgent="s3 · pim-search permission needed" />
    </div>
  );
};

// ── Status indicator explorations ────────────────────────────────

const SI_DotsAndPills = ({ theme }) => (
  <div className={`sk-app sk-${theme}`} style={{ padding: 0 }}>
    <div className="si-frame">
      <div className="si-title">A · Dot + status word, monochrome</div>
      {SESSIONS.map((s) => (
        <div key={s.id} className="si-row">
          <StatusDot status={s.status} />
          <HarnessChip harness={s.harness} />
          <span className="name">{s.name}</span>
          <span style={{ fontFamily: 'var(--sk-mono)', fontSize: 10, color: 'var(--fg-2)' }}>{s.status}</span>
        </div>
      ))}
    </div>
  </div>
);

const SI_PillsLoud = ({ theme }) => (
  <div className={`sk-app sk-${theme}`} style={{ padding: 0 }}>
    <div className="si-frame">
      <div className="si-title">B · Filled pills, color-loud</div>
      {SESSIONS.map((s) => {
        const styleMap = {
          running: { bg: 'color-mix(in srgb, var(--ok) 18%, transparent)',      fg: 'var(--ok)',      bd: 'color-mix(in srgb, var(--ok) 50%, var(--line))' },
          waiting: { bg: 'color-mix(in srgb, var(--waiting) 22%, transparent)', fg: 'var(--waiting)', bd: 'color-mix(in srgb, var(--waiting) 60%, var(--line))' },
          idle:    { bg: 'transparent',                                          fg: 'var(--fg-3)',    bd: 'var(--line)' },
          error:   { bg: 'color-mix(in srgb, var(--err) 22%, transparent)',     fg: 'var(--err)',     bd: 'color-mix(in srgb, var(--err) 50%, var(--line))' },
        };
        const st = styleMap[s.status];
        return (
          <div key={s.id} className="si-row">
            <HarnessChip harness={s.harness} />
            <span className="name">{s.name}</span>
            <span style={{
              padding: '2px 8px', borderRadius: 999,
              background: st.bg, color: st.fg, border: `1px solid ${st.bd}`,
              fontFamily: 'var(--sk-mono)', fontSize: 9.5, letterSpacing: '0.06em', textTransform: 'uppercase',
            }}>{s.status}</span>
          </div>
        );
      })}
    </div>
  </div>
);

const SI_AmbientBars = ({ theme }) => (
  <div className={`sk-app sk-${theme}`} style={{ padding: 0 }}>
    <div className="si-frame">
      <div className="si-title">C · Ambient — left bar, no labels</div>
      {SESSIONS.map((s) => {
        const colorMap = { running: 'var(--ok)', waiting: 'var(--waiting)', idle: 'var(--line-strong)', error: 'var(--err)' };
        return (
          <div key={s.id} className="si-row" style={{ position: 'relative', paddingLeft: 14 }}>
            <span style={{
              position: 'absolute', left: 0, top: 4, bottom: 4, width: 3,
              background: colorMap[s.status],
              borderRadius: 2,
              boxShadow: s.status === 'waiting' ? '0 0 6px var(--waiting)' : 'none',
              animation: s.status === 'waiting' ? 'sk-pulse 1.6s ease-in-out infinite' : 'none',
            }} />
            <HarnessChip harness={s.harness} />
            <span className="name">{s.name}</span>
            {s.badge > 0 && <span className="tab-badge">{s.badge}</span>}
          </div>
        );
      })}
    </div>
  </div>
);

// ── Tab-strip explorations ────────────────────────────────────────

const TabStyleA = ({ theme }) => (
  <div className={`sk-app sk-${theme}`}>
    <Titlebar />
    <TabStrip sessions={SESSIONS} activeId="s1" />
    <div className="si-frame" style={{ margin: 14, fontFamily: 'var(--sk-mono)', fontSize: 10, color: 'var(--fg-2)' }}>
      A · current style — top bar, status dot + harness chip + name + badge, accent line on active
    </div>
  </div>
);

const TabStyleB = ({ theme }) => (
  <div className={`sk-app sk-${theme}`}>
    <Titlebar />
    <div className="sk-tabstrip" style={{ height: 44, flexBasis: 44 }}>
      {SESSIONS.map((s, i) => (
        <div key={s.id} className={`sk-tab ${i === 0 ? 'active' : ''}`} style={{ height: 44, alignItems: 'center', flexDirection: 'column', gap: 2, justifyContent: 'center', padding: '4px 14px' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <HarnessChip harness={s.harness} />
            <span className="tab-name" style={{ fontSize: 12 }}>{s.name.split(' · ')[1] ?? s.name}</span>
            {s.badge > 0 && <span className="tab-badge">{s.badge}</span>}
          </div>
          <div style={{ fontFamily: 'var(--sk-mono)', fontSize: 9, color: 'var(--fg-3)', display: 'flex', gap: 6, alignItems: 'center' }}>
            <StatusDot status={s.status} size={5} />
            <span>{s.branch}</span>
          </div>
        </div>
      ))}
      <div className="sk-tab-newbtn" style={{ height: 44 }}>+</div>
    </div>
    <div className="si-frame" style={{ margin: 14, fontFamily: 'var(--sk-mono)', fontSize: 10, color: 'var(--fg-2)' }}>
      B · two-line tabs — branch becomes part of identity
    </div>
  </div>
);

const TabStyleC = ({ theme }) => (
  <div className={`sk-app sk-${theme}`}>
    <Titlebar />
    <div className="sk-tabstrip" style={{ background: 'var(--bg-1)', borderBottom: '1px solid var(--line)' }}>
      {SESSIONS.map((s, i) => (
        <div key={s.id}
          className="sk-tab"
          style={{
            background: i === 0 ? 'var(--bg-2)' : 'transparent',
            borderRadius: '6px 6px 0 0',
            margin: '4px 0 0 4px',
            border: i === 0 ? '1px solid var(--line)' : '1px solid transparent',
            borderBottom: i === 0 ? '1px solid var(--bg-2)' : '1px solid transparent',
            height: 32,
            position: 'relative',
            top: i === 0 ? 1 : 0,
            color: i === 0 ? 'var(--fg-0)' : 'var(--fg-2)',
          }}>
          {/* harness as colored left-edge instead of chip */}
          <span style={{ position: 'absolute', left: 0, top: 0, bottom: 0, width: 3, background: HARNESS[s.harness].bg.includes('claude') ? '#c96442' : HARNESS[s.harness].bg.includes('opencode') ? '#5b8a72' : HARNESS[s.harness].bg.includes('copilot') ? '#4a6b9a' : '#8a7a4a', borderRadius: '4px 0 0 0' }} />
          <StatusDot status={s.status} />
          <span className="tab-name">{s.name}</span>
          {s.badge > 0 && <span className="tab-badge">{s.badge}</span>}
        </div>
      ))}
      <div className="sk-tab-newbtn">+</div>
    </div>
    <div className="si-frame" style={{ margin: 14, fontFamily: 'var(--sk-mono)', fontSize: 10, color: 'var(--fg-2)' }}>
      C · Chrome-style notched tabs, harness shown as colored left edge
    </div>
  </div>
);

// ── Root ──────────────────────────────────────────────────────────

const SCALE_W = 1280;
const SCALE_H = 800;
const SI_W = 380;
const SI_H = 360;

function App() {
  return (
    <div className="canvas-host">
      <DesignCanvas>
        <DCSection
          id="layouts"
          title="Layout · the main workspace"
          subtitle="Same scenario in each — 5 sessions, s2 (BYOH) is waiting on a bash permission."
        >
          <DCArtboard id="A-dark" label="A · 2-pane (tree + editor) · dark" width={SCALE_W} height={SCALE_H}>
            <VariationA theme="dark" />
          </DCArtboard>
          <DCArtboard id="A-light" label="A · 2-pane · light" width={SCALE_W} height={SCALE_H}>
            <VariationA theme="light" />
          </DCArtboard>
          <DCArtboard id="B-dark" label="B · context-stack right pane · dark" width={SCALE_W} height={SCALE_H}>
            <VariationB theme="dark" />
          </DCArtboard>
          <DCArtboard id="D-dark" label="D · 3-pane fallback (sidebar) · dark" width={SCALE_W} height={SCALE_H}>
            <VariationD theme="dark" />
          </DCArtboard>
        </DCSection>

        <DCSection
          id="tabs"
          title="Session tabs · variations"
          subtitle="The thing you'll look at all day."
        >
          <DCArtboard id="tab-a" label="A · single-line, status + chip" width={760} height={220}>
            <TabStyleA theme="dark" />
          </DCArtboard>
          <DCArtboard id="tab-b" label="B · two-line, branch in tab" width={760} height={220}>
            <TabStyleB theme="dark" />
          </DCArtboard>
          <DCArtboard id="tab-c" label="C · notched tabs, harness as edge" width={760} height={220}>
            <TabStyleC theme="dark" />
          </DCArtboard>
        </DCSection>

        <DCSection
          id="signals"
          title="Status indicators · ambient signal"
          subtitle="How loud should a non-active session be allowed to be?"
        >
          <DCArtboard id="si-a" label="A · dot + word" width={SI_W} height={SI_H}>
            <SI_DotsAndPills theme="dark" />
          </DCArtboard>
          <DCArtboard id="si-b" label="B · filled pills" width={SI_W} height={SI_H}>
            <SI_PillsLoud theme="dark" />
          </DCArtboard>
          <DCArtboard id="si-c" label="C · ambient bar" width={SI_W} height={SI_H}>
            <SI_AmbientBars theme="dark" />
          </DCArtboard>
          <DCArtboard id="si-a-light" label="A · dot + word · light" width={SI_W} height={SI_H}>
            <SI_DotsAndPills theme="light" />
          </DCArtboard>
        </DCSection>

        <DCPostIt x={40} y={40} width={260}>
          <strong>Skein · design exploration</strong>
          <br /><br />
          One window · session-first · tabs across the top · harness is part of identity (colored chip).
          <br /><br />
          Compare layouts in the top row, then tab + signal explorations below. Open any artboard fullscreen with the expand button (top-right).
          <br /><br />
          Once you pick a direction we'll promote it to an interactive prototype with tweaks for the rest.
        </DCPostIt>
      </DesignCanvas>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById('root')).render(<App />);
