// DesignBody — the body of a `design` harness (#433): a live preview
// of an HTML file from the room's worktree, served by the Rust preview
// server, in a sandboxed iframe. The page is arbitrary worktree code:
// the frame gets `allow-scripts` and NEVER `allow-same-origin`, and its
// `postMessage` beacons are untrusted (see designPreview.ts).

import { Channel, invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { type Beacon, parseBeacon, previewUrl, pushBeacon } from "./designPreview.ts";

/** How long after the iframe's `load` we wait for the `ready` beacon
 *  before saying Skein's preview script did not run. */
const READY_TIMEOUT_MS = 3000;

interface DesignBodyProps {
	harnessId: string;
	roomId: string;
	visible: boolean;
	entry: string | undefined;
	onEntryChange: (entry: string) => void;
}

export const DesignBody = ({
	harnessId,
	roomId,
	visible,
	entry,
	onEntryChange,
}: DesignBodyProps) => {
	const [base, setBase] = useState<string | null>(null);
	const [entries, setEntries] = useState<string[] | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [attempt, setAttempt] = useState(0);
	const [version, setVersion] = useState(0);
	const [beacons, setBeacons] = useState<Beacon[]>([]);
	const [ready, setReady] = useState(false);
	const [noReady, setNoReady] = useState(false);
	const frameRef = useRef<HTMLIFrameElement | null>(null);
	const readyRef = useRef(false);
	const timerRef = useRef<number | null>(null);

	const refreshEntries = useCallback(async () => {
		try {
			setEntries(await invoke<string[]>("design_list_entries", { roomId }));
		} catch (e) {
			setError(String(e));
		}
	}, [roomId]);

	// Preview base + entry list; `attempt` is the inline retry.
	// biome-ignore lint/correctness/useExhaustiveDependencies: attempt is the retry trigger
	useEffect(() => {
		let cancelled = false;
		setError(null);
		invoke<string>("design_preview_base", { roomId })
			.then((b) => {
				if (!cancelled) setBase(b);
			})
			.catch((e) => {
				if (!cancelled) setError(String(e));
			});
		void refreshEntries();
		return () => {
			cancelled = true;
		};
	}, [roomId, attempt, refreshEntries]);

	// The watcher runs only while mounted; a tick reloads the frame.
	useEffect(() => {
		const channel = new Channel<null>();
		channel.onmessage = () => {
			setVersion((n) => n + 1);
			void refreshEntries();
		};
		let watchId: string | null = null;
		let cancelled = false;
		invoke<string>("design_watch_start", { roomId, onChange: channel })
			.then((id) => {
				if (cancelled) {
					void invoke("git_watch_stop", { id });
					return;
				}
				watchId = id;
			})
			.catch((e) => {
				if (!cancelled) setError(String(e));
			});
		return () => {
			cancelled = true;
			if (watchId !== null) void invoke("git_watch_stop", { id: watchId });
		};
	}, [roomId, refreshEntries]);

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
		if (timerRef.current !== null) window.clearTimeout(timerRef.current);
		timerRef.current = null;
	}, [url]);

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
			} else {
				setBeacons((prev) => pushBeacon(prev, b));
			}
		};
		window.addEventListener("message", onMessage);
		return () => window.removeEventListener("message", onMessage);
	}, []);

	const onFrameLoad = () => {
		if (readyRef.current) return;
		if (timerRef.current !== null) window.clearTimeout(timerRef.current);
		timerRef.current = window.setTimeout(() => {
			if (!readyRef.current) setNoReady(true);
		}, READY_TIMEOUT_MS);
	};

	let status = "loading…";
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
				<button type="button" title="Reload" onClick={() => setVersion((n) => n + 1)}>
					↻
				</button>
				<span className="dp-status">{status}</span>
			</div>
			{error !== null && (
				<div className="fp-notice warn" role="alert">
					{error}{" "}
					<button type="button" onClick={() => setAttempt((n) => n + 1)}>
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
				<iframe
					ref={frameRef}
					className="dp-frame"
					title="Design preview"
					sandbox="allow-scripts"
					src={url}
					onLoad={onFrameLoad}
				/>
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
