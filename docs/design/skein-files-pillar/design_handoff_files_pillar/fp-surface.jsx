/* ══════════════════════════════════════════════════════════════════════
   Files Pillar prototype — the file surface
   Persistent tree (VS Code sidebar) + multi-buffer editor tabs + raw text
   editor with find. The hero: LiveChangeSplit — when the agent edits the
   file you have open, the editor splits and you watch its version land.
   Kept deliberately distinct from the Live Context Diff card.
   ══════════════════════════════════════════════════════════════════════ */
const { useState: useS, useEffect: useE, useRef: useR } = React;

// ── Tiny syntax highlighter (comments / strings / keywords / numbers) ────────
const FP_KW = new Set(("import from export default function const let var return interface " +
  "type new if else for of in class extends implements public private readonly void true " +
  "false null undefined await async fn pub struct impl mut use enum match unique symbol").split(" "));

function fpHighlight(src, lang) {
  if (src === "") return [["", null]];
  if (lang === "md") {
    if (/^#{1,6}\s/.test(src)) return [[src, "fn"]];
    if (/^\s{4,}\S/.test(src)) return [[src, "str"]];
    return [[src, null]];
  }
  const out = [];
  let i = 0, buf = "";
  const flush = () => { if (buf) { out.push([buf, null]); buf = ""; } };
  while (i < src.length) {
    const ch = src[i];
    if (ch === "/" && src[i + 1] === "/") { flush(); out.push([src.slice(i), "com"]); i = src.length; break; }
    if (ch === '"' || ch === "'" || ch === "`") {
      flush();
      let j = i + 1;
      while (j < src.length && src[j] !== ch) { if (src[j] === "\\") j++; j++; }
      out.push([src.slice(i, Math.min(j + 1, src.length)), "str"]); i = j + 1; continue;
    }
    buf += ch; i++;
  }
  flush();
  const final = [];
  for (const [t, c] of out) {
    if (c) { final.push([t, c]); continue; }
    let last = 0, m; const re = /[A-Za-z_$][A-Za-z0-9_$]*|\d+(?:\.\d+)?/g;
    while ((m = re.exec(t))) {
      if (m.index > last) final.push([t.slice(last, m.index), null]);
      const w = m[0];
      if (/^\d/.test(w)) final.push([w, "num"]);
      else if (FP_KW.has(w)) final.push([w, "key"]);
      else final.push([w, null]);
      last = m.index + w.length;
    }
    if (last < t.length) final.push([t.slice(last), null]);
  }
  return final;
}
const renderTokens = (toks) => toks.map(([t, c], i) =>
  <span key={i} className={c ? "tk-" + c : undefined}>{t}</span>);

// ── Sidebar tree ─────────────────────────────────────────────────────────────
function FileTree({ activePath, liveTouch, onOpen }) {
  const [open, setOpen] = useS(() => {
    const m = {};
    (function walk(n, key) {
      if (n.type === "dir") { m[key] = !!n.open; n.children.forEach((c) => walk(c, key + "/" + c.name)); }
    })(window.FP_TREE, window.FP_TREE.name);
    return m;
  });
  const toggle = (k) => setOpen((o) => ({ ...o, [k]: !o[k] }));
  const rows = [];
  (function walk(node, key, depth) {
    if (node.type === "dir") {
      rows.push(
        <div key={key} className="tree-row dir" style={{ paddingLeft: 6 + depth * 12 }} onClick={() => toggle(key)}>
          <span className="tw-chev">{open[key] ? "▾" : "▸"}</span>
          <span className="tw-name">{node.name}</span>
        </div>);
      if (open[key]) node.children.forEach((c) => walk(c, key + "/" + c.name, depth + 1));
    } else {
      const touched = node.touched || (liveTouch && node.path === window.FP_AGENT_EDIT.path);
      rows.push(
        <div key={key}
             className={"tree-row file" + (node.path === activePath ? " active" : "") + (touched ? " touched" : "")}
             style={{ paddingLeft: 6 + depth * 12 }}
             onClick={() => onOpen(node.path)}>
          <span className="tw-file-dot"></span>
          <span className="tw-name">{node.name}</span>
          {touched && <span className="tw-pip" title="edited by an agent">●</span>}
        </div>);
    }
  })(window.FP_TREE, window.FP_TREE.name, 0);
  return <div className="fp-tree">{rows}</div>;
}

// ── Buffer tabs ──────────────────────────────────────────────────────────────
function BufferTabs({ buffers, active, dirtyset, livePath, onSelect, onClose }) {
  return (
    <div className="fp-buftabs">
      {buffers.map((p) => {
        const name = p.split("/").pop();
        const dirty = dirtyset.has(p);
        return (
          <div key={p} className={"fp-buftab" + (p === active ? " active" : "") + (p === livePath ? " flicker" : "")}
               onClick={() => onSelect(p)}>
            {p === livePath && <span className="bt-live" title="an agent is editing this"></span>}
            <span className="bt-name">{name}</span>
            {dirty
              ? <span className="bt-dirty" title="unsaved changes">●</span>
              : <span className="bt-x" title="close" onClick={(e) => { e.stopPropagation(); onClose(p); }}>×</span>}
          </div>);
      })}
    </div>);
}

// ── Plain editor ─────────────────────────────────────────────────────────────
function Editor({ file, query }) {
  const q = (query || "").toLowerCase();
  return (
    <div className="fp-code">
      {file.lines.map((ln, i) => {
        const hit = q && ln.toLowerCase().includes(q);
        return (
          <div key={i} className={"ed-line" + (hit ? " hit" : "")}>
            <span className="ed-ln">{i + 1}</span>
            <span className="ed-src">{renderTokens(fpHighlight(ln, file.lang))}</span>
          </div>);
      })}
      <div className="ed-line"><span className="ed-ln">{file.lines.length + 1}</span><span className="ed-src"></span></div>
    </div>);
}

function EditorEmpty() {
  return (
    <div className="fp-editor-empty">
      <div className="ee-glyph">◇</div>
      <div className="ee-title">No file open</div>
      <div className="ee-sub">Pick a file in the tree, or press <span className="kbd">⌘P</span> to jump to one.</div>
    </div>);
}

// ── Find bar ─────────────────────────────────────────────────────────────────
function FindBar({ q, setQ, file, onClose }) {
  const ref = useR();
  useE(() => { ref.current && ref.current.focus(); }, []);
  const count = q && file ? file.lines.filter((l) => l.toLowerCase().includes(q.toLowerCase())).length : 0;
  return (
    <div className="fp-findbar">
      <span className="fb-glyph">⌕</span>
      <input ref={ref} className="fb-input" placeholder="Find in file" value={q}
             onChange={(e) => setQ(e.target.value)}
             onKeyDown={(e) => { if (e.key === "Escape") onClose(); }} />
      <span className="fb-count">{q ? count + " lines" : ""}</span>
      <span className="fb-btn">↑</span>
      <span className="fb-btn">↓</span>
      <span className="fb-x" onClick={onClose}>×</span>
    </div>);
}

// ── The change bar above the live split ──────────────────────────────────────
function ChangeBar({ streaming, dirty, summary, onAccept, onKeepMine }) {
  return (
    <div className={"ed-changebar" + (dirty ? " dirty" : "") + (streaming ? " streaming" : "")}>
      <span className="cb-icon"><span className="chip h-claude" data-kind="claude">CC</span></span>
      <span className="cb-text">
        {streaming
          ? <>claude is editing this file — <b>{summary}</b></>
          : dirty
            ? <>claude changed this on disk while you have <b>unsaved edits</b></>
            : <>claude finished — <b>{summary}</b></>}
      </span>
      <span className="spacer"></span>
      {streaming && <span className="live-pill">LIVE</span>}
      {!streaming && dirty && <>
        <button className="cb-btn" onClick={onKeepMine}>Keep mine</button>
        <button className="cb-btn primary" onClick={onAccept}>Take theirs</button>
        <button className="cb-btn ghost" title="Jump to the Diff card">Compare ↗</button>
      </>}
      {!streaming && !dirty && <>
        <span className="cb-auto">clean buffer followed disk</span>
        <button className="cb-btn ghost" onClick={onKeepMine}>Undo</button>
      </>}
    </div>);
}

// ── The hero: live change split ──────────────────────────────────────────────
function LiveChangeSplit({ beforeLines, after, streamCount, lang, dirty }) {
  const incomingRef = useR();
  useE(() => { if (incomingRef.current) incomingRef.current.scrollTop = incomingRef.current.scrollHeight; }, [streamCount]);
  return (
    <div className="ed-split">
      <div className="ed-split-col">
        <div className="ed-split-head">
          <span className="sh-label">Yours</span>
          {dirty && <span className="sh-tag">unsaved edits</span>}
        </div>
        <div className="fp-code dim-side">
          {beforeLines.map((ln, i) => (
            <div key={i} className="ed-line">
              <span className="ed-ln">{i + 1}</span>
              <span className="ed-src">{renderTokens(fpHighlight(ln, lang))}</span>
            </div>))}
        </div>
      </div>
      <div className="ed-split-col incoming">
        <div className="ed-split-head">
          <span className="chip h-claude" data-kind="claude">CC</span>
          <span className="sh-label">claude · incoming</span>
        </div>
        <div className="fp-code" ref={incomingRef}>
          {after.slice(0, streamCount).map(([t, tag], i) => (
            <div key={i} className={"ed-line" + (tag === 1 ? " add" : tag === 2 ? " chg" : "")}>
              <span className="ed-ln">{tag === 1 ? "+" : tag === 2 ? "~" : i + 1}</span>
              <span className="ed-src">{renderTokens(fpHighlight(t, lang))}</span>
            </div>))}
          {streamCount < after.length &&
            <div className="ed-line typing"><span className="ed-ln"></span><span className="ed-src"><span className="t-cursorblock"></span></span></div>}
        </div>
      </div>
    </div>);
}

// ── The whole surface ────────────────────────────────────────────────────────
function FileSurface(props) {
  const {
    activePath, buffers, dirtyset, contentOverride, onOpen, onSelectBuffer, onCloseBuffer,
    agentEditing, streamCount, onAccept, onKeepMine, onCloseSurface,
    hasFocus, onFocus, embedded,
  } = props;
  const [findOpen, setFindOpen] = useS(false);
  const [q, setQ] = useS("");
  const base = window.FP_FILES[activePath];
  const file = base && contentOverride && contentOverride[activePath]
    ? { ...base, lines: contentOverride[activePath] } : base;
  const isAgentFile = activePath === window.FP_AGENT_EDIT.path;
  const showSplit = agentEditing && isAgentFile;
  const streaming = agentEditing && streamCount < window.FP_AGENT_EDIT.after.length;

  const parts = activePath ? activePath.split("/") : [];

  return (
    <div className="fp-surface embedded">
      <div className="fp-surface-head">
        <span className="fps-crumb">
          {activePath ? parts.map((seg, i) => (
            <span key={i}>{i > 0 && <span className="crumb-sep">/</span>}
              <span className={i === parts.length - 1 ? "crumb-leaf" : ""}>{seg}</span></span>
          )) : <span className="crumb-empty">no file open · ⌘P to jump</span>}
        </span>
        <span className="spacer"></span>
        <button className={"fps-btn" + (findOpen ? " on" : "")} title="Find in file (⌘F)"
                onClick={() => setFindOpen((v) => !v)} disabled={!file}>⌕</button>
      </div>

      <div className="fp-surface-body">
        <FileTree activePath={activePath} liveTouch={agentEditing} onOpen={onOpen} />
        <div className="fp-editor">
          <BufferTabs buffers={buffers} active={activePath} dirtyset={dirtyset}
                      livePath={agentEditing ? window.FP_AGENT_EDIT.path : null}
                      onSelect={onSelectBuffer} onClose={onCloseBuffer} />
          {findOpen && <FindBar q={q} setQ={setQ} file={file} onClose={() => { setFindOpen(false); setQ(""); }} />}
          {showSplit ? (
            <>
              <ChangeBar streaming={streaming} dirty={dirtyset.has(activePath)}
                         summary={window.FP_AGENT_EDIT.summary} onAccept={onAccept} onKeepMine={onKeepMine} />
              <LiveChangeSplit beforeLines={file.lines} after={window.FP_AGENT_EDIT.after}
                               streamCount={streamCount} lang={file.lang} dirty={dirtyset.has(activePath)} />
            </>
          ) : file ? <Editor file={file} query={q} /> : <EditorEmpty />}
        </div>
      </div>
    </div>);
}

// ── Fuzzy quick-open ─────────────────────────────────────────────────────────
function QuickOpen({ onPick, onClose }) {
  const [q, setQ] = useS("");
  const [sel, setSel] = useS(0);
  const ref = useR();
  useE(() => { ref.current && ref.current.focus(); }, []);
  const all = Object.keys(window.FP_FILES);
  const fz = (s, needle) => {
    s = s.toLowerCase(); needle = needle.toLowerCase();
    let i = 0; for (const c of needle) { i = s.indexOf(c, i); if (i < 0) return false; i++; } return true;
  };
  const list = (q ? all.filter((p) => fz(p, q)) : all).slice(0, 9);
  const pick = (i) => { if (list[i]) onPick(list[i]); };
  return (
    <div className="fp-qo-scrim" onMouseDown={onClose}>
      <div className="fp-qo" onMouseDown={(e) => e.stopPropagation()}>
        <input ref={ref} className="fp-qo-input" placeholder="Go to file…   (fuzzy — try 'lt' or 'harn')"
               value={q}
               onChange={(e) => { setQ(e.target.value); setSel(0); }}
               onKeyDown={(e) => {
                 if (e.key === "Enter") pick(sel);
                 else if (e.key === "Escape") onClose();
                 else if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => Math.min(s + 1, list.length - 1)); }
                 else if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => Math.max(s - 1, 0)); }
               }} />
        <div className="fp-qo-list">
          {list.length === 0 && <div className="fp-qo-empty">no matching files</div>}
          {list.map((p, idx) => {
            const name = p.split("/").pop();
            const dir = p.slice(0, p.length - name.length - 1);
            const live = p === window.FP_AGENT_EDIT.path;
            return (
              <div key={p} className={"fp-qo-row" + (idx === sel ? " sel" : "")}
                   onMouseEnter={() => setSel(idx)} onClick={() => pick(idx)}>
                <span className="qo-name">{name}</span>
                <span className="qo-dir">{dir}</span>
                {live && <span className="qo-live" title="an agent is editing this">●</span>}
              </div>);
          })}
        </div>
        <div className="fp-qo-foot"><span>↑↓ move</span><span>↵ open</span><span>esc close</span></div>
      </div>
    </div>);
}

Object.assign(window, { FileSurface, QuickOpen });
