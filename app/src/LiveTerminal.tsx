// LiveTerminal — xterm.js bound to a Tauri-side PTY.
//
// Mounting spawns the child via `pty_spawn` and wires three flows:
//   - PTY → terminal (Channel<PtyEvent> from Rust → term.write / exit handler)
//   - terminal → PTY (term.onData → invoke "pty_write")
//   - resize → PTY (ResizeObserver → fit → invoke "pty_resize")
//
// Unmount kills the child. Hidden panes (display:none) keep their PTY
// alive — the resize/fit path is guarded against zero-size hosts so we
// never tell xterm or the child that the terminal shrank to 1×1.
//
// When the child exits we keep the xterm and its scrollback alive
// long enough to write a "[skein] x exited (N)" line + "Press Enter
// for shell." footer. Pressing Enter calls `onCmdChange(shell)`,
// which updates the harness's stored cmd. App.tsx's HarnessBody
// derives the LiveTerminal mountKey from the cmd content, so a cmd
// change triggers a React unmount + fresh remount: new xterm, new
// PTY, no alt-screen state to reset, no scrollback to preserve. This
// is the only path back to a usable pane after a TUI exits — there's
// no in-pane retry. Chapter 5 makes harnesses resume on Skein
// restart anyway, and `claude --resume <uuid>` from the shell covers
// "come back to my conversation" without the alt-screen handover
// bug class.

import { invoke } from "@tauri-apps/api/core";
import type { PhysicalPosition } from "@tauri-apps/api/dpi";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { FitAddon } from "@xterm/addon-fit";
import type { Terminal } from "@xterm/xterm";
import { useEffect, useRef, useState } from "react";
import { HARNESS_KINDS } from "./data.tsx";
import { harnessActivity } from "./harnessActivity.ts";
import { canInsertText, formatDroppedPaths, harnessInput, insertText } from "./harnessInput.ts";
import type { GateResult } from "./harnessInput.ts";
import type { HarnessKind } from "./types.ts";
import { OVERLAY_CLOSED_EVENT } from "./useFocusRestore.ts";
import { useTerminalSpawn } from "./useTerminalSpawn.ts";

interface LiveTerminalProps {
	cmd: string[];
	cwd: string;
	// Stable identity for the spawn — used so React's StrictMode
	// double-invocation in dev doesn't spawn twice. Passing the
	// harness id is the natural choice.
	mountKey: string;
	// Stable across cmd changes (mountKey churns when the user
	// picks "Enter for shell" — see App.tsx). Used as the key into
	// the activity store so a respawn under the same harness shows
	// up as a fresh `spawning → running` transition rather than a
	// new ghost record. Epic #50.
	harnessId: string;
	// Stamped on every `harness_actions` row this harness emits
	// (issue #80). The Live Context cards query per-room.
	roomId: string;
	// Used to decide whether the L2c-1 Claude JSONL adapter should
	// attach. Only `kind === "claude"` with a non-empty `sessionId`
	// (from chapter 5's `--session-id <uuid>` pre-allocation) gets
	// the adapter; everything else falls back to the L2a idle
	// heuristic. Epic #50 L2c-1.
	harnessKind: HarnessKind;
	sessionId: string | undefined;
	// The agent baked into `cmd` (#247), re-read here so the spawn can
	// be checked against what the CLI accepts *now*. Deliberately a
	// prop and not parsed back out of the argv: reading argv to recover
	// a decision is the pattern that produced #153 and #170.
	//
	// `undefined` = no `--agent` flag, which needs no check.
	agent: string | undefined;
	// Epic #50 L2c-2: opencode embedded-server port. Required for
	// kind === "opencode" to attach the SSE adapter; `undefined`
	// means the adapter is disabled for this harness and L2a takes
	// over.
	opencodePort: number | undefined;
	// Fired when the L2c-2 adapter observes a `session.created`
	// event from opencode's SSE stream. Caller wires this to
	// persist the captured sessionId on the harness for resume.
	// `undefined` for non-opencode harnesses.
	onSessionCaptured: ((sessionId: string) => void) | undefined;
	// #116: fired when the L2c-2 adapter decides the harness followed
	// its TUI onto a DIFFERENT root session (`/new`, or an existing
	// session picked via `/sessions`) — see `followedOpencodeSession`
	// in `sessionTracking.ts`. Caller wires this to overwrite the
	// stored sessionId, same as Claude's clear/resume/fork follow.
	// `undefined` for non-opencode harnesses.
	onSessionFollowed: ((sessionId: string) => void) | undefined;
	fontSize: number;
	// #158: copy a mouse selection to the clipboard the moment it's made
	// (Settings → "Copy on select", default true). Read through a ref
	// (`copyOnSelectRef` below), like `defaultShellRef` — toggling it
	// must apply to an already-running terminal without respawning the
	// PTY, so it cannot be a dep of the mountKey-only effect that owns
	// the mouseup listener.
	copyOnSelect: boolean;
	// Default shell argv (from `default_shell`). Used when the user
	// presses Enter on the post-exit prompt to drop into a usable shell.
	defaultShell: string[];
	// True iff this terminal is the one the user can currently see
	// and interact with: its room is active, no picker is up in
	// front of it, and it's the room's active harness. We focus the
	// xterm whenever this flips true (or on mount with `visible:
	// true`) so keyboard-driven room/harness switches don't leave
	// focus stranded on document.body. Issue #22.
	visible: boolean;
	// Persists a new cmd against this harness so a Skein restart
	// re-spawns the shell instead of the dead CLI.
	onCmdChange: (cmd: string[]) => void;
}

export const LiveTerminal = ({
	cmd,
	cwd,
	mountKey,
	harnessId,
	roomId,
	harnessKind,
	sessionId,
	agent,
	opencodePort,
	onSessionCaptured,
	onSessionFollowed,
	fontSize,
	copyOnSelect,
	defaultShell,
	visible,
	onCmdChange,
}: LiveTerminalProps) => {
	const containerRef = useRef<HTMLDivElement>(null);
	// #41: null while nothing is being dragged over this pane; a
	// `GateResult` while it is — `ok: true` renders the "will insert"
	// label, `ok: false` renders the refusal reason. Driven by the
	// drag-drop effect below, which only subscribes while `visible`.
	const [dropGate, setDropGate] = useState<GateResult | null>(null);
	// #158: transient copy/paste feedback — "Copied", "Nothing
	// selected — …", "Paste failed", etc. `null` renders nothing.
	// Cleared, and its auto-dismiss timer reset, on every new hint; the
	// timer is also cleared on unmount/respawn. The clipboard
	// writeText/readText callbacks below each check `cancelled` before
	// calling `showHint` too — a promise settling after teardown must
	// not setState (React warning) or arm an orphan timer that outlives
	// this effect.
	const [hint, setHint] = useState<string | null>(null);
	const hintTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
	// Track live-spawn state by mountKey so StrictMode's double effect
	// doesn't spawn twice, and so a re-mount with the same harness can
	// reuse the previous spawn id when we add reconnects later.
	const spawnedRef = useRef<string | null>(null);
	// Used by the clipboard key handler — needs the PTY id at keystroke
	// time, which the spawn .then sets later.
	const ptyIdRef = useRef<string | null>(null);
	// Refs to the xterm + fit addon so the font-size effect below can
	// retune them without re-spawning the PTY.
	const termRef = useRef<Terminal | null>(null);
	const fitRef = useRef<FitAddon | null>(null);
	// #116 step two: the mount effect only runs on `mountKey` changes (see
	// the exhaustive-deps note below), so its attach closure must read the
	// CURRENT sessionId at attach time — including one that lands between
	// render and the effect actually spawning the PTY — not the value
	// captured when the effect was defined. Step four reuses this same
	// ref as the opencode adapter's LIVE getter, passed straight into
	// `attachOpencodeEvents`: that adapter is attached once per PTY, but
	// the harness's own sessionId changes under it whenever `/new` or a
	// `/sessions` pick is followed, so a value closed over at attach
	// time would go stale the first time that happens.
	const sessionIdRef = useRef(sessionId);
	sessionIdRef.current = sessionId;
	// #116 step two: the Claude JSONL adapter's unsubscribe, paired with
	// the sessionId it was attached with, so the re-point effect below can
	// tell "still the same session" from "/clear moved us to a new one"
	// without re-running the whole spawn effect (which would kill the
	// PTY). `null` whenever no adapter is attached (PTY not live yet,
	// non-Claude harness, or no sessionId at attach time).
	const claudeAdapterRef = useRef<{ detach: () => void; sessionId: string } | null>(null);

	// Sync the latest props into refs so the long-lived effect's closure
	// always reads current values. defaultShell starts empty and gets
	// hydrated by the async `default_shell` invoke; onCmdChange is
	// recreated on every App render. Refs avoid having to re-run the
	// PTY-owning effect on those changes.
	const defaultShellRef = useRef(defaultShell);
	defaultShellRef.current = defaultShell;
	const onCmdChangeRef = useRef(onCmdChange);
	onCmdChangeRef.current = onCmdChange;
	// #158: same reasoning as defaultShellRef — the mouseup listener set
	// up once per mountKey reads this at fire time, not at effect-setup
	// time, so toggling the Settings checkbox takes effect immediately.
	const copyOnSelectRef = useRef(copyOnSelect);
	copyOnSelectRef.current = copyOnSelect;

	// The mount effect (spawns/kills the PTY on mountKey, attaches the
	// L2c event adapters, registers the #238 nudge seam) and the #116
	// Claude-adapter re-point effect both live in this hook now —
	// LiveTerminal.tsx's own body was over the ~600-line house limit
	// (#19). Called here, at the position the mount effect used to
	// occupy, so declaration order relative to the effects below is
	// unchanged.
	useTerminalSpawn({
		cmd,
		cwd,
		mountKey,
		harnessId,
		roomId,
		harnessKind,
		sessionId,
		agent,
		opencodePort,
		onSessionCaptured,
		onSessionFollowed,
		fontSize,
		containerRef,
		spawnedRef,
		ptyIdRef,
		termRef,
		fitRef,
		hintTimerRef,
		sessionIdRef,
		claudeAdapterRef,
		defaultShellRef,
		onCmdChangeRef,
		copyOnSelectRef,
		setHint,
	});

	// Issue #22: focus the xterm whenever this pane becomes visible —
	// covers keyboard-driven room switches (Mod+1..9, palette,
	// Mod+Tab), harness-within-room switches, and the picker → pick
	// → newly-active-harness flow. The mount effect runs before this
	// one (declaration order), so termRef is populated by the time we
	// dereference it. We *don't* track focus on visible→false: hiding
	// the pane via display:none already drops focus naturally; trying
	// to "restore" focus elsewhere would fight whatever just received
	// it (modal, command palette, etc.).
	// Issue #33: when an overlay (Settings, command palette, etc.)
	// dismisses, useFocusRestore in the overlay fires
	// `skein:overlay-closed` on window. The currently-visible
	// terminal grabs focus so the user can keep typing — regardless
	// of where focus was before the overlay opened (it might have
	// been on a chrome button rather than a terminal, in which case
	// "restoring" the original focus would land back on the button).
	useEffect(() => {
		if (!visible) return;
		const onOverlayClosed = () => termRef.current?.focus();
		window.addEventListener(OVERLAY_CLOSED_EVENT, onOverlayClosed);
		return () => window.removeEventListener(OVERLAY_CLOSED_EVENT, onOverlayClosed);
	}, [visible]);

	useEffect(() => {
		if (visible) termRef.current?.focus();
	}, [visible]);

	// Live font-size changes: retune the existing terminal without
	// re-spawning the PTY. fit() recomputes rows/cols at the new cell
	// size; we then tell the PTY to match so the child sees the resize.
	//
	// Containment for #16: the harness column and per-harness wrapper
	// have `overflow: hidden` set (`.sk-harness-col` in styles.css and
	// the inline style in `App.tsx`). Without that, xterm's canvas
	// pushes the flex column taller when the font grows, fit reads the
	// stretched parent height, and the row count never decreases —
	// content overflows and stays overflowed. With containment in
	// place, fit reads the constrained parent and reduces rows
	// correctly.
	useEffect(() => {
		const term = termRef.current;
		const fit = fitRef.current;
		const host = containerRef.current;
		if (!term || !fit) return;
		term.options.fontSize = fontSize;
		if (!host || host.clientWidth === 0 || host.clientHeight === 0) return;
		try {
			fit.fit();
		} catch {
			return;
		}
		const id = ptyIdRef.current;
		if (id) void invoke("pty_resize", { id, rows: term.rows, cols: term.cols });
	}, [fontSize]);

	// #41: native OS file drop, visible-pane only. `dragDropEnabled` in
	// tauri.conf.json (#271) routes real paths through the webview's own
	// `onDragDropEvent` instead of the browser's `drop` event, which
	// carries no paths at all. Subscribed only while `visible` — a
	// hidden pane (display:none, PTY still alive) shouldn't steal a drop
	// meant for whichever pane the user is actually looking at.
	//
	// `capabilities.pty` is checked here too even though App.tsx only
	// ever mounts LiveTerminal for pty-capable kinds (`files` renders
	// FilesBody instead) — belt and suspenders against this component
	// ever being reused for a non-PTY kind.
	useEffect(() => {
		if (!visible || !HARNESS_KINDS[harnessKind].capabilities.pty) return;
		let cancelled = false;
		let unlisten: (() => void) | null = null;

		const evaluateGate = (): GateResult =>
			canInsertText({
				capabilities: HARNESS_KINDS[harnessKind].capabilities,
				activity: harnessActivity.get(harnessId),
				registered: harnessInput.isRegistered(harnessId),
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
					setDropGate(null);
					return;
				}
				if (payload.type === "drop") {
					if (insidePane(payload.position) && payload.paths.length > 0) {
						const gate = evaluateGate();
						if (gate.ok) {
							insertText(harnessId, harnessKind, formatDroppedPaths(payload.paths));
							termRef.current?.focus();
						}
					}
					setDropGate(null);
					return;
				}
				// `enter` / `over`: show or clear the overlay per hit-test.
				setDropGate(insidePane(payload.position) ? evaluateGate() : null);
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
			setDropGate(null);
		};
	}, [visible, harnessId, harnessKind]);

	return (
		<div className="sk-terminal-drop-host">
			<div ref={containerRef} style={{ width: "100%", height: "100%" }} />
			{dropGate && (
				<div className={`sk-terminal-drop-overlay${dropGate.ok ? "" : " refused"}`}>
					<div className="sk-terminal-drop-label">
						{dropGate.ok ? "Drop to insert the path" : dropGate.reason}
					</div>
				</div>
			)}
			{hint && (
				<div className="sk-terminal-hint-overlay">
					<div className="sk-terminal-hint-label">{hint}</div>
				</div>
			)}
		</div>
	);
};
