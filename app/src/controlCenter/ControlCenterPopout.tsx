// Root of the Control Center pop-out window (#493). A thin renderer: main
// owns the state stores and sends snapshots (popoutProtocol.ts). This window
// never mounts App, so it spawns no PTYs and never touches the db.

import { emitTo, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import { ControlCenterView } from "./ControlCenter.tsx";
import { appearanceClassName, isAppearanceKey, readAppearance } from "./popoutAppearance.ts";
import { loadCcPopout, saveCcPopout } from "./popoutPrefs.ts";
import {
	type CcFocusPayload,
	type CcSnapshotPayload,
	EV_DOCK,
	EV_FOCUS,
	EV_READY,
	EV_SNAPSHOT,
} from "./popoutProtocol.ts";
import {
	clockOffset,
	debounce,
	effectiveNow,
	isSentinelPosition,
	PIN_TOOLTIP,
	toLogicalGeometry,
} from "./popoutWindow.ts";
import "./ControlCenterPopout.css";

const GEOMETRY_DEBOUNCE_MS = 500;
const CLOCK_TICK_MS = 15_000;

export function ControlCenterPopout() {
	const [snap, setSnap] = useState<{ payload: CcSnapshotPayload; offset: number } | null>(null);
	const [tick, setTick] = useState(0);
	const [pinned, setPinned] = useState(() => loadCcPopout().alwaysOnTop);

	// Theme / density / font size follow main live: main's usePersistedState
	// writes localStorage, which fires `storage` here (same origin).
	const [appearance, setAppearance] = useState(() =>
		readAppearance((k) => localStorage.getItem(k)),
	);
	useEffect(() => {
		const onStorage = (e: StorageEvent) => {
			if (isAppearanceKey(e.key)) setAppearance(readAppearance((k) => localStorage.getItem(k)));
		};
		window.addEventListener("storage", onStorage);
		return () => window.removeEventListener("storage", onStorage);
	}, []);

	// Listeners first, then announce readiness so main's reply is not missed.
	useEffect(() => {
		let cancelled = false;
		const off = listen<CcSnapshotPayload>(EV_SNAPSHOT, (e) => {
			setSnap({ payload: e.payload, offset: clockOffset(e.payload.now, Date.now()) });
		});
		void off.then(() => {
			if (!cancelled) void emitTo("main", EV_READY).catch(() => {});
		});
		return () => {
			cancelled = true;
			void off.then((f) => f());
		};
	}, []);

	// Relative times keep advancing between snapshots.
	useEffect(() => {
		const id = setInterval(() => setTick((n) => n + 1), CLOCK_TICK_MS);
		return () => clearInterval(id);
	}, []);

	// A user close (OS button) is the only close this window sees: main's
	// quit cascades from Rust via destroy(), which emits no CloseRequested.
	useEffect(() => {
		const w = getCurrentWindow();
		const off = w.onCloseRequested(() => {
			saveCcPopout({ open: false });
		});
		return () => {
			void off.then((f) => f());
		};
	}, []);

	// Geometry, logical units, debounced.
	useEffect(() => {
		const w = getCurrentWindow();
		const save = debounce(() => {
			void Promise.all([w.outerPosition(), w.innerSize(), w.scaleFactor(), w.isMinimized()])
				.then(([pos, size, scale, minimized]) => {
					// Position is outer, size is inner: creation width/height are inner.
					if (minimized || isSentinelPosition(pos)) return;
					saveCcPopout({ geometry: toLogicalGeometry(pos, size, scale) });
				})
				.catch(() => {});
		}, GEOMETRY_DEBOUNCE_MS);
		const offs = [w.onMoved(save), w.onResized(save)];
		return () => {
			save.cancel();
			for (const o of offs) void o.then((f) => f());
		};
	}, []);

	const onFocus = useCallback((roomId: string, harnessId?: string) => {
		const payload: CcFocusPayload = harnessId === undefined ? { roomId } : { roomId, harnessId };
		void emitTo("main", EV_FOCUS, payload).catch(() => {});
	}, []);

	const togglePin = useCallback(() => {
		const next = !pinned;
		setPinned(next);
		saveCcPopout({ alwaysOnTop: next });
		void getCurrentWindow()
			.setAlwaysOnTop(next)
			.catch(() => setPinned(!next));
	}, [pinned]);

	// Tell main first (it shows the in-app view and raises itself), then
	// close like the X does. Main never touches this window.
	const dockBack = useCallback(() => {
		void emitTo("main", EV_DOCK).catch(() => {});
		saveCcPopout({ open: false });
		void getCurrentWindow().close();
	}, []);

	const shell = (children: ReactNode) => (
		<div
			className={appearanceClassName(appearance)}
			style={{ ["--cfs" as string]: `${appearance.chromeFontPt}px` }}
		>
			{children}
		</div>
	);

	if (!snap) return shell(<div className="cc-popout-waiting">Waiting for Skein…</div>);

	// `tick` only forces a re-render so effectiveNow re-reads the clock.
	void tick;
	const header = (
		<div className="cc-toolbar cc-popout-toolbar">
			<button
				type="button"
				className="cc-popout-btn"
				aria-pressed={pinned}
				title={PIN_TOOLTIP}
				onClick={togglePin}
			>
				{pinned ? "Pinned on top" : "Pin on top"}
			</button>
			<button
				type="button"
				className="cc-popout-btn"
				title="Close this window and show the Control Center inside Skein again"
				onClick={dockBack}
			>
				Dock back
			</button>
		</div>
	);
	return shell(
		<ControlCenterView
			sections={snap.payload.sections}
			activeRoomId={snap.payload.activeRoomId}
			now={effectiveNow(snap.offset, Date.now())}
			onFocus={onFocus}
			header={header}
		/>,
	);
}
