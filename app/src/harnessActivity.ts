// Per-harness activity state machine + event hook (epic #50, L1+L3+L2a).
//
// The single source of truth for "what is this harness doing right
// now?" — read by the bottom status bar (#29), by the harness tab
// dots, and (in follow-on PRs) by the notification surfaces (#12),
// the per-room aggregate (#50 L4), and the cross-harness activity
// feed (#50 L7).
//
// Today we have exactly one signal flowing from a harness: bytes
// over the PTY `Channel<String>`. This module derives a real state
// machine from that signal using the idle-heuristic strategy (#50
// L2a): output → `running`, sustained silence → `idle`, PTY exit
// → `exited`. Pattern-match and harness-native strategies (L2b /
// L2c) will plug in later by calling the same `setPhase` mutator —
// consumers won't know or care which strategy fed the transition.
//
// Why not Rust-side: this state is purely derived from a stream
// the frontend already receives. Putting the model in Rust would
// add a second IPC channel and split logic across the boundary
// for no win. If we ever need cross-restart persistence (epic L6)
// the natural shape is "frontend emits transitions, Rust appends
// to a log" — the state machine itself can stay here.

import { useCallback, useSyncExternalStore } from "react";
import { matchesWaitingPrompt, stripAnsi } from "./harnessPatterns.ts";
import type { Status } from "./types.ts";

export type ActivityPhase = "spawning" | "running" | "idle" | "waiting" | "exited";

export interface HarnessActivity {
	phase: ActivityPhase;
	/// Epoch ms of the most recent PTY output. `null` until first
	/// chunk arrives — useful for distinguishing "spawning, never
	/// produced output" from "spawned, ran, went idle."
	lastOutputAt: number | null;
	/// Set when `phase === "exited"`. `null` if the PTY exited
	/// without a numeric code (signal-killed on some platforms).
	exitCode: number | null;
	spawnedAt: number;
	/// Has the user typed anything into this harness since it
	/// spawned (paste / arrow keys / any byte that wasn't a
	/// focus-in/-out escape)? Used by L5a notification logic to
	/// tell "user did a task" cycles apart from "startup banner
	/// printed then quieted." Ephemeral; reset on every spawn.
	hasUserInput: boolean;
	/// True when a harness-native adapter (epic #50 L2c) is wired
	/// up for this harness — Claude JSONL tail today, opencode
	/// later. While set, the idle tick stands down and only the
	/// adapter (plus PTY exit) writes phase transitions. The PTY
	/// chunk stream still updates `lastOutputAt` for diagnostics.
	authoritative: boolean;
	/// Rolling tail of stripped PTY output, used by the L2b
	/// pattern-match fallback (copilot / byoh / shell). Only
	/// maintained when `authoritative === false` — the L2c
	/// adapters supersede pattern matching for kinds that have
	/// real event streams. Capped at `TAIL_MAX_CHARS` so it
	/// doesn't grow without bound; the matcher only looks at the
	/// last ~256 chars anyway.
	tail: string;
}

/// Sustained silence threshold for `running → idle`. Hard-coded for
/// v1; epic #50 L5e moves this into Settings.
const IDLE_AFTER_MS = 8_000;
/// Shorter silence threshold for `running → waiting` via L2b
/// pattern match. The user types `sudo X`, "Password:" appears,
/// output stops — we want the dot to flip blue near-instantly, not
/// 8 s later. 500 ms is generous enough to avoid firing during
/// mid-stream output that happens to contain a prompt-looking
/// substring, while tight enough to feel responsive.
const PATTERN_WAITING_AFTER_MS = 500;
/// Maximum tail-buffer size per harness. The matcher only scans
/// the last 256 chars; we keep more to handle large chunks that
/// arrive in one PTY read (xterm splits at no fixed boundary) but
/// cap so a chatty shell session doesn't grow memory unbounded.
const TAIL_MAX_CHARS = 2_048;
/// How often the background tick scans for idle transitions. A
/// faster tick gives tighter detection latency; 1s strikes a
/// sensible balance — at worst the user sees "idle" up to a second
/// late, which is below the perceptual threshold for a status dot.
const TICK_INTERVAL_MS = 1_000;
/// Window during which PTY output is treated as "our fault, not the
/// child's." Triggered explicitly by callers (e.g. LiveTerminal on
/// visibility flip — `term.focus()` sends a focus-in event to the
/// child, which many TUIs answer with a full redraw). Without this,
/// switching to a long-idle harness pops it back to `running` for 8s
/// before settling, which contradicts the actual state. 800 ms is
/// generous enough to cover slow repaints; if real output arrives
/// after the window, the normal path kicks back in.
const INDUCED_MUTE_MS = 800;

const store = new Map<string, HarnessActivity>();
const listeners = new Map<string, Set<() => void>>();
const muteUntil = new Map<string, number>();
let tickHandle: ReturnType<typeof setInterval> | null = null;

/// Global transition callback: receives every real phase change
/// plus the `source` string identifying which strategy fired it
/// (`l2a-idle`, `l2b-pattern`, `l2c1-claude-end-turn`, …).
///
/// Used by App-level notification logic (#12 L5a tab badges, L5b
/// OS notifications, L5c toasts) which need to react to transitions
/// across every harness, and by the L6 sqlite writer that persists
/// each transition with its provenance for the L7 activity feed.
export type TransitionListener = (
	id: string,
	from: ActivityPhase,
	to: ActivityPhase,
	source: TransitionSource,
) => void;
const transitionListeners = new Set<TransitionListener>();

/// Free-form string identifying which detection strategy or
/// lifecycle event fired a given phase transition. Persisted to
/// `harness_events.source` and surfaced as the "why" chip in the
/// L7 activity feed. The set is open-ended — new strategies
/// (future L2d, additional adapters) just pick new strings — but
/// the canonical values used today are exposed here so call sites
/// can use the constants instead of stringly-typed literals.
export type TransitionSource = string;

export const TRANSITION_SOURCE = {
	// PTY-driven (L1 substrate).
	PtyOutput: "pty-output",
	PtyExit: "pty-exit",
	// L2a — idle heuristic on the worktree-watcher tick.
	L2aIdle: "l2a-idle",
	// L2b — generic prompt-pattern fallback.
	L2bPattern: "l2b-pattern",
	L2bDrained: "l2b-drained",
	// L2c-1 — Claude JSONL adapter.
	L2c1ClaudeAssistant: "l2c1-claude-assistant",
	L2c1ClaudeEndTurn: "l2c1-claude-end-turn",
	L2c1ClaudeToolUse: "l2c1-claude-tool-use",
	L2c1ClaudeToolResult: "l2c1-claude-tool-result",
	L2c1ClaudeUserPrompt: "l2c1-claude-user-prompt",
	// L2c-2 — opencode SSE adapter.
	L2c2OpencodeBusy: "l2c2-opencode-busy",
	L2c2OpencodeIdle: "l2c2-opencode-idle",
	L2c2OpencodeMessageDelta: "l2c2-opencode-message-delta",
	L2c2OpencodeToolUse: "l2c2-opencode-tool-use",
} as const;

const emit = (id: string): void => {
	const set = listeners.get(id);
	if (!set) return;
	for (const cb of set) cb();
};

const setPhase = (
	id: string,
	phase: ActivityPhase,
	source: TransitionSource,
	patch?: Partial<HarnessActivity>,
): void => {
	const cur = store.get(id);
	if (!cur) return;
	// Suppress no-op transitions so consumers don't churn on
	// continuous output (every chunk would otherwise emit). Only
	// real phase changes notify subscribers; `lastOutputAt`
	// mutates silently inside `recordOutput`.
	if (cur.phase === phase && !patch) return;
	const from = cur.phase;
	store.set(id, { ...cur, phase, ...patch });
	emit(id);
	if (from !== phase) {
		for (const cb of transitionListeners) cb(id, from, phase, source);
	}
};

const ensureTick = (): void => {
	if (tickHandle !== null) return;
	tickHandle = setInterval(() => {
		const now = Date.now();
		for (const [id, a] of store) {
			// Harnesses with an authoritative source (Claude JSONL
			// tail, opencode SSE adapter) write their own phase
			// from the L2c module. The idle heuristic would fight
			// the adapter — e.g. when Claude is mid-turn but the
			// PTY is briefly quiet during a tool call, L2a would
			// flip to `idle` while L2c says `running`. Skip these.
			if (a.authoritative) continue;
			if (a.phase !== "running" || a.lastOutputAt === null) continue;
			const quiet = now - a.lastOutputAt;
			// L2b: pattern-match → waiting on a shorter threshold
			// than idle. Triggered when output has been quiet for
			// PATTERN_WAITING_AFTER_MS (500 ms) AND the tail buffer
			// matches a known prompt regex (sudo password, [y/n],
			// "Press Enter", …). The shorter threshold makes the
			// dot flip blue near-instantly when a sudo prompt
			// appears, instead of waiting the full 8 s idle window.
			//
			// If the tail doesn't match a known prompt, the
			// harness falls through to the L2a `→ idle` check
			// below — same behaviour as pre-L2b.
			if (quiet >= PATTERN_WAITING_AFTER_MS && matchesWaitingPrompt(a.tail)) {
				setPhase(id, "waiting", TRANSITION_SOURCE.L2bPattern);
				continue;
			}
			if (quiet >= IDLE_AFTER_MS) {
				setPhase(id, "idle", TRANSITION_SOURCE.L2aIdle);
			}
		}
	}, TICK_INTERVAL_MS);
};

const stopTickIfIdle = (): void => {
	if (store.size === 0 && tickHandle !== null) {
		clearInterval(tickHandle);
		tickHandle = null;
	}
};

export const harnessActivity = {
	/// Record a fresh spawn. Call from LiveTerminal once `pty_spawn`
	/// resolves successfully.
	spawned(id: string): void {
		const now = Date.now();
		store.set(id, {
			phase: "spawning",
			lastOutputAt: null,
			exitCode: null,
			spawnedAt: now,
			hasUserInput: false,
			authoritative: false,
			tail: "",
		});
		ensureTick();
		emit(id);
	},

	/// Mark a harness as having an authoritative L2c adapter
	/// attached (Claude JSONL tail, opencode event stream, …).
	/// While set, the L2a idle tick skips this harness — only
	/// `setRunningFromAdapter` / `setWaitingFromAdapter` and
	/// `exited` write its phase. Idempotent.
	attachAuthoritativeSource(id: string): void {
		const cur = store.get(id);
		if (!cur || cur.authoritative) return;
		store.set(id, { ...cur, authoritative: true });
	},

	/// Adapter detached — fall back to the L2a heuristic for this
	/// harness. The next chunk or tick will re-evaluate.
	detachAuthoritativeSource(id: string): void {
		const cur = store.get(id);
		if (!cur || !cur.authoritative) return;
		store.set(id, { ...cur, authoritative: false });
	},

	/// L2c adapter says the harness is doing work. Bypasses the
	/// recordOutput throttle so the transition fires on the
	/// adapter event boundary, not on the next PTY chunk. The
	/// `source` identifies which adapter event drove the transition
	/// (e.g. `l2c1-claude-assistant`, `l2c2-opencode-busy`) and
	/// flows through to the persisted event log for L7 attribution.
	setRunningFromAdapter(id: string, source: TransitionSource): void {
		const cur = store.get(id);
		if (!cur || cur.phase === "exited") return;
		setPhase(id, "running", source);
	},

	/// L2c adapter says the harness is awaiting user input.
	/// Bypasses the chunk throttle for the same reason.
	setWaitingFromAdapter(id: string, source: TransitionSource): void {
		const cur = store.get(id);
		if (!cur || cur.phase === "exited") return;
		setPhase(id, "waiting", source);
	},

	/// Record user input (keystroke / paste) on a harness. Flips
	/// `hasUserInput` to true exactly once per spawn; subsequent
	/// keystrokes are no-ops. Doesn't emit — consumers that care
	/// (notification logic) read the flag lazily at transition time.
	recordInput(id: string): void {
		const cur = store.get(id);
		if (!cur || cur.hasUserInput) return;
		store.set(id, { ...cur, hasUserInput: true });
	},

	/// Record a chunk of PTY output. Side-effects: bumps
	/// `lastOutputAt`; appends the stripped chunk to the L2b tail
	/// buffer (only when no L2c adapter is attached); transitions
	/// `spawning|idle → running` if applicable; ignored when already
	/// `exited`. Skipped entirely during an active mute window so
	/// induced redraws (focus events, resize) don't reset the idle
	/// timer.
	///
	/// When a harness has an L2c adapter attached (`authoritative`),
	/// PTY chunks still bump `lastOutputAt` for diagnostics but do
	/// NOT change phase and do NOT feed the tail buffer. The
	/// adapter is the truth source for those harnesses; running the
	/// pattern matcher on Claude / opencode TUI output would only
	/// invite false positives (an assistant message that quotes
	/// "(y/n)" shouldn't flip the dot).
	///
	/// `chunk` is optional for backwards compatibility — callers
	/// that don't have the raw bytes (e.g. synthetic recordings)
	/// can pass `undefined` and the tail buffer stays empty for
	/// that harness.
	recordOutput(id: string, chunk?: string): void {
		const cur = store.get(id);
		if (!cur || cur.phase === "exited") return;
		const now = Date.now();
		const mute = muteUntil.get(id);
		if (mute !== undefined && now < mute) return;
		if (cur.authoritative) {
			// Adapter owns phase. Update lastOutputAt silently so any
			// future detach-fallback to L2a starts with a fresh
			// timestamp; don't touch phase or tail.
			cur.lastOutputAt = now;
			return;
		}
		// L2b: append stripped output to the tail buffer. The
		// matcher only looks at the last 256 chars, but we keep
		// 2 KB so a single fat PTY chunk doesn't immediately blow
		// past the matcher's window.
		if (chunk !== undefined && chunk.length > 0) {
			const stripped = stripAnsi(chunk);
			if (stripped.length > 0) {
				const combined = cur.tail + stripped;
				cur.tail = combined.length > TAIL_MAX_CHARS ? combined.slice(-TAIL_MAX_CHARS) : combined;
			}
		}
		// L2b: if we were in `waiting` and the freshly-arrived
		// output drained the prompt out of the tail (user
		// answered, child kept going), flip back to running.
		// Without this, "Password:" → user types → output continues
		// → we'd stay stuck in waiting because the pattern matched
		// at the moment of transition but not anymore.
		if (cur.phase === "waiting") {
			if (matchesWaitingPrompt(cur.tail)) {
				cur.lastOutputAt = now;
				return;
			}
			setPhase(id, "running", TRANSITION_SOURCE.L2bDrained, { lastOutputAt: now });
			return;
		}
		if (cur.phase === "running") {
			// Silent mutation: every PTY chunk fires this — emitting
			// would re-render every subscriber on every chunk. The
			// tick reads the latest `lastOutputAt` so detection
			// stays correct.
			cur.lastOutputAt = now;
			return;
		}
		setPhase(id, "running", TRANSITION_SOURCE.PtyOutput, { lastOutputAt: now });
	},

	/// Mute incoming output for this harness for ~800 ms. Call right
	/// before causing an action that's likely to provoke an "induced"
	/// redraw from the child — currently used by `LiveTerminal` when
	/// it forwards a focus-in/-out escape (`\x1b[I` / `\x1b[O`) to
	/// the child. Many TUIs (Claude Code, opencode) redraw their
	/// whole screen in response, which we don't want counted as the
	/// child being "active" — the child only redrew because we
	/// poked it.
	muteInducedOutput(id: string): void {
		muteUntil.set(id, Date.now() + INDUCED_MUTE_MS);
	},

	/// Record PTY exit. Transitions to `exited` regardless of prior
	/// phase. Idempotent.
	exited(id: string, code: number | null): void {
		const cur = store.get(id);
		if (!cur) {
			// A late exit for a harness we never saw spawn (shouldn't
			// normally happen but worth recording so consumers see
			// the truth).
			store.set(id, {
				phase: "exited",
				lastOutputAt: null,
				exitCode: code,
				spawnedAt: Date.now(),
				hasUserInput: false,
				authoritative: false,
				tail: "",
			});
			emit(id);
			return;
		}
		if (cur.phase === "exited") return;
		setPhase(id, "exited", TRANSITION_SOURCE.PtyExit, { exitCode: code });
	},

	/// Drop a harness from the store entirely. Call from LiveTerminal
	/// on unmount so we don't accumulate dead entries.
	forget(id: string): void {
		if (!store.has(id) && !listeners.has(id) && !muteUntil.has(id)) return;
		store.delete(id);
		listeners.delete(id);
		muteUntil.delete(id);
		stopTickIfIdle();
	},

	get(id: string): HarnessActivity | null {
		return store.get(id) ?? null;
	},

	subscribe(id: string, cb: () => void): () => void {
		let set = listeners.get(id);
		if (!set) {
			set = new Set();
			listeners.set(id, set);
		}
		set.add(cb);
		return () => {
			const s = listeners.get(id);
			if (!s) return;
			s.delete(cb);
			if (s.size === 0) listeners.delete(id);
		};
	},

	/// Subscribe to every phase transition across every harness.
	/// One callback fires for each real transition with `(id, from,
	/// to)`. Returns an unsubscribe. Used by App-level notification
	/// logic that fans out to multiple rooms.
	subscribeTransitions(cb: TransitionListener): () => void {
		transitionListeners.add(cb);
		return () => {
			transitionListeners.delete(cb);
		};
	},
};

/// React hook: subscribes to the activity store for a single
/// harness and returns its current state. Returns `null` for
/// unknown ids (e.g. archived harnesses) so callers can fall
/// back gracefully.
export function useHarnessActivity(id: string | null): HarnessActivity | null {
	return useSyncExternalStore(
		(cb) => {
			if (id === null) return () => {};
			return harnessActivity.subscribe(id, cb);
		},
		() => (id === null ? null : harnessActivity.get(id)),
	);
}

/// Map the internal activity phase onto the existing display
/// `Status` enum used by `StatusDot` and the status bar. Keep
/// the mapping centralized so future strategies (additional L2c
/// adapters, eventual L2b pattern fallback) refine it in one place.
export function activityToStatus(activity: HarnessActivity | null): Status {
	if (!activity) return "running";
	switch (activity.phase) {
		case "spawning":
		case "running":
			return "running";
		case "idle":
			return "idle";
		case "waiting":
			return "waiting";
		case "exited":
			return "exited";
	}
}

/// `activityToStatus` with the "acknowledged" downgrade applied:
/// when the phase is `waiting` and the harness has zero pending
/// notifications, render as `idle` (grey) instead of `waiting`
/// (blue + pulse). The user has already been to the harness
/// since the last transition; the dot's job there is "telling you
/// what's new," and there's nothing new to tell.
///
/// The bottom-bar TEXT label still uses the underlying phase
/// (`waiting`) so callers can honestly report what Claude is
/// doing; only the visual indicator collapses.
export function effectiveStatus(
	activity: HarnessActivity | null,
	pendingNotifications: number,
): Status {
	const base = activityToStatus(activity);
	if (base === "waiting" && pendingNotifications === 0) return "idle";
	return base;
}

/// Priority order for combining multiple harness statuses into a
/// single room-level status (epic #50 L4). Higher = more important
/// to surface on the room dot. `waiting` lands top — a harness
/// blocked on user input is the most urgent thing a room can be
/// doing.
const STATUS_PRIORITY: Record<Status, number> = {
	waiting: 5,
	running: 4,
	idle: 3,
	exited: 2,
	error: 1,
};

/// Minimal shape `useRoomActivity` needs from each harness: an id
/// for store lookup + the pending-notifications count for the
/// effective-status downgrade. `pendingNotifications` is explicitly
/// `| undefined` so it accepts the `Harness` type (where the field
/// is optional) under `exactOptionalPropertyTypes`.
export interface RoomHarnessRef {
	id: string;
	pendingNotifications?: number | undefined;
}

const aggregateRoomStatus = (harnesses: readonly RoomHarnessRef[]): Status | null => {
	let best: Status | null = null;
	for (const h of harnesses) {
		const a = store.get(h.id);
		if (!a) continue;
		// Per-harness effective status: waiting downgrades to idle
		// when the harness has been viewed since the last
		// transition. Otherwise a room with one waiting-but-
		// acknowledged harness would keep pulsing the room tab
		// even though the user knows.
		const s = effectiveStatus(a, h.pendingNotifications ?? 0);
		if (best === null || STATUS_PRIORITY[s] > STATUS_PRIORITY[best]) {
			best = s;
		}
	}
	return best;
};

/// React hook: subscribes to every harness in a room and returns
/// the aggregate room status. Returns `null` when no harness in
/// the list has a record yet (first paint before LiveTerminal
/// effects fire) so callers can fall back to the persisted
/// `room.status`.
///
/// Takes the full harness records (rather than just ids) so the
/// aggregation can apply the same acknowledged-downgrade rule
/// `effectiveStatus` uses per-harness — a room dot shouldn't pulse
/// for a harness the user has already seen.
///
/// Caller responsibility: pass a stable `harnesses` reference
/// (use useMemo). `useSyncExternalStore` re-subscribes whenever
/// `subscribe` changes; a fresh array reference each render would
/// thrash the listener Sets without changing behaviour.
export function useRoomActivity(harnesses: readonly RoomHarnessRef[]): Status | null {
	const subscribe = useCallback(
		(cb: () => void) => {
			const unsubs = harnesses.map((h) => harnessActivity.subscribe(h.id, cb));
			return () => {
				for (const u of unsubs) u();
			};
		},
		[harnesses],
	);
	const getSnapshot = useCallback(() => aggregateRoomStatus(harnesses), [harnesses]);
	return useSyncExternalStore(subscribe, getSnapshot);
}
