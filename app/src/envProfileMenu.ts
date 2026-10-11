// Pure model of the per-harness "Env profile" section in the Actions menu
// (#615): which entries to list and which is checked. No React, no stores.

import { effectiveProfile, profileIsStale } from "./spawnedEnvProfile.ts";

export interface EnvProfileMenuItem {
	id: string;
	label: string;
	/** What choosing the item stores on the harness; undefined removes the key. */
	value: string | undefined;
	checked: boolean;
	/** The harness override names a profile that no longer exists. */
	missing: boolean;
}

export function envProfileMenuItems(
	override: string | undefined,
	active: string | null,
	names: readonly string[],
): EnvProfileMenuItem[] {
	const missingOverride =
		override !== undefined && !names.some((n) => n.trim() === override.trim());
	const items: EnvProfileMenuItem[] = [
		{
			id: "follow",
			label: `Follow global (${active ?? "none"})`,
			value: undefined,
			checked: override === undefined,
			missing: false,
		},
	];
	if (missingOverride) {
		items.push({
			id: `missing:${override}`,
			label: `${override} (missing — following global)`,
			value: override,
			checked: true,
			missing: true,
		});
	}
	for (const name of names) {
		items.push({
			id: `profile:${name}`,
			label: name,
			value: name,
			checked: override?.trim() === name.trim(),
			missing: false,
		});
	}
	return items;
}

/** "Started with X — restart to apply Y", or null when nothing is stale. */
export function staleLine(
	spawned: string | null | undefined,
	effective: string | null,
): string | null {
	if (!profileIsStale(spawned, effective)) return null;
	const show = (n: string | null | undefined) => (n == null ? "no profile" : n);
	return `Started with ${show(spawned)} — restart to apply ${show(effective)}`;
}

/** Toast after a per-harness choice, or null when the running harness already
 *  matches (or has not spawned). */
export function overrideToastMessage(
	harnessName: string,
	spawned: string | null | undefined,
	override: string | undefined,
	active: string | null,
	names: string[],
): string | null {
	const effective = effectiveProfile(override, active, names);
	if (!profileIsStale(spawned, effective)) return null;
	const what = effective === null ? "no profile" : `env profile "${effective}"`;
	return `${harnessName} will use ${what} after a restart.`;
}
