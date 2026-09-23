// Tuning constants for the harness activity state machine (epic #50).
// Split out of harnessActivity.ts (#19) — pure data, no behaviour. See
// harnessActivity.ts for the module this belongs to.

/// Sustained silence threshold for `running → idle`. Hard-coded for
/// v1; epic #50 L5e moves this into Settings.
export const IDLE_AFTER_MS = 8_000;
/// Shorter silence threshold for `running → waiting` via L2b
/// pattern match. The user types `sudo X`, "Password:" appears,
/// output stops — we want the dot to flip blue near-instantly, not
/// 8 s later. 500 ms is generous enough to avoid firing during
/// mid-stream output that happens to contain a prompt-looking
/// substring, while tight enough to feel responsive.
export const PATTERN_WAITING_AFTER_MS = 500;
/// Maximum tail-buffer size per harness. The matcher only scans
/// the last 256 chars; we keep more to handle large chunks that
/// arrive in one PTY read (xterm splits at no fixed boundary) but
/// cap so a chatty shell session doesn't grow memory unbounded.
export const TAIL_MAX_CHARS = 2_048;
/// How often the background tick scans for idle transitions. A
/// faster tick gives tighter detection latency; 1s strikes a
/// sensible balance — at worst the user sees "idle" up to a second
/// late, which is below the perceptual threshold for a status dot.
export const TICK_INTERVAL_MS = 1_000;
/// Window during which PTY output is treated as "our fault, not the
/// child's." Triggered explicitly by callers (e.g. LiveTerminal on
/// visibility flip — `term.focus()` sends a focus-in event to the
/// child, which many TUIs answer with a full redraw). Without this,
/// switching to a long-idle harness pops it back to `running` for 8s
/// before settling, which contradicts the actual state. 800 ms is
/// generous enough to cover slow repaints; if real output arrives
/// after the window, the normal path kicks back in.
export const INDUCED_MUTE_MS = 800;
/// How long an attached adapter may stay silent after the user submits
/// a prompt before the watchdog gives up on it (#259). Claude writes
/// the prompt row the moment it is submitted and opencode's stream
/// says `connected` before anything else, so a healthy adapter speaks
/// well inside this. Gated on a prompt rather than on spawn because
/// Claude creates no transcript at all until the first one: a fresh
/// harness left at its prompt is healthy, not silent.
export const ADAPTER_SILENT_AFTER_MS = 10_000;
/// #273 — how long an authoritative harness may sit in `spawning` with
/// no launch signal (`launchSignalAt` still `null`) before the tick
/// hands it back to L2a. Unlike `ADAPTER_SILENT_AFTER_MS`, this isn't
/// gated on a prompt — Claude writes no transcript until the first one,
/// so a harness with #215 injection off, or a kind whose CLI has no
/// launch hook, would otherwise sit in `spawning` forever with
/// `authoritative` true and the L2a tick standing down for it (the
/// original #273 bug). 15 s is generous compared with a real injected
/// start, where the hook lands a second or two after spawn — but it
/// deliberately accepts that a brand-new room sitting at an unanswered
/// "trust this folder?" dialog WILL hit this timer and fall back to
/// L2a. That's correct, not a regression: it's strictly better than
/// sitting in `spawning` forever, the input gates in `harnessInput.ts`
/// still refuse a harness that hasn't proven itself, and
/// `noteLaunchSignal` recovers the harness the moment the dialog is
/// accepted and the hook actually fires.
export const LAUNCH_SILENT_AFTER_MS = 15_000;
/// #277 (epic #298), Rule 3 — how long a deferred end-of-turn waits,
/// once the working-subagent set is observed empty, before flushing to
/// `waiting` on its own. Measured on 1009 real delegations: after a
/// subagent's transcript reaches a terminal row, the main session's
/// next row (proof it woke up and is working the delegation's result)
/// appears in 27 ms median, 1.7 s p90, 54.1 s max — every one of the
/// 1009 woke up. 60 s sits comfortably above the worst observed
/// wake-up, so this only ever fires when the session genuinely did not
/// come back.
export const DELEGATION_SETTLE_MS = 60_000;
/// #277, Rule 4 — the safety net for a lost subagent signal: while a
/// deferral is armed and the working set is non-empty, no event from
/// any of those subagents for this long presumes them gone. Measured
/// on 1038 real subagents: 12.4% have an internal quiet gap over 2
/// min, 4.4% over 5 min, 2.4% over 10 min (p99 ≈ 2 h) — ordinary tool
/// calls, not stuck sessions. A false "presumed gone" only costs one
/// early notification (today's behaviour), so the cost is asymmetric
/// and the constant leans long rather than risk a permanently
/// suppressed harness.
export const DELEGATION_CEILING_MS = 15 * 60_000;
