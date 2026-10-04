// Which harness kind wrote something (#538). A departed harness has no
// entry in the room any more, so a lookup by id alone can't answer; the
// backend stamps each action row with the writer's kind. Resolution order:
// the stored kind (if the registry knows it), the current harness's kind,
// else unknown (null). Never a guess: "byoh" made departed harnesses read
// as a shell.

import { HARNESS_ORDER } from "./data.tsx";
import type { HarnessKind } from "./types.ts";

export function isHarnessKind(s: string | null | undefined): s is HarnessKind {
	return s != null && (HARNESS_ORDER as readonly string[]).includes(s);
}

export function resolveHarnessKind(
	storedKind: string | null | undefined,
	harnessId: string,
	current: ReadonlyMap<string, HarnessKind> | readonly { id: string; kind: HarnessKind }[],
): HarnessKind | null {
	if (isHarnessKind(storedKind)) return storedKind;
	if (Array.isArray(current)) {
		return (
			(current as readonly { id: string; kind: HarnessKind }[]).find((h) => h.id === harnessId)
				?.kind ?? null
		);
	}
	return (current as ReadonlyMap<string, HarnessKind>).get(harnessId) ?? null;
}

/** The prop type every chip-rendering component takes. */
export type HarnessKindOf = (harnessId: string, storedKind?: string | null) => HarnessKind | null;
