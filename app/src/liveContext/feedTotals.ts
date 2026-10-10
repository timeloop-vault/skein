// Session cost/token totals for the Activity card head. Split out of
// feedItems.tsx (#460).

import { createRoomFold, deriveRoom, foldRows } from "./roomFold.ts";
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
export function sessionTotals(actions: readonly HarnessAction[]): { usd: number; tokens: number } {
	const fold = createRoomFold();
	foldRows(fold, actions);
	return deriveRoom(fold).totals;
}

/// `18.4k tok` — the k-abbreviated head style the per-turn hair-line
/// comment reserves. Exact below 1000 so a quiet session doesn't read
/// `0.4k`.
export function formatTokensShort(tokens: number): string {
	if (tokens < 1000) return `${tokens}`;
	return `${(tokens / 1000).toFixed(1)}k`;
}
