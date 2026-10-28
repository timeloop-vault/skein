// Harness context sizes (#368). `useHarnessContextSizes(rooms)` is mounted
// once (App) and keeps a module-level store, keyed by harness id, current for
// every claude/opencode harness in the open rooms. Popovers read it through
// `getHarnessContextSize` / `subscribeContextSizes` (imperative) or
// `useHarnessContextSize` (React). A harness with no value has no entry,
// never rendered as 0.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useMemo, useSyncExternalStore } from "react";
import { applyResponse, CONTEXT_SIZE_KINDS, type ContextSizeRow } from "./contextSize.ts";
import type { Room } from "./types.ts";

let sizes: ReadonlyMap<string, number> = new Map();
// The harness ids tracked by the latest render; a response is checked against
// THIS, not against the set its request was made with.
let tracked: ReadonlySet<string> = new Set();
// Newest request sequence issued per harness id (see applyResponse).
const latestSeq = new Map<string, number>();
let seqCounter = 0;
const listeners = new Set<() => void>();

function commit(next: ReadonlyMap<string, number>): void {
	if (next === sizes) return;
	sizes = next;
	for (const l of listeners) l();
}

/** Subscribe to any change in the stored sizes; returns the unsubscriber. */
export function subscribeContextSizes(cb: () => void): () => void {
	listeners.add(cb);
	return () => {
		listeners.delete(cb);
	};
}

/** This harness's latest-turn context size, or undefined when unknown. */
export function getHarnessContextSize(harnessId: string): number | undefined {
	return sizes.get(harnessId);
}

export function useHarnessContextSize(harnessId: string): number | undefined {
	return useSyncExternalStore(subscribeContextSizes, () => sizes.get(harnessId));
}

// Not handled, on purpose: after a harness restarts into a new session, or
// after /clear or a compaction, the previous value stays until that harness's
// next turn ends. The next turn_cost row replaces it, and turn_cost rows carry
// no session we can compare cheaply (opencode's timestamp is 0).
//
// An invoke failure likewise keeps a previously shown value rather than
// clearing it: a transient db error should not blank the UI, and a harness
// never shows 0 for a missing value.
async function refresh(harnessIds: string[]): Promise<void> {
	const issued = new Map<string, number>();
	for (const id of harnessIds) {
		const seq = ++seqCounter;
		latestSeq.set(id, seq);
		issued.set(id, seq);
	}
	try {
		const rows = await invoke<ContextSizeRow[]>("db_harness_context_sizes", { harnessIds });
		commit(applyResponse(sizes, latestSeq, issued, tracked, rows));
	} catch (err) {
		console.warn("[skein] db_harness_context_sizes failed:", err);
	}
}

interface ActionEvent {
	harnessId: string;
	kind: string;
}

/** Mount once with the open rooms. */
export function useHarnessContextSizes(rooms: readonly Room[]): void {
	// JSON of the tracked harness ids: ids may contain any character, so no
	// delimiter scheme; a stable primitive to key effects on.
	const key = JSON.stringify(
		rooms.flatMap((r) =>
			r.harnesses.filter((h) => CONTEXT_SIZE_KINDS.has(h.kind)).map((h) => h.id),
		),
	);
	const ids = useMemo(() => JSON.parse(key) as string[], [key]);

	useEffect(() => {
		tracked = new Set(ids);
		const kept = new Map([...sizes].filter(([id]) => tracked.has(id)));
		if (kept.size !== sizes.size) commit(kept);
		if (ids.length > 0) void refresh(ids);
	}, [ids]);

	useEffect(() => {
		const unlisten = listen<ActionEvent>("harness-action", (e) => {
			const a = e.payload;
			if (a.kind === "turn_cost" && tracked.has(a.harnessId)) void refresh([a.harnessId]);
		});
		return () => {
			void unlisten.then((f) => f());
		};
	}, []);
}
