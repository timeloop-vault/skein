// What a failed boot hydrate should do (#167). `hydratedOnceRef` flips as
// soon as `db_load_rooms` succeeds, so a late rejection from a concurrent
// sibling hydrate (dev StrictMode double mount) is not allowed to park the
// autosave. The daemon prepares the resume cmds inside that load (#731), so
// there is no separate resume call left to fail.

export interface HydrateFailure {
	/** Message for the retry card, or null to stay silent. */
	message: string | null;
	/** The once-guard value to leave behind. */
	hydratedOnce: boolean;
}

/** Decide what a rejected hydrate does.
 *  - Rooms already installed (`loaded`): a sibling won; stay silent.
 *  - Any other error after the rows loaded (`hydratedOnce`): silent, as in
 *    #167; before they loaded: the card. */
export function hydrateFailure(
	err: unknown,
	loaded: boolean,
	hydratedOnce: boolean,
): HydrateFailure {
	if (loaded) return { message: null, hydratedOnce };
	if (hydratedOnce) return { message: null, hydratedOnce };
	return { message: err instanceof Error ? err.message : String(err), hydratedOnce };
}
