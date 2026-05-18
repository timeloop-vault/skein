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
}

/// Sustained silence threshold for `running → idle`. Hard-coded for
/// v1; epic #50 L5e moves this into Settings.
const IDLE_AFTER_MS = 8_000;
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

/// Global transition callback: receives every real phase change.
/// Used by App-level notification logic (#12 L5a tab badges, L5b
/// OS notifications, L5c toasts) which need to react to transitions
/// across every harness without subscribing to each id separately.
export type TransitionListener = (id: string, from: ActivityPhase, to: ActivityPhase) => void;
const transitionListeners = new Set<TransitionListener>();

const emit = (id: string): void => {
	const set = listeners.get(id);
	if (!set) return;
	for (const cb of set) cb();
};

const setPhase = (id: string, phase: ActivityPhase, patch?: Partial<HarnessActivity>): void => {
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
		for (const cb of transitionListeners) cb(id, from, phase);
	}
};

const ensureTick = (): void => {
	if (tickHandle !== null) return;
	tickHandle = setInterval(() => {
		const now = Date.now();
		for (const [id, a] of store) {
			// Harnesses with an authoritative source (Claude JSONL
			// tail, future opencode adapter) write their own phase
			// from the L2c module. The idle heuristic would fight
			// the adapter — e.g. when Claude is mid-turn but the
			// PTY is briefly quiet during a tool call, L2a would
			// flip to `idle` while L2c says `running`. Skip these.
			if (a.authoritative) continue;
			if (a.phase === "running" && a.lastOutputAt !== null) {
				if (now - a.lastOutputAt >= IDLE_AFTER_MS) {
					setPhase(id, "idle");
				}
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
	/// adapter event boundary, not on the next PTY chunk.
	setRunningFromAdapter(id: string): void {
		const cur = store.get(id);
		if (!cur || cur.phase === "exited") return;
		setPhase(id, "running");
	},

	/// L2c adapter says the harness is awaiting user input.
	/// Bypasses the chunk throttle for the same reason.
	setWaitingFromAdapter(id: string): void {
		const cur = store.get(id);
		if (!cur || cur.phase === "exited") return;
		setPhase(id, "waiting");
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
	/// `lastOutputAt`; transitions `spawning|idle → running` if
	/// applicable; ignored when already `exited`. Skipped entirely
	/// during an active mute window so induced redraws (focus
	/// events, resize) don't reset the idle timer.
	///
	/// When a harness has an L2c adapter attached (`authoritative`),
	/// PTY chunks still bump `lastOutputAt` for diagnostics but do
	/// NOT change phase. Without this guard the resume case
	/// regresses: adapter sets `waiting` from the history probe →
	/// Claude redraws its TUI on attach → `recordOutput` overrides
	/// back to `running`. Same trap during a normal turn: adapter
	/// emits `AwaitingPrompt` on `end_turn`, Claude redraws on the
	/// next keystroke or focus event, `recordOutput` flips it back
	/// to `running`. Found in v0.1.9-dev — every Claude harness
	/// stayed green forever; badges fired on the brief `→ waiting`
	/// moment then the dot reverted.
	recordOutput(id: string): void {
		const cur = store.get(id);
		if (!cur || cur.phase === "exited") return;
		const now = Date.now();
		const mute = muteUntil.get(id);
		if (mute !== undefined && now < mute) return;
		if (cur.authoritative) {
			// Adapter owns phase. Update lastOutputAt silently so any
			// future detach-fallback to L2a starts with a fresh
			// timestamp; don't touch phase.
			cur.lastOutputAt = now;
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
		setPhase(id, "running", { lastOutputAt: now });
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
			});
			emit(id);
			return;
		}
		if (cur.phase === "exited") return;
		setPhase(id, "exited", { exitCode: code });
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
/// (yellow + pulse). The user has already been to the harness
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
