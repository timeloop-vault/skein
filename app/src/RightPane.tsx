// The right pane's tab strip — Live Context and Review (#212).
//
// The right pane held exactly one thing (the Live Context card stack)
// until the review surface needed somewhere to live. A harness tab was
// the obvious home and the wrong one: harnesses are many-per-room by
// construction, and a review is one per room. A right-pane tab keeps it
// singular, keeps the agent's terminal visible beside it — which is the
// point of reviewing in the room at all — and lets the `Splitter` the
// pane already sits in do the width negotiation: drag it wide to read a
// diff, drag it back when you are done.
//
// Both panes stay mounted and are hidden with `display:none`, exactly
// as rooms and harnesses are, so switching tabs never costs a re-fetch
// or loses the Activity feed's scroll position.
//
// This is deliberately a small, named seam. When panes become movable
// (the eventual answer, since no one arrangement suits everyone), the
// thing that needs a stable identity per pane is this list.

import { useCallback } from "react";
import { LiveContext } from "./liveContext/index.ts";
import { useRoomActions } from "./liveContext/store.ts";
import { ReviewPane } from "./review/ReviewPane.tsx";
import type { Harness, HarnessKind } from "./types.ts";
import "./rightPane.css";

export type RightPaneTab = "context" | "review";

const TABS: Array<{ id: RightPaneTab; label: string; title: string }> = [
	{ id: "context", label: "Live Context", title: "what the agents are doing right now" },
	{ id: "review", label: "Review", title: "review this branch — diff and comments" },
];

export const RightPane = ({
	roomId,
	cwd,
	roomName,
	harnesses,
	visible,
	showTurnCosts,
	onToggleTurnCosts,
	onBranchChange,
	tab,
	onTabChange,
}: {
	roomId: string;
	cwd: string;
	/** Only the land dialog needs it, to name what is being landed. */
	roomName: string;
	harnesses: Harness[];
	visible: boolean;
	showTurnCosts: boolean;
	onToggleTurnCosts: () => void;
	onBranchChange?: ((roomId: string, branch: string | null) => void) | undefined;
	tab: RightPaneTab;
	onTabChange: (tab: RightPaneTab) => void;
}) => {
	// The review pane needs the room's action rows for per-hunk harness
	// attribution (D4). `useRoomActions` is a subscription to a shared
	// store, so reading it here and in LiveContext costs one backfill.
	const { actions } = useRoomActions(roomId);

	const harnessKindOf = useCallback(
		(harnessId: string): HarnessKind => harnesses.find((h) => h.id === harnessId)?.kind ?? "byoh",
		[harnesses],
	);

	return (
		<div className="sk-rp">
			<div className="sk-rp-tabs">
				{TABS.map((t) => (
					<button
						type="button"
						key={t.id}
						className={`sk-rp-tab${tab === t.id ? " active" : ""}`}
						title={t.title}
						onClick={() => onTabChange(t.id)}
					>
						{t.label}
					</button>
				))}
			</div>

			<div className="sk-rp-body" style={{ display: tab === "context" ? "flex" : "none" }}>
				<LiveContext
					roomId={roomId}
					cwd={cwd}
					harnesses={harnesses}
					visible={visible && tab === "context"}
					showTurnCosts={showTurnCosts}
					onToggleTurnCosts={onToggleTurnCosts}
					onBranchChange={onBranchChange}
				/>
			</div>

			<div className="sk-rp-body" style={{ display: tab === "review" ? "flex" : "none" }}>
				{/* `visible` gates the work, not the mount: the review runs a
				    worktree watcher and a merge-base walk, and a room whose
				    review tab is not on screen should pay for neither. */}
				<ReviewPane
					roomId={roomId}
					cwd={cwd}
					roomName={roomName}
					actions={actions}
					harnessKindOf={harnessKindOf}
					visible={visible && tab === "review"}
				/>
			</div>
		</div>
	);
};
