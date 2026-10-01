// Session cost/token totals for the Activity card head. Split out of
// feedItems.tsx (#460).

import { costFromClaude, stepFromOpencode } from "./feedItemsHelpers.ts";
import { type Payload, num, parsePayload, str } from "./payload.ts";
import type { HarnessAction } from "./store.ts";

/// Sub-cent costs render as a floor rather than a misleading "$0.00".
export function formatUsd(usd: number): string {
	return usd < 0.005 ? "<$0.01" : `$${usd.toFixed(2)}`;
}

/// (`Activity · 619 events · $1.42 · 18.4k tok`).
///
/// Cost has two sources and they are not interchangeable:
///
/// - **Claude** — `turn_cost` carries usage but no money (#91), so
///   summing it yields `$0.00` however long the session ran. `cost_state`
///   (#199) is Claude's own cumulative roll-up, so the newest row per
///   harness *is* the answer — snapshots supersede, never accumulate.
/// - **opencode** — `turn_cost` carries a real per-step `cost`, and
///   there is no session roll-up, so the steps are summed.
///
/// Both de-dupe exactly as `flattenFeed` does (Claude by `request_id`,
/// opencode by payload identity) or a re-emission would inflate the head.
///
/// One deliberate divergence from the rendered hair-lines: opencode steps
/// belonging to a turn that never reached a terminal step are counted
/// here though `flattenFeed` drops them. They are real spend — a turn in
/// flight has already cost money — so the head can read slightly higher
/// than the sum of the visible lines.
export function sessionTotals(actions: HarnessAction[]): { usd: number; tokens: number } {
	const seenRequestIds = new Set<string>();
	const seenSteps = new Set<string>();
	/// Latest `cost_state` per harness. Keyed because a room can hold
	/// several Claude harnesses, each with its own session roll-up.
	const rollup = new Map<string, number>();
	let tokens = 0;
	let usd = 0;
	for (const a of actions) {
		if (a.kind === "cost_state") {
			const v = num(parsePayload(a.payload).total_cost_usd);
			if (v !== undefined) rollup.set(a.harnessId, v);
			continue;
		}
		if (a.kind !== "turn_cost") continue;
		const p: Payload = parsePayload(a.payload);
		if ("usage" in p || "stop_reason" in p) {
			const cost = costFromClaude(p);
			if (!cost || cost.tokens <= 0) continue;
			const requestId = str(p.request_id);
			if (requestId) {
				if (seenRequestIds.has(requestId)) continue;
				seenRequestIds.add(requestId);
			}
			tokens += cost.tokens;
			usd += cost.usd;
		} else if ("tokens" in p || "reason" in p) {
			const stepKey = `${a.harnessId}|${a.payload}`;
			if (seenSteps.has(stepKey)) continue;
			seenSteps.add(stepKey);
			const step = stepFromOpencode(p);
			tokens += step.tokens;
			usd += step.usd;
		}
	}
	for (const v of rollup.values()) usd += v;
	return { usd, tokens };
}

/// `18.4k tok` — the k-abbreviated head style the per-turn hair-line
/// comment reserves. Exact below 1000 so a quiet session doesn't read
/// `0.4k`.
export function formatTokensShort(tokens: number): string {
	if (tokens < 1000) return `${tokens}`;
	return `${(tokens / 1000).toFixed(1)}k`;
}
