// #676: a Skein-owned opencode sidecar that dies under a live TUI. The TUI
// process is left alone; the pane just says what happened.

import { listen } from "@tauri-apps/api/event";

export interface SidecarExitPayload {
	harnessId: string;
	generation: number;
	code: number | null;
}

/** The exit code (null = unknown) of THIS harness's CURRENT sidecar, or
 *  undefined when the event is for someone else. */
export function matchSidecarExit(
	p: SidecarExitPayload,
	harnessId: string,
	generation: number | undefined,
): { code: number | null } | undefined {
	if (p.harnessId !== harnessId || generation === undefined || p.generation !== generation) {
		return undefined;
	}
	return { code: p.code };
}

/** The pane line, written after the PTY's own exit message. */
export function sidecarStoppedLine(code: number | null): string {
	return `[2m[opencode server stopped (exit ${code === null ? "?" : code}) — use Restart harness to reconnect][0m

`;
}

/** Remembers a sidecar death for one mount until the bound PTY exits (Rust
 *  emits the death first, then ends the TUI, so the line lands after the
 *  exit message and the TUI cannot repaint over it). */
export function createSidecarDeathRecord(write: (s: string) => void) {
	let died: { code: number | null } | undefined;
	let ptyExited = false;
	return {
		/** The death event; if the PTY already exited (the two transports are
		 *  unordered) it writes the line itself. */
		note: (code: number | null) => {
			if (ptyExited) write(sidecarStoppedLine(code));
			else died = { code };
		},
		/** Called at PTY exit: the line to write now, once; "" when the sidecar
		 *  has not died (yet, a later `note` then writes it). */
		take: (): string => {
			ptyExited = true;
			const d = died;
			died = undefined;
			return d ? sidecarStoppedLine(d.code) : "";
		},
	};
}

/** Subscribe for one pane; returns the unsubscribe. Safe to call it before
 *  the listener has registered. */
export function watchSidecarExit(
	harnessId: string,
	getGeneration: () => number | undefined,
	onDied: (code: number | null) => void,
): () => void {
	let stopped = false;
	let unlisten: (() => void) | null = null;
	void listen<SidecarExitPayload>("skein://opencode-sidecar-exit", (ev) => {
		const m = matchSidecarExit(ev.payload, harnessId, getGeneration());
		if (m && !stopped) onDied(m.code);
	})
		.then((u) => {
			if (stopped) u();
			else unlisten = u;
		})
		.catch(() => {});
	return () => {
		stopped = true;
		unlisten?.();
		unlisten = null;
	};
}
