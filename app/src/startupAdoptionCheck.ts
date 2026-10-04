// #539 — the probing half of startup adoption (`startupAdoption.ts` holds
// the rule and the registry). Not a hook: every side effect is injected so
// it is testable in node.

import { adoptionDecision, startupAdoption } from "./startupAdoption.ts";

export type AdoptionHarness = {
	kind: string;
	sessionId: string | undefined;
	cwd: string | undefined;
};

export type AdoptionDeps = {
	lookup: (roomId: string, harnessId: string) => AdoptionHarness | null;
	isShell: (harnessId: string) => boolean;
	/** Does this session's transcript exist? May reject. */
	exists: (sessionId: string, cwd: string) => Promise<boolean>;
	adopt: (roomId: string, harnessId: string, sessionId: string) => void;
	/** Per-stat bound, so a hung invoke cannot wedge `inFlight`. Default 5000. */
	timeoutMs?: number;
};

let inFlight = false;

function statWithTimeout(deps: AdoptionDeps, sessionId: string, cwd: string): Promise<boolean> {
	let timer: ReturnType<typeof setTimeout> | undefined;
	const timeout = new Promise<never>((_, reject) => {
		timer = setTimeout(() => reject(new Error("stat timed out")), deps.timeoutMs ?? 5000);
	});
	return Promise.race([deps.exists(sessionId, cwd), timeout]).finally(() => clearTimeout(timer));
}

export async function runAdoptionCheck(deps: AdoptionDeps): Promise<void> {
	if (inFlight) return;
	inFlight = true;
	try {
		for (const harnessId of startupAdoption.harnessIds()) {
			await checkOne(deps, harnessId);
		}
	} finally {
		inFlight = false;
	}
}

function stillBound(deps: AdoptionDeps, roomId: string, harnessId: string, bound: string) {
	const h = deps.lookup(roomId, harnessId);
	if (h?.kind !== "claude" || deps.isShell(harnessId) || !h.cwd) return null;
	return h.sessionId === bound ? h : null;
}

async function checkOne(deps: AdoptionDeps, harnessId: string): Promise<void> {
	const pending = startupAdoption.pending(harnessId);
	if (!pending) return;
	const { roomId, bound, candidates } = pending;
	const h = stillBound(deps, roomId, harnessId, bound);
	if (!h?.cwd) {
		startupAdoption.forget(harnessId);
		return;
	}
	const cwd = h.cwd;
	// Never adopt on doubt: a failed stat of the bound id reads as present.
	const boundExists = await statWithTimeout(deps, bound, cwd).catch(() => true);
	if (boundExists) {
		startupAdoption.forget(harnessId);
		return;
	}
	let found: string | null = null;
	for (const candidate of candidates) {
		const ok = await statWithTimeout(deps, candidate, cwd).catch(() => false);
		if (adoptionDecision(boundExists, ok) === "adopt") {
			found = candidate;
			break;
		}
	}
	if (found === null) return;
	// The stored id may have moved during the awaits.
	if (!stillBound(deps, roomId, harnessId, bound)) {
		startupAdoption.forget(harnessId);
		return;
	}
	deps.adopt(roomId, harnessId, found);
	startupAdoption.forget(harnessId);
}
