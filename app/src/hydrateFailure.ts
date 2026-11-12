// What a failed boot hydrate should do (#167, #730). `hydratedOnceRef`
// flips as soon as `db_load_rooms` succeeds, so a late rejection from a
// concurrent sibling hydrate (dev StrictMode double mount) is not allowed to
// park the autosave. #730 added a daemon call after that point
// (`rooms_resume_cmds`); its rejection has to reach the retry card all the
// same, or the app boots blank with no way out.

/** `rooms_resume_cmds` rejected. Nothing was applied (it runs before any
 *  state is set), and mounting un-rewritten cmds would start fresh
 *  conversations instead of resuming (#153), so the hydrate fails whole. */
export class ResumePrepareError extends Error {
	constructor(readonly cause: unknown) {
		super(
			`Couldn't prepare the saved harnesses to resume: ${
				cause instanceof Error ? cause.message : String(cause)
			}`,
		);
		this.name = "ResumePrepareError";
	}
}

export interface HydrateFailure {
	/** Message for the retry card, or null to stay silent. */
	message: string | null;
	/** The once-guard value to leave behind. */
	hydratedOnce: boolean;
}

/** Decide what a rejected hydrate does.
 *  - Rooms already installed (`loaded`): a sibling won; stay silent.
 *  - The resume call failed: always show the card, and release the guard so
 *    Retry (and a late sibling's own failure) can act on a clean slate.
 *  - Any other error after the rows loaded (`hydratedOnce`): silent, as in
 *    #167; before they loaded: the card. */
export function hydrateFailure(
	err: unknown,
	loaded: boolean,
	hydratedOnce: boolean,
): HydrateFailure {
	if (loaded) return { message: null, hydratedOnce };
	if (err instanceof ResumePrepareError) return { message: err.message, hydratedOnce: false };
	if (hydratedOnce) return { message: null, hydratedOnce };
	return { message: err instanceof Error ? err.message : String(err), hydratedOnce };
}
