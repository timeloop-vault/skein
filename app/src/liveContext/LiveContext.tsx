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

import { useCallback, useEffect, useMemo, useState } from "react";
import { usePersistedState } from "../prefs.ts";
import type { Harness, HarnessKind } from "../types.ts";
import { ActivityCardBody, IDLE_AFTER_MS } from "./ActivityCard.tsx";
import { type CardLayout, CardStack, DEFAULT_LAYOUT } from "./CardStack.tsx";
import { DiffCardBody } from "./DiffCard.tsx";
import "./chrome.css";
import { PlanCardBody } from "./PlanCard.tsx";
import { RoomSubtitle } from "./RoomSubtitle.tsx";
import { useRoomActions } from "./store.ts";
import { useGitBranchWatcher } from "./useGitBranchWatcher.ts";

interface LiveContextProps {
	roomId: string;
	cwd: string;
	/** The room's harnesses — used to resolve a row's `harnessId` to a
	 *  `HarnessKind` for its chip (the action store carries ids, chips
	 *  render by kind). */
	harnesses: Harness[];
	/** True iff this room is the active one (its `.sk-right` is
	 *  display:flex, not none). The Activity card needs it to re-pin its
	 *  auto-tail when shown — a hidden card can't measure scroll. */
	visible: boolean;
	/** Per-turn cost hair-lines in the Activity feed. User-level pref
	 *  owned by App (one LiveContext is mounted per room — instance-local
	 *  state would desync across rooms), toggled from the Activity card
	 *  head. Off by default (handover §12, resolved in the buildmap). */
	showTurnCosts: boolean;
	onToggleTurnCosts: () => void;
	/**
	 * Fired with the current HEAD branch on every git-watcher tick (or
	 * `null` for detached HEAD / non-git folders). The status bar reads
	 * this so a `git checkout` inside a harness reflects within the
	 * watcher's debounce window (issue #18) — previously owned by
	 * `LiveStatus`, now that the card stack is the right pane.
	 */
	onBranchChange?: (branch: string | null) => void;
}

/// Idle-duration display: "14m" under an hour, "2h 14m" past it — the
/// §10 long-quiet artboard's format. Minute floor; never sub-minute
/// (the threshold guarantees at least 90 s).
function formatIdle(ms: number): string {
	const mins = Math.floor(ms / 60_000);
	if (mins < 60) return `${mins}m`;
	return `${Math.floor(mins / 60)}h ${mins % 60}m`;
}

export const LiveContext = ({
	roomId,
	cwd,
	harnesses,
	visible,
	showTurnCosts,
	onToggleTurnCosts,
	onBranchChange,
}: LiveContextProps) => {
	const { actions, liveIds } = useRoomActions(roomId);
	const [layout, setLayout] = usePersistedState<CardLayout>(
		`liveContext:layout:${roomId}`,
		DEFAULT_LAYOUT,
	);

	// Resolve harnessId → kind for row chips. Unknown ids (a harness
	// closed since the action was logged) fall back to "byoh" so the
	// chip still renders rather than crashing.
	const harnessKindOf = useMemo(() => {
		const byId = new Map<string, HarnessKind>(harnesses.map((h) => [h.id, h.kind]));
		return (harnessId: string): HarnessKind => byId.get(harnessId) ?? "byoh";
	}, [harnesses]);

	// Keep the status-bar branch live (issue #18) until the Diff card's
	// status/diff fetch lands in D3.
	useGitBranchWatcher(cwd, onBranchChange);

	// "· idle 2h 14m" in the Activity head once the room's been silent
	// past the tail threshold (§10 long-quiet). A per-minute ticker
	// (visible rooms only) keeps the figure fresh; minute granularity is
	// all the format shows.
	const lastActionTs = useMemo(() => {
		let m = 0;
		for (const a of actions) if (a.timestampMs > m) m = a.timestampMs;
		return m;
	}, [actions]);
	const [now, setNow] = useState(() => Date.now());
	useEffect(() => {
		if (!visible) return;
		setNow(Date.now());
		const t = setInterval(() => setNow(Date.now()), 60_000);
		return () => clearInterval(t);
	}, [visible]);
	const idleFor =
		lastActionTs > 0 && now - lastActionTs > IDLE_AFTER_MS
			? formatIdle(now - lastActionTs)
			: undefined;

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

	return (
		<div className="lc-pane">
			<RoomSubtitle actions={actions} />
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
						body: <DiffCardBody />,
					},
					{
						label: "Plan",
						meta: <span>0 now</span>,
						body: <PlanCardBody />,
					},
					{
						label: "Activity",
						meta: (
							<>
								<span>{actions.length} events</span>
								{idleFor && <span className="idle-for">· idle {idleFor}</span>}
								{/* The whole card head is the collapse click target, so
								    the toggle must not let its click bubble. */}
								<button
									type="button"
									className={`lc-cost-toggle ${showTurnCosts ? "on" : ""}`}
									aria-pressed={showTurnCosts}
									title={showTurnCosts ? "hide per-turn cost lines" : "show per-turn cost lines"}
									onClick={(e) => {
										e.stopPropagation();
										onToggleTurnCosts();
									}}
								>
									$ costs
								</button>
							</>
						),
						body: (
							<ActivityCardBody
								actions={actions}
								liveIds={liveIds}
								harnessKindOf={harnessKindOf}
								// Collapsing the card display:nones its body exactly like
								// a room switch does, so it must gate the scroll/animation
								// effects the same way — otherwise rows arriving while
								// collapsed replay their slide-in en masse on expand, and
								// the auto-tail never re-pins.
								visible={visible && !layout.collapsed[2]}
								showTurnCosts={showTurnCosts}
							/>
						),
					},
				]}
			/>
		</div>
	);
};
