// Which mount key each harness was last ensured with in this webview (#731).
// The daemon owns the process; the view only decides whether this mount is
// the SAME view of it (attach, reuse the running process) or a deliberate new
// one (Restart harness, Enter-for-shell and repoint all change the key).
// Module-level on purpose: it must outlive the terminal component, whose
// React StrictMode double-run and tab re-mounts reuse the key.
//
// Reading the flag and recording the key are separate steps: a mount that is
// cancelled before its ensure call goes out (StrictMode's first run) must not
// consume the restart, or the second run would attach to the old process.
// Entries are never pruned: one short string per harness id seen in this
// page's lifetime, and unmount is not "gone for good" (archive keeps the id).

const lastKey = new Map<string, string>();

/** The terminal's mount key: harness id, spawnGen and the spawn argv (the
 *  tool's, or the shell's in shell mode, #752). Value-equal across
 *  content-identical renders. Pure. */
export function mountKeyFor(
	harnessId: string,
	spawnGen: number | undefined,
	argv: readonly string[],
): string {
	return `${harnessId}:${spawnGen ?? 0}:${argv.join("\x00")}`;
}

/** Whether the daemon must stop the running process first: only when a
 *  previous key was dispatched in this webview and differs. The first mount
 *  after a page load and a re-mount with the same key return false. Pure. */
export function restartFor(harnessId: string, key: string): boolean {
	const prev = lastKey.get(harnessId);
	return prev !== undefined && prev !== key;
}

/** Records `key` as the harness's current mount. Call right before the
 *  `harness_ensure_running` invoke goes out, and nowhere earlier. */
export function commitMount(harnessId: string, key: string): void {
	lastKey.set(harnessId, key);
}

/** Test helper: drop everything. */
export function resetMounts(): void {
	lastKey.clear();
}
