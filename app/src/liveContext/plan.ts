// Plan-state reducer for the Plan card — issue #80 D4.
//
// The Plan card shows CURRENT plan state per harness, not a feed, so the
// `plan_change` rows must be folded down. The two harnesses diverge (see
// docs/live-context-d2-buildmap.md):
//
//   - opencode (`todowrite`) emits a FULL todo list every time
//     (`plan_item.op === "write"`, `plan_item.items` = the whole list) —
//     latest snapshot wins, earlier rows are discarded.
//   - Claude (`TaskCreate`/`TaskUpdate`) emits incremental deltas keyed
//     by `plan_item.id`: a create carries the subject + initial status,
//     an update carries only a `status_change.{from,to}` — so the
//     subject must be carried forward from the create.
//
// A given harness only ever uses one style, so we detect per harness by
// whether any of its rows is an `op:"write"`.

import { type Payload, obj, parsePayload, str } from "./payload.ts";
import type { HarnessAction } from "./store.ts";

/// Display status, normalised across both harnesses' vocabularies. The
/// CSS class is `lc-plan-row.<status>`.
export type PlanStatus = "done" | "now" | "next" | "cancelled";

export interface PlanItem {
	/** Stable-ish React key within its group. */
	key: string;
	status: PlanStatus;
	text: string;
	/** opencode only; absent / `low` render no pill (handover §5.2 styles
	 *  only high + med). */
	priority?: "high" | "med";
	/** opencode "now" we synthesised because the list had no explicit
	 *  in_progress — rendered with a soft "· inferred" annotation. */
	inferred?: boolean;
}

export interface PlanGroup {
	harnessId: string;
	items: PlanItem[];
}

/// Map a backend status string to the display vocabulary. Unknown /
/// pending / absent all read as "next" (not started).
function displayStatus(s: string | undefined): PlanStatus {
	switch (s) {
		case "completed":
		case "done":
			return "done";
		case "in_progress":
		case "active":
			return "now";
		case "cancelled":
		case "canceled":
			return "cancelled";
		default:
			return "next";
	}
}

/// opencode priority → pill class. Only high + medium are styled
/// (handover §5.2); `low` and absent get no pill rather than a
/// med-coloured "low" that would misread.
function displayPriority(p: string | undefined): "high" | "med" | undefined {
	if (p === "high") return "high";
	if (p === "medium" || p === "med") return "med";
	return undefined;
}

/// opencode's todo list historically lacked an in_progress state, so the
/// handover infers "now" as the first not-started item after the most
/// recent done one. Real data now does carry in_progress, so this only
/// fires as a fallback: when nothing is explicitly "now" but at least one
/// item is done.
function inferNow(items: PlanItem[]): void {
	if (items.some((i) => i.status === "now")) return;
	let lastDone = -1;
	for (let i = 0; i < items.length; i++) {
		if (items[i]?.status === "done") lastDone = i;
	}
	if (lastDone === -1) return;
	for (let i = lastDone + 1; i < items.length; i++) {
		const it = items[i];
		if (it?.status === "next") {
			it.status = "now";
			it.inferred = true;
			return;
		}
	}
}

/// Fold opencode's latest full snapshot into display items.
function opencodeItems(lastWrite: Payload, harnessId: string): PlanItem[] {
	const pi = obj(lastWrite.plan_item);
	const raw = pi && Array.isArray(pi.items) ? pi.items : [];
	const items: PlanItem[] = [];
	for (let i = 0; i < raw.length; i++) {
		const it = obj(raw[i]);
		if (!it) continue;
		const text = str(it.content) ?? "";
		if (!text) continue;
		const item: PlanItem = {
			key: `${harnessId}-${i}`,
			status: displayStatus(str(it.status)),
			text,
		};
		const pri = displayPriority(str(it.priority));
		if (pri) item.priority = pri;
		items.push(item);
	}
	inferNow(items);
	return items;
}

/// Fold Claude's create/update deltas, keyed by plan_item.id, preserving
/// creation order. Update rows carry no subject, so it's carried forward
/// from the create; status comes from `status_change.to`.
function claudeItems(rows: HarnessAction[], harnessId: string): PlanItem[] {
	const byId = new Map<string, { subject: string; status: string }>();
	const order: string[] = [];
	for (const row of rows) {
		const pi = obj(parsePayload(row.payload).plan_item);
		if (!pi) continue;
		const op = str(pi.op);
		const id = str(pi.id);
		if (!id) continue;
		if (op === "create") {
			if (!byId.has(id)) order.push(id);
			byId.set(id, { subject: str(pi.subject) ?? id, status: str(pi.status) ?? "pending" });
		} else if (op === "update") {
			const existing = byId.get(id);
			const to = str(obj(pi.status_change)?.to);
			if (existing) {
				if (to) existing.status = to;
			} else {
				// Update with no preceding create (history truncated before
				// the create row) — show it anyway, keyed by id.
				order.push(id);
				byId.set(id, { subject: id, status: to ?? "pending" });
			}
		}
	}
	return order.map((id) => {
		const e = byId.get(id);
		return {
			key: `${harnessId}-${id}`,
			status: displayStatus(e?.status),
			text: e?.subject ?? id,
		};
	});
}

/// Reduce all of a room's `plan_change` rows into current per-harness
/// plan groups, in each harness's first-appearance order. Rows must be in
/// ascending id (chronological) order — the store keeps them that way.
export function reducePlan(actions: HarnessAction[]): PlanGroup[] {
	const rowsByHarness = new Map<string, HarnessAction[]>();
	const order: string[] = [];
	for (const a of actions) {
		if (a.kind !== "plan_change") continue;
		let list = rowsByHarness.get(a.harnessId);
		if (!list) {
			list = [];
			rowsByHarness.set(a.harnessId, list);
			order.push(a.harnessId);
		}
		list.push(a);
	}
	const groups: PlanGroup[] = [];
	for (const harnessId of order) {
		const rows = rowsByHarness.get(harnessId);
		if (!rows || rows.length === 0) continue;
		// opencode if any row is a full-list write; else Claude deltas.
		let lastWrite: Payload | undefined;
		for (const row of rows) {
			const p = parsePayload(row.payload);
			if (str(obj(p.plan_item)?.op) === "write") lastWrite = p;
		}
		const items = lastWrite ? opencodeItems(lastWrite, harnessId) : claudeItems(rows, harnessId);
		if (items.length > 0) groups.push({ harnessId, items });
	}
	return groups;
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
