// localStorage-backed app preferences.
//
// These are app-wide, not per-room: theme, density, font size, split
// sizes. We keep them out of sqlite because Rust never reads them and
// the WebView's localStorage already lives in the app data dir.

import { type Dispatch, type SetStateAction, useEffect, useState } from "react";

import type { HarnessKind } from "./types";

const KEY_PREFIX = "skein:";

export const usePersistedState = <T>(key: string, initial: T): [T, Dispatch<SetStateAction<T>>] => {
	const fullKey = KEY_PREFIX + key;
	const [value, setValue] = useState<T>(() => {
		try {
			const raw = localStorage.getItem(fullKey);
			if (raw === null) return initial;
			return JSON.parse(raw) as T;
		} catch {
			// Corrupted blob (most likely a shape change between versions).
			// Fall back to the default rather than crashing the app.
			return initial;
		}
	});
	useEffect(() => {
		localStorage.setItem(fullKey, JSON.stringify(value));
	}, [fullKey, value]);
	return [value, setValue];
};

// ── New Room memory (#226, #231) ───────────────────────────────────
//
// The New Room dialog used to start blank every time, so the third room
// of the day in the same place cost exactly as much as the first. This
// is the whole memory: the last folder successfully submitted, plus
// per-folder defaults, so somewhere you always branch from `main` with
// Claude opens that way.
//
// Keyed on **folders, not repos** (#231). A repo contributes its root
// (worktrees resolve to the main checkout first, so every room in a
// repo shares one entry); a plain folder contributes itself. Only the
// git fields differ — `baseBranch` is `""` for a non-repo and
// `branchMode` is meaningless there, while `harness` is worth carrying
// forward for any folder at all. Treating repos as the only thing worth
// remembering made a non-git room prefill either nothing or, once the
// memory had seen one repo, an unrelated one.
//
// Deliberately localStorage and not sqlite: Rust never reads it, and a
// wiped blob costs one Browse…, not data. The per-room durable fact
// ("which repo did this room come from") is a different question and
// belongs on `Room` — see #164.

export interface FolderDefaults {
	/** Empty for a folder that is not a git repo. */
	baseBranch: string;
	harness: HarnessKind;
	/** The agent the starting harness was created with (#247), or
	 *  absent for "the tool's own default" — which is also what every
	 *  entry written before this field means, so absent has to keep
	 *  meaning that rather than becoming a gap to fill in.
	 *
	 *  Remembered for the same reason `harness` is: the starting harness
	 *  of the next room in this folder is nearly always the last one,
	 *  and an agent is now half of what that harness *is*. Not keyed by
	 *  kind — one folder, one starting harness, one agent; the per-kind
	 *  default is #248's, and it belongs in Settings, not here. */
	agent?: string;
	/** Meaningless for a non-repo folder; kept so repos round-trip cleanly. */
	branchMode: "worktree" | "current";
	/** Epoch ms. Only used for MRU ordering, which has no UI yet. */
	lastUsed: number;
}

export interface NewRoomMemory {
	/** Last folder successfully submitted, or null on a fresh install. */
	last: string | null;
	folders: Record<string, FolderDefaults>;
}

export const EMPTY_NEW_ROOM_MEMORY: NewRoomMemory = { last: null, folders: {} };

/**
 * The stored `folders` map, or an empty one.
 *
 * `usePersistedState` recovers from an unparseable blob but not from a
 * shape that parsed into the wrong thing, and this key has already held
 * a `{ last, repos }` shape during #226's review. Reading through this
 * means an older blob degrades to "no memory yet" instead of throwing
 * on `Object.entries(undefined)`.
 */
const foldersOf = (memory: NewRoomMemory): Record<string, FolderDefaults> => memory.folders ?? {};

/** Per-folder defaults, or undefined for a folder we have not seen. */
export const defaultsFor = (memory: NewRoomMemory, folder: string): FolderDefaults | undefined =>
	folder ? foldersOf(memory)[folder] : undefined;

/**
 * Fold a successful room creation into the memory.
 *
 * Called only after the room is actually created — never on open or on
 * typing. A create that failed must not teach the dialog anything, or
 * one bad path poisons every subsequent open.
 */
export const rememberFolder = (
	memory: NewRoomMemory,
	folder: string,
	defaults: Omit<FolderDefaults, "lastUsed">,
): NewRoomMemory => ({
	last: folder,
	folders: {
		...foldersOf(memory),
		[folder]: { ...defaults, lastUsed: Date.now() },
	},
});

export interface RecentFolder {
	folder: string;
	defaults: FolderDefaults;
}

/**
 * Known folders, most recently used first, capped at `limit` (#233).
 *
 * The cap is what keeps the list honest without a prune step: folders
 * are never validated here — checking each one would be a round-trip
 * per row for a list that is usually only glanced at — so a folder that
 * has since been deleted stays in the memory until enough newer ones
 * push it off the end. Picking a dead one is caught by the dialog's
 * `missing` status, which is the same check every other path goes
 * through.
 */
export const recentFolders = (memory: NewRoomMemory, limit = 8): RecentFolder[] =>
	Object.entries(foldersOf(memory))
		.sort(([, a], [, b]) => b.lastUsed - a.lastUsed)
		.slice(0, limit)
		.map(([folder, defaults]) => ({ folder, defaults }));
