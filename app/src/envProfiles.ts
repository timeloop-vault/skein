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
	renames: Renames = {},
): string | null =>
	(draft ?? null) === resolveRenamed(renames, base ?? null)
		? resolveRenamed(renames, latest)
		: (draft ?? null);

/** Original name -> current name for profiles renamed in an unsaved draft
 *  (or, in the store, by saves this session). Composed across repeated
 *  renames, so A->B->C is recorded as A->C. */
export type Renames = Record<string, string>;

const hasKey = (renames: Renames, key: string): boolean => Object.keys(renames).includes(key);

/** Record that the profile currently named `from` is now named `to`. */
export const recordRename = (renames: Renames, from: string, to: string): Renames => {
	if (from === to) return renames;
	const next = { ...renames };
	const origin = Object.keys(next).find((k) => next[k] === from) ?? from;
	if (origin === to) delete next[origin];
	else next[origin] = to;
	return next;
};

/** Forget the rename of the profile now called `name`, because it was deleted. */
export const dropRename = (renames: Renames, name: string): Renames => {
	const next: Renames = {};
	for (const [k, v] of Object.entries(renames)) if (v !== name) next[k] = v;
	return next;
};

/** Map a possibly-old name to its renamed one. With `existing`, a name that
 *  still exists is left alone. */
export const resolveRenamed = (
	renames: Renames,
	name: string | null,
	existing?: readonly string[],
): string | null => {
	if (name === null || existing?.includes(name)) return name;
	return hasKey(renames, name) ? (renames[name] ?? name) : name;
};

/** The renames a save will actually apply: values trimmed the way
 *  `normalizeProfiles` trims them, no-ops and vanished targets dropped. */
export const finalRenames = (renames: Renames, savedNames: readonly string[]): Renames => {
	const out: Renames = {};
	for (const [k, v] of Object.entries(renames)) {
		const to = v.trim();
		if (to !== k && savedNames.includes(to)) out[k] = to;
	}
	return out;
};

/** Point every per-harness override at a renamed profile's new name. */
export const renameHarnessProfiles = <
	R extends { harnesses: { envProfile?: string | undefined }[] },
>(
	rooms: R[],
	renames: Renames,
): R[] => {
	if (Object.keys(renames).length === 0) return rooms;
	return rooms.map((r) =>
		r.harnesses.some((h) => h.envProfile !== undefined && hasKey(renames, h.envProfile))
			? {
					...r,
					harnesses: r.harnesses.map((h) =>
						h.envProfile !== undefined && hasKey(renames, h.envProfile)
							? { ...h, envProfile: renames[h.envProfile] }
							: h,
					),
				}
			: r,
	);
};
