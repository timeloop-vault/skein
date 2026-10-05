// Docking a design harness into the right pane's Design tab (#551).
//
// Pure state helpers, no React. `docked` is keyed by harness id (absent =
// undocked, so an undock deletes the key); `picks` is the harness a room's
// Design tab last showed. Both are persisted maps, so every answer here is
// derived from them and from the room's live harness list — a closed
// harness or a stale pick can never leave the tab pointing at nothing.

import type { Harness } from "./types.ts";

export type DockedDesign = Record<string, true>;
export type DesignPicks = Record<string, string>;

/** The room's design harnesses that are docked, in room order. */
export const dockedDesignHarnesses = (harnesses: Harness[], docked: DockedDesign): Harness[] =>
	harnesses.filter((h) => h.kind === "design" && docked[h.id] === true);

/** The harness the Design tab shows: the pick while it is still docked,
 *  otherwise the first docked one. */
export const shownDesignHarness = (
	dockedList: Harness[],
	pick: string | undefined,
): Harness | undefined => dockedList.find((h) => h.id === pick) ?? dockedList[0];

/** The tab to render. "design" with nothing docked reads as "context" —
 *  derived, never written back, so docking again restores it. */
export const effectiveRightPaneTab = <T extends string>(tab: T | "design", hasDocked: boolean) =>
	tab === "design" && !hasDocked ? "context" : tab;

/** Immutable dock/undock; returns `prev` itself when nothing changes. */
export const withDocked = (
	prev: DockedDesign,
	harnessId: string,
	docked: boolean,
): DockedDesign => {
	if ((prev[harnessId] === true) === docked) return prev;
	if (docked) return { ...prev, [harnessId]: true };
	const { [harnessId]: _gone, ...rest } = prev;
	return rest;
};
