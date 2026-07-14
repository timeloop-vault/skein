/* ══════════════════════════════════════════════════════════════════════
   Files Pillar prototype — Skein shell chrome
   Files is now a HARNESS TYPE, picked from "+ harness" like a shell.
   HarnessTabs renders the tab row (incl. Files tabs + add menu); the
   column BODY is composed by the app (terminal / shell / file surface).
   ══════════════════════════════════════════════════════════════════════ */
const { useState: useStateShell, useRef: useRefShell, useEffect: useEffectShell } = React;

// ── Title bar ────────────────────────────────────────────────────────────────
function TitleBar({ onCog }) {
  return (
    <div className="sk-titlebar">
      <span className="sk-app-name"><span className="dot">●</span> skein</span>
      <span className="sk-titlebar-session">local · skein</span>
      <div className="sk-titlebar-actions">
        <button className="sk-cog-btn" title="Settings" onClick={onCog}>
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>
        </button>
      </div>
    </div>
  );
}

// ── Room tab strip ───────────────────────────────────────────────────────────
function RoomTabs() {
  return (
    <div className="sk-tabstrip">
      <div className="sk-tab">
        <div className="row-1">
          <span className="dot st-idle" data-state="idle"></span>
          <span className="name">local · orc-assistant</span>
          <span className="sk-tab-close">×</span>
        </div>
        <div className="row-2">
          <span>refactor/app-state-machine</span><span>·</span>
          <span style={{ display: "flex", gap: "2px", alignItems: "center" }}><span className="chip h-opencode" data-kind="opencode">oc</span><span className="chip h-byoh" data-kind="byoh">sh</span></span>
        </div>
      </div>
      <div className="sk-tab active">
        <div className="row-1">
          <span className="dot st-running" data-state="running"></span>
          <span className="name">local · skein</span>
          <span className="sk-tab-close">×</span>
        </div>
        <div className="row-2">
          <span>main</span><span>·</span>
          <span style={{ display: "flex", gap: "3px", alignItems: "center" }}><span className="chip h-claude" data-kind="claude">CC</span><span className="chip h-byoh" data-kind="byoh">sh</span><span className="chip h-byoh" data-kind="byoh">sh</span><span className="ht-files-glyph" style={{ fontSize: "10px" }}>◇</span></span>
        </div>
      </div>
      <div className="sk-tab">
        <div className="row-1">
          <span className="dot st-idle" data-state="idle"></span>
          <span className="name">local · dorc-579-impl</span>
          <span className="sk-tab-close">×</span>
        </div>
        <div className="row-2">
          <span style={{ display: "flex", gap: "2px" }}><span className="chip h-opencode" data-kind="opencode">oc</span></span>
        </div>
      </div>
      <div className="sk-tab-newbtn">+</div>
    </div>
  );
}

// ── "+ harness" — the shipped plain button, untouched. It opens the in-pane
//    harness PICKER (a card grid), exactly as in Skein Prototype. Files is one
//    more card in that grid — not a dropdown, not a direct add.
function AddHarness({ onAdd }) {
  return <div className="sk-harness-add" title="add a harness" onClick={() => onAdd()}>+ harness</div>;
}

// The kinds offered by the picker. First four are the shipped agent kinds
// (verbatim names/descriptions from Skein Prototype); Files is the new one.
const PICKER_KINDS = [
  { kind: "claude",   chip: "CC", cls: "h-claude",   name: "Claude Code", desc: "Anthropic. Direct API." },
  { kind: "opencode", chip: "oc", cls: "h-opencode", name: "opencode",    desc: "Local server, OSS." },
  { kind: "copilot",  chip: "gh", cls: "h-copilot",  name: "Copilot CLI", desc: "GitHub entitlement." },
  { kind: "byoh",     chip: "sk", cls: "h-byoh",     name: "Skein BYOH",  desc: "Built-in agent loop." },
  { kind: "files",    glyph: "◇",                    name: "Files",       desc: "Browse + edit the worktree." },
];

// In-pane picker shown in the harness column body when + harness is clicked.
function HarnessPicker({ onPick }) {
  return (
    <div className="sk-empty-harness">
      <h3>Add a harness</h3>
      <p>Pick an agent for this workspace. All harnesses see the same worktree.</p>
      <div className="sk-harness-grid">
        {PICKER_KINDS.map((k) => (
          <div key={k.kind} className="sk-harness-card" onClick={() => onPick(k.kind)}>
            <div className="head">
              {k.glyph
                ? <span className="hp-glyph">{k.glyph}</span>
                : <span className={"chip " + k.cls} data-kind={k.kind}>{k.chip}</span>}
              <span className="h-name">{k.name}</span>
            </div>
            <div className="h-desc">{k.desc}</div>
          </div>
        ))}
      </div>
    </div>
  );
}

// ── Harness tab row ──────────────────────────────────────────────────────────
function HarnessTabs({ harnesses, active, onSelect, onClose, onAdd, agentEditing, filesState, agentPath }) {
  const listRef = useRefShell();
  const activeTabRef = useRefShell();
  useEffectShell(() => {
    const list = listRef.current, el = activeTabRef.current;
    if (!list || !el) return;
    const elRight = el.offsetLeft + el.offsetWidth;
    if (elRight > list.scrollLeft + list.clientWidth) list.scrollLeft = elRight - list.clientWidth + 8;
    else if (el.offsetLeft < list.scrollLeft) list.scrollLeft = Math.max(0, el.offsetLeft - 8);
  }, [active, harnesses.length]);
  return (
    <div className="sk-harness-tabs">
      <div className="sk-harness-tablist" ref={listRef}>
      {harnesses.map((h) => {
        const isFiles = h.kind === "files";
        const pip = isFiles && agentEditing && (filesState[h.id]?.buffers || []).includes(agentPath) && active !== h.id;
        return (
          <div key={h.id} ref={h.id === active ? activeTabRef : null}
               className={"sk-harness-tab" + (h.id === active ? " active" : "") + (isFiles ? " files" : "")}
               onClick={() => onSelect(h.id)}>
            <span className={"dot st-" + (h.state || "idle")} data-state={h.state || "idle"}></span>
            {isFiles
              ? <span className="ht-files-glyph">◇</span>
              : <span className={"chip h-" + h.kind} data-kind={h.kind}>{h.kind === "claude" ? "CC" : h.kind === "opencode" ? "oc" : "sh"}</span>}
            <span className="ht-name">{h.name}</span>
            {pip && <span className="ht-livepip" title="an agent is editing a file open here"></span>}
            <span className="ht-x" onClick={(e) => { e.stopPropagation(); onClose(h.id); }}>×</span>
          </div>
        );
      })}
        <div className="sk-harness-meta"><span>skein · main</span></div>
      </div>
      <AddHarness onAdd={onAdd} />
    </div>
  );
}

// ── Terminal body (Claude Code PTY render) ───────────────────────────────────
function TerminalBody() {
  return (
    <div className="sk-term">
      {window.FP_TERM.map((ln, i) => {
        if (ln.c === "sp") return <span key={i} className="t-spacer"></span>;
        const cls = "t-line" + (ln.c === "bold" ? " t-bold" : ln.c === "dim" ? " t-dim" : "");
        return (
          <span key={i} className={cls}>
            {ln.c === "bullet" && <span className="t-bullet">● </span>}
            {ln.c === "cogit" && <span className="t-cogit">✻ </span>}
            {ln.t}
            {ln.mono && <span className="t-accent">{"`" + ln.mono + "`"}</span>}
            {ln.tail}
            {ln.thinking && <span className="blink"></span>}
          </span>
        );
      })}
      <span className="t-spacer"></span>
      <div className="t-inputblock"><span className="t-accent">›</span> <span className="t-cursorblock"></span></div>
    </div>
  );
}
function TerminalFoot() {
  return (
    <div className="sk-term-foot">
      <div className="row">
        <span className="mode">-- INSERT --</span>
        <span className="auto">▸▸ auto mode on</span>
        <span className="t-dim">(shift+tab to cycle)</span>
        <span className="slash">/rc</span>
      </div>
    </div>
  );
}
// ── Generic shell body (for added shells) ────────────────────────────────────
function ShellBody({ name }) {
  return (
    <div className="sk-term sk-shell-body">
      <span className="t-line t-dim">{name} — bash 5.2 · ~/git/private/skein</span>
      <span className="t-spacer"></span>
      <div className="t-inputblock"><span className="t-accent">$</span> <span className="t-cursorblock"></span></div>
    </div>
  );
}

// ── Live Context stack (right) ───────────────────────────────────────────────
function LcCard({ label, meta, flex, children }) {
  return (
    <div className="lc-card" style={{ flex }}>
      <div className="lc-card-head">
        <span className="chev">▾</span>
        <span className="label">{label}</span>
        <span className="meta">{meta}</span>
      </div>
      <div className="lc-card-body">{children}</div>
    </div>
  );
}

function LiveContext({ peek, onExpand, onOpenFileAtHunk }) {
  if (peek) {
    return (
      <div className="lc-peek" onClick={onExpand} title="Expand Live Context">
        <div className="lc-peek-label">LIVE&nbsp;CONTEXT</div>
        <div className="lc-peek-pips">
          <span className="dot st-running" data-state="running"></span>
          <span className="lc-peek-count">3</span>
          <span className="lc-peek-sub">diff</span>
          <span className="lc-peek-count">5</span>
          <span className="lc-peek-sub">act</span>
        </div>
        <div className="lc-peek-chev">‹</div>
      </div>
    );
  }
  return (
    <div className="sk-right">
      <div className="lc-pane">
        <div className="lc-subtitle">
          <span className="glyph">AT</span>
          <span className="text">Guarding the editor→PTY focus leak in LiveTerminal.tsx before it bites again.</span>
          <span className="meta">now</span>
          {onExpand && <span className="lc-collapse" onClick={onExpand} title="Collapse to peek">›</span>}
        </div>
        <div className="lc-stack">
          <LcCard label="Diff" flex="1.15"
                  meta={<><span className="pulse"></span> auto-follow <span className="dim">· claude</span></>}>
            <div className="lc-diff">
              <div className="lc-diff-tabs">
                {window.FP_DIFF_TABS.map((t, i) => (
                  <div key={i} className={"lc-diff-tab" + (t.active ? " active" : "") + (t.live ? " flicker" : "")}
                       onClick={() => onOpenFileAtHunk && onOpenFileAtHunk("src/" + t.file)}>
                    <span className="chip h-claude" data-kind="claude">CC</span>
                    <span>{t.file}</span>
                    <span className="da">+{t.add}</span>
                    {t.del > 0 && <span className="dd">−{t.del}</span>}
                  </div>
                ))}
              </div>
              <div className="lc-diff-body">
                <div className="lc-minihunk">
                  <div className="mh-line ctx"><span className="mh-ln">22</span>    stream.onData((d) =&gt; t.write(d));</div>
                  <div className="mh-line add"><span className="mh-ln">23</span>+   // guard PTY writes behind pane focus</div>
                  <div className="mh-line add"><span className="mh-ln">24</span>+   t.onData((d) =&gt; {"{"} if (focused) stream.send(d); {"}"});</div>
                  <div className="mh-line del"><span className="mh-ln">25</span>−   t.onData((d) =&gt; stream.send(d));</div>
                  <div className="mh-hint">click a tab to open that file in a Files harness →</div>
                </div>
              </div>
            </div>
          </LcCard>
          <div className="lc-divider"></div>
          <LcCard label="Plan" flex="0.8" meta={<>1 now <span className="dim">· 1/4</span></>}>
            <div className="lc-plan">
              {window.FP_PLAN.map((p, i) => (
                <div key={i} className={"lc-plan-row " + p.s}>
                  <span className="box">{p.s === "done" ? "✓" : p.s === "now" ? "◆" : ""}</span>
                  <span className="txt">{p.t}</span>
                </div>
              ))}
            </div>
          </LcCard>
          <div className="lc-divider"></div>
          <LcCard label="Activity" flex="1.05" meta={<>5 events <span className="dim">· $0.08</span></>}>
            <div className="lc-activity">
              {window.FP_ACTIVITY.map((a, i) => (
                <div key={i} className={"lc-row" + (a.kuser ? " k-user" : "")}>
                  <span className="time">{a.time}</span>
                  <span className="by"><span className="chip h-claude" data-kind="claude">CC</span></span>
                  <span className="gist">
                    <span className={"glyph gc-" + a.gc}>{a.g}</span>
                    <span className="tool">{a.tool}</span>{" "}
                    <span className="target">{a.tgt}</span>
                  </span>
                  <span className="right">{a.right}</span>
                </div>
              ))}
              <div className="lc-tail"><span className="blinker"></span><span>tailing — new rows appear live</span></div>
            </div>
          </LcCard>
        </div>
      </div>
    </div>
  );
}

// ── Bottom status bar ────────────────────────────────────────────────────────
function StatusBar({ focusLabel }) {
  return (
    <div className="sk-statusbar">
      <span className="seg"><span className="chip h-claude" data-kind="claude">CC</span> Claude Code</span>
      <span className="seg"><span className="dot st-running" data-state="running"></span> running</span>
      <span className="seg">main</span>
      <span className="seg path">~/git/private/skein</span>
      <span className="spacer"></span>
      <span className="seg sb-focus" title="Which harness currently has the keyboard">
        <span className="sb-kbd">⌨</span> {focusLabel}
      </span>
      <span className="seg sb-hints">⌘E files · ⌘P open · ⌘S save</span>
    </div>
  );
}

Object.assign(window, {
  TitleBar, RoomTabs, HarnessTabs, HarnessPicker, TerminalBody, TerminalFoot, ShellBody, LiveContext, StatusBar,
});
