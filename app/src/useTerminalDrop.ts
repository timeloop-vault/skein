// useTerminalDrop — native OS file drop onto a terminal pane (#41).
//
// `dragDropEnabled` in tauri.conf.json (#271) routes real paths through the
// webview's own `onDragDropEvent` instead of the browser's `drop` event, which
// carries no paths at all. Subscribed only while `visible` — a hidden pane
// (display:none, PTY still alive) shouldn't steal a drop meant for whichever
// pane the user is actually looking at.
//
// `capabilities.pty` is checked too even though App.tsx only ever mounts
// LiveTerminal for pty-capable kinds — belt and suspenders.

import type { PhysicalPosition } from "@tauri-apps/api/dpi";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { Terminal } from "@xterm/xterm";
import { type RefObject, useEffect, useState } from "react";
import { HARNESS_KINDS } from "./data.tsx";
import { checkGate } from "./gateCheck.ts";
import { harnessActivity } from "./harnessActivity.ts";
import { formatDroppedPaths, harnessInput, insertText } from "./harnessInput.ts";
import type { GateResult } from "./harnessInputGate.ts";
import { checkRegistered } from "./harnessInputGate.ts";
import { createDropGate } from "./terminalDrop.ts";
import type { HarnessKind } from "./types.ts";

/// Returns what the drag overlay shows: null while nothing is dragged over
/// the pane, else the gate answer (`ok` = "will insert", else the reason).
export function useTerminalDrop(
	harnessId: string,
	harnessKind: HarnessKind,
	visible: boolean,
	containerRef: RefObject<HTMLDivElement | null>,
	termRef: RefObject<Terminal | null>,
): GateResult | null {
	const [dropGate, setDropGate] = useState<GateResult | null>(null);

	useEffect(() => {
		if (!visible || !HARNESS_KINDS[harnessKind].capabilities.pty) return;
		let cancelled = false;
		let unlisten: (() => void) | null = null;

		const gate = createDropGate({
			local: () =>
				checkRegistered({
					capabilities: HARNESS_KINDS[harnessKind].capabilities,
					activity: harnessActivity.get(harnessId),
					registered: harnessInput.isRegistered(harnessId),
				}),
			ask: () => checkGate(harnessId, null, "insert"),
			set: setDropGate,
		});

		// Tauri's drag-drop position is a PhysicalPosition (physical
		// pixels); getBoundingClientRect() is in CSS (logical) pixels,
		// so divide by devicePixelRatio before comparing. NOTE: macOS's
		// reported units for this event are unverified — if drops land
		// off-target on a Retina display, this hit-test is where to
		// look first.
		const insidePane = (position: PhysicalPosition): boolean => {
			const host = containerRef.current;
			if (!host) return false;
			const dpr = window.devicePixelRatio || 1;
			const x = position.x / dpr;
			const y = position.y / dpr;
			const rect = host.getBoundingClientRect();
			return x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom;
		};

		void getCurrentWebview()
			.onDragDropEvent((event) => {
				if (cancelled) return;
				const payload = event.payload;
				if (payload.type === "leave") {
					gate.end();
					return;
				}
				if (payload.type === "drop") {
					gate.end();
					if (insidePane(payload.position) && payload.paths.length > 0) {
						insertText(harnessId, harnessKind, formatDroppedPaths(payload.paths))
							.then((result) => {
								if (result.ok) termRef.current?.focus();
							})
							.catch((err) => {
								console.warn("[skein] file drop insert failed:", err);
							});
					}
					return;
				}
				// `enter` / `over`: show or clear the overlay per hit-test.
				if (insidePane(payload.position)) gate.hover();
				else gate.end();
			})
			.then((fn) => {
				if (cancelled) {
					fn();
					return;
				}
				unlisten = fn;
			});

		return () => {
			cancelled = true;
			unlisten?.();
			gate.end();
		};
	}, [visible, harnessId, harnessKind, containerRef, termRef]);

	return dropGate;
}
