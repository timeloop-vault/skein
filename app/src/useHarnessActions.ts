// Harness/room action helpers, extracted out of App.tsx (#19) — pure
// move, no behaviour change. Owns `switchHarnessInRoom` (the shared
// setActiveHarnessId writer), `jumpToHarness` (#65: activate a room AND
// focus a specific harness within it — click-to-jump surfaces), the
// alerted-room/-harness cyclers (#67, `cycleAlertedRoom` /
// `cycleAlertedHarness`, plus the private `topPendingHarness` helper
// they share), `closeHarness` (with its #185 unsaved-files-in-a-Files-
// harness confirm), `updateHarnessCmd` (the Enter-for-shell respawn
// path), `addHarness` (#189's + harness picker toggle), and the #241
// inline-rename trio (`startRenameRoom`/`endRenameRoom`/
// `commitRenameRoom`).
//
// Called from App.tsx right after `useRoomsStore`/`useRoomStripNav`,
// before `useHarnessCreation` — `switchHarnessInRoom` feeds that hook's
// call, and `jumpToHarness` feeds `useHarnessNotifications` just after
// it. None of the functions here actually depend on anything either of
// those hooks returns (they only close over `setRooms`/`activeRooms`/
// `activeRoomId`/`setActiveRoomId`/`setShowPicker`/`setRenaming`, all
// already available at that point), so there is no real ordering cycle
// — everything below moved into this single hook call rather than
// splitting across two.

import type { Dispatch, SetStateAction } from "react";
import { useCallback } from "react";
import { decideCloseHarness, type RequestResult } from "./agentRequests.ts";
import { confirmDialog } from "./confirmDialog.ts";
import { filesRegistry } from "./filesRegistry.ts";
import { harnessActivity } from "./harnessActivity.ts";
import type { RenameTarget } from "./RoomStrip.tsx";
import type { Room } from "./types.ts";

export function useHarnessActions(
	setRooms: Dispatch<SetStateAction<Room[]>>,
	setActiveRoomId: Dispatch<SetStateAction<string>>,
	activeRoomId: string,
	activeRooms: Room[],
	setShowPicker: Dispatch<SetStateAction<string | null>>,
	setRenaming: Dispatch<SetStateAction<RenameTarget | null>>,
) {
	// biome-ignore lint/correctness/useExhaustiveDependencies: setRenaming is a plain useState setter passed in from App.tsx (#19) — stable across renders like any local useState, but biome can't prove that through a function parameter.
	const startRenameRoom = useCallback(
		(roomId: string, host: "group" | "tab" = "tab") => setRenaming({ roomId, host }),
		[],
	);
	// biome-ignore lint/correctness/useExhaustiveDependencies: setRenaming is a plain useState setter passed in from App.tsx (#19) — stable across renders like any local useState, but biome can't prove that through a function parameter.
	const endRenameRoom = useCallback(() => setRenaming(null), []);
	// biome-ignore lint/correctness/useExhaustiveDependencies: setRooms is a plain useState setter passed in from App.tsx (#19) — stable across renders like any local useState, but biome can't prove that through a function parameter.
	const commitRenameRoom = useCallback((roomId: string, name: string) => {
		setRooms((prev) => prev.map((r) => (r.id === roomId ? { ...r, name } : r)));
	}, []);

	const switchHarnessInRoom = (roomId: string, harnessId: string) => {
		setRooms((prev) =>
			prev.map((r) => (r.id === roomId ? { ...r, activeHarnessId: harnessId } : r)),
		);
	};

	// Jump to a specific harness: activate its room AND focus it within
	// that room. Every click-to-jump surface (toast, status-bar urgent
	// indicator, future inbox) should land on the harness that wanted
	// attention, not just its room (#65).
	const jumpToHarness = (roomId: string, harnessId: string) => {
		setActiveRoomId(roomId);
		switchHarnessInRoom(roomId, harnessId);
	};

	// #67: the same "highest-pending harness, ties broken by harness
	// order" picker the urgent indicator uses (#65).
	const topPendingHarness = (room: Room) =>
		[...room.harnesses]
			.filter((h) => (h.pendingNotifications ?? 0) > 0)
			.sort((a, b) => (b.pendingNotifications ?? 0) - (a.pendingNotifications ?? 0))[0] ??
		room.harnesses[0];

	// #67: step to the next/previous room that has any pending
	// notifications, landing on its most-pending harness. Wraps; no-op if
	// nothing is pending. If the current room isn't alerted, a forward
	// step starts at the first alerted room (backward at the last).
	const cycleAlertedRoom = (delta: number) => {
		const alerted = activeRooms.filter(
			(r) => r.harnesses.reduce((a, h) => a + (h.pendingNotifications ?? 0), 0) > 0,
		);
		if (alerted.length === 0) return;
		const idx = alerted.findIndex((r) => r.id === activeRoomId);
		const nextIdx =
			idx === -1
				? delta > 0
					? 0
					: alerted.length - 1
				: (idx + delta + alerted.length) % alerted.length;
		const next = alerted[nextIdx];
		if (!next) return;
		const winner = topPendingHarness(next);
		if (winner) jumpToHarness(next.id, winner.id);
		else setActiveRoomId(next.id);
	};

	// #67: step across every alerted harness (room order × harness order),
	// visiting each once before wrapping. Lands on the exact harness.
	const cycleAlertedHarness = (delta: number) => {
		const tuples: Array<{ roomId: string; harnessId: string }> = [];
		for (const r of activeRooms) {
			for (const h of r.harnesses) {
				if ((h.pendingNotifications ?? 0) > 0) tuples.push({ roomId: r.id, harnessId: h.id });
			}
		}
		if (tuples.length === 0) return;
		const curRoom = activeRooms.find((r) => r.id === activeRoomId);
		const idx = tuples.findIndex(
			(t) => t.roomId === activeRoomId && t.harnessId === curRoom?.activeHarnessId,
		);
		const nextIdx =
			idx === -1
				? delta > 0
					? 0
					: tuples.length - 1
				: (idx + delta + tuples.length) % tuples.length;
		const next = tuples[nextIdx];
		if (next) jumpToHarness(next.roomId, next.harnessId);
	};

	const closeHarness = (roomId: string, harnessId: string) => {
		const proceed = () =>
			setRooms((prev) =>
				prev.map((r) => {
					if (r.id !== roomId) return r;
					const remaining = r.harnesses.filter((h) => h.id !== harnessId);
					if (remaining.length === 0) return r;
					const first = remaining[0];
					if (!first) return r;
					return { ...r, harnesses: remaining, activeHarnessId: first.id };
				}),
			);
		// #185: a files harness may hold unsaved buffers (memory-only).
		// #242: in-app confirm, not plugin-dialog's native one.
		const dirty = filesRegistry.dirtyNames(harnessId);
		if (dirty.length > 0) {
			void confirmDialog({
				title: "Unsaved changes",
				message: `${dirty.length} unsaved file${dirty.length === 1 ? "" : "s"} (${dirty.join(", ")}) will be discarded. Close anyway?`,
				confirmLabel: "Close anyway",
				cancelLabel: "Cancel",
				kind: "warning",
			})
				.then((ok) => {
					if (ok) proceed();
				})
				.catch((err: unknown) => {
					console.error("[skein] confirmDialog failed:", err);
				});
			return;
		}
		proceed();
	};

	// #411: `close_harness` agent verb — same removal `closeHarness` above
	// performs, but never confirms (there's no dialog for an agent call
	// to answer) and refuses outright instead of asking. The refusal
	// ladder (unknown harness / the room's last harness / an open
	// permission dialog / unsaved Files buffers) lives in the pure
	// `decideCloseHarness` so it's unit-tested without this hook; this is
	// only the state mutation once decided, matching `proceed` above
	// harness-for-harness — including its "always land on the first
	// remaining harness" behaviour, since that's what "remove it exactly
	// like closing its tab" means here.
	const closeHarnessForAgent = (
		roomId: string,
		harnessId: string,
	): RequestResult<{ harnessId: string; phase: string }> => {
		const phases = harnessActivity.phaseSnapshot();
		const decision = decideCloseHarness(
			activeRooms,
			roomId,
			harnessId,
			(hid) => phases[hid],
			filesRegistry.dirtyNames,
		);
		if (!decision.ok) return decision;
		const { phase } = decision.value;
		setRooms((prev) =>
			prev.map((r) => {
				if (r.id !== roomId) return r;
				const remaining = r.harnesses.filter((h) => h.id !== harnessId);
				if (remaining.length === 0) return r;
				const first = remaining[0];
				if (!first) return r;
				return { ...r, harnesses: remaining, activeHarnessId: first.id };
			}),
		);
		return { ok: true, value: { harnessId, phase } };
	};

	// When a harness's child exits and the user picks the shell-fallback
	// path, LiveTerminal calls this so the new cmd persists to the DB
	// and a Skein restart re-spawns the shell.
	const updateHarnessCmd = (roomId: string, harnessId: string, cmd: string[]) => {
		// Every cmd change is a deliberate respawn (Enter-for-shell), so
		// bump spawnGen too — that's what remounts the terminal when the
		// new cmd equals the old one (shell→shell, #53).
		setRooms((prev) =>
			prev.map((r) =>
				r.id === roomId
					? {
							...r,
							harnesses: r.harnesses.map((h) =>
								h.id === harnessId ? { ...h, cmd, spawnGen: (h.spawnGen ?? 0) + 1 } : h,
							),
						}
					: r,
			),
		);
	};

	// #433: a design harness's chosen preview entry. Persisted with the
	// rooms blob; no spawnGen bump — there is no process to respawn.
	const setHarnessDesignEntry = (roomId: string, harnessId: string, entry: string) => {
		setRooms((prev) =>
			prev.map((r) =>
				r.id === roomId
					? {
							...r,
							harnesses: r.harnesses.map((h) =>
								h.id === harnessId ? { ...h, designEntry: entry } : h,
							),
						}
					: r,
			),
		);
	};

	// #189: clicking + harness again toggles the picker closed.
	const addHarness = (roomId: string) => setShowPicker((cur) => (cur === roomId ? null : roomId));

	return {
		startRenameRoom,
		endRenameRoom,
		commitRenameRoom,
		switchHarnessInRoom,
		jumpToHarness,
		cycleAlertedRoom,
		cycleAlertedHarness,
		closeHarness,
		closeHarnessForAgent,
		updateHarnessCmd,
		setHarnessDesignEntry,
		addHarness,
	};
}
