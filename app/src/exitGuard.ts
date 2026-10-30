// What every path that ends the process runs first: quit and window
// close (useAppWindowEffects.ts), the post-update restart (#692) and the
// update install itself, which on Windows exits the process (#698).
// Both live in SettingsAbout.tsx. Files buffer text lives only in memory
// (#185), so each one asks before discarding it.

import { invoke } from "@tauri-apps/api/core";
import { confirmDialog } from "./confirmDialog.ts";
import { filesRegistry } from "./filesRegistry.ts";
import { flushPerfSummary } from "./perfTelemetry.ts";
import { flushRoomsBounded } from "./roomsFlush.ts";

// Stops stacked prompts when close is clicked twice.
let promptOpen = false;

/** True when nothing is unsaved or the user accepts discarding it. */
export async function confirmDiscardUnsaved(
	action: "Quit" | "Restart" | "Install",
): Promise<boolean> {
	const dirty = filesRegistry.anyDirty();
	if (dirty.length === 0) return true;
	if (promptOpen) return false;
	promptOpen = true;
	try {
		const shown = dirty.slice(0, 5).join(", ") + (dirty.length > 5 ? ", …" : "");
		return await confirmDialog({
			title: "Unsaved changes",
			message: `${dirty.length} unsaved file${dirty.length === 1 ? "" : "s"} (${shown}) will be discarded. ${action} anyway?`,
			confirmLabel: `${action} anyway`,
			cancelLabel: "Cancel",
			kind: "warning",
		});
	} finally {
		promptOpen = false;
	}
}

/**
 * The steps every exit but quit runs first. Resolves false when the user
 * keeps Skein running. Unlike quit it fails CLOSED: a confirm that cannot
 * be shown rejects, since quitting by hand is still possible and lost
 * text is not recoverable.
 */
export async function prepareExit(action: "Restart" | "Install"): Promise<boolean> {
	// #594: land the debounced rooms save before anything can end the app.
	await flushRoomsBounded();
	if (!(await confirmDiscardUnsaved(action))) return false;
	flushPerfSummary("shutdown");
	// Edits made while the confirm was open.
	await flushRoomsBounded();
	return true;
}

/**
 * #692: relaunch into an update the updater has installed in place.
 * Resolves false when the user keeps Skein running. The restart tears
 * the windows down without a CloseRequested, so the quit handler never
 * sees it and `prepareExit` runs the same steps.
 */
export async function restartApp(): Promise<boolean> {
	if (!(await prepareExit("Restart"))) return false;
	await invoke("app_restart");
	return true;
}

/**
 * #698: install a downloaded update. On Windows the installer launches
 * and the process exits inside `install()`, so the guard runs before it.
 * Resolves false, without installing, when the user keeps Skein running.
 */
export async function installUpdate(update: { install(): Promise<void> }): Promise<boolean> {
	if (!(await prepareExit("Install"))) return false;
	await update.install();
	return true;
}
