// The bottom status bar (#19), moved out of App.tsx's main render —
// pure move, no behaviour change. Props are named after the App.tsx
// locals they replace so the JSX body didn't need to change.

import { AgentStatusBarSeg, LiveStatusBarChip } from "./HarnessColumn.tsx";
import { HChip } from "./components.tsx";
import { HARNESS_KINDS } from "./data.tsx";
import { harnessActivity, statusLabel } from "./harnessActivity.ts";
import type { Harness, Room } from "./types.ts";

export const StatusBar = ({
	activeHarness,
	room,
	liveBranches,
	notifyUrgent,
	activeRooms,
	activeRoomId,
	permissionHarnessIds,
	jumpToHarness,
	switchRoom,
}: {
	activeHarness: Harness;
	room: Room;
	liveBranches: Record<string, string | null>;
	notifyUrgent: boolean;
	activeRooms: Room[];
	activeRoomId: string;
	permissionHarnessIds: ReadonlySet<string>;
	jumpToHarness: (roomId: string, harnessId: string) => void;
	switchRoom: (id: string) => void;
}) => {
	return (
		<div className="sk-statusbar">
			<span className="seg">
				<HChip kind={activeHarness.kind} />
				<span>{HARNESS_KINDS[activeHarness.kind].name}</span>
			</span>
			<AgentStatusBarSeg harness={activeHarness} />
			{/* #49 phase A: name the keyboard holder. Bodies are
			    mutually exclusive, so this is purely informative —
			    but "who has the keyboard" should never need guessing. */}
			<span className="seg" title="Keyboard input goes to the active body">
				⌨ {HARNESS_KINDS[activeHarness.kind].capabilities.pty ? "terminal" : "editor"}
			</span>
			<LiveStatusBarChip harness={activeHarness} />
			{(() => {
				// Prefer the live branch (updated on every watcher tick) but
				// fall back to room.branch on first render before LiveStatus
				// has had a chance to refresh. Issue #18.
				const live = liveBranches[room.id];
				const branch = live === undefined ? room.branch : (live ?? undefined);
				if (!branch) return null;
				const drifted = live !== undefined && live !== null && room.branch && live !== room.branch;
				return (
					<span className="seg" title={drifted ? `worktree branch was ${room.branch}` : undefined}>
						{branch}
						{drifted && <span style={{ color: "var(--warn)", marginLeft: 4 }}>•</span>}
					</span>
				);
			})()}
			{room.cwd && (
				<span className="seg sk-statusbar-cwd" title={room.cwd}>
					{room.cwd}
				</span>
			)}
			<span className="spacer" />
			{notifyUrgent &&
				(() => {
					// #86: a harness blocked on permission ranks above a
					// plain pending-notifications backlog — it's a
					// harder stop, and the room order it's found in
					// breaks ties the same way L5d's own scan does.
					for (const r of activeRooms) {
						if (r.id === activeRoomId) continue;
						const h = r.harnesses.find((hh) => permissionHarnessIds.has(hh.id));
						if (!h) continue;
						// #298: name the subagent when the dialog belongs to
						// one, same wording as `statusLabel` (tool omitted
						// here, as before this change).
						const permAgentType = harnessActivity.get(h.id)?.permissionAgentType;
						return (
							<span
								className="seg sk-statusbar-urgent"
								title={`Jump to ${r.name}`}
								onClick={() => jumpToHarness(r.id, h.id)}
							>
								<span className="dot-tiny st-permission" />
								{r.name} · {h.name} {statusLabel("permission", undefined, permAgentType)}
							</span>
						);
					}
					// L5d — urgent segment. Scan active (non-archived,
					// non-active) rooms for any pending notifications;
					// surface the room with the biggest backlog so
					// the user has a one-click jump to whatever
					// needs attention most. Ties broken by room
					// order. Hidden when no room has anything pending,
					// or when the surface is disabled in Settings (L5e).
					let target: { room: Room; total: number } | null = null;
					for (const r of activeRooms) {
						if (r.id === activeRoomId) continue;
						const total = r.harnesses.reduce((acc, h) => acc + (h.pendingNotifications ?? 0), 0);
						if (total === 0) continue;
						if (!target || total > target.total) target = { room: r, total };
					}
					if (!target) return null;
					return (
						<span
							className="seg sk-statusbar-urgent"
							title={`Jump to ${target.room.name}`}
							onClick={() => {
								// Land on the harness that drove the room to the top —
								// most pending, ties broken by harness order (#65).
								const winner =
									[...target.room.harnesses]
										.filter((h) => (h.pendingNotifications ?? 0) > 0)
										.sort(
											(a, b) => (b.pendingNotifications ?? 0) - (a.pendingNotifications ?? 0),
										)[0] ?? target.room.harnesses[0];
								if (winner) jumpToHarness(target.room.id, winner.id);
								else switchRoom(target.room.id);
							}}
						>
							<span className="dot-tiny st-waiting" />
							{target.room.name}
							<span className="sk-statusbar-urgent-count">{target.total}</span>
						</span>
					);
				})()}
		</div>
	);
};
