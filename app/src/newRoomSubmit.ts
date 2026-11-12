import type { CreationOutcome } from "./creationOutcome.ts";
import type { CreateRoomArgs } from "./NewRoomDialogTypes.ts";
import type { CreateRoomOutcome, CreateRoomRemember } from "./worktreeRoom.ts";

// The New Room dialog's submit flow (#730), split out of `useNewRoomForm.tsx`
// so it can be tested without React: build the args, commit them, and only
// then teach the folder what was created.

export interface NewRoomSubmitDeps {
	/** Validates the form and produces the args (`createRoomArgs`, bound). */
	prepare: () => Promise<CreateRoomOutcome>;
	/** Resolves once the room exists, or says why it does not. */
	commit: (args: CreateRoomArgs) => Promise<CreationOutcome>;
	/** Called only on a path that genuinely created the room — a create
	 *  that throws (or fails validation) must not teach the dialog
	 *  anything. */
	remember: (base: string, template?: string) => void;
	setError: (error: string | null) => void;
	setBusy: (busy: boolean) => void;
}

/** Runs the whole create. Failures are reported through `setError` and
 *  `setBusy(false)`; success leaves busy set, since the dialog closes. */
export const submitNewRoom = async ({
	prepare,
	commit,
	remember,
	setError,
	setBusy,
}: NewRoomSubmitDeps): Promise<void> => {
	setBusy(true);
	setError(null);
	try {
		const outcome = await prepare();
		if (!outcome.ok) {
			setError(outcome.error);
			setBusy(false);
			return;
		}
		const created = await commit(outcome.args);
		if (!created.ok) {
			// #730: nothing was created; the dialog stays open, input intact.
			setError(created.error);
			setBusy(false);
			return;
		}
		// #227: only a successful create teaches the folder its branch
		// template — a failed one must not poison the next open with a
		// prefix that never actually landed.
		const kept: CreateRoomRemember = outcome.remember;
		remember(kept.baseBranch, kept.branchTemplate);
	} catch (err: unknown) {
		const msg = err instanceof Error ? err.message : String(err);
		setError(msg);
		setBusy(false);
	}
};
