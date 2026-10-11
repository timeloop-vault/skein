// Pure rules for "which running harnesses would start differently now"
// (#615), shared by the status-bar switch toast and the chip's marker.

import { effectiveProfile, profileIsStale } from "./spawnedEnvProfile.ts";
import type { Room } from "./types.ts";

/** Harnesses with a recorded spawn whose profile differs from the one that
 *  would apply under `active`. `spawnedOf` is `spawnedProfile`. */
export function countStaleHarnesses(
	rooms: readonly Room[],
	active: string | null,
	known: string[],
	spawnedOf: (harnessId: string) => string | null | undefined,
): number {
	let n = 0;
	for (const r of rooms) {
		for (const h of r.harnesses) {
			if (profileIsStale(spawnedOf(h.id), effectiveProfile(h.envProfile, active, known))) n++;
		}
	}
	return n;
}

export function switchToastMessage(name: string | null, stale: number): string {
	const head = `Env profile ${name === null ? "none" : `"${name}"`} applies to harnesses started from now on`;
	if (stale === 0) return `${head}.`;
	const noun = stale === 1 ? "harness keeps" : "harnesses keep";
	return `${head} — ${stale} running ${noun} their current env; restart ${stale === 1 ? "it" : "them"} to apply.`;
}

export function staleTitle(
	spawned: string | null | undefined,
	effective: string | null,
): string | null {
	if (!profileIsStale(spawned, effective)) return null;
	const from = spawned == null ? "no profile" : `profile "${spawned}"`;
	const to = effective === null ? "no profile" : `"${effective}"`;
	return `This harness started with ${from} — restart to apply ${to}`;
}
