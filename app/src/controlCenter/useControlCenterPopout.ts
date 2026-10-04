// Main-window half of the Control Center pop-out (#493). Opens/raises the
// pop-out window, tracks whether it exists, and while it does computes the
// rows (this webview owns the state stores) and broadcasts throttled
// snapshots. Protocol: popoutProtocol.ts; persistence: popoutPrefs.ts.

import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { availableMonitors } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef, useState } from "react";
import type { StripSegment } from "../roomGroups.ts";
import type { Room } from "../types.ts";
import { loadCcPopout, saveCcPopout } from "./popoutPrefs.ts";
import {
	type CcFocusPayload,
	type CcSnapshotPayload,
	EV_FOCUS,
	EV_READY,
	EV_SNAPSHOT,
	POPOUT_LABEL,
} from "./popoutProtocol.ts";
import {
	createThrottle,
	SNAPSHOT_THROTTLE_MS,
	snapshotChanged,
	type Throttled,
} from "./popoutSync.ts";
import { isRectVisible, isSentinelPosition, type LogicalRect } from "./popoutWindow.ts";
import { useControlCenterSections } from "./useControlCenterSections.ts";

const DEFAULT_SIZE = { width: 980, height: 640 };

async function raiseWindow(w: WebviewWindow): Promise<void> {
	await w.unminimize().catch(() => {});
	await w.show().catch(() => {});
	await w.setFocus().catch(() => {});
}

export function useControlCenterPopout(
	rooms: Room[],
	segments: StripSegment[],
	activeRoomId: string,
	/** Activate a room (and harness) the way an in-app row click does. */
	focusRoom: (roomId: string, harnessId?: string) => void,
) {
	const [poppedOut, setPoppedOut] = useState(false);
	const poppedOutRef = useRef(false);
	poppedOutRef.current = poppedOut;

	const { sections, now } = useControlCenterSections(rooms, segments, poppedOut);
	const latest = useRef<CcSnapshotPayload>({ sections, activeRoomId, now });
	latest.current = { sections, activeRoomId, now };
	const throttle = useRef<Throttled<CcSnapshotPayload> | null>(null);
	const lastSent = useRef<{ json: string; now: number } | null>(null);

	const attach = useCallback((w: WebviewWindow) => {
		setPoppedOut(true);
		void w.once("tauri://destroyed", () => setPoppedOut(false));
		void w.once("tauri://error", () => {
			// A failed duplicate create must not hide a live pop-out.
			void WebviewWindow.getByLabel(POPOUT_LABEL).then((existing) => {
				if (!existing) setPoppedOut(false);
			});
		});
	}, []);
	const opening = useRef(false);

	/** Raise an existing pop-out; false when there is none. */
	const raise = useCallback(async (): Promise<boolean> => {
		const w = await WebviewWindow.getByLabel(POPOUT_LABEL);
		if (!w) return false;
		await raiseWindow(w);
		return true;
	}, []);

	const open = useCallback(async () => {
		if (opening.current) return;
		opening.current = true;
		try {
			if (await raise()) return;
			const prefs = loadCcPopout();
			const g = prefs.geometry;
			const monitors = await availableMonitors().catch(() => []);
			const work: LogicalRect[] = monitors.map((m) => {
				const s = m.scaleFactor > 0 ? m.scaleFactor : 1;
				return {
					x: m.workArea.position.x / s,
					y: m.workArea.position.y / s,
					width: m.workArea.size.width / s,
					height: m.workArea.size.height / s,
				};
			});
			// Unknown monitors (empty list) cannot prove a position is gone.
			const placeable =
				g !== null &&
				g !== undefined &&
				(work.length === 0 ? !isSentinelPosition(g) : isRectVisible(g, work));
			const w = new WebviewWindow(POPOUT_LABEL, {
				url: "index.html",
				title: "Skein — Control Center",
				width: g?.width ?? DEFAULT_SIZE.width,
				height: g?.height ?? DEFAULT_SIZE.height,
				...(g && placeable ? { x: g.x, y: g.y } : { center: true }),
				minWidth: 520,
				minHeight: 240,
				alwaysOnTop: prefs.alwaysOnTop,
				focus: true,
			});
			saveCcPopout({ open: true });
			attach(w);
		} finally {
			opening.current = false;
		}
	}, [raise, attach]);

	// A reloaded main webview (dev) may find the pop-out still alive.
	useEffect(() => {
		void WebviewWindow.getByLabel(POPOUT_LABEL).then((w) => {
			if (w) attach(w);
		});
	}, [attach]);

	// Restore after a restart: once rooms have hydrated, reopen if it was open
	// at quit (only the pop-out itself records a user close).
	const restored = useRef(false);
	useEffect(() => {
		if (restored.current || rooms.length === 0) return;
		restored.current = true;
		if (loadCcPopout().open) void open();
	}, [rooms.length, open]);

	// Broadcast. One throttle per open period; a changed-nothing snapshot is dropped.
	useEffect(() => {
		if (!poppedOut) return;
		lastSent.current = null;
		const t = createThrottle<CcSnapshotPayload>((payload) => {
			const c = snapshotChanged(lastSent.current, payload);
			if (!c.send) return;
			lastSent.current = { json: c.json, now: payload.now };
			void emitTo(POPOUT_LABEL, EV_SNAPSHOT, payload).catch(() => {});
		}, SNAPSHOT_THROTTLE_MS);
		throttle.current = t;
		t.push(latest.current);
		return () => {
			t.cancel();
			throttle.current = null;
		};
	}, [poppedOut]);

	// biome-ignore lint/correctness/useExhaustiveDependencies: the dependencies ARE the change triggers
	useEffect(() => {
		throttle.current?.push(latest.current);
	}, [sections, now, activeRoomId]);

	// Pop-out -> main: a fresh snapshot on ready, and row clicks.
	const focusRef = useRef(focusRoom);
	focusRef.current = focusRoom;
	useEffect(() => {
		const offs = [
			listen(EV_READY, () => {
				lastSent.current = null;
				const t = throttle.current;
				if (!t) return;
				t.push(latest.current);
				t.flush();
			}),
			listen<CcFocusPayload>(EV_FOCUS, (e) => {
				focusRef.current(e.payload.roomId, e.payload.harnessId);
				void invoke("window_raise_main").catch((err: unknown) => {
					console.warn("[skein] window_raise_main failed:", err);
				});
			}),
		];
		return () => {
			for (const o of offs) void o.then((off) => off());
		};
	}, []);

	return { poppedOut, poppedOutRef, open, raise };
}
