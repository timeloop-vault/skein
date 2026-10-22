// #599: always-on, low-overhead main-thread telemetry, summarised into the
// Rust log (skein.log) as ONE line per 5 minutes (and one on shutdown).
//
// Measures: (1) timer drift of a 250 ms probe (how late the main thread let
// it run), with stall counts over 50/200 ms; (2) `longtask` entries where the
// webview supports them (WKWebView does not: reported as n/a); (3) named
// spans around hot paths, accumulated as count/total/max plus p50/p95 (bounded per-area
// reservoir) per area. A single drift sample over STALL_WARN_MS also emits one
// timestamped WARN (target "perf") at once, so a long freeze can be placed in
// time rather than only counted in the next summary.
//
// Add an area: add a member to `PerfArea`, then wrap the hot path with
// `span` / `spanAsync`, or time it yourself and call `recordSpan`. Pick the
// kind honestly: `span` (sync) records "main" = main-thread time, i.e. jank
// you caused; `spanAsync` records "wall" = latency including awaited
// IPC/I/O, which does NOT block the main thread. Wall areas print as
// `area[wall]`; never compare the two. For code you time by hand (e.g.
// keystroke-to-echo or PTY-chunk spans) pass `recordSpan(area, ms, "wall")`
// when the interval spans an await/IPC round trip, else the default "main".
//
// Some areas are measured in Rust (off the main thread) and appended to this
// same line by the `perf_summary_log` command; currently `reviewDiscovery.batch`
// (`app/src-tauri/src/perf_spans.rs`). Those are NOT members of `PerfArea`,
// which holds frontend spans only.
//
// Must stay negligible: nothing is logged per event, buffers are bounded,
// and a span is two performance.now() calls plus a map update.

import { invoke } from "@tauri-apps/api/core";
import { logToRust } from "./frontendLog.ts";
import { formatPerfSummary, SampleBuffer, SpanAccumulator, type SpanKind } from "./perfStats.ts";

export type PerfArea =
	| "liveContext.apply"
	| "liveContext.flush"
	| "rooms.save"
	| "branchWatch.tick"
	| "pty.writeIpc"
	| "pty.echo"
	| "pty.echo.key"
	| "pty.echo.enter"
	| "pty.echo.ctrl"
	| "pty.echo.mouse"
	| "pty.echo.paste"
	| "pty.echo@waiting"
	| "pty.echo@running"
	| "pty.echo@other";

const PROBE_MS = 250;

/** A counted drift sample strictly over this many ms is logged immediately. */
export const STALL_WARN_MS = 1000;

/** Pure decision: the WARN message for a drift sample, or null if not a stall. */
export function stallWarning(driftMs: number): string | null {
	return driftMs > STALL_WARN_MS ? `ui stall drift=${Math.round(driftMs)}ms` : null;
}

const SUMMARY_MS = 5 * 60 * 1000;

const drift = new SampleBuffer();
const spans = new SpanAccumulator<PerfArea>();
let longtasks = 0;
let longtaskSupported = false;
let windowStart = now();
let getCounts: () => { rooms: number; harnesses: number } = () => ({ rooms: 0, harnesses: 0 });
let stop: (() => void) | null = null;

function now(): number {
	return typeof performance !== "undefined" ? performance.now() : Date.now();
}

/** Record a duration you timed yourself. `kind` "main" (default) = main-thread
 *  time of synchronous work; "wall" = wall-clock latency including awaited
 *  IPC/I/O, during which the main thread was NOT necessarily busy. Wall
 *  areas print as `area[wall]` in the summary. */
export function recordSpan(area: PerfArea, ms: number, kind: SpanKind = "main"): void {
	spans.record(area, ms, kind);
}

/** Time a SYNCHRONOUS function: recorded as "main" (main-thread time). */
export function span<T>(area: PerfArea, fn: () => T): T {
	const t0 = now();
	try {
		return fn();
	} finally {
		spans.record(area, now() - t0, "main");
	}
}

/** Time an async function: recorded as "wall" (latency including IPC and
 *  awaited I/O, NOT main-thread blocking). Do not read it as jank. */
export async function spanAsync<T>(area: PerfArea, fn: () => Promise<T>): Promise<T> {
	const t0 = now();
	try {
		return await fn();
	} finally {
		spans.record(area, now() - t0, "wall");
	}
}

let shutdownEmitted = false;

/** Emit the summary line. `reason=shutdown` is emitted at most once per
 *  process; later shutdown calls (e.g. pagehide after a confirmed quit)
 *  are no-ops. */
export function flushPerfSummary(reason: "interval" | "shutdown"): void {
	if (reason === "shutdown") {
		if (shutdownEmitted) return;
		shutdownEmitted = true;
	}
	const t = now();
	let counts = { rooms: 0, harnesses: 0 };
	try {
		counts = getCounts();
	} catch {
		// counts are best-effort
	}
	const line = formatPerfSummary({
		reason,
		windowSec: (t - windowStart) / 1000,
		drift: drift.summary(),
		longtasks: longtaskSupported ? longtasks : null,
		rooms: counts.rooms,
		harnesses: counts.harnesses,
		spans: spans.snapshot(),
	});
	try {
		void invoke("perf_summary_log", { line }).catch(() => {});
	} catch {
		// No Tauri runtime, or invoke threw synchronously: telemetry must not throw.
	}
	drift.reset();
	spans.reset();
	longtasks = 0;
	windowStart = t;
}

/** Starts the probe, longtask observer and 5-minute summary timer.
 *
 *  Idempotent while running: a second call WITHOUT an intervening stop
 *  (a concurrent caller) only replaces the counts provider (latest wins)
 *  and returns a no-op stop, so that caller's cleanup cannot kill the
 *  telemetry the first one owns. React StrictMode's mount, cleanup, mount
 *  is the expected case and works: the cleanup runs the real stop, so the
 *  second mount starts fresh and owns the real stop. */
export function startPerfTelemetry(counts: () => { rooms: number; harnesses: number }): () => void {
	getCounts = counts;
	if (stop) return () => {};
	windowStart = now();

	const cleanups: (() => void)[] = [];
	const hasDoc = typeof document !== "undefined";
	let lastVisChange = 0;
	let expected = now() + PROBE_MS;
	let prevTick = now();

	if (hasDoc) {
		const onVis = () => {
			lastVisChange = now();
		};
		document.addEventListener("visibilitychange", onVis);
		cleanups.push(() => document.removeEventListener("visibilitychange", onVis));
	}

	const probe = setInterval(() => {
		const t = now();
		const hidden = hasDoc && document.visibilityState === "hidden";
		// Throttled timers while hidden, or an interval that spanned a
		// visibility change, say nothing about main-thread congestion.
		if (!hidden && lastVisChange <= prevTick) {
			const d = Math.max(0, t - expected);
			drift.add(d);
			const warn = stallWarning(d);
			if (warn) logToRust("warn", "perf", warn);
		}
		prevTick = t;
		expected = t + PROBE_MS;
	}, PROBE_MS);
	cleanups.push(() => clearInterval(probe));

	if (typeof PerformanceObserver !== "undefined") {
		try {
			const supported = PerformanceObserver.supportedEntryTypes ?? [];
			if (supported.includes("longtask")) {
				const obs = new PerformanceObserver((list) => {
					longtasks += list.getEntries().length;
				});
				obs.observe({ entryTypes: ["longtask"] });
				longtaskSupported = true;
				cleanups.push(() => obs.disconnect());
			}
		} catch {
			longtaskSupported = false;
		}
	}

	const summary = setInterval(() => flushPerfSummary("interval"), SUMMARY_MS);
	cleanups.push(() => clearInterval(summary));

	const stopFn = () => {
		for (const c of cleanups) c();
		stop = null;
	};
	stop = stopFn;
	return stopFn;
}
