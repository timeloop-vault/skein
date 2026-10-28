// Room context size (#368): the lead harness's latest-turn input tokens.
// Pure helpers only; the fetching lives in useRoomContextSizes.ts and the
// formula in Rust (`db_harness_context_sizes`).

import type { HarnessKind } from "./types.ts";

/** Kinds whose `turn_cost` rows carry a context-size source. Hermes's
 *  turn_cost is a cumulative delta (no source); shell/files/copilot/remote
 *  have none. */
export const CONTEXT_SIZE_KINDS: ReadonlySet<HarnessKind> = new Set<HarnessKind>([
	"claude",
	"opencode",
]);

/** The harness whose context size stands for the room: the first in room
 *  order whose kind has a source. The mailbox's lead (mail_routing.rs) is
 *  the first harness that can read mail; for claude/opencode rooms that is
 *  usually the same harness, but mail eligibility depends on injection and
 *  MCP-allowlist settings that are irrelevant here, so we pick by having a
 *  source instead. */
export function contextLeadHarness<H extends { id: string; kind: HarnessKind }>(room: {
	harnesses: readonly H[];
}): H | undefined {
	return room.harnesses.find((h) => CONTEXT_SIZE_KINDS.has(h.kind));
}

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

export function contextSizeTitle(n: number, harnessName: string): string {
	return `Context: ${Math.round(n).toLocaleString("en-US")} tokens — latest turn of ${harnessName} (input + cache)`;
}

/** What a room shows: the lead's tokens and which harness they came from. */
export interface ContextSizeEntry {
	tokens: number;
	harnessName: string;
}

/** The current lead of a room, keyed by harness id in `ContextLeads`. */
export interface ContextLead {
	roomId: string;
	harnessName: string;
}
export type ContextLeads = ReadonlyMap<string, ContextLead>;

export interface ContextSizeRow {
	harnessId: string;
	tokens: number;
	timestampMs: number;
}

/** Fold one `db_harness_context_sizes` response into the per-room state.
 *  `issued` is the sequence number each requested harness was given when the
 *  request went out; `latest` is the newest sequence issued per harness now.
 *  A harness's value is applied only if its request is still the newest for
 *  that harness (a slow older response never overwrites a newer one) and its
 *  (room, harness) pair is still a lead (a response for a closed room or a
 *  lost lead never re-adds an entry). Returns `state` itself when nothing
 *  changed, so callers can skip notifying. */
export function applyResponse(
	state: ReadonlyMap<string, ContextSizeEntry>,
	latest: ReadonlyMap<string, number>,
	issued: ReadonlyMap<string, number>,
	currentLeads: ContextLeads,
	rows: readonly ContextSizeRow[],
): ReadonlyMap<string, ContextSizeEntry> {
	const byHarness = new Map(rows.map((r) => [r.harnessId, r.tokens]));
	let next: Map<string, ContextSizeEntry> | undefined;
	for (const [harnessId, seq] of issued) {
		if (latest.get(harnessId) !== seq) continue;
		const lead = currentLeads.get(harnessId);
		if (!lead) continue;
		const tokens = byHarness.get(harnessId);
		const prev = state.get(lead.roomId);
		if (tokens === undefined) {
			if (prev === undefined) continue;
			next ??= new Map(state);
			next.delete(lead.roomId);
		} else if (prev?.tokens !== tokens || prev.harnessName !== lead.harnessName) {
			next ??= new Map(state);
			next.set(lead.roomId, { tokens, harnessName: lead.harnessName });
		}
	}
	return next ?? state;
}
