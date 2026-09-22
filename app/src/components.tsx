// Shared, low-level components used across the app.

import {
	type KeyboardEvent as ReactKeyboardEvent,
	type MouseEvent as ReactMouseEvent,
	type PointerEvent as ReactPointerEvent,
	useEffect,
	useRef,
	useState,
} from "react";
import {
	type AgentInfo,
	type AgentListing,
	kindHasAgents,
	listHarnessAgents,
	validateAgent,
} from "./agents.ts";
import { HARNESS_KINDS } from "./data.tsx";
import type { AgentLabel } from "./harnessAgent.ts";
import { type DefaultAgents, defaultAgentFor } from "./prefs.ts";
import { commitRoomName } from "./roomName.ts";
import type { Harness, HarnessKind, Room, Status } from "./types.ts";
import { OVERLAY_CLOSED_EVENT } from "./useFocusRestore.ts";

// Drop indicator side relative to a tab. useTabDrag's hit-test picks
// "before" if the cursor is left of the tab's horizontal midpoint,
// "after" if right. CSS pseudo-elements render an accent-colored bar
// on the matching edge so the user can see where the drop will land.
// Issue #26; rebuilt on pointer events for #271 (see tabDrag.ts /
// useTabDrag.ts) so the webview's native file-drop handler (#41) can
// share the window — HTML5 DnD and `dragDropEnabled: true` can't
// coexist.
export type DropSide = "before" | "after" | null;

// Drag-related props that both tab kinds share. All optional so the
// presentational tab can render without drag wiring (e.g. in tests).
// `dragKind`/`dragId`/`dragRoomId` are hit-test data attributes read by
// useTabDrag's `elementFromPoint` walk — pointer capture retargets
// pointermove/up to the tab the drag started on, so the DOM event's own
// `target` can't say what's under the cursor now.
export interface DragProps {
	dragging?: boolean;
	dropSide?: DropSide;
	dragKind?: "room" | "harness";
	dragId?: string;
	dragRoomId?: string;
	/** #76: group-aware room drag — which strip segment this tab's drag
	 *  belongs to, and whether the tab IS that segment (a plain room, a
	 *  real or placeholder group lead) or only a non-lead MEMBER of one
	 *  (see roomGroups.ts's `resolveTopDrop`/`resolveRowDrop`). RoomTab-only; HarnessTab
	 *  never sets these. */
	dragSegId?: string;
	dragRole?: "segment" | "member";
	onPointerDown?: (e: ReactPointerEvent<HTMLDivElement>) => void;
	onPointerMove?: (e: ReactPointerEvent<HTMLDivElement>) => void;
	onPointerUp?: (e: ReactPointerEvent<HTMLDivElement>) => void;
	onPointerCancel?: (e: ReactPointerEvent<HTMLDivElement>) => void;
	onLostPointerCapture?: (e: ReactPointerEvent<HTMLDivElement>) => void;
	/** One-shot: true if the click following pointerup is the tail of a
	 *  real drag (threshold crossed) and should not select/activate the
	 *  tab. Consumed by the wrapper below, not passed through to onClick. */
	suppressClick?: () => boolean;
}

// #68: size is owned by CSS (density --chip / --dot tokens + context
// overrides in styles.css), not per-call-site numbers.
// #132: data-kind / data-status feed the shared hover popover
// (statusPopover.ts), which replaces the native title= (slow, unstyled,
// and it couldn't show state). aria-label keeps the info available to
// screen readers.
export const HChip = ({
	kind,
	harnessId,
	agent,
}: {
	kind: HarnessKind;
	harnessId?: string;
	/** #248: the harness's agent label, for the popover. Omitted where the
	 *  chip is a kind rather than one harness (pickers, room-tab row). */
	agent?: AgentLabel | null | undefined;
}) => {
	const k = HARNESS_KINDS[kind];
	// #141: harnessId lets the popover read this harness's OWN live state
	// (so a room-tab summary chip shows its real state, not the room
	// aggregate). Omitted where there's no single harness behind the chip.
	return (
		<span
			className={`h-chip ${k.chip}`}
			data-kind={kind}
			data-harness-id={harnessId}
			data-agent-key={agent?.key}
			data-agent-value={agent?.value}
			aria-label={k.name}
		>
			{k.label}
		</span>
	);
};

export const StatusDot = ({ status }: { status: Status }) => (
	<span className={`tab-status st-${status}`} data-status={status} aria-label={status} />
);

// ── Tabs / chrome ──────────────────────────────────────────────────

// #241: the room tab's name span turns into this on a double-click.
// No keystroke/pointer event may reach the tab underneath — a bare
// keydown would otherwise re-enter the window-level shortcut dispatch
// (App.tsx's `onKey`), and pointerdown/click/dblclick would start a tab
// drag (#271) or select/close the tab.
//
// Focus goes back to the terminal on Enter/Escape only, by dispatching
// `skein:overlay-closed` from those handlers — deliberately NOT via
// `useFocusRestore`, whose unmount-cleanup dispatch is wrong here twice
// over: (1) dev StrictMode runs every effect cleanup once right after
// mount, so the terminal grabbed focus, the input blurred, and blur's
// commit closed the editor the instant it opened; (2) a blur commit
// means the user clicked somewhere else on purpose, and yanking focus
// to the terminal would fight that click.
// Exported so `RoomStrip.tsx`'s `GroupTab` can reuse it directly — the
// top-row group tab renames the SAME `Room.name` field as the lead's
// own second-row tab (#241), just via a different host component.
export const RoomNameInput = ({
	initial,
	onCommit,
	onCancel,
}: {
	initial: string;
	onCommit: (name: string) => void;
	onCancel: () => void;
}) => {
	const [value, setValue] = useState(initial);
	const inputRef = useRef<HTMLInputElement>(null);
	// Enter/blur both commit; Escape cancels. Guards against firing
	// both (Enter's commit unmounts this input, which then blurs).
	const doneRef = useRef(false);

	// Synchronous focus/select on mount, deliberately not deferred to a
	// rAF/setTimeout: the command-palette-invoked path (App.tsx's
	// "Rename room") sets `renamingRoomId` and closes the palette in the
	// same event handler, so this component mounts in the same commit as
	// `CommandPalette` unmounts. React runs every passive-effect cleanup
	// in a commit (including `CommandPalette`'s `useFocusRestore`, which
	// focuses the terminal) before any passive-effect setup in that same
	// commit — so as long as this effect fires here and not later, it
	// runs after the terminal steals focus and wins the tug-of-war.
	useEffect(() => {
		const el = inputRef.current;
		if (!el) return;
		el.focus();
		el.select();
	}, []);

	const commit = () => {
		if (doneRef.current) return;
		doneRef.current = true;
		onCommit(commitRoomName(initial, value));
	};
	const cancel = () => {
		if (doneRef.current) return;
		doneRef.current = true;
		onCancel();
	};

	return (
		<input
			ref={inputRef}
			className="name name-input"
			value={value}
			onChange={(e) => setValue(e.target.value)}
			onBlur={commit}
			// #271: React dispatches bubbling synthetic events target-first,
			// so this fires before the tab root's own onPointerDown — the
			// stopPropagation here reaches (and short-circuits) `startDrag`
			// before it can call `setPointerCapture`, so clicking inside the
			// input to move the caret never lets the root capture the
			// pointer in the first place. click/dblclick are stopped for the
			// same reason: no accidental select/close/re-trigger-rename
			// while editing.
			onPointerDown={(e) => e.stopPropagation()}
			onClick={(e) => e.stopPropagation()}
			onDoubleClick={(e) => e.stopPropagation()}
			onKeyDown={(e: ReactKeyboardEvent<HTMLInputElement>) => {
				e.stopPropagation();
				if (e.key === "Enter") {
					e.preventDefault();
					commit();
					window.dispatchEvent(new Event(OVERLAY_CLOSED_EVENT));
				} else if (e.key === "Escape") {
					e.preventDefault();
					cancel();
					window.dispatchEvent(new Event(OVERLAY_CLOSED_EVENT));
				}
			}}
		/>
	);
};

export const RoomTab = ({
	r,
	active,
	onClick,
	onClose,
	renaming,
	onStartRename,
	onRename,
	onRenameEnd,
	dragging,
	dropSide,
	dragKind,
	dragId,
	dragRoomId,
	dragSegId,
	dragRole,
	onPointerDown,
	onPointerMove,
	onPointerUp,
	onPointerCancel,
	onLostPointerCapture,
	suppressClick,
}: {
	r: Room;
	active: boolean;
	onClick: () => void;
	onClose: () => void;
	/** #241: inline rename. All optional — a caller that doesn't wire
	 *  these (the group lead placeholder, tests) just gets the static
	 *  name span with no way to enter rename mode. */
	renaming?: boolean | undefined;
	onStartRename?: (() => void) | undefined;
	onRename?: ((name: string) => void) | undefined;
	onRenameEnd?: (() => void) | undefined;
} & DragProps) => (
	<div
		className={`sk-tab ${active ? "active" : ""} ${dragging ? "dragging" : ""} ${dropSide ? `drop-${dropSide}` : ""}`}
		onClick={() => {
			// #271: a real drag's pointerup is followed by a click on the
			// same element — swallow that one so dropping doesn't also
			// select the tab. A plain click (no drag) passes straight through.
			if (suppressClick?.()) return;
			onClick();
		}}
		onDoubleClick={(e: ReactMouseEvent<HTMLDivElement>) => {
			// #241/#316: the dblclick handler lives on the TAB ROOT, not the
			// `.name` span, because useTabDrag's `startDrag` calls
			// `e.currentTarget.setPointerCapture` on every pointerdown on
			// this root — and in Chromium/WebView2, once this element has
			// pointer capture, the click/dblclick that follows is dispatched
			// to the CAPTURING element regardless of where the cursor
			// visually is, so `e.target` is always this div and a dblclick
			// handler on the span itself never fires. Hit-test the real
			// point instead (same `elementFromPoint` technique useTabDrag.ts
			// uses to find what's under the cursor during a drag) and only
			// start a rename if that point is actually over this tab's own
			// `.name` span.
			if (!onStartRename || renaming) return;
			const hit = document.elementFromPoint(e.clientX, e.clientY);
			const nameEl = hit instanceof Element ? hit.closest(".name") : null;
			if (!nameEl || !e.currentTarget.contains(nameEl)) return;
			onStartRename();
		}}
		data-drag-kind={dragKind}
		data-drag-id={dragId}
		data-drag-room={dragRoomId}
		data-drag-seg={dragSegId}
		data-drag-role={dragRole}
		onPointerDown={onPointerDown}
		onPointerMove={onPointerMove}
		onPointerUp={onPointerUp}
		onPointerCancel={onPointerCancel}
		onLostPointerCapture={onLostPointerCapture}
	>
		<div className="row-1">
			<StatusDot status={r.status} />
			{/* #132: task tooltip lives on the name, not the whole tab, so
			    hovering a dot/chip shows only the status popover (not the
			    native tooltip on top of it). */}
			{renaming ? (
				<RoomNameInput
					initial={r.name}
					onCommit={(name) => {
						onRename?.(name);
						onRenameEnd?.();
					}}
					onCancel={() => onRenameEnd?.()}
				/>
			) : (
				// #241: dblclick-to-rename is wired on the tab ROOT, not here
				// — see its handler's comment for why.
				<span className="name" title={r.task}>
					{r.name}
				</span>
			)}
			{r.badge > 0 && <span className="tab-badge">{r.badge}</span>}
			{/* #241: hidden mid-rename — closing out from under the input
			    would archive the room `commit`/`onBlur` is about to write
			    a name onto, and a stray click here is an easy miss when the
			    span has just been replaced by an input in the same spot. */}
			{!renaming && (
				<span
					className="sk-tab-close"
					title="Close room"
					onClick={(e) => {
						e.stopPropagation();
						onClose();
					}}
				>
					×
				</span>
			)}
		</div>
		<div className="row-2">
			{r.branch && (
				<>
					<span>{r.branch}</span>
					<span>·</span>
				</>
			)}
			<span style={{ display: "flex", gap: 2 }}>
				{r.harnesses.map((h) => (
					<HChip key={h.id} kind={h.kind} harnessId={h.id} />
				))}
			</span>
		</div>
	</div>
);

export const HarnessTab = ({
	h,
	agent,
	active,
	closable,
	onClick,
	onClose,
	dragging,
	dropSide,
	dragKind,
	dragId,
	dragRoomId,
	onPointerDown,
	onPointerMove,
	onPointerUp,
	onPointerCancel,
	onLostPointerCapture,
	suppressClick,
}: {
	h: Harness;
	/** #248: surfaced in the hover popover, not on the tab — the tab is
	 *  already dense. */
	agent?: AgentLabel | null;
	active: boolean;
	closable: boolean;
	onClick: () => void;
	onClose: () => void;
} & DragProps) => (
	<div
		className={`sk-harness-tab ${active ? "active" : ""} ${dragging ? "dragging" : ""} ${dropSide ? `drop-${dropSide}` : ""}`}
		data-htab={h.id}
		onClick={() => {
			// #271: see RoomTab's onClick — same swallow-the-post-drop-click.
			if (suppressClick?.()) return;
			onClick();
		}}
		data-drag-kind={dragKind}
		data-drag-id={dragId}
		data-drag-room={dragRoomId}
		onPointerDown={onPointerDown}
		onPointerMove={onPointerMove}
		onPointerUp={onPointerUp}
		onPointerCancel={onPointerCancel}
		onLostPointerCapture={onLostPointerCapture}
	>
		<StatusDot status={h.status} />
		<HChip kind={h.kind} harnessId={h.id} agent={agent} />
		<span className="ht-name">{h.name}</span>
		{closable && (
			<span
				className="ht-x"
				onClick={(e) => {
					e.stopPropagation();
					onClose();
				}}
			>
				×
			</span>
		)}
	</div>
);

/** The ⚠ an agent earns by not being able to see the review tools.
 *
 *  Shared by both pickers because the thing it warns about is the same
 *  in both: an agent whose `tools` allowlist omits MCP cannot see
 *  Skein's review server *at all*, with a healthy connection and
 *  nothing in any log (#215). Nobody would find this out by using it —
 *  they would find out that the agent never answers a review comment. */
export const NO_REVIEW_TOOLS_TITLE =
	"This agent's tools list leaves out MCP, so it cannot see Skein's review tools. " +
	"Picking it switches off the review loop for this harness.";

export const NoReviewToolsBadge = () => (
	<span className="sk-agent-warn" title={NO_REVIEW_TOOLS_TITLE}>
		⚠ no review tools
	</span>
);

/** Fetch the agents a kind accepts in `cwd`, for as long as a picker
 *  wants them. `null` kind = nothing to fetch yet.
 *
 *  Re-fetches on every (kind, cwd) change rather than caching: the
 *  whole point of asking the CLI is that the answer moves — an agent
 *  file added while Skein was open must show up on the next open of
 *  the picker, not on the next restart. */
export const useAgentListing = (kind: HarnessKind | null, cwd: string) => {
	const [listing, setListing] = useState<AgentListing | null>(null);
	useEffect(() => {
		if (!kind || !kindHasAgents(kind)) {
			setListing(null);
			return undefined;
		}
		if (!cwd) {
			// No folder to ask about. A degraded listing rather than
			// `null`, which reads as "still loading" and would leave the
			// step spinning forever on a room that has no cwd.
			setListing({
				agents: [],
				degraded: `no folder to ask ${HARNESS_KINDS[kind].name} about`,
				unsupported: false,
			});
			return undefined;
		}

		let cancelled = false;
		setListing(null);
		void listHarnessAgents(kind, cwd).then((l) => {
			if (!cancelled) setListing(l);
		});
		return () => {
			cancelled = true;
		};
	}, [kind, cwd]);
	return listing;
};

/** Second step of the picker: which agent this harness runs as.
 *
 *  `(tool default)` is the first row and not a decoration — it is what
 *  every harness before #247 did, and it means "whatever the tool's own
 *  `agent` setting says", which no named row can express. Not labelled
 *  plain "(default)" since #248: the Settings default is a *name*, and
 *  the two must not read as the same thing.
 *
 *  #248: the kind's Settings default arrives preselected and Enter takes
 *  it, so a settled-in user spends one keystroke rather than a hunt. A
 *  default the CLI has since dropped is not preselected — the row it
 *  would have highlighted is gone, and quietly highlighting something
 *  else would spawn an agent nobody chose — so the step falls back to
 *  `(tool default)` and says why. */
const AgentStep = ({
	kind,
	listing,
	preferred,
	active,
	onPick,
	onBack,
}: {
	kind: HarnessKind;
	listing: AgentListing | null;
	/** The kind's default from Settings, if any. */
	preferred: string | undefined;
	/** Whether this picker's room is the one on screen. */
	active: boolean;
	onPick: (agent: string | undefined) => void;
	onBack: () => void;
}) => {
	const tool = HARNESS_KINDS[kind].name;
	// Before the list arrives the default cannot be disproved, so it is
	// preselected; the spawn re-checks the name anyway (#247).
	const gone = listing !== null && validateAgent(preferred, listing).kind === "unknown";
	const selected = gone ? undefined : preferred;

	// Enter *spawns*, so the listener is held to the one picker the user
	// is looking at. Every room's column stays mounted and a picker can
	// be left open in a room the user switched away from; unscoped, an
	// Enter typed into another room's terminal would spawn a harness here,
	// out of sight. Same for an Enter aimed at a field or a modal.
	useEffect(() => {
		if (!active) return undefined;
		const onKey = (e: KeyboardEvent) => {
			if (e.key !== "Enter" || e.repeat || e.defaultPrevented) return;
			const t = e.target instanceof HTMLElement ? e.target : null;
			if (
				t?.closest("input, textarea, select, [contenteditable], .sk-modal, .sk-palette, .xterm")
			) {
				return;
			}
			e.preventDefault();
			onPick(selected);
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [active, onPick, selected]);

	// The highlighted row is what Enter takes, so it has to be on screen:
	// a default near the bottom of a long plugin list would otherwise sit
	// out of view while Enter spawns it. Re-run when the list lands, since
	// the row does not exist until then. Scrolls the list only —
	// `scrollIntoView` would also scroll the panes around the picker.
	const listRef = useRef<HTMLDivElement | null>(null);
	// biome-ignore lint/correctness/useExhaustiveDependencies: selected + listing are deliberate triggers — the highlighted row moves or first appears without the effect reading either
	useEffect(() => {
		const list = listRef.current;
		const row = list?.querySelector<HTMLElement>(".sk-agent-row.selected");
		if (!list || !row) return;
		const l = list.getBoundingClientRect();
		const r = row.getBoundingClientRect();
		if (r.top >= l.top && r.bottom <= l.bottom) return;
		list.scrollTop += r.top - l.top - (l.height - r.height) / 2;
	}, [selected, listing]);

	return (
		<>
			<h3>{tool} — which agent?</h3>
			<p>
				Bound at launch and fixed for the life of the conversation. Skein re-passes it on every
				resume. Enter picks the highlighted row.
			</p>
			<div className="sk-agent-list" ref={listRef}>
				<div
					className={`sk-agent-row ${selected === undefined ? "selected" : ""}`}
					onClick={() => onPick(undefined)}
				>
					<div className="head">
						<span className="a-name">(tool default)</span>
					</div>
					<div className="a-desc">No agent named — whatever {tool} is configured to use.</div>
				</div>
				{gone && preferred && (
					<div className="sk-agent-note warn">
						Your default "{preferred}" is not one {tool} offers here any more.
					</div>
				)}
				{listing === null ? (
					<div className="sk-agent-note">asking {tool}…</div>
				) : (
					<>
						{listing.degraded && (
							<div className="sk-agent-note warn">
								This list may be incomplete — {listing.degraded}
							</div>
						)}
						{listing.agents.map((a) => (
							<AgentRow
								key={a.name}
								agent={a}
								selected={a.name === selected}
								isDefault={a.name === preferred}
								onClick={() => onPick(a.name)}
							/>
						))}
					</>
				)}
			</div>
			<button className="sk-btn" type="button" onClick={onBack}>
				← Back
			</button>
		</>
	);
};

const AgentRow = ({
	agent,
	selected,
	isDefault,
	onClick,
}: {
	agent: AgentInfo;
	selected: boolean;
	isDefault: boolean;
	onClick: () => void;
}) => (
	<div
		className={`sk-agent-row ${selected ? "selected" : ""}`}
		onClick={onClick}
		title={agent.name}
	>
		<div className="head">
			<span className="a-name">{agent.name}</span>
			{isDefault && <span className="sk-agent-tag">your default</span>}
			{!agent.allowsReviewTools && <NoReviewToolsBadge />}
		</div>
		{agent.description && <div className="a-desc">{agent.description}</div>}
	</div>
);

// The full-pane harness picker: `+ harness` swaps the body slot for
// this card grid until a kind is picked. (The files-pillar design
// proposed an anchored dropdown here instead; the owner prefers the
// pane — 2026-07-14.) Iterating the registry means new kinds (Files)
// appear as cards automatically.
//
// #247 makes it two-step for the kinds that take `--agent`: the grid
// swaps for an agent list instead of spawning, because the agent is
// bound at launch and Claude cannot change it afterwards — "pick it
// later" is not on offer. Kinds without the capability still spawn on
// click, so the extra step only appears where it buys something.
export const HarnessPicker = ({
	cwd,
	defaultAgents,
	active,
	onPick,
	onCancel,
}: {
	/** The room's worktree — project agents resolve relative to it. */
	cwd: string;
	/** Settings' per-kind defaults (#248), preselected in the agent step. */
	defaultAgents: DefaultAgents;
	/** The picker's room is the active one — gates the agent step's Enter. */
	active: boolean;
	onPick: (kind: HarnessKind, agent?: string) => void;
	onCancel: () => void;
}) => {
	const [step, setStep] = useState<HarnessKind | null>(null);
	// Fetched as soon as the kind is chosen, not on hover or on mount:
	// the probe runs the harness CLI, and a grid that shells out five
	// times just to be looked at would be a poor trade for a subtitle.
	const listing = useAgentListing(step, cwd);
	return (
		<div className="sk-empty-harness">
			{/* #189: the picker needs a way out that isn't picking. Esc still
			    cancels the whole picker from the agent step too — Back is for
			    changing your mind about the kind, not for leaving. */}
			<span className="sk-empty-harness-x" title="Cancel (Esc)" onClick={onCancel}>
				×
			</span>
			{step ? (
				<AgentStep
					kind={step}
					listing={listing}
					preferred={defaultAgentFor(defaultAgents, step)}
					active={active}
					onPick={(agent) => onPick(step, agent)}
					onBack={() => setStep(null)}
				/>
			) : (
				<>
					<h3>Add a harness</h3>
					<p>Pick an agent for this workspace. All harnesses see the same worktree.</p>
					<div className="sk-harness-grid">
						{(
							Object.values(HARNESS_KINDS) as { id: HarnessKind; name: string; desc: string }[]
						).map((k) => (
							<div
								key={k.id}
								className="sk-harness-card"
								onClick={() => (kindHasAgents(k.id) ? setStep(k.id) : onPick(k.id))}
							>
								<div className="head">
									<HChip kind={k.id} /> <span className="h-name">{k.name}</span>
								</div>
								<div className="h-desc">{k.desc}</div>
							</div>
						))}
					</div>
				</>
			)}
		</div>
	);
};
