// Plan item vocabulary and the two per-harness item builders (snapshot,
// delta). Split out of plan.ts so roomFold.ts can use them without a cycle.

import { obj, type Payload, str } from "./payload.ts";

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
	/** #538: first non-null stored kind among the harness's rows. */
	harnessKind: string | null;
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

/// Fold the latest full snapshot into display items. `priority` is
/// opencode-only; Claude's `TodoWrite` items carry `content` + `status`
/// and no priority, which reads as "no pill" rather than a wrong one.
export function snapshotItems(lastWrite: Payload, harnessId: string): PlanItem[] {
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

/// Claude `TaskCreate`/`TaskUpdate` deltas folded so far for one harness,
/// keyed by plan_item.id, with creation order.
export interface DeltaState {
	byId: Map<string, { subject: string; status: string }>;
	order: string[];
}

/// Apply one delta row's `plan_item`. Update rows carry no subject, so it
/// is carried forward from the create; status comes from `status_change.to`.
export function applyDelta(state: DeltaState, pi: Payload): void {
	const op = str(pi.op);
	const id = str(pi.id);
	if (!id) return;
	const { byId, order } = state;
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

export function deltaItems(state: DeltaState, harnessId: string): PlanItem[] {
	return state.order.map((id) => {
		const e = state.byId.get(id);
		return {
			key: `${harnessId}-${id}`,
			status: displayStatus(e?.status),
			text: e?.subject ?? id,
		};
	});
}
