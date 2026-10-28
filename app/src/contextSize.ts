// Harness context size (#368): a harness's latest-turn input tokens.
// Pure helpers only; the fetching lives in useHarnessContextSizes.ts and the
// formula in Rust (`db_harness_context_sizes`).

import type { HarnessKind } from "./types.ts";

/** Kinds whose `turn_cost` rows carry a context-size source. Hermes's
 *  turn_cost is a cumulative delta (no source); shell/files/copilot/remote
 *  have none. */
export const CONTEXT_SIZE_KINDS: ReadonlySet<HarnessKind> = new Set<HarnessKind>([
	"claude",
	"opencode",
]);

/** "999", "1.2k", "142k", "1.2M". */
export function formatContextTokens(n: number): string {
	const v = Math.max(0, Math.round(n));
	if (v < 1000) return String(v);
	if (v < 10_000) {
		const k = Math.round(v / 100) / 10;
		// 9950+ rounds up to 10.0k; show it as a whole number.
		return `${Number.isInteger(k) ? k : k.toFixed(1)}k`;
	}
	if (v < 999_500) return `${Math.round(v / 1000)}k`;
	const m = Math.round(v / 100_000) / 10;
	return `${Number.isInteger(m) ? m : m.toFixed(1)}M`;
}

/** "context 142k". */
export function contextLabel(tokens: number): string {
	return `context ${formatContextTokens(tokens)}`;
}

export interface ContextSizeRow {
	harnessId: string;
	tokens: number;
	timestampMs: number;
}

/** Fold one `db_harness_context_sizes` response into the per-harness state.
 *  `issued` is the sequence number each requested harness was given when the
 *  request went out; `latest` is the newest sequence issued per harness now.
 *  A harness's value is applied only if its request is still the newest for
 *  that harness (a slow older response never overwrites a newer one) and it
 *  is still tracked (a response for a closed room never re-adds an entry).
 *  Returns `state` itself when nothing changed, so callers can skip
 *  notifying. */
export function applyResponse(
	state: ReadonlyMap<string, number>,
	latest: ReadonlyMap<string, number>,
	issued: ReadonlyMap<string, number>,
	tracked: ReadonlySet<string>,
	rows: readonly ContextSizeRow[],
): ReadonlyMap<string, number> {
	const byHarness = new Map(rows.map((r) => [r.harnessId, r.tokens]));
	let next: Map<string, number> | undefined;
	for (const [harnessId, seq] of issued) {
		if (latest.get(harnessId) !== seq) continue;
		if (!tracked.has(harnessId)) continue;
		const tokens = byHarness.get(harnessId);
		const prev = state.get(harnessId);
		if (tokens === undefined) {
			if (prev === undefined) continue;
			next ??= new Map(state);
			next.delete(harnessId);
		} else if (prev !== tokens) {
			next ??= new Map(state);
			next.set(harnessId, tokens);
		}
	}
	return next ?? state;
}
