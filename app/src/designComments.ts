// designComments — the pure half of element comments in the design pane
// (#434): which anchors to ask the iframe to locate, how located results
// become placements and numbered pins, and which of them are worth
// writing back. No React, no iframe; everything an anchor carries is
// evidence to be shown as TEXT, never as markup.

import type { HostMessage } from "./designPreview.ts";
import {
	type AnchorState,
	type ElementAnchor,
	type ElementRect,
	type LocateResult,
	type Placement,
	matchElement,
} from "./elementAnchor.ts";
import type { ReviewThread } from "./review/api.ts";

/** An element thread: `element` is set on every one the backend returns
 *  for an entry. */
export type ElementThread = ReviewThread;

export interface SeenWrite {
	threadId: string;
	seen: { state: AnchorState; selector?: string; rect?: ElementRect; files: string[] };
}

type Anchors = Extract<HostMessage, { type: "locate" }>["anchors"];
type Pins = Extract<HostMessage, { type: "pins" }>["pins"];

/** Rect movement smaller than this is layout noise, not a change. */
const RECT_EPSILON = 2;

const isOpen = (t: ElementThread): boolean => t.resolvedMs == null;

/** Threads that take part in placement: element threads still open. */
export const placeable = (threads: readonly ElementThread[]): ElementThread[] =>
	threads.filter((t) => t.element !== undefined && isOpen(t));

/** Changes only when the set of threads to locate does, so a refetch that
 *  merely refreshes `lastSeen` does not trigger another locate. */
export const placementSignature = (threads: readonly ElementThread[]): string =>
	placeable(threads)
		.map((t) => `${t.id}\u0000${t.element?.anchor.selector ?? ""}`)
		.join("\u0001");

export const buildLocateAnchors = (threads: readonly ElementThread[]): Anchors =>
	placeable(threads).flatMap((t) => {
		const a = t.element?.anchor;
		if (!a) return [];
		const out: Anchors[number] = { id: t.id, selector: a.selector, tag: a.tag, text: a.text };
		if (a.odId) out.odId = a.odId;
		return [out];
	});

export const placeThreads = (
	threads: readonly ElementThread[],
	results: readonly { id: string; found: LocateResult }[],
): Map<string, Placement> => {
	const byId = new Map(results.map((r) => [r.id, r.found]));
	const out = new Map<string, Placement>();
	for (const t of placeable(threads)) {
		const found = byId.get(t.id);
		if (t.element && found) out.set(t.id, matchElement(t.element.anchor, found));
	}
	return out;
};

/** 1-based position in the side list, for every thread. */
export const pinNumbers = (threads: readonly ElementThread[]): Map<string, number> =>
	new Map(threads.map((t, i) => [t.id, i + 1]));

/** Where a lost thread's ghost pin goes: the last place it was seen. */
const evidenceRect = (t: ElementThread): ElementRect | undefined =>
	t.element?.lastSeen?.rect ?? t.element?.anchor.rect;

export const buildPins = (
	threads: readonly ElementThread[],
	placements: ReadonlyMap<string, Placement>,
): Pins => {
	const numbers = pinNumbers(threads);
	const pins: Pins = [];
	for (const t of placeable(threads)) {
		const p = placements.get(t.id);
		const n = numbers.get(t.id);
		if (!p || n === undefined) continue;
		const rect = p.at?.rect ?? evidenceRect(t);
		if (rect) pins.push({ n, state: p.state, rect });
	}
	return pins;
};

const rectMoved = (a: ElementRect | undefined, b: ElementRect | undefined): boolean => {
	if (!a || !b) return a !== b;
	return (
		Math.abs(a.x - b.x) > RECT_EPSILON ||
		Math.abs(a.y - b.y) > RECT_EPSILON ||
		Math.abs(a.w - b.w) > RECT_EPSILON ||
		Math.abs(a.h - b.h) > RECT_EPSILON
	);
};

/** The placements worth persisting: state, selector or rect differ from
 *  what is stored, or the stored answer is `unknown` (its content stamp no
 *  longer matches the disk). `written` holds threads already written for
 *  this load, so an answer the backend cannot confirm is not retried. */
export const seenWrites = (
	threads: readonly ElementThread[],
	placements: ReadonlyMap<string, Placement>,
	files: string[],
	written: ReadonlySet<string>,
): SeenWrite[] => {
	const out: SeenWrite[] = [];
	for (const t of placeable(threads)) {
		const p = placements.get(t.id);
		if (!p || written.has(t.id) || !t.element) continue;
		const last = t.element.lastSeen;
		const selector = p.at?.selector;
		const rect = p.at?.rect;
		const changed =
			t.element.state === "unknown" ||
			!last ||
			last.state !== p.state ||
			last.selector !== selector ||
			rectMoved(last.rect, rect);
		if (!changed) continue;
		const seen: SeenWrite["seen"] = { state: p.state, files };
		if (selector !== undefined) seen.selector = selector;
		if (rect) seen.rect = rect;
		out.push({ threadId: t.id, seen });
	}
	return out;
};

/** One line naming the element, plain text. */
export const elementSummary = (a: ElementAnchor): string => {
	const id = a.odId ? `[${a.odId}]` : a.selector;
	const text = a.text.replace(/\s+/g, " ").trim();
	const clipped = text.length > 80 ? `${text.slice(0, 80)}…` : text;
	return clipped ? `<${a.tag}> ${id} "${clipped}"` : `<${a.tag}> ${id}`;
};

export const sourceLabel = (a: Pick<ElementAnchor, "source">): string | undefined =>
	a.source ? `${a.source.file}:${a.source.line}` : undefined;

/** The state to show for a thread: this load's computed placement when
 *  there is one, else what the backend trusts. */
export const displayState = (
	t: ElementThread,
	placements: ReadonlyMap<string, Placement>,
): string => placements.get(t.id)?.state ?? t.element?.state ?? "unknown";
