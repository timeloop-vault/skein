// Live Context — right-pane card stack (issue #80).
//
// Replaces the old Status/Files tabbed pane. A vertical stack of three
// resizable cards — Diff, Plan, Activity — that render what every agent
// in the room is doing, sourced from the `harness_actions` table via
// `useRoomActions`.
//
// This is the D1 slice: card-stack chrome (collapse, drag-resize,
// per-room layout persistence), the room subtitle (latest away_summary),
// and the live subscription wired through to the Activity header's event
// count. The card *bodies* are filled in later slices:
//   - D2: Activity card rows + burst-collapse + auto-tail
//   - D3: Diff card (Monaco diff + auto-focus + flicker)
//   - D4: Plan card + sub-agent inspector
//
// Spec: docs/live-context-handover.md. Visual reference:
// docs/design/skein/project/Live Context.html.

import { Channel, invoke } from "@tauri-apps/api/core";
import { Fragment, type ReactNode, useCallback, useEffect, useRef } from "react";
import { type HarnessAction, useRoomActions } from "./liveContext.ts";
import { usePersistedState } from "./prefs.ts";

interface LiveContextProps {
	roomId: string;
	cwd: string;
	/**
	 * Fired with the current HEAD branch on every git-watcher tick (or
	 * `null` for detached HEAD / non-git folders). The status bar reads
	 * this so a `git checkout` inside a harness reflects within the
	 * watcher's debounce window (issue #18) — previously owned by
	 * `LiveStatus`, now that the card stack is the right pane.
	 */
	onBranchChange?: (branch: string | null) => void;
}

/** Per-room card layout: flex weights + collapsed flags, indices
 *  [diff, plan, activity]. Persisted to localStorage keyed by room. */
interface CardLayout {
	weights: [number, number, number];
	collapsed: [boolean, boolean, boolean];
}

const DEFAULT_LAYOUT: CardLayout = {
	weights: [1, 1, 1],
	collapsed: [false, false, false],
};

/** Minimum flex weight per card while dragging — 8% of the total
 *  (handover §8) so a card can't be dragged to nothing. */
const MIN_WEIGHT_FRACTION = 0.08;

export const LiveContext = ({ roomId, cwd, onBranchChange }: LiveContextProps) => {
	const { actions } = useRoomActions(roomId);
	const [layout, setLayout] = usePersistedState<CardLayout>(
		`liveContext:layout:${roomId}`,
		DEFAULT_LAYOUT,
	);

	// Keep the status-bar branch live. The old LiveStatus ran a full
	// git status/diff watcher; D1 only needs the branch (the Diff card's
	// status/diff fetch lands in D3). One lightweight watcher tick →
	// git_head_branch keeps issue #18 working in the meantime.
	useGitBranchWatcher(cwd, onBranchChange);

	const toggleCollapse = useCallback(
		(i: number) => {
			setLayout((prev) => {
				const collapsed = [...prev.collapsed] as [boolean, boolean, boolean];
				collapsed[i] = !collapsed[i];
				return { ...prev, collapsed };
			});
		},
		[setLayout],
	);

	const subtitle = deriveSubtitle(actions);

	return (
		<div className="lc-pane">
			<RoomSubtitle subtitle={subtitle} />
			<CardStack
				layout={layout}
				onLayoutChange={setLayout}
				onToggleCollapse={toggleCollapse}
				cards={[
					{
						label: "Diff",
						meta: (
							<>
								<span className="pulse" /> auto-follow
							</>
						),
						body: <DiffPlaceholder />,
					},
					{
						label: "Plan",
						meta: <span>0 now</span>,
						body: <PlanPlaceholder />,
					},
					{
						label: "Activity",
						meta: <span>{actions.length} events</span>,
						body: <ActivityPlaceholder count={actions.length} />,
					},
				]}
			/>
		</div>
	);
};

// ── Card stack + drag-resize ───────────────────────────────────────

interface CardDef {
	label: string;
	meta: ReactNode;
	body: ReactNode;
}

interface CardStackProps {
	layout: CardLayout;
	onLayoutChange: (next: CardLayout) => void;
	onToggleCollapse: (i: number) => void;
	cards: [CardDef, CardDef, CardDef];
}

const CardStack = ({ layout, onLayoutChange, onToggleCollapse, cards }: CardStackProps) => {
	const stackRef = useRef<HTMLDivElement>(null);

	// Drag a divider between card `i` and `i+1`: move flex weight from
	// one to the other, proportional to the pointer's vertical travel
	// over the stack height. Persisted on release via onLayoutChange.
	const onDividerDown = (i: number) => (e: React.PointerEvent) => {
		e.preventDefault();
		const stack = stackRef.current;
		if (!stack) return;
		const height = stack.getBoundingClientRect().height;
		if (height <= 0) return;
		const startY = e.clientY;
		const startWeights = [...layout.weights] as [number, number, number];
		const sum = startWeights[0] + startWeights[1] + startWeights[2];
		const minW = sum * MIN_WEIGHT_FRACTION;
		// The two cards either side of this divider. Captured up front
		// so the move handler does no tuple-by-variable indexing.
		const wA = startWeights[i] ?? 0;
		const wB = startWeights[i + 1] ?? 0;

		let latest = layout;
		const onMove = (ev: PointerEvent) => {
			const deltaPx = ev.clientY - startY;
			let deltaW = (deltaPx / height) * sum;
			// Clamp so neither adjacent card drops below the minimum.
			deltaW = Math.max(minW - wA, Math.min(deltaW, wB - minW));
			const weights = [...startWeights] as [number, number, number];
			weights[i] = wA + deltaW;
			weights[i + 1] = wB - deltaW;
			latest = { ...layout, weights };
			onLayoutChange(latest);
		};
		const onUp = () => {
			window.removeEventListener("pointermove", onMove);
			window.removeEventListener("pointerup", onUp);
			onLayoutChange(latest);
		};
		window.addEventListener("pointermove", onMove);
		window.addEventListener("pointerup", onUp);
	};

	return (
		<div className="lc-stack" ref={stackRef}>
			{cards.map((card, i) => (
				<Fragment key={card.label}>
					<div
						className={`lc-card ${layout.collapsed[i] ? "collapsed" : ""}`}
						style={{ flex: layout.weights[i] }}
					>
						<div className="lc-card-head" onClick={() => onToggleCollapse(i)}>
							<span className="chev">▾</span>
							<span className="label">{card.label}</span>
							<span className="meta">{card.meta}</span>
						</div>
						<div className="lc-card-body">{card.body}</div>
					</div>
					{/* Pointer-drag resize affordance. Keyboard resize is
					    out of scope for v1 (handover §8), so no ARIA role —
					    it's a mouse-only divider, not a focusable widget. */}
					{i < cards.length - 1 && <div className="lc-divider" onPointerDown={onDividerDown(i)} />}
				</Fragment>
			))}
		</div>
	);
};

// ── Room subtitle ──────────────────────────────────────────────────

interface Subtitle {
	text: string;
	age: string;
	empty: boolean;
}

const RoomSubtitle = ({ subtitle }: { subtitle: Subtitle }) => (
	<div className={`lc-subtitle ${subtitle.empty ? "is-empty" : ""}`}>
		<span className="glyph">{subtitle.empty ? "IDLE" : "AT"}</span>
		<span className="text">{subtitle.text}</span>
		{!subtitle.empty && <span className="meta">{subtitle.age}</span>}
	</div>
);

/** Latest `away_summary` action becomes the subtitle; otherwise the
 *  empty state. away_summary payload is `{ content: string }`. */
function deriveSubtitle(actions: HarnessAction[]): Subtitle {
	for (let i = actions.length - 1; i >= 0; i--) {
		const a = actions[i];
		if (a && a.kind === "away_summary") {
			let content = "";
			try {
				content = (JSON.parse(a.payload) as { content?: string }).content ?? "";
			} catch {
				content = "";
			}
			if (content) {
				return { text: content, age: relativeAge(a.timestampMs), empty: false };
			}
		}
	}
	return { text: "No agent has worked here yet", age: "", empty: true };
}

function relativeAge(ms: number): string {
	if (ms <= 0) return "";
	const secs = Math.max(0, Math.round((Date.now() - ms) / 1000));
	if (secs < 60) return `${secs}s ago`;
	const mins = Math.round(secs / 60);
	if (mins < 60) return `${mins}m ago`;
	const hrs = Math.round(mins / 60);
	if (hrs < 24) return `${hrs}h ago`;
	return `${Math.round(hrs / 24)}d ago`;
}

// ── Placeholder bodies (replaced in D2–D4) ─────────────────────────

const DiffPlaceholder = () => (
	<div className="lc-empty">
		<div className="lc-empty-inner">
			<div className="big">◇</div>
			when an agent edits a file, its diff appears here
		</div>
	</div>
);

const PlanPlaceholder = () => (
	<div className="lc-empty">
		<div className="lc-empty-inner">
			<div className="big">·</div>
			no plan items yet — agents will populate this as they work
		</div>
	</div>
);

const ActivityPlaceholder = ({ count }: { count: number }) => {
	if (count === 0) {
		return (
			<div className="lc-empty">
				<div className="lc-empty-inner">
					<div className="big">☰</div>
					activity will appear here as agents work
				</div>
			</div>
		);
	}
	return (
		<div className="lc-activity">
			<div
				style={{
					padding: "10px 12px",
					fontFamily: "var(--sk-mono)",
					fontSize: 11,
					color: "var(--fg-2)",
				}}
			>
				{count} action{count === 1 ? "" : "s"} captured — row rendering lands in the next slice
			</div>
			<div className="lc-tail">
				<span className="blinker" />
				tailing — new rows will appear live
			</div>
		</div>
	);
};

// ── git branch watcher ─────────────────────────────────────────────
// Lightweight successor to LiveStatus's watcher for the branch-only
// need in D1. Re-fetches HEAD branch on each debounced filesystem tick.

function useGitBranchWatcher(cwd: string, onBranchChange?: (branch: string | null) => void) {
	const cbRef = useRef(onBranchChange);
	cbRef.current = onBranchChange;

	useEffect(() => {
		let cancelled = false;

		const refreshBranch = async () => {
			try {
				const repo = await invoke<boolean>("git_is_repo", { path: cwd });
				if (cancelled) return;
				if (!repo) {
					cbRef.current?.(null);
					return;
				}
				const branch = await invoke<string | null>("git_head_branch", { path: cwd });
				if (!cancelled) cbRef.current?.(branch);
			} catch (err: unknown) {
				const msg = err instanceof Error ? err.message : String(err);
				console.warn(`[skein] branch watch failed for ${cwd}:`, msg);
			}
		};

		void refreshBranch();

		const channel = new Channel<null>();
		channel.onmessage = () => {
			void refreshBranch();
		};
		let watchId: string | null = null;
		invoke<string>("git_watch_start", { path: cwd, onChange: channel })
			.then((id) => {
				if (cancelled) {
					void invoke("git_watch_stop", { id });
					return;
				}
				watchId = id;
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.error("[skein] git_watch_start failed:", msg);
			});

		return () => {
			cancelled = true;
			if (watchId) void invoke("git_watch_stop", { id: watchId });
		};
	}, [cwd]);
}
