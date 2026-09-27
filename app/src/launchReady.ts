// #404: the launch ping (`launchSignalAt`, #273) says the harness's CLI
// reported its own launch — nothing about whether it can actually take
// input yet. Skein's own SessionStart hook fires asynchronously, but
// another plugin's SYNCHRONOUS SessionStart hook can go on blocking
// Claude's TUI for real seconds after that ping, and a paste landing in
// that window is simply lost — there is no composer to catch it.
// Bracketed-paste mode is not a usable substitute signal either: Claude
// turns it on at first paint, well before the TUI can accept a paste
// (measured live, #388).
//
// The only thing actually observable from here is PTY quiet: once
// output has stopped for a bit, the CLI is very likely sitting still at
// its own prompt rather than mid-redraw. This module is pure and
// DOM-free — no store, no timers — `harnessInput.ts`'s `canSendPrompt`
// is the only caller, and it gathers `lastOutputAt` from
// `harnessActivity` the same way every other check in that gate does.
//
// Live measurement (2026-09-28, Windows, Claude Code 2.1.283, ConPTY): a
// normal fresh Claude paints its prompt ~4.4–5.7 s after spawn, then its
// PTY goes fully silent for ~9.8 s (one tiny non-visual blip after
// that). A blocking synchronous SessionStart hook from another plugin
// emits NO PTY output while it blocks — no spinner, nothing at hook
// end — so this gate proves only that the TUI finished painting; it
// cannot see a silent hook block. The backstop for that case is #388's
// settlement plus #404's own `recoverUnheardSilence`. In the same
// measurement, a paste written during the block was echoed into the
// input box, not lost; the Enter racing the hook was not tested (no
// submit), so where exactly the field loss happens is still open.

/// How long the harness's PTY has to stay quiet, measured from the
/// later of the launch ping and the last output chunk, before the first
/// paste is allowed through. Live measurement (2026-09-28, Windows,
/// Claude Code 2.1.283, ConPTY): a normal fresh Claude's startup burst
/// has gaps under 1 s, then goes silent for ~9.8 s once its prompt is
/// painted, so 1.5 s of quiet fires ~1 s after the prompt settles and
/// never inside the burst itself. It does NOT outlast a blocking
/// synchronous SessionStart hook — that hook emits no PTY output at
/// all, so this constant can't see it; see the module header.
export const LAUNCH_QUIET_MS = 1500;

/// Upper bound on how long the gate will hold a paste waiting for
/// quiet, measured from the launch ping itself. A harness that keeps
/// repainting past this point is simply a chatty TUI — a silent
/// blocking hook never reaches this cap, since it produces no PTY
/// output to keep resetting quiet (see the module header) — but either
/// way, waiting longer only delays a paste that would otherwise never
/// go out at all, so past the cap the gate lets it through and leans on
/// #388's own paste/submit settlement (`mailSettle.ts`) to roll back a
/// submit that still didn't land.
export const LAUNCH_READY_CAP_MS = 15_000;

/// Everything `launchSettled` needs, gathered by the caller
/// (`canSendPrompt`) so this stays pure and testable with no store.
export interface LaunchSettledInput {
	/// Has the L2c adapter delivered at least one event since spawn
	/// (`HarnessActivity.adapterHeard`)? A heard transcript means a
	/// prompt already reached this harness once before, so the gate this
	/// module adds is scoped OUT — there is nothing left to prove.
	adapterHeard: boolean;
	/// `HarnessActivity.launchSignalAt` — `null` means the harness never
	/// reported its own launch (injection off, or an authoritative kind
	/// with no launch hook), in which case this gate has nothing to key
	/// on and stands down.
	launchSignalAt: number | null;
	/// `HarnessActivity.lastOutputAt` — the most recent PTY chunk, bumped
	/// on every chunk including while `authoritative` (#404 relies on
	/// that: `recordOutput` keeps updating it for an adapter-owned
	/// harness even though phase itself doesn't move). Not gospel,
	/// though: `recordOutput` returns before touching it at all while a
	/// `muteInducedOutput` window is active (`INDUCED_MUTE_MS` after a
	/// forwarded focus in/out escape), so real output inside that window
	/// is invisible here — a focus toggle landing near the ping can make
	/// the terminal read quiet earlier than it actually went quiet. The
	/// #388 settlement and #404's own recovery are the backstop for a
	/// paste let through on that stale a read.
	lastOutputAt: number | null;
	/// The current time, in the same clock as the two fields above.
	nowMs: number;
}

export type LaunchSettledResult = { settled: true } | { settled: false; retryInMs: number };

/// Whether a freshly launch-signalled harness's PTY has been quiet long
/// enough to trust a first paste to it. Rules, in order:
///
///  - `adapterHeard` → settled. A transcript that has already spoken
///    once is proof input has already landed; this gate only exists for
///    the narrower case of a session whose sole proof of life is the
///    launch ping.
///  - `launchSignalAt === null` → settled. Nothing to measure quiet
///    against, and every other check in `canSendPrompt` already covers
///    "is anything watching this harness at all".
///  - Otherwise, quiet is measured from `quietSince = max(launchSignalAt,
///    lastOutputAt ?? launchSignalAt)` — a chunk older than the ping
///    itself (a stale `lastOutputAt` from before this launch) must never
///    pull `quietSince` backwards. Settled once `nowMs - quietSince >=
///    LAUNCH_QUIET_MS`, OR once `nowMs - launchSignalAt >=
///    LAUNCH_READY_CAP_MS` regardless of how recently output arrived —
///    the cap outranks quiet so a harness that repaints forever doesn't
///    wait forever.
///  - Otherwise not settled, with `retryInMs` set to whichever bound is
///    closer — the smaller of the remaining quiet window and the
///    remaining time to the cap — so a caller retrying on a timer (the
///    #386 mail retry already re-runs `canSendPrompt` on its own
///    schedule) doesn't have to re-derive that itself.
export function launchSettled(input: LaunchSettledInput): LaunchSettledResult {
	const { adapterHeard, launchSignalAt, lastOutputAt, nowMs } = input;
	if (adapterHeard || launchSignalAt === null) return { settled: true };
	const quietSince = Math.max(launchSignalAt, lastOutputAt ?? launchSignalAt);
	const quietElapsed = nowMs - quietSince;
	const capElapsed = nowMs - launchSignalAt;
	if (quietElapsed >= LAUNCH_QUIET_MS || capElapsed >= LAUNCH_READY_CAP_MS) {
		return { settled: true };
	}
	const remainingQuiet = LAUNCH_QUIET_MS - quietElapsed;
	const remainingCap = LAUNCH_READY_CAP_MS - capElapsed;
	return { settled: false, retryInMs: Math.min(remainingQuiet, remainingCap) };
}
