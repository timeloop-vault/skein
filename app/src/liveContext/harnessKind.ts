import type { HarnessAction } from "./store.ts";

/// #538: the first non-null stored harness kind among `rows` (they share
/// one harness), or null when only legacy rows are present.
export function firstStoredKind(rows: readonly HarnessAction[]): string | null {
	for (const r of rows) if (r.harnessKind) return r.harnessKind;
	return null;
}
