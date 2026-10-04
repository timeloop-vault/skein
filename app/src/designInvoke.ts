// designInvoke — the pure pieces of the agent's invoke_element (#549): arg
// parsing, and turning the iframe's `invoked` beacon into the verb's answer.
// The policy is "single, accepted match only": a tier show_element would
// refuse to highlight is never acted on, and nothing is ever guessed.

import { isOmitted, type RequestResult } from "./agentRequestsShared.ts";
import {
	type DesignTarget,
	parseAnchor,
	parseReveal,
	parseStateArgs,
	type ShowElementRequest,
	type ShowElementResult,
	type ShowTier,
} from "./designControl.ts";
import type { ElementDescriptor } from "./elementAnchor.ts";

export type InvokeAction = "tap" | "swipe";
export type InvokeDirection = "left" | "right" | "up" | "down";

export const DEFAULT_SWIPE_DISTANCE = 120;
export const MIN_SWIPE_DISTANCE = 8;
export const MAX_SWIPE_DISTANCE = 2000;

export type InvokeElementRequest = ShowElementRequest & {
	action: InvokeAction;
	direction?: InvokeDirection;
	distance?: number;
};

export interface InvokeElementResult {
	tier: ShowTier;
	invoked: boolean;
	action: InvokeAction;
	element: ElementDescriptor | null;
	count?: number;
	invalidSelector?: true;
	/** Present only when invoked: the element had a non-zero rect. */
	visible?: boolean;
	/** Present only when invoked. */
	domChanged?: boolean;
}

/** The iframe's `invoked` beacon. */
export type InvokedBeacon = {
	type: "invoked";
	requestId: string;
	count: number;
	element: ElementDescriptor | null;
	invalid?: boolean;
	busy?: boolean;
	notVisible?: boolean;
	visible?: boolean;
	domChanged?: boolean;
};

const DIRECTIONS: readonly string[] = ["left", "right", "up", "down"];

export const BUSY_ERROR =
	"busy: the user is picking an element in this design pane; try again once they finish";

export const NOT_VISIBLE_ERROR =
	"not_visible: the target has no layout, so a swipe has nothing to act on. Either the design pane is not on screen (pass reveal: true, which works when the user is in this room and has no harness picker open, or ask the user to show the design harness) or the element itself is not rendered";

export const REVEAL_SKIPPED_ERROR =
	"not_visible: reveal: true did not bring the design pane on screen, because the user is not in this room or has a harness picker open there, so a swipe has no layout to act on. Ask the user to show the design harness, then try again";

/** When reveal was requested and did not take effect, `not_visible` must not
 * tell the agent to pass the reveal it already passed. */
export function afterReveal(err: unknown, revealed: boolean | undefined): unknown {
	if (revealed === false && err instanceof Error && err.message === NOT_VISIBLE_ERROR) {
		return new Error(REVEAL_SKIPPED_ERROR);
	}
	return err;
}

export function parseInvokeElementArgs(
	raw: unknown,
): RequestResult<DesignTarget & InvokeElementRequest> {
	const t = parseStateArgs(raw);
	if (!t.ok) return t;
	const r = raw as Record<string, unknown>;
	const bad = (m: string): { ok: false; error: string } => ({
		ok: false,
		error: `bad_arguments: ${m}`,
	});
	let action: InvokeAction = "tap";
	if (!isOmitted(r.action)) {
		if (r.action !== "tap" && r.action !== "swipe") return bad('action must be "tap" or "swipe"');
		action = r.action;
	}
	const out: DesignTarget & InvokeElementRequest = { ...t.value, action };
	const reveal = parseReveal(r);
	if (!reveal.ok) return reveal;
	if (reveal.value) out.reveal = true;
	if (!isOmitted(r.selector)) {
		if (typeof r.selector !== "string" || !r.selector) {
			return bad("selector must be a non-empty string");
		}
		out.selector = r.selector;
	}
	if (!isOmitted(r.anchor)) {
		const anchor = parseAnchor(r.anchor);
		if (!anchor) return bad("anchor must be an object with a selector or tag string");
		out.anchor = anchor;
	}
	if ((out.selector === undefined) === (out.anchor === undefined)) {
		return bad("exactly one of selector or anchor is required");
	}
	if (action === "tap") {
		if (!isOmitted(r.direction) || !isOmitted(r.distance)) {
			return bad("direction and distance only apply to a swipe");
		}
		return { ok: true, value: out };
	}
	if (typeof r.direction !== "string" || !DIRECTIONS.includes(r.direction)) {
		return bad("a swipe needs direction: left, right, up or down");
	}
	out.direction = r.direction as InvokeDirection;
	let distance = DEFAULT_SWIPE_DISTANCE;
	if (!isOmitted(r.distance)) {
		const d = r.distance;
		if (
			typeof d !== "number" ||
			!Number.isInteger(d) ||
			d < MIN_SWIPE_DISTANCE ||
			d > MAX_SWIPE_DISTANCE
		) {
			return bad(`distance must be an integer from ${MIN_SWIPE_DISTANCE} to ${MAX_SWIPE_DISTANCE}`);
		}
		distance = d;
	}
	out.distance = distance;
	return { ok: true, value: out };
}

/** The answer for a CSS-selector request. A busy pane is an error, not an answer. */
export function invokeSelectorResult(b: InvokedBeacon, action: InvokeAction): InvokeElementResult {
	if (b.busy) throw new Error(BUSY_ERROR);
	if (b.notVisible) throw new Error(NOT_VISIBLE_ERROR);
	if (b.invalid) {
		return { tier: "not_found", invoked: false, action, element: null, invalidSelector: true };
	}
	if (b.count === 1 && b.element) {
		return {
			tier: "selector",
			invoked: true,
			action,
			element: b.element,
			count: 1,
			visible: b.visible === true,
			domChanged: b.domChanged === true,
		};
	}
	if (b.count > 1) {
		return { tier: "ambiguous", invoked: false, action, element: null, count: b.count };
	}
	return { tier: "not_found", invoked: false, action, element: null, count: 0 };
}

/** The same acceptance rule as show_element's highlight: only a placement it
 *  would highlight (anchored/reanchored with an element) may be acted on. */
export const placementAccepted = (p: ShowElementResult): boolean => p.highlighted && !!p.element;

/** The answer for an anchor request. `placed` is the placement's verdict;
 *  `beacon` is the invoke round trip, null when none was made or it failed. */
export function invokeAnchorResult(
	placed: ShowElementResult,
	beacon: InvokedBeacon | null,
	action: InvokeAction,
): InvokeElementResult {
	const base: InvokeElementResult = {
		tier: placed.tier,
		invoked: false,
		action,
		element: placed.element,
		...(placed.count !== undefined ? { count: placed.count } : {}),
	};
	if (!placementAccepted(placed) || !beacon) return base;
	if (beacon.busy) throw new Error(BUSY_ERROR);
	if (beacon.notVisible) throw new Error(NOT_VISIBLE_ERROR);
	if (beacon.count !== 1) return base;
	return {
		...base,
		invoked: true,
		visible: beacon.visible === true,
		domChanged: beacon.domChanged === true,
	};
}
