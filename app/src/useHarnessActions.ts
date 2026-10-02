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

import { invoke } from "@tauri-apps/api/core";
import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import { useCallback } from "react";
import { decideCloseHarness, type RequestResult } from "./agentRequests.ts";
import { confirmDialog } from "./confirmDialog.ts";
import { HARNESS_KINDS } from "./data.tsx";
import { filesRegistry } from "./filesRegistry.ts";
import { harnessActivity, TRANSITION_SOURCE } from "./harnessActivity.ts";
import { harnessInput } from "./harnessInput.ts";
import type { GateResult } from "./harnessInputGate.ts";
import { canRestart, restartArgv } from "./harnessRestart.ts";
import { mailHold } from "./mailHold.ts";
import type { RenameTarget } from "./RoomStrip.tsx";
import { stillExists } from "./roomsStoreProbe.ts";
import type { Room } from "./types.ts";

/// #490: harness ids with a restart currently awaiting.
const restartsInFlight = new Set<string>();

export function useHarnessActions(
	setRooms: Dispatch<SetStateAction<Room[]>>,
	setActiveRoomId: Dispatch<SetStateAction<string>>,
	activeRoomId: string,
	activeRooms: Room[],
	setShowPicker: Dispatch<SetStateAction<string | null>>,
	setRenaming: Dispatch<SetStateAction<RenameTarget | null>>,
	roomsRef: MutableRefObject<Room[]>,
	setOpencodePorts: Dispatch<SetStateAction<Map<string, number>>>,
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

	// #490: restart a harness in place — kill its process and respawn from
	// its own record (resume form). Callable from code (#491); refuses with
	// a reason, doing nothing, unless `canRestart` passes against LIVE state
	// read now, not render-time props. The gate is evaluated twice: up
	// front, and again after the async port/probe work, because a turn or
	// keystroke can land while awaiting.
	const restartHarness = async (roomId: string, harnessId: string): Promise<GateResult> => {
		// One restart per harness at a time: the awaits below leave a window
		// where a second call would pass the same gates and respawn twice.
		if (restartsInFlight.has(harnessId)) {
			return { ok: false, reason: "a restart is already in progress" };
		}
		restartsInFlight.add(harnessId);
		try {
			return await doRestart(roomId, harnessId);
		} finally {
			restartsInFlight.delete(harnessId);
		}
	};

	const findHarness = (roomId: string, harnessId: string) =>
		roomsRef.current.find((r) => r.id === roomId)?.harnesses.find((x) => x.id === harnessId);

	const doRestart = async (roomId: string, harnessId: string): Promise<GateResult> => {
		const h = findHarness(roomId, harnessId);
		if (!h) return { ok: false, reason: "that harness no longer exists" };
		const gate = (): GateResult =>
			canRestart({
				kind: h.kind,
				capabilities: HARNESS_KINDS[h.kind].capabilities,
				phase: harnessActivity.get(harnessId)?.phase ?? null,
				mailHeld: mailHold.get(harnessId).held,
				draft: harnessInput.draft(harnessId),
			});
		const first = gate();
		if (!first.ok) return first;
		// No sessionId = nothing to resume. Claude would land in its session
		// picker; opencode's `--continue` would pick the most recent
		// conversation in the cwd, which harnesses in a room share, so it
		// could attach a sibling's conversation. Both refuse. A fresh Claude
		// harness spawned with `--session-id` whose transcript isn't written
		// yet also refuses here; that is acceptable. Existence uses the boot
		// path's probe.
		if (!h.sessionId || !(await stillExists(h))) {
			return { ok: false, reason: "no conversation to resume yet" };
		}
		// A fresh embedded-server port: the old one dies with the process.
		// The opencode SSE adapter reads its port from the `opencodePorts`
		// map (HarnessColumn -> LiveTerminal prop), not from the argv.
		let port: number | undefined;
		if (h.kind === "opencode") {
			try {
				port = await invoke<number>("pick_free_port");
			} catch (err) {
				console.warn("[skein] pick_free_port failed; not restarting:", err);
				return { ok: false, reason: "couldn't allocate a port for the restarted harness" };
			}
		}
		const second = gate();
		if (!second.ok) return second;
		// Re-read: sessionId may have changed (/clear, /resume) during the awaits.
		const fresh = findHarness(roomId, harnessId);
		if (!fresh) return { ok: false, reason: "that harness no longer exists" };
		const activity = harnessActivity.get(harnessId);
		const fromPhase = activity?.phase ?? "unknown";
		// Both updates below must land in the same render (React batches
		// them in this tick); otherwise LiveTerminal remounts on the
		// spawnGen bump with the OLD port.
		if (port !== undefined) {
			const p = port;
			setOpencodePorts((prev) => new Map(prev).set(harnessId, p));
		}
		updateHarnessCmd(roomId, harnessId, restartArgv(fresh, port));
		// Source "user-restart" marks a user-initiated respawn: the new
		// process starts out `spawning`.
		invoke("db_record_harness_event", {
			harnessId,
			roomId,
			fromPhase,
			toPhase: "spawning",
			timestampMs: Date.now(),
			hasUserInput: activity?.hasUserInput ?? false,
			source: TRANSITION_SOURCE.UserRestart,
		}).catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.warn(`[skein] db_record_harness_event failed for ${harnessId}:`, msg);
		});
		return { ok: true };
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
		restartHarness,
		setHarnessDesignEntry,
		addHarness,
	};
}
