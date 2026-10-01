// DesignBody — the body of a `design` harness (#433): a live preview
// of an HTML file from the room's worktree, served by the Rust preview
// server, in a sandboxed iframe. The page is arbitrary worktree code:
// the frame gets `allow-scripts` and NEVER `allow-same-origin`, and its
// `postMessage` beacons are untrusted (see designPreview.ts).

import { useCallback, useEffect, useRef, useState } from "react";
import { DesignComments } from "./DesignComments.tsx";
import {
	type Beacon,
	type HostMessage,
	hostMessage,
	parseBeacon,
	previewUrl,
	pushBeacon,
} from "./designPreview.ts";
import { useDesignComments } from "./useDesignComments.ts";
import { useDesignPreview } from "./useDesignPreview.ts";
import "./design.css";
import "./FilesBody.css";

/** How long after the iframe's `load` we wait for the `ready` beacon
 *  before saying Skein's preview script did not run. */
const READY_TIMEOUT_MS = 3000;

interface DesignBodyProps {
	harnessId: string;
	roomId: string;
	cwd: string;
	visible: boolean;
	entry: string | undefined;
	onEntryChange: (entry: string) => void;
}

export const DesignBody = ({
	harnessId,
	roomId,
	cwd,
	visible,
	entry,
	onEntryChange,
}: DesignBodyProps) => {
	const { base, entries, error, version, retry, reload } = useDesignPreview(roomId);
	const [beacons, setBeacons] = useState<Beacon[]>([]);
	const [ready, setReady] = useState(false);
	const [noReady, setNoReady] = useState(false);
	const [readyCount, setReadyCount] = useState(0);
	const [picking, setPicking] = useState(false);
	const frameRef = useRef<HTMLIFrameElement | null>(null);
	const readyRef = useRef(false);
	const timerRef = useRef<number | null>(null);

	const post = useCallback((msg: HostMessage) => {
		frameRef.current?.contentWindow?.postMessage(hostMessage(msg), "*");
	}, []);

	const c = useDesignComments({
		harnessId,
		roomId,
		cwd,
		visible,
		entry,
		onEntryChange,
		post,
		ready,
		readyCount,
	});
	const { setDraft, setSelected, relocate, onLocated, clearPlacements } = c;

	// One entry and none chosen: pick it and persist.
	useEffect(() => {
		if (entry === undefined && entries?.length === 1 && entries[0] !== undefined) {
			onEntryChange(entries[0]);
		}
	}, [entry, entries, onEntryChange]);

	const url = base !== null && entry !== undefined ? previewUrl(base, entry, version) : null;

	// A new load: forget the last one's beacons and readiness.
	// biome-ignore lint/correctness/useExhaustiveDependencies: url is the trigger
	useEffect(() => {
		setBeacons([]);
		setReady(false);
		setNoReady(false);
		readyRef.current = false;
		setPicking(false);
		clearPlacements();
		if (timerRef.current !== null) window.clearTimeout(timerRef.current);
		timerRef.current = null;
	}, [url]);

	// Escape leaves pick mode even when the host, not the frame, has focus.
	useEffect(() => {
		if (!picking) return;
		const onKey = (e: KeyboardEvent) => {
			if (e.key !== "Escape") return;
			setPicking(false);
			post({ type: "pick-cancel" });
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [picking, post]);

	const togglePick = () => {
		if (picking) {
			setPicking(false);
			post({ type: "pick-cancel" });
		} else {
			setPicking(true);
			post({ type: "pick-start" });
		}
	};

	useEffect(
		() => () => {
			if (timerRef.current !== null) window.clearTimeout(timerRef.current);
		},
		[],
	);

	// Beacons from the frame: only its own window is believed.
	useEffect(() => {
		const onMessage = (e: MessageEvent) => {
			const frame = frameRef.current;
			if (!frame || e.source !== frame.contentWindow) return;
			const b = parseBeacon(e.data);
			if (!b) return;
			if (b.type === "ready") {
				readyRef.current = true;
				setReady(true);
				setNoReady(false);
				setReadyCount((n) => n + 1);
			} else if (b.type === "picked") {
				setPicking(false);
				setDraft(b.element);
			} else if (b.type === "dom-changed") {
				// The page finished (re)rendering or resized: its pins may be off.
				relocate();
			} else if (b.type === "pick-cancelled") {
				setPicking(false);
			} else if (b.type === "located") {
				onLocated(b);
			} else {
				setBeacons((prev) => pushBeacon(prev, b));
			}
		};
		window.addEventListener("message", onMessage);
		return () => window.removeEventListener("message", onMessage);
	}, [setDraft, relocate, onLocated]);

	const onFrameLoad = () => {
		if (readyRef.current) return;
		if (timerRef.current !== null) window.clearTimeout(timerRef.current);
		timerRef.current = window.setTimeout(() => {
			if (!readyRef.current) setNoReady(true);
		}, READY_TIMEOUT_MS);
	};

	let status = error !== null ? "" : "loading…";
	if (entries !== null && entries.length === 0) status = "no HTML files";
	else if (url !== null) status = ready ? `live · ${entry}` : (entry ?? "");

	return (
		<div
			className="dp-body"
			data-harness={harnessId}
			data-visible={visible}
			style={{ display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}
		>
			<div className="dp-toolbar">
				<select
					value={entry ?? ""}
					disabled={!entries || entries.length === 0}
					onChange={(e) => onEntryChange(e.target.value)}
				>
					{entry === undefined && <option value="">choose an entry…</option>}
					{entry !== undefined && !entries?.includes(entry) && (
						<option value={entry}>{entry}</option>
					)}
					{entries?.map((p) => (
						<option key={p} value={p}>
							{p}
						</option>
					))}
				</select>
				<button type="button" title="Reload" onClick={reload}>
					↻
				</button>
				<button
					type="button"
					className={`dp-comment-toggle${picking ? " on" : ""}`}
					title="Pick an element to comment on (Esc cancels)"
					aria-pressed={picking}
					disabled={!ready || entry === undefined}
					onClick={togglePick}
				>
					Comment
				</button>
				<span className="dp-status">{status}</span>
			</div>
			{error !== null && (
				<div className="fp-notice warn" role="alert">
					{error}{" "}
					<button type="button" onClick={retry}>
						Retry
					</button>
				</div>
			)}
			{noReady && (
				<div className="fp-notice warn">
					Skein's preview script did not run — a Content-Security-Policy on the page or a script
					error can cause this. Error reporting is unavailable.
				</div>
			)}
			{entries !== null && entries.length === 0 && error === null ? (
				<div className="fp-empty">
					<div className="glyph">◐</div>
					<div className="title">No HTML files in this worktree</div>
					<div className="hint">Add an .html file and it will show up here.</div>
				</div>
			) : url !== null ? (
				<div className="dp-main">
					<iframe
						ref={frameRef}
						className="dp-frame"
						title="Design preview"
						sandbox="allow-scripts"
						src={url}
						onLoad={onFrameLoad}
					/>
					{(c.draft !== null || c.threads.length > 0 || c.commentError !== null) && (
						<DesignComments
							roomId={roomId}
							entry={entry}
							ready={ready}
							threads={c.threads}
							placements={c.placements}
							draft={c.draft}
							selected={c.selected}
							busy={c.busy}
							commentError={c.commentError}
							act={c.act}
							onSelect={(id) => setSelected((s) => ({ id, tick: (s?.tick ?? 0) + 1 }))}
							onSubmitDraft={c.submitDraft}
							onCancelDraft={() => setDraft(null)}
						/>
					)}
				</div>
			) : (
				<div className="fp-empty" />
			)}
			{beacons.length > 0 && (
				<ul className="dp-beacons">
					<li>{beacons.length} problem(s) in this load</li>
					{beacons.map((b, i) => (
						<li key={i}>
							{b.type === "resource-error"
								? `${b.tag} failed to load: ${b.url}`
								: b.type === "script-error"
									? `${b.message}${b.url ? ` (${b.url}${b.line !== undefined ? `:${b.line}` : ""})` : ""}`
									: ""}
						</li>
					))}
				</ul>
			)}
		</div>
	);
};
