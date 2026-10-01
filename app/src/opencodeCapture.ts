// Phase 2b: opencode has no Claude-style --session-id pre-allocation.
// Snapshot opencode's existing sessions for this cwd, spawn the
// harness, then poll the same query looking for an id that wasn't in
// the snapshot AND isn't already claimed by some other Skein harness.
// First match wins; that's this harness's session.
//
// Why polling at all (not a file/db watcher): the capture window is
// short relative to a session lifetime, and opencode writes the row
// once. Watcher's lifetime cost > polling's burst.
//
// Why a long timeout (5 minutes): opencode appears to write the
// session row only on the first user input, not at spawn — so a
// short window misses it whenever the user takes a beat to start
// typing. 5 min covers nearly every realistic case; on timeout we
// quietly leave sessionId undefined and resume falls back to
// phase-5a's --continue.
//
// `claimedIds` returns the set of session ids any *other* harness
// has already captured. If two opencode harnesses spawn in the same
// cwd within seconds, the snapshot diff alone can't tell them apart;
// excluding already-claimed ids breaks the tie deterministically.

import { invoke } from "@tauri-apps/api/core";

export const captureOpencodeSessionId = async (
	cwd: string,
	claimedIds: () => Set<string>,
	onCapture: (sessionId: string) => void,
): Promise<void> => {
	let snapshot: string[];
	try {
		snapshot = await invoke<string[]>("opencode_list_sessions", { cwd });
	} catch (err) {
		console.warn("[skein] opencode capture: snapshot failed", err);
		return;
	}
	const before = new Set(snapshot);
	const startedAt = Date.now();
	const deadline = startedAt + 5 * 60 * 1000;
	console.info(`[skein] opencode capture started for ${cwd} (snapshot ${before.size} sessions)`);
	while (Date.now() < deadline) {
		// Backoff: tight (250 ms) for the first 5 s in case opencode
		// is fast, then 1 s for the next 25 s, then 5 s thereafter.
		const elapsed = Date.now() - startedAt;
		const waitMs = elapsed < 5_000 ? 250 : elapsed < 30_000 ? 1_000 : 5_000;
		await new Promise((resolve) => setTimeout(resolve, waitMs));
		try {
			const current = await invoke<string[]>("opencode_list_sessions", { cwd });
			const taken = claimedIds();
			const fresh = current.find((id) => !before.has(id) && !taken.has(id));
			if (fresh) {
				console.info(`[skein] opencode capture: ${fresh} (${cwd})`);
				onCapture(fresh);
				return;
			}
		} catch {
			// Transient — try again on the next tick.
		}
	}
	console.warn(`[skein] opencode capture timed out for ${cwd}`);
};
