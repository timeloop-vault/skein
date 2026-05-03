// localStorage-backed app preferences.
//
// These are app-wide, not per-room: theme, density, font size, split
// sizes. We keep them out of sqlite because Rust never reads them and
// the WebView's localStorage already lives in the app data dir.

import { type Dispatch, type SetStateAction, useEffect, useState } from "react";

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
