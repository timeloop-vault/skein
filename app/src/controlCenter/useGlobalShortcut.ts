// OS-wide "raise the Control Center" shortcut (#559). Main window only: the
// pop-out loads the same index.html, but main.tsx renders App (and so this
// hook) only outside the pop-out, and the capability grants the plugin to
// "main" alone.

import {
	isRegistered,
	register,
	type ShortcutEvent,
	unregister,
} from "@tauri-apps/plugin-global-shortcut";
import { useEffect, useRef, useState } from "react";
import { GLOBAL_CONTROL_CENTER_ACCELERATOR } from "../shortcuts.ts";

/** The plugin fires on press AND release; only a press is an invocation. */
export const isPress = (e: Pick<ShortcutEvent, "state">): boolean => e.state === "Pressed";

/** Runs async tasks strictly one after another, whatever each one does. A
 *  StrictMode mount/cleanup/mount (or a fast toggle) therefore issues
 *  register, unregister, register in order, so a late register can never
 *  land after the unregister meant for it, or the reverse. */
export const createSerial = () => {
	let tail: Promise<unknown> = Promise.resolve();
	return <T>(task: () => Promise<T>): Promise<T> => {
		const run = tail.then(task, task);
		tail = run.catch(() => {});
		return run;
	};
};

const serial = createSerial();

/** Registers while `enabled`; returns true when registration failed. */
export function useGlobalControlCenterShortcut(enabled: boolean, onTrigger: () => void): boolean {
	const [failed, setFailed] = useState(false);
	const triggerRef = useRef(onTrigger);
	triggerRef.current = onTrigger;

	useEffect(() => {
		if (!enabled) {
			setFailed(false);
			return;
		}
		let cancelled = false;
		serial(async () => {
			// A window reload never runs effect cleanup, so the previous page's
			// registration can still be live on the Rust side.
			try {
				if (await isRegistered(GLOBAL_CONTROL_CENTER_ACCELERATOR)) {
					await unregister(GLOBAL_CONTROL_CENTER_ACCELERATOR);
				}
			} catch {
				// best effort; register below reports a real failure
			}
			await register(GLOBAL_CONTROL_CENTER_ACCELERATOR, (e) => {
				if (isPress(e)) triggerRef.current();
			});
		}).then(
			() => {
				if (!cancelled) setFailed(false);
			},
			(err: unknown) => {
				console.warn("[skein] global shortcut register failed:", err);
				if (!cancelled) setFailed(true);
			},
		);
		return () => {
			cancelled = true;
			// Queued after the register above, so it always undoes it.
			void serial(() => unregister(GLOBAL_CONTROL_CENTER_ACCELERATOR)).catch(() => {});
		};
	}, [enabled]);

	return failed;
}
