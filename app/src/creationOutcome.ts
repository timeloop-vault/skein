// What a user-driven creation tells its caller (#730). The New Room dialog
// sets a busy flag before it commits and has to know whether the room was
// made: on a refused daemon argv call nothing exists, so the dialog must
// come back usable with the user's input intact rather than say "Creating…"
// forever.

import { ArgvPrepareError } from "./argvFailure.ts";

export type CreationOutcome = { ok: true } | { ok: false; error: string };

/** Run a creation for a user entry point. A rejected daemon argv call is
 *  reported (toast, via `report`) and answered with `{ ok: false }`; anything
 *  else is a bug and still throws. Agent callers do not come through here:
 *  they call the throwing creation directly and answer their request with it. */
export async function runUserCreation(
	create: () => Promise<unknown>,
	report: (err: ArgvPrepareError) => void,
): Promise<CreationOutcome> {
	try {
		await create();
		return { ok: true };
	} catch (err) {
		if (!(err instanceof ArgvPrepareError)) throw err;
		report(err);
		return { ok: false, error: err.message };
	}
}
