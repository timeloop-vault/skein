// Rows for the Control Center: live snapshots folded through the pure
// model. Shared by the in-app view and the pop-out broadcast (#493).

import { useMemo } from "react";
import type { StripSegment } from "../roomGroups.ts";
import type { Room } from "../types.ts";
import { buildControlCenter, type CCSection } from "./model.ts";
import { useControlCenterData } from "./useControlCenterData.ts";

export function useControlCenterSections(
	rooms: Room[],
	segments: StripSegment[],
	visible: boolean,
): { sections: CCSection[]; now: number } {
	const snaps = useControlCenterData(rooms, visible);
	const sections = useMemo(
		() => (visible ? buildControlCenter(segments, snaps) : []),
		[visible, segments, snaps],
	);
	// One instant for every row; `snaps` changes identity on each clock tick.
	// biome-ignore lint/correctness/useExhaustiveDependencies: recomputed when snaps change
	const now = useMemo(() => Date.now(), [snaps]);
	return { sections, now };
}
