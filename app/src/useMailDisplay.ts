// Mail's display duties (#329, #413), the whole of what the webview does with
// mail since the daemon delivers it itself (#733): keep `mailStore.ts`'s
// per-harness unread marker fresh, and mirror the daemon's held-mail set into
// `mailHold.ts`. Called once from `useAppBackgroundWiring` with the current
// (non-archived) rooms; the tab marker lives in `HarnessTab` via
// `useUnreadMail` / `useMailHold`.
//
// The marker refreshes on `skein://mail-changed` (a message landed for the
// harness, or it read some) and once when a harness first appears. The held
// set is seeded from `mail_held_snapshot` and kept by `skein://mail-held`
// (`mailHeldMirror.ts` orders the two so neither loses an update).

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";
import { logBoth } from "./frontendLog.ts";
import { mailHeldMirror } from "./mailHeldMirrorInstance.ts";
import { mailHold } from "./mailHold.ts";
import { type MailUnread, mailStore } from "./mailStore.ts";
import type { Room } from "./types.ts";

export function useMailDisplay(rooms: readonly Room[]): void {
	const roomOfRef = useRef<Map<string, string>>(new Map());
	const seededRef = useRef<Set<string>>(new Set());

	const refreshMarker = (roomId: string, harnessId: string): void => {
		invoke<MailUnread>("mail_unread", { roomId, harnessId })
			.then((res) => {
				mailStore.set(harnessId, res.count, res.fromRoomNames);
			})
			.catch((err: unknown) => {
				logBoth(
					"warn",
					"skein::mail",
					`[skein] mail_unread failed for ${harnessId}: ${String(err)}`,
				);
			});
	};

	// biome-ignore lint/correctness/useExhaustiveDependencies: refreshMarker closes only over refs and module singletons.
	useEffect(() => {
		const roomOf = roomOfRef.current;
		const seen = new Set<string>();
		for (const room of rooms) {
			for (const h of room.harnesses) {
				seen.add(h.id);
				roomOf.set(h.id, room.id);
			}
		}
		// Forget harnesses that are gone (closed, or the room archived).
		for (const id of [...roomOf.keys()]) {
			if (seen.has(id)) continue;
			roomOf.delete(id);
			seededRef.current.delete(id);
			mailStore.forget(id);
			mailHold.forget(id);
			mailHeldMirror.forget(id);
		}
		for (const id of seen) {
			if (seededRef.current.has(id)) continue;
			seededRef.current.add(id);
			const roomId = roomOf.get(id);
			if (roomId !== undefined) refreshMarker(roomId, id);
		}
	}, [rooms]);

	// biome-ignore lint/correctness/useExhaustiveDependencies: refreshMarker closes only over refs and module singletons.
	useEffect(() => {
		const unlistenPromise = listen<{ roomId: string; harnessId: string }>(
			"skein://mail-changed",
			(event) => {
				const { harnessId } = event.payload;
				const roomId = roomOfRef.current.get(harnessId);
				if (roomId !== undefined) refreshMarker(roomId, harnessId);
			},
		);
		return () => {
			void unlistenPromise.then((un) => un());
		};
	}, []);

	// Listen first, then ask for the snapshot: an event in between is buffered
	// by the mirror and replayed on top of it.
	useEffect(() => {
		let cancelled = false;
		const unlistenPromise = listen<{ harnessId: string; held: boolean; seq?: number }>(
			"skein://mail-held",
			(event) => mailHeldMirror.event(event.payload),
		);
		void unlistenPromise.then(() =>
			invoke<{ seq: number; harnessIds: string[] }>("mail_held_snapshot")
				.then((snap) => {
					if (!cancelled) mailHeldMirror.snapshot(snap.harnessIds, snap.seq);
				})
				.catch((err: unknown) => {
					logBoth("warn", "skein::mail", `[skein] mail_held_snapshot failed: ${String(err)}`);
				}),
		);
		return () => {
			cancelled = true;
			void unlistenPromise.then((un) => un());
		};
	}, []);
}
