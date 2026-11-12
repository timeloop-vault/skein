// What a rejected daemon argv call looks like to the user (#730). The argv
// builders used to be pure TypeScript and could not fail; now each awaits
// `harness_fresh_cmd` / `harness_resume_cmd` / `rooms_resume_cmds`. The
// restart path answers with a refusal (useHarnessActions.ts); the actions
// that have no result to return (add harness, new room, reopen, repoint)
// push the same kind of "info" toast through a late-bound sink, because the
// toast stack lives in a hook that is mounted after the ones that need it.

import type { NewToast } from "./toastStack.ts";
import type { Harness, Room } from "./types.ts";

/** A daemon argv call rejected. Thrown by the creation paths so an agent-API
 *  request is answered with it (useAgentRequests.ts' catch) while user
 *  entry points turn it into a toast. */
export class ArgvPrepareError extends Error {
	constructor(readonly detail: string) {
		super(`internal: couldn't prepare the harness command (${detail})`);
		this.name = "ArgvPrepareError";
	}
}

/** Run a daemon argv call, logging a rejection and rethrowing it as an
 *  `ArgvPrepareError`. */
export async function prepareOrThrow<T>(what: string, call: () => Promise<T>): Promise<T> {
	try {
		return await call();
	} catch (err) {
		if (err instanceof ArgvPrepareError) throw err;
		console.error(`[skein] ${what} failed:`, err);
		throw new ArgvPrepareError(err instanceof Error ? err.message : String(err));
	}
}

/** The toast for a refused action; the same wording style as restart's. */
export function argvFailureToast(
	action: string,
	room: Pick<Room, "id" | "name">,
	harness: Pick<Harness, "id" | "kind" | "name"> | undefined,
	now: number,
	rand: number,
): NewToast {
	return {
		id: `t_${now}_${rand.toString(36).slice(2, 8)}`,
		roomId: room.id,
		harnessId: harness?.id ?? "",
		kind: harness?.kind ?? "byoh",
		roomName: room.name,
		harnessName: harness?.name ?? "",
		state: "info",
		message: `Can't ${action}: couldn't prepare the harness command`,
	};
}

let sink: ((toast: NewToast) => void) | null = null;

/** App.tsx wires the toast stack's `pushToast` in once it exists. */
export function setArgvFailureSink(fn: ((toast: NewToast) => void) | null): void {
	sink = fn;
}

/** Tell the user an action was refused. No room to attach it to (a brand
 *  new room that never existed, an empty app) = log only. */
export function reportArgvFailure(
	action: string,
	err: unknown,
	room: Pick<Room, "id" | "name"> | undefined,
	harness?: Pick<Harness, "id" | "kind" | "name">,
): void {
	console.error(`[skein] can't ${action}:`, err);
	if (!room || !sink) return;
	sink(argvFailureToast(action, room, harness, Date.now(), Math.random()));
}
