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

// ── New Room memory (#226) ─────────────────────────────────────────
//
// The New Room dialog used to start blank every time, so the third room
// of the day against the same repo cost exactly as much as the first.
// This is the whole memory: the last repo root that was successfully
// submitted, plus per-repo defaults so a repo you always branch from
// `main` with Claude opens that way.
//
// Deliberately localStorage and not sqlite: Rust never reads it, and a
// wiped blob costs one Browse…, not data. The per-room durable fact
// ("which repo did this room come from") is a different question and
// belongs on `Room` — see #164.

export interface RepoDefaults {
	baseBranch: string;
	harness: HarnessKind;
	branchMode: "worktree" | "current";
	/** Epoch ms. Only used for MRU ordering, which has no UI yet. */
	lastUsed: number;
}

export interface RepoMemory {
	/** Last repo root successfully submitted, or null on a fresh install. */
	last: string | null;
	repos: Record<string, RepoDefaults>;
}

export const EMPTY_REPO_MEMORY: RepoMemory = { last: null, repos: {} };

/**
 * Fold a successful room creation into the memory.
 *
 * Called only after the room is actually created — never on open or on
 * typing. A create that failed must not teach the dialog anything, or
 * one bad path poisons every subsequent open.
 */
export const rememberRepo = (
	memory: RepoMemory,
	root: string,
	defaults: Omit<RepoDefaults, "lastUsed">,
): RepoMemory => ({
	last: root,
	repos: { ...memory.repos, [root]: { ...defaults, lastUsed: Date.now() } },
});

/** Known repo roots, most recently used first. No UI yet — see #226 direction 2. */
export const recentRepos = (memory: RepoMemory): string[] =>
	Object.entries(memory.repos)
		.sort(([, a], [, b]) => b.lastUsed - a.lastUsed)
		.map(([root]) => root);
