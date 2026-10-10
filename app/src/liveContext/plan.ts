// Plan-state reducer for the Plan card — issue #80 D4.
//
// The Plan card shows CURRENT plan state per harness, not a feed, so the
// `plan_change` rows must be folded down. Two shapes exist (see
// docs/live-context-d2-buildmap.md):
//
//   - SNAPSHOT (`plan_item.op === "write"`, `plan_item.items` = the whole
//     list) — latest wins, earlier rows discarded. opencode's `todowrite`
//     and Claude's `TodoWrite` both send this.
//   - DELTA — Claude's `TaskCreate`/`TaskUpdate`, keyed by `plan_item.id`:
//     a create carries the subject + initial status, an update carries
//     only a `status_change.{from,to}`, so the subject must be carried
//     forward from the create.
//
// The shape is a property of the tool, not the harness, so we detect it
// per harness from the rows themselves: any `op:"write"` present means
// snapshot. A harness that switched tools mid-history therefore lands on
// its latest snapshot and ignores the older deltas, which is the right
// answer — the snapshot is the whole list.
//
// The fold itself lives in roomFold.ts (#593); reducePlan is a wrapper.

import type { PlanGroup } from "./planItems.ts";
import { createRoomFold, deriveRoom, foldRows } from "./roomFold.ts";
import type { HarnessAction } from "./store.ts";

export type { PlanGroup, PlanItem, PlanStatus } from "./planItems.ts";

/// Reduce all of a room's `plan_change` rows into current per-harness
/// plan groups, in each harness's first-appearance order. Rows must be in
/// ascending id (chronological) order — the store keeps them that way.
export function reducePlan(actions: readonly HarnessAction[]): PlanGroup[] {
	const fold = createRoomFold();
	foldRows(fold, actions);
	return deriveRoom(fold).planGroups;
}

/// Header tallies for the card meta — `N now · done/total`.
export function planTotals(groups: PlanGroup[]): { now: number; done: number; total: number } {
	let now = 0;
	let done = 0;
	let total = 0;
	for (const g of groups) {
		for (const it of g.items) {
			total++;
			if (it.status === "now") now++;
			else if (it.status === "done") done++;
		}
	}
	return { now, done, total };
}
