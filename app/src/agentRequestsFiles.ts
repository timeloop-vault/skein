// The `files.dirty` agent request (#734): the daemon asks which of the given
// harnesses have unsaved Files buffers, since that text lives only in this
// webview's memory (`filesRegistry`). Pure and DOM-free; the registry lookup
// is injected.

import { asRecord, type RequestResult } from "./agentRequestsShared.ts";

export const FILES_DIRTY_KIND = "files.dirty";

export const isFilesKind = (kind: string): boolean => kind === FILES_DIRTY_KIND;

export interface DirtyHarness {
	harnessId: string;
	names: string[];
}

export interface FilesDirtyAnswer {
	dirty: DirtyHarness[];
}

export function parseFilesDirtyArgs(raw: unknown): RequestResult<{ harnessIds: string[] }> {
	const args = asRecord(raw);
	if (!args) return { ok: false, error: "bad_arguments: args must be an object" };
	const ids = args.harnessIds;
	if (!Array.isArray(ids) || !ids.every((x): x is string => typeof x === "string")) {
		return { ok: false, error: "bad_arguments: harnessIds must be an array of strings" };
	}
	return { ok: true, value: { harnessIds: ids } };
}

/** Only harnesses with at least one dirty buffer, in the order asked. */
export function buildFilesDirtyAnswer(
	raw: unknown,
	dirtyNames: (harnessId: string) => string[],
): RequestResult<FilesDirtyAnswer> {
	const p = parseFilesDirtyArgs(raw);
	if (!p.ok) return p;
	const dirty: DirtyHarness[] = [];
	for (const harnessId of p.value.harnessIds) {
		const names = dirtyNames(harnessId);
		if (names.length > 0) dirty.push({ harnessId, names });
	}
	return { ok: true, value: { dirty } };
}
