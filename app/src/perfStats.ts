// Pure statistics for the #599 main-thread telemetry. No DOM, no timers,
// no Tauri: perfTelemetry.ts owns the clocks and the log call.

export const SAMPLE_CAP = 4096;

/** Nearest-rank percentile over an ASCENDING-sorted array: the value at
 *  rank ceil(p/100 * n) (1-based). Empty input yields 0. */
export function percentile(sorted: readonly number[], p: number): number {
	if (sorted.length === 0) return 0;
	const rank = Math.max(1, Math.ceil((p / 100) * sorted.length));
	return sorted[Math.min(rank, sorted.length) - 1] ?? 0;
}

export interface Summary {
	count: number;
	p50: number;
	p95: number;
	max: number;
}

export function summarise(samples: readonly number[]): Summary {
	const sorted = [...samples].sort((a, b) => a - b);
	return {
		count: sorted.length,
		p50: percentile(sorted, 50),
		p95: percentile(sorted, 95),
		max: sorted[sorted.length - 1] ?? 0,
	};
}

/** Stall thresholds (strictly over, ms) counted exactly by SampleBuffer. */
export const STALL_LOW_MS = 50;
export const STALL_HIGH_MS = 200;

/** Bounded sample buffer. Once `cap` is reached it keeps a uniform random
 *  reservoir (Algorithm R), so percentiles stay representative. `count`,
 *  `max` and the over-threshold counts are tracked exactly, outside the
 *  reservoir. `random` is injectable for tests. */
export class SampleBuffer {
	private items: number[] = [];
	private total = 0;
	private maxSeen = 0;
	private over50 = 0;
	private over200 = 0;

	constructor(
		private readonly cap = SAMPLE_CAP,
		private readonly random: () => number = Math.random,
	) {}

	add(v: number): void {
		this.total++;
		if (v > this.maxSeen) this.maxSeen = v;
		if (v > STALL_LOW_MS) this.over50++;
		if (v > STALL_HIGH_MS) this.over200++;
		if (this.items.length < this.cap) {
			this.items.push(v);
			return;
		}
		const j = Math.floor(this.random() * this.total);
		if (j < this.cap) this.items[j] = v;
	}

	get count(): number {
		return this.total;
	}

	/** Exact counts; p50/p95 from the (possibly sampled) reservoir. */
	summary(): Summary & { over50: number; over200: number } {
		const s = summarise(this.items);
		return {
			...s,
			count: this.total,
			max: this.maxSeen,
			over50: this.over50,
			over200: this.over200,
		};
	}

	reset(): void {
		this.items = [];
		this.total = 0;
		this.maxSeen = 0;
		this.over50 = 0;
		this.over200 = 0;
	}
}

/** "main" = main-thread time of synchronous work; "wall" = wall-clock
 *  latency including awaited IPC/I/O (the main thread was NOT busy for it). */
export type SpanKind = "main" | "wall";

export interface SpanStat {
	count: number;
	totalMs: number;
	maxMs: number;
	kind: SpanKind;
}

/** Accumulates span durations per area; remembers the kind per area (the
 *  latest recorded kind wins). */
export class SpanAccumulator<A extends string> {
	private stats = new Map<A, SpanStat>();

	record(area: A, ms: number, kind: SpanKind = "main"): void {
		const cur = this.stats.get(area);
		if (cur) {
			cur.count++;
			cur.totalMs += ms;
			cur.kind = kind;
			if (ms > cur.maxMs) cur.maxMs = ms;
		} else {
			this.stats.set(area, { count: 1, totalMs: ms, maxMs: ms, kind });
		}
	}

	snapshot(): [A, SpanStat][] {
		return [...this.stats.entries()].map(([a, s]) => [a, { ...s }]);
	}

	reset(): void {
		this.stats.clear();
	}
}

export interface PerfSummaryInput {
	reason: string;
	windowSec: number;
	drift: Summary & { over50: number; over200: number };
	/** null = PerformanceObserver longtask unsupported. */
	longtasks: number | null;
	rooms: number;
	harnesses: number;
	spans: readonly (readonly [string, SpanStat])[];
}

/** Whole milliseconds, rounded to nearest. */
const ms = (v: number): string => `${Math.round(v)}ms`;

/** ONE line: no room names, paths or payloads. Zero-count areas omitted. */
export function formatPerfSummary(i: PerfSummaryInput): string {
	const d = i.drift;
	const parts = [
		`perf summary reason=${i.reason} window=${Math.round(i.windowSec)}s drift p50=${ms(d.p50)} p95=${ms(d.p95)} max=${ms(d.max)} samples=${d.count} stalls>50ms=${d.over50} stalls>200ms=${d.over200} longtasks=${i.longtasks === null ? "n/a" : i.longtasks} rooms=${i.rooms} harnesses=${i.harnesses}`,
	];
	for (const [area, s] of i.spans) {
		if (s.count === 0) continue;
		parts.push(
			`${area}${s.kind === "wall" ? "[wall]" : ""} n=${s.count} total=${ms(s.totalMs)} max=${ms(s.maxMs)}`,
		);
	}
	return parts.join(" | ");
}
