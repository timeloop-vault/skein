/* ══════════════════════════════════════════════════════════════════════
   Files Pillar prototype — app wiring
   Files is a harness type. The harness column hosts whichever harness is
   active (Claude Code / shell / Files); only one body shows at a time, so
   a keystroke can never reach a hidden PTY. Live Context stays on the
   right (Full / Peek). No Column placement — that waits for detachable panes.
   ══════════════════════════════════════════════════════════════════════ */
const { useState, useEffect, useRef, useCallback } = React;
const AGENT_PATH = window.FP_AGENT_EDIT.path;
const AFTER_LEN = window.FP_AGENT_EDIT.after.length;
const DEFAULT_BUFFERS = ["src/LiveTerminal.tsx", "src/harnessEvents.ts", "src/types.ts"];

function Director(props) {
  const { width, setWidth, lcMode, setLcMode, theme, setTheme,
          agentEditing, onScenario, onReset, dirtyActive, onToggleDirty, activeIsFiles } = props;
  const [open, setOpen] = useState(true);
  const Seg = ({ value, cur, set, children }) => (
    <button className={"seg-btn" + (value === cur ? " on" : "")} onClick={() => set(value)}>{children}</button>);
  return (
    <div className={"fpctrl" + (open ? "" : " collapsed")}>
      <div className="fpc-hd" onClick={() => setOpen((v) => !v)}>
        <span className="fpc-dot"></span><b>Prototype controls</b>
        <span className="fpc-iss">#49 · files</span>
        <span className="fpc-min">{open ? "–" : "+"}</span>
      </div>
      <div className="fpc-body">
        <div className="fpc-model">
          <b>Files is a harness.</b> Add it from <span className="kbdlike">+ harness</span> like a
          shell — open as many as you like. Only one harness body shows at a time.
        </div>
        <div className="fpc-sec">
          <span className="lab">Screen width</span>
          <div className="seg">
            <Seg value="ultra" cur={width} set={setWidth}>Ultrawide</Seg>
            <Seg value="laptop" cur={width} set={setWidth}>Laptop</Seg>
          </div>
          <span className="fpc-hint">{width === "ultra" ? "Room for Live Context Full." : "Capped ~1280 — Live Context defaults to Peek."}</span>
        </div>
        <div className="fpc-sec">
          <span className="lab">Live Context</span>
          <div className="seg">
            <Seg value="full" cur={lcMode} set={setLcMode}>Full</Seg>
            <Seg value="peek" cur={lcMode} set={setLcMode}>Peek</Seg>
          </div>
        </div>
        <div className="fpc-sec">
          <span className="lab">Scenario · agent edits your open file</span>
          <button className="fpc-run" onClick={onScenario} disabled={agentEditing}>
            {agentEditing ? "▶ running…" : "▶ Claude edits LiveTerminal.tsx"}
          </button>
          <label className="fpc-check" onClick={onToggleDirty}>
            <span className={"fpc-box" + (dirtyActive ? " on" : "")}>{dirtyActive ? "✓" : ""}</span>
            <span>Give the open file unsaved edits first</span>
          </label>
          <span className="fpc-hint">{dirtyActive
            ? "Dirty → you choose: keep mine / take theirs."
            : "Clean → the buffer quietly follows disk."}</span>
          <span className="fpc-hint">{activeIsFiles
            ? "You're on the Files harness — you'll see it stream in live."
            : "You're on a terminal — the Files tab will pip; click it to watch."}</span>
          <button className="fpc-reset" onClick={onReset}>Reset</button>
        </div>
        <div className="fpc-sec">
          <span className="lab">Theme</span>
          <div className="seg">
            <Seg value="dark" cur={theme} set={setTheme}>Dark</Seg>
            <Seg value="light" cur={theme} set={setTheme}>Light</Seg>
          </div>
        </div>
      </div>
    </div>);
}

function App() {
  const [width, setWidthRaw] = useState("ultra");
  const [lcMode, setLcMode] = useState("full");
  const [theme, setTheme] = useState("dark");

  const [harnesses, setHarnesses] = useState([
    { id: "h1", kind: "claude", name: "Claude Code", state: "running" },
    { id: "h2", kind: "byoh", name: "shell", state: "idle" },
    { id: "h3", kind: "byoh", name: "shell", state: "idle" },
    { id: "h4", kind: "files", name: "Files", state: "idle" },
  ]);
  const [active, setActive] = useState("h4");
  const [filesState, setFilesState] = useState({
    h4: { buffers: [...DEFAULT_BUFFERS], active: "src/LiveTerminal.tsx" },
  });
  const [dirty, setDirty] = useState(() => new Set());
  const [override, setOverride] = useState({});
  const [quick, setQuick] = useState(false);
  const [agentEditing, setAgentEditing] = useState(false);
  const [streamCount, setStreamCount] = useState(0);

  const hRef = useRef(harnesses); hRef.current = harnesses;
  const actRef = useRef(active); actRef.current = active;
  const fsRef = useRef(filesState); fsRef.current = filesState;
  const idRef = useRef(4);
  const lastPtyRef = useRef("h1");

  const activeH = harnesses.find((h) => h.id === active) || null;
  const activeIsFiles = activeH && activeH.kind === "files";

  const setWidth = useCallback((w) => { setWidthRaw(w); setLcMode(w === "laptop" ? "peek" : "full"); }, []);

  // ── harness ops ──
  const selectHarness = useCallback((id) => {
    setActive(id);
    const h = hRef.current.find((x) => x.id === id);
    if (h && h.kind !== "files") lastPtyRef.current = id;
    setQuick(false);
  }, []);
  const addHarness = useCallback((kind) => {
    const id = "h" + (++idRef.current);
    const name = kind === "files" ? "Files" : kind === "claude" ? "Claude Code" : kind === "opencode" ? "opencode" : "shell";
    setHarnesses((hs) => [...hs, { id, kind, name, state: kind === "claude" ? "running" : "idle" }]);
    if (kind === "files") setFilesState((fs) => ({ ...fs, [id]: { buffers: [], active: null } }));
    else lastPtyRef.current = id;
    setActive(id);
    return id;
  }, []);
  const closeHarness = useCallback((id) => {
    setHarnesses((hs) => {
      const idx = hs.findIndex((h) => h.id === id);
      const next = hs.filter((h) => h.id !== id);
      setActive((a) => (a === id ? (next[Math.max(0, idx - 1)]?.id || next[0]?.id || null) : a));
      return next;
    });
    setFilesState((fs) => { const n = { ...fs }; delete n[id]; return n; });
  }, []);

  // ── file ops (operate on the active Files harness, creating one if needed) ──
  const targetFilesHarness = () => {
    const hs = hRef.current, a = actRef.current;
    if (hs.find((h) => h.id === a)?.kind === "files") return a;
    return hs.find((h) => h.kind === "files")?.id || null;
  };
  const openFile = useCallback((path) => {
    if (!window.FP_FILES[path]) return;
    let fid = targetFilesHarness();
    if (!fid) fid = addHarness("files");
    setFilesState((fs) => {
      const cur = fs[fid] || { buffers: [], active: null };
      const buffers = cur.buffers.includes(path) ? cur.buffers : [...cur.buffers, path];
      return { ...fs, [fid]: { buffers, active: path } };
    });
    setActive(fid); setQuick(false);
  }, [addHarness]);
  const selectBuffer = useCallback((fid, path) => {
    setFilesState((fs) => ({ ...fs, [fid]: { ...fs[fid], active: path } }));
    setActive(fid);
  }, []);
  const closeBuffer = useCallback((fid, path) => {
    setFilesState((fs) => {
      const cur = fs[fid]; const idx = cur.buffers.indexOf(path);
      const buffers = cur.buffers.filter((p) => p !== path);
      const nextActive = cur.active === path ? (buffers[Math.max(0, idx - 1)] || null) : cur.active;
      return { ...fs, [fid]: { buffers, active: nextActive } };
    });
    setDirty((d) => { const n = new Set(d); n.delete(path); return n; });
  }, []);
  const toggleDirty = useCallback(() => {
    const fid = targetFilesHarness(); if (!fid) return;
    const p = fsRef.current[fid]?.active; if (!p) return;
    setDirty((d) => { const n = new Set(d); n.has(p) ? n.delete(p) : n.add(p); return n; });
  }, []);

  // ── scenario ──
  const startScenario = useCallback(() => {
    let fid = hRef.current.find((h) => h.kind === "files")?.id;
    if (!fid) fid = addHarness("files");
    setFilesState((fs) => {
      const cur = fs[fid] || { buffers: [], active: null };
      const buffers = cur.buffers.includes(AGENT_PATH) ? cur.buffers : [...cur.buffers, AGENT_PATH];
      return { ...fs, [fid]: { buffers, active: AGENT_PATH } };
    });
    setOverride((o) => { const n = { ...o }; delete n[AGENT_PATH]; return n; });
    setAgentEditing(true); setStreamCount(0);
  }, [addHarness]);
  const acceptEdit = useCallback(() => {
    const plain = window.FP_AGENT_EDIT.after.map(([t]) => t);
    setOverride((o) => ({ ...o, [AGENT_PATH]: plain }));
    setDirty((d) => { const n = new Set(d); n.delete(AGENT_PATH); return n; });
    setAgentEditing(false); setStreamCount(0);
  }, []);
  const keepMine = useCallback(() => { setAgentEditing(false); setStreamCount(0); }, []);
  const reset = useCallback(() => {
    setAgentEditing(false); setStreamCount(0); setOverride({}); setDirty(new Set());
    setHarnesses([
      { id: "h1", kind: "claude", name: "Claude Code", state: "running" },
      { id: "h2", kind: "byoh", name: "shell", state: "idle" },
      { id: "h3", kind: "byoh", name: "shell", state: "idle" },
      { id: "h4", kind: "files", name: "Files", state: "idle" },
    ]);
    setFilesState({ h4: { buffers: [...DEFAULT_BUFFERS], active: "src/LiveTerminal.tsx" } });
    setActive("h4"); idRef.current = 4; lastPtyRef.current = "h1";
  }, []);

  // stream incoming edit
  useEffect(() => {
    if (!agentEditing || streamCount >= AFTER_LEN) return;
    const id = setTimeout(() => setStreamCount((c) => c + 1), streamCount === 0 ? 380 : 120);
    return () => clearTimeout(id);
  }, [agentEditing, streamCount]);
  // clean buffer auto-follows disk after edit lands
  useEffect(() => {
    if (agentEditing && streamCount >= AFTER_LEN && !dirty.has(AGENT_PATH)) {
      const id = setTimeout(acceptEdit, 1300);
      return () => clearTimeout(id);
    }
  }, [agentEditing, streamCount, dirty, acceptEdit]);

  // ── keyboard ──
  useEffect(() => {
    const h = (e) => {
      const mod = e.metaKey || e.ctrlKey; if (!mod) return;
      const k = e.key.toLowerCase();
      if (k === "e") {
        e.preventDefault();
        const hs = hRef.current, a = actRef.current;
        const isF = hs.find((x) => x.id === a)?.kind === "files";
        if (isF) { const pty = lastPtyRef.current && hs.find((x) => x.id === lastPtyRef.current) ? lastPtyRef.current : hs.find((x) => x.kind !== "files")?.id; if (pty) setActive(pty); }
        else { const f = hs.find((x) => x.kind === "files")?.id; if (f) setActive(f); else addHarness("files"); }
      } else if (k === "p") { e.preventDefault(); setQuick(true); }
      else if (k === "s") {
        e.preventDefault();
        const fid = targetFilesHarness(); const p = fid && fsRef.current[fid]?.active;
        if (p) setDirty((d) => { const n = new Set(d); n.delete(p); return n; });
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [addHarness]);

  const fs = activeIsFiles ? (filesState[active] || { buffers: [], active: null }) : null;
  const dirtyActive = fs && fs.active ? dirty.has(fs.active) : false;
  const focusLabel = activeIsFiles ? "editor · Files"
    : activeH && activeH.kind === "claude" ? "Claude Code · terminal"
    : activeH ? activeH.name + " · terminal" : "—";

  const surfaceProps = activeIsFiles ? {
    activePath: fs.active, buffers: fs.buffers, dirtyset: dirty, contentOverride: override,
    onOpen: openFile, onSelectBuffer: (p) => selectBuffer(active, p), onCloseBuffer: (p) => closeBuffer(active, p),
    agentEditing, streamCount, onAccept: acceptEdit, onKeepMine: keepMine, embedded: true,
  } : null;

  const lc = lcMode === "peek"
    ? <window.LiveContext peek onExpand={() => setLcMode("full")} />
    : <window.LiveContext onExpand={() => setLcMode("peek")} onOpenFileAtHunk={openFile} />;

  return (
    <div className="fp-viewport">
      <div className="fp-stage" style={{ maxWidth: width === "laptop" ? 1280 : "none" }}>
        <div className={"sk-app " + (theme === "dark" ? "sk-dark" : "sk-light") + " density-regular"}>
          <window.TitleBar />
          <window.RoomTabs />

          <div className="sk-workspace">
            <div className="sk-harness-col">
              <window.HarnessTabs
                harnesses={harnesses} active={active}
                onSelect={selectHarness} onClose={closeHarness} onAdd={addHarness}
                agentEditing={agentEditing} filesState={filesState} agentPath={AGENT_PATH} />
              {activeIsFiles
                ? <window.FileSurface {...surfaceProps} />
                : activeH && activeH.kind === "claude"
                  ? <><window.TerminalBody /><window.TerminalFoot /></>
                  : <window.ShellBody name={activeH ? activeH.name : "shell"} />}
            </div>
            <div className="sk-splitter-x"></div>
            {lc}
          </div>

          <window.StatusBar focusLabel={focusLabel} />
        </div>
      </div>

      <Director
        width={width} setWidth={setWidth}
        lcMode={lcMode} setLcMode={setLcMode}
        theme={theme} setTheme={setTheme}
        agentEditing={agentEditing} onScenario={startScenario} onReset={reset}
        dirtyActive={dirtyActive} onToggleDirty={toggleDirty} activeIsFiles={activeIsFiles} />

      {quick && <window.QuickOpen onPick={openFile} onClose={() => setQuick(false)} />}
    </div>);
}

ReactDOM.createRoot(document.getElementById("root")).render(<App />);
