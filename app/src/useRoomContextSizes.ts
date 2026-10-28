// Room context sizes (#368). `useRoomContextSizes(rooms)` is mounted once
// (App) and keeps a module-level store current; `useRoomContextSize(roomId)`
// subscribes ONE room's value, so a tab or header re-renders only when its
// own number moves (#594's per-room memoisation stays intact). A room with
// no value has no entry — never rendered as 0.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useMemo, useSyncExternalStore } from "react";
import {
	applyResponse,
	type ContextLeads,
	type ContextSizeEntry,
	type ContextSizeRow,
	contextLeadHarness,
} from "./contextSize.ts";
import type { Room } from "./types.ts";

let sizes: ReadonlyMap<string, ContextSizeEntry> = new Map();
// The leads of the latest render; a response is checked against THIS, not
// against the snapshot its request was made with.
let currentLeads: ContextLeads = new Map();
// Newest request sequence issued per harness id (see applyResponse).
const latestSeq = new Map<string, number>();
let seqCounter = 0;
const listeners = new Set<() => void>();

function commit(next: ReadonlyMap<string, ContextSizeEntry>): void {
	if (next === sizes) return;
	sizes = next;
	for (const l of listeners) l();
}

const subscribe = (l: () => void) => {
	listeners.add(l);
	return () => {
		listeners.delete(l);
	};
};

/** This room's context size and the harness it was measured on, or undefined
 *  when unknown. */
export function useRoomContextSize(roomId: string): ContextSizeEntry | undefined {
	return useSyncExternalStore(subscribe, () => sizes.get(roomId));
}

// Not handled, on purpose: after a harness restarts into a new session, or
// after /clear or a compaction, the previous value stays until that harness's
// next turn ends. The next turn_cost row replaces it, and turn_cost rows carry
// no session we can compare cheaply (opencode's timestamp is 0).
//
// An invoke failure likewise keeps a previously shown value rather than
// clearing it: a transient db error should not blank the UI, and a room never
// shows 0 for a missing value.
async function refresh(harnessIds: string[]): Promise<void> {
	const issued = new Map<string, number>();
	for (const id of harnessIds) {
		const seq = ++seqCounter;
		latestSeq.set(id, seq);
		issued.set(id, seq);
	}
	try {
		const rows = await invoke<ContextSizeRow[]>("db_harness_context_sizes", { harnessIds });
		commit(applyResponse(sizes, latestSeq, issued, currentLeads, rows));
	} catch (err) {
		console.warn("[skein] db_harness_context_sizes failed:", err);
	}
}

interface ActionEvent {
	harnessId: string;
	kind: string;
}

/** Mount once with the open rooms. */
export function useRoomContextSizes(rooms: readonly Room[]): void {
	// JSON of [harnessId, roomId, harnessName] triples — ids may contain any
	// character, so no delimiter scheme; a stable primitive to key effects on.
	const key = JSON.stringify(
		rooms.flatMap((r) => {
			const lead = contextLeadHarness(r);
			return lead ? [[lead.id, r.id, lead.name]] : [];
		}),
	);
	const leads = useMemo<ContextLeads>(() => {
		const triples = JSON.parse(key) as [string, string, string][];
		return new Map(triples.map(([h, roomId, harnessName]) => [h, { roomId, harnessName }]));
	}, [key]);

	useEffect(() => {
		currentLeads = leads;
		const present = new Set([...leads.values()].map((l) => l.roomId));
		const kept = new Map([...sizes].filter(([roomId]) => present.has(roomId)));
		if (kept.size !== sizes.size) commit(kept);
		const ids = [...leads.keys()];
		if (ids.length > 0) void refresh(ids);
	}, [leads]);

	useEffect(() => {
		const unlisten = listen<ActionEvent>("harness-action", (e) => {
			const a = e.payload;
			if (a.kind === "turn_cost" && currentLeads.has(a.harnessId)) void refresh([a.harnessId]);
		});
		return () => {
			void unlisten.then((f) => f());
		};
	}, []);
}
