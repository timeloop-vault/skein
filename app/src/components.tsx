// Shared, low-level components shared across the prototype.
//
// They map 1:1 to the design's tiny atoms (chips, dots, terminal panes,
// file tree, diff editor, plan, activity feed). The composition lives
// in App.tsx — these are the leaves.

import type { CSSProperties, ReactNode } from "react";
import { HARNESS_KINDS } from "./data.tsx";
import type {
	ActivityEvent,
	DiffLine,
	Harness,
	HarnessKind,
	PlanItem,
	Session,
	SessionData,
	Status,
	TreeNode,
} from "./types.ts";

export const HChip = ({ kind, size = 14 }: { kind: HarnessKind; size?: number }) => {
	const k = HARNESS_KINDS[kind];
	const style: CSSProperties = {
		width: size,
		height: size,
		fontSize: Math.round(size * 0.62),
		borderRadius: Math.max(2, size / 4),
	};
	return (
		<span className={`h-chip ${k.chip}`} style={style} title={k.name}>
			{k.label}
		</span>
	);
};

export const StatusDot = ({ status, size = 6 }: { status: Status; size?: number }) => (
	<span className={`tab-status st-${status}`} style={{ width: size, height: size }} />
);

// ── Terminal panels ────────────────────────────────────────────────
// Each harness gets its own typographic fingerprint. We're emulating the
// real CLIs (claude / opencode / gh copilot / a hypothetical skein BYOH)
// rather than embedding them — but the look should be familiar enough
// that a user reads "claude is running" without having to ask.

export const ClaudePanel = ({ harnessId }: { harnessId: string }) => (
	<div className="sk-term term-claude">
		<div className="sk-shell-prompt">
			<span className="ps1-host">user@skein</span> <span className="ps1-path">~/code/skein</span>{" "}
			<span className="ps1-arrow">❯</span>{" "}
			<span className="ps1-cmd">claude --resume {harnessId}</span>
		</div>
		<div className="header">
			<span className="name">● Claude Code</span> <span className="dim">v1.18.2 · sonnet-4.5</span>
			<br />
			<span className="dim">cwd: /home/stefan/code/skein · model: sonnet-4.5</span>
		</div>
		<span className="line dim">─ resumed conversation ─</span>
		<br />
		<br />
		<span className="line">
			<span className="accent">&gt;</span>{" "}
			<span className="em">
				Wire up the title bar with traffic-light buttons on macOS and a custom drag region. Keep it
				30px tall.
			</span>
		</span>
		<br />
		<br />
		<span className="line">
			<span className="bullet">●</span> Reading the existing window setup.
		</span>
		<br />
		<br />
		<span className="line muted">⚡ Read(src-tauri/tauri.conf.json)</span>
		<span className="line dim"> ⤷ 22 lines</span>
		<span className="line muted">⚡ Read(src/components/Titlebar.tsx)</span>
		<span className="line dim"> ⤷ 22 lines</span>
		<br />
		<span className="line">
			<span className="bullet">●</span> Setting <span className="em">titleBarStyle: "Overlay"</span>{" "}
			on macOS, <span className="em">decorations: false</span> elsewhere, and adding a drag region
			to the new <span className="em">Titlebar</span> component.
		</span>
		<br />
		<br />
		<span className="line muted">⚡ Edit(src-tauri/tauri.conf.json)</span>
		<span className="line ok"> ⤷ +3 −1</span>
		<span className="line muted">⚡ Write(src/components/Titlebar.tsx)</span>
		<span className="line ok"> ⤷ 38 lines</span>
		<br />
		<span className="line">
			<span className="bullet">●</span> Done. The traffic lights are inset 10px from the left, the
			app name centers, and the bar is draggable except over interactive elements.
		</span>
		<br />
		<br />
		<span className="line">
			Want me to run <span className="accent">pnpm tauri dev</span> to verify?
		</span>
		<br />
		<br />
		<span className="line dim">┍─ chat ─────────────────────────────────────────────</span>
		<span className="line">
			<span className="accent">&gt; </span>
			<span className="blink" />
		</span>
		<span className="line dim">┕── ⏎ send · ⇧⏎ newline · ! bash · / commands</span>
	</div>
);

export const OpenCodePanel = ({ harnessId }: { harnessId: string }) => (
	<div className="sk-term term-opencode">
		<div className="sk-shell-prompt">
			<span className="ps1-host">user@skein</span> <span className="ps1-path">~/code/skein</span>{" "}
			<span className="ps1-arrow">❯</span>{" "}
			<span className="ps1-cmd">opencode --session {harnessId}</span>
		</div>
		<div className="header">
			<span className="name">opencode</span> <span className="dim">0.4.1 · sonnet-4.5</span>
		</div>
		<span className="line dim">session restored. 2 messages.</span>
		<br />
		<br />
		<span className="line">
			<span className="tag">[user]</span>{" "}
			<span className="em">
				Look at what the other harness just did to Titlebar.tsx and tell me if the spacing matches
				the design system.
			</span>
		</span>
		<br />
		<br />
		<span className="line">
			<span className="tag">[assistant]</span> Reading the diff in the worktree…
		</span>
		<br />
		<br />
		<span className="line muted"> ┌ read_file</span>
		<span className="line muted"> │ path: src/components/Titlebar.tsx</span>
		<span className="line ok"> └ ok · 38 lines</span>
		<br />
		<span className="line">
			The 30px height is fine but the inner gap is <span className="em">10px</span> where
		</span>
		<span className="line">
			the design tokens specify <span className="em">--space-3</span> (12px). Two-pixel
		</span>
		<span className="line">inconsistency — small but it'll compound elsewhere.</span>
		<br />
		<br />
		<span className="line dim">───────────────────────────────────────────────────</span>
		<span className="line">
			<span className="tag">&gt;</span> <span className="blink" />
		</span>
	</div>
);

export const ByohPanel = ({
	harnessId,
	onApprove,
}: {
	harnessId: string;
	onApprove: () => void;
}) => (
	<div className="sk-term term-byoh">
		<div className="sk-shell-prompt">
			<span className="ps1-host">user@skein</span> <span className="ps1-path">~/code/skein</span>{" "}
			<span className="ps1-arrow">❯</span>{" "}
			<span className="ps1-cmd">skein agent --session {harnessId}</span>
		</div>
		<div className="header">
			<span className="name">skein-byoh</span> <span className="dim">v0 · sonnet-4.5</span>
		</div>
		<span className="line dim">connecting to anthropic… ok. 6 tools registered.</span>
		<br />
		<br />
		<span className="line you">
			user&gt;{" "}
			<span className="em">
				Add a debounce around the fs watcher so the diff pane doesn't flicker on every keystroke.
				80ms feels right.
			</span>
		</span>
		<br />
		<br />
		<span className="line">
			agent&gt; Looking at <span className="em">src/fs/watcher.rs</span> — the watcher pipes raw{" "}
			<span className="em">notify</span>
		</span>
		<span className="line"> events straight to the channel. I'll wrap that in a debounce.</span>
		<br />
		<br />
		<span className="line dim">
			[tool] read_file <span className="accent">src/fs/watcher.rs</span>
		</span>
		<span className="line dim"> → ok (38 lines)</span>
		<span className="line dim">
			[tool] grep <span className="accent">"recommended_watcher"</span>
		</span>
		<span className="line dim"> → ok (1 match)</span>
		<span className="line dim">
			[tool] str_replace <span className="accent">src/fs/watcher.rs</span>
		</span>
		<span className="line dim"> → ok (+5 −1)</span>
		<br />
		<span className="line">
			agent&gt; Added a 80ms debounce and an extension filter (rs, toml, md).
		</span>
		<span className="line"> Want me to run the test suite to confirm nothing regressed?</span>
		<br />
		<br />
		<span className="line">
			<span className="info">[tool] bash</span>{" "}
			<span className="accent">cargo test --package skein-core fs::watcher</span>
		</span>
		<div className="box warn">
			<span className="em">⚡ Permission required · bash</span>
			<br />
			<span className="dim"> $ cargo test --package skein-core fs::watcher</span>
		</div>
		<span className="line dim">
			&nbsp;&nbsp;<span className="key">y</span> approve once <span className="key">a</span> always
			for this session <span className="key">n</span> deny <span className="key">e</span> edit
		</span>
		<br />
		<span className="line">
			user&gt;{" "}
			<span style={{ color: "var(--accent)", cursor: "pointer" }} onClick={onApprove}>
				y
			</span>
			<span className="blink" />
		</span>
	</div>
);

export const ByohResolvedPanel = ({ harnessId }: { harnessId: string }) => (
	<div className="sk-term term-byoh">
		<div className="sk-shell-prompt">
			<span className="ps1-host">user@skein</span> <span className="ps1-path">~/code/skein</span>{" "}
			<span className="ps1-arrow">❯</span>{" "}
			<span className="ps1-cmd">skein agent --session {harnessId}</span>
		</div>
		<div className="header">
			<span className="name">skein-byoh</span> <span className="dim">v0 · sonnet-4.5</span>
		</div>
		<span className="line dim">
			[tool] str_replace <span className="accent">src/fs/watcher.rs</span> → ok
		</span>
		<br />
		<span className="line">agent&gt; Want me to run the test suite?</span>
		<span className="line ok">user&gt; y</span>
		<span className="line dim">
			[tool] bash <span className="accent">cargo test fs::watcher</span>
		</span>
		<span className="line ok">
			→ ok (12 passed, 0 failed) <span className="dim">in 4.1s</span>
		</span>
		<br />
		<span className="line">agent&gt; All passed. The debounce is in. Want me to commit it as</span>
		<span className="line">
			<span className="em">fs: debounce watcher events to 80ms</span>?
		</span>
		<br />
		<span className="line">
			user&gt; <span className="blink" />
		</span>
	</div>
);

export const CopilotPanel = ({ harnessId }: { harnessId: string }) => (
	<div className="sk-term term-copilot">
		<div className="sk-shell-prompt">
			<span className="ps1-host">user@laptop</span>{" "}
			<span className="ps1-path">~/repos/pim</span> <span className="ps1-arrow">❯</span>{" "}
			<span className="ps1-cmd">gh copilot suggest --session {harnessId}</span>
		</div>
		<div className="header">
			<span className="name">GitHub Copilot CLI</span>{" "}
			<span className="dim">· gpt-5 (entitlement)</span>
		</div>
		<span className="line dim">authenticated as stefan-i · example enterprise plan</span>
		<br />
		<br />
		<span className="line">
			<span className="tag">?</span>{" "}
			<span className="em">
				The reindex is bringing the search API to its knees. Look at how we're reading from the
				canonical store.
			</span>
		</span>
		<br />
		<br />
		<span className="line muted">⊣ Inspecting the indexer entry point.</span>
		<br />
		<span className="line muted">┌─ file ops</span>
		<span className="line muted">│ read src/indexer/main.rs</span>
		<span className="line muted">│ grep "BATCH_SIZE" 4 matches</span>
		<span className="line muted">└─</span>
		<br />
		<span className="line">
			The reindexer's pulling <span className="em">50k rows</span> per batch and we're seeing
		</span>
		<span className="line">
			memory pressure spikes. I'd drop that to <span className="em">5k</span> and add a 50ms
		</span>
		<span className="line">yield between batches. Going to draft the change.</span>
		<br />
		<br />
		<span className="line dim">copilot ⤷</span>
		<span className="line">
			<span className="tag">?</span> <span className="blink" />
		</span>
	</div>
);

export const CopilotErroredPanel = ({
	harnessId,
	onRetry,
	onReauth,
}: {
	harnessId: string;
	onRetry: () => void;
	onReauth: () => void;
}) => (
	<div className="sk-term term-copilot">
		<div className="sk-shell-prompt">
			<span className="ps1-host">user@laptop</span>{" "}
			<span className="ps1-path">~/repos/pim</span> <span className="ps1-arrow">❯</span>{" "}
			<span className="ps1-cmd">gh copilot suggest --session {harnessId}</span>
		</div>
		<div className="header">
			<span className="name">GitHub Copilot CLI</span>{" "}
			<span className="dim">· gpt-5 (entitlement)</span>
		</div>
		<span className="line dim">authenticated as stefan-i · example enterprise plan</span>
		<br />
		<br />
		<span className="line">
			<span className="tag">?</span>{" "}
			<span className="em">Add a similarity_search endpoint that uses the pgvector index.</span>
		</span>
		<br />
		<br />
		<span className="line muted">┌─ file ops</span>
		<span className="line muted">│ read src/embeddings/pgvector.rs</span>
		<span className="line muted">│ edit src/embeddings/pgvector.rs (+24 −2)</span>
		<span className="line muted">└─</span>
		<br />
		<span className="line">Hooking up an HNSW index on the embeddings column. The query</span>
		<span className="line">side is straightforward — the harder part is keeping the</span>
		<span className="line">migration online. I'll spell it out…</span>
		<br />
		<br />
		<span className="line dim">[stream]</span>
		<div className="box err-box">
			<span className="em err">✕ Stream interrupted · 401 Unauthorized</span>
			<br />
			<span className="dim"> github copilot subscription token expired</span>
			<br />
			<span className="dim"> request_id: rq_8d2f1c · 12:15:08</span>
			<br />
			<br />
			<span className="dim"> Skein paused this harness. Your worktree changes are safe;</span>
			<br />
			<span className="dim"> the partial reply above stays for context. Re-auth and resume,</span>
			<br />
			<span className="dim"> or retry on the same context.</span>
		</div>
		<br />
		<span className="line">
			{"  "}
			<span
				style={{ color: "var(--accent)", cursor: "pointer", textDecoration: "underline" }}
				onClick={onReauth}
			>
				[ Re-authenticate Copilot ]
			</span>
			{"   "}
			<span style={{ cursor: "pointer", textDecoration: "underline" }} onClick={onRetry}>
				[ Retry on same context ]
			</span>
			{"   "}
			<span className="dim">[ Switch harness ]</span>
		</span>
	</div>
);

// ── Right-pane primitives ──────────────────────────────────────────

export const FileTree = ({ tree }: { tree: TreeNode[] }) => (
	<div className="sk-tree">
		{tree.map((n, i) => (
			<div
				key={i}
				className={`sk-tree-row ${n.kind} ${n.touched ? "touched" : ""} ${
					n.active ? "active" : ""
				}`}
			>
				{Array.from({ length: n.depth }).map((_, k) => (
					<span key={k} className="indent" />
				))}
				<span className="icon">{n.kind === "dir" ? (n.open ? "▾" : "▸") : "·"}</span>
				<span>{n.name}</span>
				{n.touched && <span className="badge">{n.touched}</span>}
			</div>
		))}
	</div>
);

export const DiffEditor = ({
	activeFile,
	diff,
}: {
	activeFile: SessionData["activeFile"];
	diff: DiffLine[];
}) => (
	<div className="sk-editor">
		<div className="sk-editor-head">
			<span className="path">{activeFile.path}</span>
			<span style={{ color: "var(--fg-3)" }}>· modified just now</span>
			{activeFile.adds > 0 && <span className="delta-add">+{activeFile.adds}</span>}
			{activeFile.dels > 0 && <span className="delta-del">−{activeFile.dels}</span>}
		</div>
		<div className="sk-code">
			{diff.map((l, i) => (
				<div
					key={i}
					className={`sk-line ${l.kind === "add" ? "add" : l.kind === "del" ? "del" : ""}`}
				>
					<div className="gutter">
						<span className="ln">{l.n1}</span>
						<span className="ln">{l.n2}</span>
					</div>
					<span className="marker">{l.kind === "add" ? "+" : l.kind === "del" ? "−" : ""}</span>
					<span className="src">{l.src}</span>
				</div>
			))}
		</div>
	</div>
);

export const PlanCard = ({ plan }: { plan: PlanItem[] }) => (
	<div className="sk-todo">
		{plan.map((t, i) => (
			<div key={i} className={`row ${t.state}`}>
				<span className="box">{t.state === "done" ? "✓" : t.state === "now" ? "◆" : ""}</span>
				<span style={{ flex: 1 }}>{t.text}</span>
				<span className="by">
					<span style={{ fontFamily: "var(--sk-mono)" }}>{t.by}</span>
				</span>
			</div>
		))}
	</div>
);

export const ActivityFeed = ({ activity }: { activity: ActivityEvent[] }) => (
	<div className="sk-activity">
		{activity.map((a, i) => (
			<div key={i} className="a-row">
				<span className="a-time">{a.time}</span>
				<span className="a-by">
					<HChip kind={a.kind} size={11} />
				</span>
				<span className="a-msg">{a.msg}</span>
			</div>
		))}
		{activity.length === 0 && (
			<div style={{ color: "var(--fg-3)", fontSize: 10.5 }}>No activity yet.</div>
		)}
	</div>
);

// ── Tabs / chrome ──────────────────────────────────────────────────

export const SessionTab = ({
	s,
	active,
	onClick,
}: {
	s: Session;
	active: boolean;
	onClick: () => void;
}) => (
	<div className={`sk-tab ${active ? "active" : ""}`} onClick={onClick} title={s.task}>
		<div className="row-1">
			<StatusDot status={s.status} />
			<span className="name">{s.name}</span>
			{s.badge > 0 && <span className="tab-badge">{s.badge}</span>}
		</div>
		<div className="row-2">
			<span>{s.branch}</span>
			<span>·</span>
			<span style={{ display: "flex", gap: 2 }}>
				{s.harnesses.map((h) => (
					<HChip key={h.id} kind={h.kind} size={9} />
				))}
			</span>
		</div>
	</div>
);

export const HarnessTab = ({
	h,
	active,
	onClick,
	onClose,
}: {
	h: Harness;
	active: boolean;
	onClick: () => void;
	onClose: () => void;
}) => (
	<div className={`sk-harness-tab ${active ? "active" : ""}`} onClick={onClick}>
		<StatusDot status={h.status} size={5} />
		<HChip kind={h.kind} size={11} />
		<span className="ht-name">{h.name}</span>
		<span
			className="ht-x"
			onClick={(e) => {
				e.stopPropagation();
				onClose();
			}}
		>
			×
		</span>
	</div>
);

export const HarnessPicker = ({ onPick }: { onPick: (kind: HarnessKind) => void }) => (
	<div className="sk-empty-harness">
		<h3>Add a harness</h3>
		<p>Pick an agent for this workspace. All harnesses see the same worktree.</p>
		<div className="sk-harness-grid">
			{(Object.values(HARNESS_KINDS) as { id: HarnessKind; name: string; desc: string }[]).map(
				(k) => (
					<div key={k.id} className="sk-harness-card" onClick={() => onPick(k.id)}>
						<div className="head">
							<HChip kind={k.id} size={16} /> <span className="h-name">{k.name}</span>
						</div>
						<div className="h-desc">{k.desc}</div>
					</div>
				),
			)}
		</div>
	</div>
);

// ── Empty / idle / fullpane wrappers ──────────────────────────────

export const FullPaneHead = ({ title, meta }: { title: string; meta?: ReactNode }) => (
	<div className="sk-fullpane-head">
		<span className="title">{title}</span>
		{meta != null && <span className="meta">{meta}</span>}
	</div>
);
