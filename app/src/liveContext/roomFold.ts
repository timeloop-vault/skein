// Incremental fold of a room's derived Live Context state (#593).
//
// Plan groups, session totals, the away-summary subtitle, the event count
// and the idle basis all used to be recomputed by rescanning the whole
// history on every new row. Here each row is folded once (O(1), payload
// parsed at most once and only for the kinds that matter) and
// `deriveRoom` costs by plan items / harnesses, never by rows folded.
//
// Rows must arrive in ascending-id order, each batch newer than anything
// already folded. `reducePlan` and `sessionTotals` are thin wrappers over
// this, so each rule has exactly one implementation.

import { costFromClaude, stepFromOpencode } from "./feedItemsHelpers.ts";
import { num, obj, type Payload, parsePayload, str } from "./payload.ts";
import {
	applyDelta,
	type DeltaState,
	deltaItems,
	type PlanGroup,
	snapshotItems,
} from "./planItems.ts";
import type { HarnessAction } from "./store.ts";

interface HarnessPlan {
	/** #538: first non-null stored kind among the harness's plan rows. */
	kind: string | null;
	/** Latest full-list snapshot; once set, deltas are ignored for good. */
	lastWrite: Payload | undefined;
	delta: DeltaState;
}

export interface RoomFold {
	eventCount: number;
	lastActionTs: number;
	/** Bumped once per folded plan_change row; lets deriveRoom reuse planGroups. */
	planVersion: number;
	/** Insertion order = each harness's first plan_change appearance. */
	plans: Map<string, HarnessPlan>;
	seenRequestIds: Set<string>;
	seenSteps: Set<string>;
	/** Latest `cost_state` per harness; snapshots supersede. */
	rollup: Map<string, number>;
	usd: number;
	tokens: number;
	awaySummary: { content: string; timestampMs: number } | null;
}

export interface RoomDerived {
	planGroups: PlanGroup[];
	/** The fold's planVersion this was derived at. */
	planVersion: number;
	totals: { usd: number; tokens: number };
	awaySummary: { content: string; timestampMs: number } | null;
	eventCount: number;
	lastActionTs: number;
}

export function createRoomFold(): RoomFold {
	return {
		eventCount: 0,
		lastActionTs: 0,
		planVersion: 0,
		plans: new Map(),
		seenRequestIds: new Set(),
		seenSteps: new Set(),
		rollup: new Map(),
		usd: 0,
		tokens: 0,
		awaySummary: null,
	};
}

function foldPlan(fold: RoomFold, a: HarnessAction): void {
	let plan = fold.plans.get(a.harnessId);
	if (!plan) {
		plan = { kind: null, lastWrite: undefined, delta: { byId: new Map(), order: [] } };
		fold.plans.set(a.harnessId, plan);
	}
	fold.planVersion++;
	if (plan.kind === null && a.harnessKind) plan.kind = a.harnessKind;
	const p = parsePayload(a.payload);
	const pi = obj(p.plan_item);
	if (str(pi?.op) === "write") plan.lastWrite = p;
	else if (pi && !plan.lastWrite) applyDelta(plan.delta, pi);
}

function foldCost(fold: RoomFold, a: HarnessAction): void {
	const p: Payload = parsePayload(a.payload);
	if ("usage" in p || "stop_reason" in p) {
		const cost = costFromClaude(p);
		if (!cost || cost.tokens <= 0) return;
		const requestId = str(p.request_id);
		if (requestId) {
			if (fold.seenRequestIds.has(requestId)) return;
			fold.seenRequestIds.add(requestId);
		}
		fold.tokens += cost.tokens;
		fold.usd += cost.usd;
	} else if ("tokens" in p || "reason" in p) {
		const stepKey = `${a.harnessId}|${a.payload}`;
		if (fold.seenSteps.has(stepKey)) return;
		fold.seenSteps.add(stepKey);
		const step = stepFromOpencode(p);
		fold.tokens += step.tokens;
		fold.usd += step.usd;
	}
}

/// Latest `away_summary` with non-empty `content` wins; payload is
/// `{ content: string }`.
function foldAway(fold: RoomFold, a: HarnessAction): void {
	const content = str(parsePayload(a.payload).content);
	if (content) fold.awaySummary = { content, timestampMs: a.timestampMs };
}

export function foldRows(fold: RoomFold, rows: readonly HarnessAction[]): void {
	for (const a of rows) {
		fold.eventCount++;
		if (a.timestampMs > fold.lastActionTs) fold.lastActionTs = a.timestampMs;
		switch (a.kind) {
			case "plan_change":
				foldPlan(fold, a);
				break;
			case "turn_cost":
				foldCost(fold, a);
				break;
			case "cost_state": {
				const v = num(parsePayload(a.payload).total_cost_usd);
				if (v !== undefined) fold.rollup.set(a.harnessId, v);
				break;
			}
			case "away_summary":
				foldAway(fold, a);
				break;
		}
	}
}

/// `prev` must derive from the SAME fold object (versions restart on a
/// rebuilt fold). Unchanged pieces keep their previous identity so memoised
/// consumers (PlanCardBody, planTally, the subtitle) skip: `planGroups` is
/// reused unless a plan_change row was folded since, `totals` and
/// `awaySummary` unless their values moved.
export function deriveRoom(fold: RoomFold, prev?: RoomDerived): RoomDerived {
	let planGroups: PlanGroup[];
	if (prev && prev.planVersion === fold.planVersion) {
		planGroups = prev.planGroups;
	} else {
		planGroups = [];
		for (const [harnessId, plan] of fold.plans) {
			const items = plan.lastWrite
				? snapshotItems(plan.lastWrite, harnessId)
				: deltaItems(plan.delta, harnessId);
			if (items.length > 0) planGroups.push({ harnessId, harnessKind: plan.kind, items });
		}
	}
	let usd = fold.usd;
	for (const v of fold.rollup.values()) usd += v;
	const totals =
		prev && prev.totals.usd === usd && prev.totals.tokens === fold.tokens
			? prev.totals
			: { usd, tokens: fold.tokens };
	const a = fold.awaySummary;
	const awaySummary =
		prev &&
		prev.awaySummary?.content === a?.content &&
		prev.awaySummary?.timestampMs === a?.timestampMs
			? prev.awaySummary
			: a
				? { ...a }
				: null;
	return {
		planGroups,
		planVersion: fold.planVersion,
		totals,
		awaySummary,
		eventCount: fold.eventCount,
		lastActionTs: fold.lastActionTs,
	};
}
