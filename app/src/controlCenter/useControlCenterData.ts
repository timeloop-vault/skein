// Live data for the Control Center (#492): one RoomSnapshot per open
// room. The synchronous half (harness phase/label/times) is read from the
// in-memory stores on every render and re-rendered by their
// subscriptions; the async half (sign-off, last mail status line, branch)
// is fetched only while the view is visible.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { backgroundTasks } from "../backgroundTasks.ts";
import { HARNESS_KINDS } from "../data.tsx";
import { harnessActivity } from "../harnessActivity.ts";
import { harnessDisplayStatus } from "../harnessActivityLabels.ts";
import type { HarnessActivity } from "../harnessActivityTypes.ts";
import { fetchSignoff } from "../review/signoff.ts";
import { subagents } from "../subagents.ts";
import type { Harness, Room } from "../types.ts";
import {
	type HarnessSnapshot,
	isTurnStart,
	maxOrNull,
	type RoomSnapshot,
	type SignoffSnapshot,
} from "./model.ts";

const REFRESH_MS = 10_000;
export const CLOCK_MS = 30_000;
const SYNC_MS = 1_500;

function snapshotSignature(s: HarnessSnapshot): string {
	return `${s.id}:${s.status}:${s.display}:${s.label}:${s.lastActivityAt ?? ""}:${s.since ?? ""}`;
}

interface LastStatusRow {
	roomId: string;
	body: string;
	createdMs: number;
}

interface Remote {
	signoff: SignoffSnapshot | null;
	branch: string | null;
}

/// When something meaningful last happened. `lastOutputAt` is deliberately
/// not used: every PTY chunk bumps it, including cursor/prompt redraws and
/// the resize redraw when the workspace is shown or hidden. An agent
/// (authoritative adapter) has real turn signals and phase changes; a shell
/// or other non-adapter PTY only has a submitted line (its phase also flips
/// idle -> running on a bare redraw, so phaseSince lies) or its spawn/exit.
function meaningfulActivityAt(activity: HarnessActivity | null | undefined): number | null {
	if (!activity) return null;
	const stamps: (number | null)[] = [activity.lastSubmitAt];
	if (activity.authoritative) {
		stamps.push(activity.lastTurnSignal?.at ?? null, activity.phaseSince);
	} else {
		stamps.push(activity.spawnedAt, activity.phase === "exited" ? activity.phaseSince : null);
	}
	return maxOrNull(stamps);
}

function harnessSnapshot(h: Harness, turnStartedAt: number | null): HarnessSnapshot {
	const activity = harnessActivity.get(h.id);
	const pty = HARNESS_KINDS[h.kind].capabilities.pty;
	const { status: display, label } = harnessDisplayStatus(
		pty,
		activity,
		h.pendingNotifications ?? 0,
		subagents.workingCount(h.id),
		backgroundTasks.workingCount(h.id),
	);
	return {
		id: h.id,
		name: h.name,
		kind: h.kind,
		status: activity?.phase ?? "idle",
		display,
		label,
		lastActivityAt: meaningfulActivityAt(activity),
		since: activity?.phaseSince ?? null,
		turnStartedAt,
	};
}

async function fetchRoom(room: Room): Promise<Remote> {
	const cwd = room.cwd ?? "";
	const [signoff, branch] = await Promise.all([
		cwd
			? fetchSignoff(room.id, cwd).then(
					(s): SignoffSnapshot => ({
						approved: s.approved,
						stale: s.stale,
						unresolvedCount: s.unresolvedCount,
						unaddressedCount: s.unaddressedCount,
					}),
					() => null,
				)
			: Promise.resolve(null),
		cwd
			? invoke<string | null>("git_head_branch", { path: cwd }).then(
					(b) => b,
					() => null,
				)
			: Promise.resolve(null),
	]);
	return { signoff, branch };
}

export function useControlCenterData(
	rooms: Room[],
	visible: boolean,
): ReadonlyMap<string, RoomSnapshot> {
	const [version, bump] = useReducer((n: number) => n + 1, 0);
	const [remote, setRemote] = useState<ReadonlyMap<string, Remote>>(new Map());
	const [statuses, setStatuses] = useState<ReadonlyMap<string, LastStatusRow>>(new Map());
	const [now, setNow] = useState(() => Date.now());
	const roomsRef = useRef(rooms);
	roomsRef.current = rooms;
	// Latest new-turn start per harness; recorded even while closed.
	const turnStarts = useRef(new Map<string, number>());

	useEffect(
		() =>
			harnessActivity.subscribeTransitions((id, from, to) => {
				if (isTurnStart(from, to)) turnStarts.current.set(id, Date.now());
			}),
		[],
	);

	// Stable key so the subscriptions only re-attach when the harness set changes.
	const harnessKey = rooms.flatMap((r) => r.harnesses.map((h) => h.id)).join(",");

	// Live stores -> re-render.
	useEffect(() => {
		if (!visible) return;
		bump();
		const unsubs = harnessKey
			.split(",")
			.filter((id) => id !== "")
			.flatMap((id) => [
				harnessActivity.subscribe(id, bump),
				subagents.subscribe(id, bump),
				backgroundTasks.subscribe(id, bump),
			]);
		// Poll a cheap signature of what the rows show: `lastActivityAt` reads
		// `lastTurnSignal` and `lastSubmitAt`, which the activity store mutates
		// in place without emit (harnessActivityDeferral.ts, harnessActivityIo.ts).
		let lastSig = "";
		const poll = setInterval(() => {
			const sig = roomsRef.current
				.flatMap((r) => r.harnesses.map((h) => snapshotSignature(harnessSnapshot(h, null))))
				.join("|");
			if (sig !== lastSig) {
				lastSig = sig;
				bump();
			}
		}, SYNC_MS);
		return () => {
			for (const u of unsubs) u();
			clearInterval(poll);
		};
	}, [visible, harnessKey]);

	// Relative-time clock.
	useEffect(() => {
		if (!visible) return;
		setNow(Date.now());
		const t = setInterval(() => setNow(Date.now()), CLOCK_MS);
		return () => clearInterval(t);
	}, [visible]);

	// Async parts.
	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef is read live; roomIds key re-runs on set change
	useEffect(() => {
		if (!visible) return;
		let cancelled = false;
		let inFlight = false;
		let again = false;

		const run = async () => {
			if (cancelled) return;
			if (inFlight) {
				again = true;
				return;
			}
			inFlight = true;
			try {
				const current = roomsRef.current;
				const [rows, results] = await Promise.all([
					invoke<LastStatusRow[]>("mail_last_status_by_room").catch((e: unknown) => {
						console.warn("[skein] mail_last_status_by_room failed:", e);
						return null;
					}),
					Promise.all(
						current.map(async (r): Promise<[string, Remote | null]> => {
							try {
								return [r.id, await fetchRoom(r)];
							} catch (e) {
								console.warn(`[skein] control center fetch failed for ${r.id}:`, e);
								return [r.id, null];
							}
						}),
					),
				]);
				if (cancelled) return;
				if (rows) {
					setStatuses(new Map(rows.map((row) => [row.roomId, row])));
				}
				setRemote((prev) => {
					const next = new Map<string, Remote>();
					for (const [id, res] of results) {
						const old = prev.get(id);
						if (res) next.set(id, res);
						else if (old) next.set(id, old);
					}
					return next;
				});
			} finally {
				inFlight = false;
				if (again && !cancelled) {
					again = false;
					void run();
				}
			}
		};

		void run();
		const timer = setInterval(() => void run(), REFRESH_MS);
		const unlistens = ["skein://review-changed", "skein://mail-changed"].map((ev) =>
			listen(ev, () => void run()),
		);
		return () => {
			cancelled = true;
			clearInterval(timer);
			for (const u of unlistens) {
				void u.then((off) => off());
			}
		};
	}, [visible, rooms.map((r) => `${r.id}@${r.cwd ?? ""}`).join("|")]);

	// biome-ignore lint/correctness/useExhaustiveDependencies: version/now are the re-read triggers for the external stores
	return useMemo(() => {
		const out = new Map<string, RoomSnapshot>();
		if (!visible) return out;
		for (const room of rooms) {
			const r = remote.get(room.id);
			const s = statuses.get(room.id);
			out.set(room.id, {
				roomId: room.id,
				harnesses: room.harnesses.map((h) =>
					harnessSnapshot(h, turnStarts.current.get(h.id) ?? null),
				),
				signoff: r?.signoff ?? null,
				lastStatus: s ? { body: s.body, createdMs: s.createdMs } : null,
				branch: r?.branch ?? room.branch ?? null,
			});
		}
		return out;
	}, [rooms, remote, statuses, version, now, visible]);
}
