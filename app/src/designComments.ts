// designComments — the pure half of element comments in the design pane
// (#434): which anchors to ask the iframe to locate, how located results
// become placements and numbered pins, and which of them are worth
// writing back. No React, no iframe; everything an anchor carries is
// evidence to be shown as TEXT, never as markup.

import type { HostMessage } from "./designPreview.ts";
import type { Change } from "./designProposal.ts";
import {
	type AnchorState,
	type ElementAnchor,
	type ElementDescriptor,
	type ElementRect,
	type LocateResult,
	matchElement,
	type Placement,
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

export interface ProposalItem {
	id: string;
	anchor: Anchors[number];
	changes: Change[];
}

/** The proposals the iframe should preview: open, unaddressed element
 *  threads that carry one. Addressed or resolved ones are left out, which
 *  is how their overlay goes away. */
export const buildProposalItems = (threads: readonly ElementThread[]): ProposalItem[] => {
	const withProposal = new Set(threads.filter((t) => t.proposal && !t.addressed).map((t) => t.id));
	return buildLocateAnchors(threads.filter((t) => withProposal.has(t.id))).flatMap((anchor) => {
		const changes = threads.find((t) => t.id === anchor.id)?.proposal?.changes;
		return changes ? [{ id: anchor.id, anchor, changes }] : [];
	});
};

/** The anchor a new element thread stores: what the page reported, plus
 *  the entry it was picked in. Shared by comments and proposals. */
export const anchorFor = (element: ElementDescriptor, entry: string): ElementAnchor => ({
	...element,
	entry,
});

/** Side-list label for a proposal thread; a lost one still says so. */
export const proposalLabel = (t: ElementThread, placementState: string): string => {
	const n = t.proposal?.changes.length ?? 0;
	if (t.addressed) return "proposal · addressed";
	if (placementState === "lost") return "proposal · lost";
	return `proposal · ${n} ${n === 1 ? "change" : "changes"}`;
};

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

/** What this pane last wrote for a thread, and in which page load. */
export interface WrittenSeen {
	state: AnchorState;
	selector?: string;
	rect?: ElementRect;
	load: number;
}

/** The placements worth persisting. The baseline is what this pane last
 *  wrote for the thread, else the stored `lastSeen`; a write is due when
 *  state, selector or rect (beyond a couple of px) differ from it. A stored
 *  `unknown` (its content stamp no longer matches the disk) also earns one
 *  write per page `load`, so an answer the backend cannot confirm is not
 *  retried on every re-locate. */
export const seenWrites = (
	threads: readonly ElementThread[],
	placements: ReadonlyMap<string, Placement>,
	files: string[],
	written: ReadonlyMap<string, WrittenSeen>,
	load: number,
): SeenWrite[] => {
	const out: SeenWrite[] = [];
	for (const t of placeable(threads)) {
		const p = placements.get(t.id);
		if (!p || !t.element) continue;
		const selector = p.at?.selector;
		const rect = p.at?.rect;
		const mine = written.get(t.id);
		const base = mine ?? t.element.lastSeen;
		const unknownDue = t.element.state === "unknown" && mine?.load !== load;
		const changed =
			unknownDue ||
			!base ||
			base.state !== p.state ||
			base.selector !== selector ||
			rectMoved(base.rect, rect);
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

/** Open element threads the page has not answered for yet (no live
 *  placement). Non-empty after a `located` means a thread or a reply was
 *  lost on the way, and the pane should ask again. */
export const unplaced = (
	threads: readonly ElementThread[],
	placements: ReadonlyMap<string, Placement>,
): ElementThread[] => placeable(threads).filter((t) => !placements.has(t.id));

/** The state to show for a thread. A live placement always wins; with
 *  none, the stored state is shown only while no page is being asked
 *  (`live` false) — once the page is up, an unanswered thread reads
 *  `locating` rather than repeating a stored claim the pins contradict.
 *  Resolved threads have no pin, so they keep their stored state. */
export const displayState = (
	t: ElementThread,
	placements: ReadonlyMap<string, Placement>,
	live = false,
): string => {
	const p = placements.get(t.id);
	if (p) return p.state;
	if (live && t.resolvedMs == null && t.element) return "locating";
	return t.element?.state ?? "unknown";
};
