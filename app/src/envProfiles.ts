// Pure helpers for env profiles (#615). Kept free of React so the
// Settings editor and a later status-bar switch share one set of rules.

import type { EnvProfile, SpawnSettings } from "./types.ts";

type ProfileSettings = Pick<SpawnSettings, "envProfiles" | "activeEnvProfile">;

export const profileNames = (settings: ProfileSettings): string[] =>
	(settings.envProfiles ?? []).map((p) => p.name);

/** Activate a profile by name; an unknown name clears the selection. */
export const withActiveProfile = <T extends ProfileSettings>(
	settings: T,
	name: string | null,
): T => ({
	...settings,
	activeEnvProfile: name !== null && profileNames(settings).includes(name) ? name : null,
});

/** Rename profile `index`, carrying the active selection along if it was
 *  the one renamed. */
export const renameProfile = <T extends ProfileSettings>(
	settings: T,
	index: number,
	name: string,
): T => {
	const profiles = settings.envProfiles ?? [];
	const old = profiles[index]?.name;
	return {
		...settings,
		envProfiles: profiles.map((p, i) => (i === index ? { ...p, name } : p)),
		activeEnvProfile:
			old !== undefined && settings.activeEnvProfile === old
				? name
				: (settings.activeEnvProfile ?? null),
	};
};

/** Delete profile `index`, clearing the active selection if it was it. */
export const deleteProfile = <T extends ProfileSettings>(settings: T, index: number): T => {
	const profiles = settings.envProfiles ?? [];
	const old = profiles[index]?.name;
	return {
		...settings,
		envProfiles: profiles.filter((_, i) => i !== index),
		activeEnvProfile:
			old !== undefined && settings.activeEnvProfile === old
				? null
				: (settings.activeEnvProfile ?? null),
	};
};

/** Append an empty profile with a fresh, unused name. */
export const addProfile = <T extends ProfileSettings>(settings: T): T => {
	const taken = new Set(profileNames(settings));
	let n = taken.size + 1;
	while (taken.has(`Profile ${n}`)) n++;
	const profile: EnvProfile = { name: `Profile ${n}`, vars: [] };
	return { ...settings, envProfiles: [...(settings.envProfiles ?? []), profile] };
};

/** Why the profile list can't be saved, or null. Names are compared
 *  after trimming; the first problem found wins. */
export const validateProfiles = (profiles: EnvProfile[]): string | null => {
	const seen = new Set<string>();
	for (const p of profiles) {
		const name = p.name.trim();
		if (!name) return "A profile needs a name.";
		if (seen.has(name)) return `Two profiles are named "${name}".`;
		seen.add(name);
	}
	return null;
};

/** Trim names and drop variable rows with no key, keeping the active
 *  selection pointing at the trimmed name. */
export const normalizeProfiles = <T extends ProfileSettings>(settings: T): T => {
	const profiles = settings.envProfiles ?? [];
	const active = settings.activeEnvProfile;
	const activeIndex = active == null ? -1 : profiles.findIndex((p) => p.name === active);
	const next = profiles.map((p) => ({
		name: p.name.trim(),
		vars: p.vars.filter((v) => v.key.trim()).map((v) => ({ ...v, key: v.key.trim() })),
	}));
	return {
		...settings,
		envProfiles: next,
		activeEnvProfile: activeIndex >= 0 ? (next[activeIndex]?.name ?? null) : null,
	};
};

/** Which active profile a Settings save should write. The draft's value was
 *  copied from `base` when the form last synced; if the user did not touch it,
 *  a chip/palette switch made since then (`latest`) must win, or saving an
 *  unrelated setting would silently revert it. */
export const activeOnSave = (
	draft: string | null | undefined,
	base: string | null | undefined,
	latest: string | null,
): string | null => ((draft ?? null) === (base ?? null) ? latest : (draft ?? null));
