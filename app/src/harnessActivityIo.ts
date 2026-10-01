// harnessActivity.ts split (#458): user input and PTY output. Methods are spread into the
// `harnessActivity` store object there; shared state lives in harnessActivityCore.ts.

import { INDUCED_MUTE_MS, TAIL_MAX_CHARS } from "./harnessActivityConstants.ts";
import {
	isDecisiveInput,
	muteUntil,
	setPhase,
	shouldArmWatchdog,
	store,
} from "./harnessActivityCore.ts";
import { TRANSITION_SOURCE } from "./harnessActivityTypes.ts";
import { matchesWaitingPrompt, stripAnsi } from "./harnessPatterns.ts";

export const ioMethods = {
	/// Record user input (keystroke / paste) on a harness. `data` is
	/// the exact bytes that keystroke sends to the child (xterm's
	/// `onKey` reports the same string `onData` would carry for it —
	/// real user input, not an auto-response the terminal generates for
	/// a device query).
	///
	/// Two independent effects: flips `hasUserInput` to true exactly
	/// once per spawn (subsequent keystrokes are no-ops there); and, if
	/// the harness is blocked on `permission`, checks whether `data` is
	/// "decisive" (see `isDecisiveInput`) and if so moves it to
	/// `running`. Claude gives no "the dialog was answered" signal of
	/// its own — the only way the user can answer it is by typing into
	/// the PTY, so that keystroke IS the signal (#86). Doesn't emit for
	/// the `hasUserInput` flip alone — consumers that care read the
	/// flag lazily at transition time.
	///
	/// #259: the first Enter on a harness whose attached adapter has
	/// said nothing yet arms the silent-adapter watchdog.
	recordInput(id: string, data: string): void {
		const cur = store.get(id);
		if (!cur) return;
		const submitted = data.includes("\r") || data.includes("\n");
		// #423: recorded unconditionally, unlike `promptSubmittedAt` below.
		if (submitted) cur.lastSubmitAt = Date.now();
		const armWatchdog = shouldArmWatchdog(cur) && submitted;
		if (!cur.hasUserInput || armWatchdog) {
			store.set(id, {
				...cur,
				hasUserInput: true,
				...(armWatchdog ? { promptSubmittedAt: Date.now() } : null),
			});
		}
		if (cur.phase === "permission" && isDecisiveInput(data)) {
			setPhase(id, "running", TRANSITION_SOURCE.UserInputPermission);
		}
	},

	/// #363: arm the #259 silent-adapter watchdog for a prompt submitted
	/// through the `sendPrompt` seam — `create_room`'s first prompt, a
	/// mail nudge, anything that pastes and submits without ever
	/// touching xterm's `onKey` and so never reaches `recordInput` above.
	/// Shares `recordInput`'s arm guard (`shouldArmWatchdog`, factored
	/// into `harnessActivityCore.ts` so the two can't drift apart) minus
	/// the "does `data` contain a newline" check — a `sendPrompt` call
	/// IS the submit, unconditionally, the same as a typed Enter.
	///
	/// Deliberately narrow: it only arms the timer. Doesn't touch
	/// `hasUserInput` — that flag means a human typed something, and a
	/// programmatic submit isn't that. Refuses to arm at all while
	/// `phase === "permission"` — an armed timer would eventually strip
	/// authority (`degradeSilentAdapter`) out from under a harness whose
	/// dialog is simply still open, not silent — and never clears
	/// `permission` or moves phase either way: #86 established that only
	/// a decisive user keystroke is proof a dialog was answered, and a
	/// programmatic submit is not one. `canSendPrompt` already refuses to
	/// send into `permission`, but this method doesn't lean on that; it
	/// makes the same call on its own terms, independently, since nothing
	/// stops a future caller from invoking it directly. No emit, matching
	/// `recordInput`'s own arm path — `promptSubmittedAt` is bookkeeping
	/// the tick reads, not a phase change any subscriber needs to hear
	/// about.
	notePromptSubmitted(id: string): void {
		const cur = store.get(id);
		// #423: a seam submit is a submit whatever the watchdog decides.
		if (cur) cur.lastSubmitAt = Date.now();
		if (!cur || cur.phase === "permission" || !shouldArmWatchdog(cur)) return;
		store.set(id, { ...cur, promptSubmittedAt: Date.now() });
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
		// #86: a harness blocked on `permission` stays blocked no
		// matter what repaints across the PTY — the dialog itself is
		// what's producing this output. Only decisive user input
		// (`recordInput`) or the adapter's own resolution signal may
		// move it; falling through to the L2b tail/pattern logic below
		// could otherwise misread the dialog's own text as a drained
		// prompt and flip back to running early.
		if (cur.phase === "permission") {
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
};
