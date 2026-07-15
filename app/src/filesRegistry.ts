// filesRegistry — per-Files-harness dirty-buffer registry (#185).
//
// FilesBody instances register themselves; App consults the registry
// before closing a files harness, closing a room, or letting the
// window close — unsaved buffer text exists only in memory, so every
// destroy path must go through a prompt. Module singleton, same
// pattern as the harnessActivity store.

export interface FilesEntry {
	/** Display names of buffers with unsaved changes. */
	dirtyNames: () => string[];
}

const entries = new Map<string, FilesEntry>();

export const filesRegistry = {
	/** Register a Files harness's entry; returns the unregister fn. */
	register(harnessId: string, entry: FilesEntry): () => void {
		entries.set(harnessId, entry);
		return () => {
			entries.delete(harnessId);
		};
	},
	dirtyNames(harnessId: string): string[] {
		return entries.get(harnessId)?.dirtyNames() ?? [];
	},
	/** Dirty buffer names across the given harnesses (all when omitted). */
	anyDirty(harnessIds?: string[]): string[] {
		const ids = harnessIds ?? [...entries.keys()];
		return ids.flatMap((id) => entries.get(id)?.dirtyNames() ?? []);
	},
};
