// #627: the Hermes profiles a harness can run in, and whether Skein's MCP
// entry is in each one's config. DTOs mirror the `hermes_*` Tauri commands;
// the ordering, labels and the consent decision are pure so they can be
// tested without the shell.

import { invoke } from "@tauri-apps/api/core";
import { SKEIN_HERMES_PROFILE } from "./harnessCmd.ts";

export type SkeinEntryState = "present" | "absent" | "refused" | "noConfig";

export interface HermesProfileInfo {
	name: string;
	/** False for a profile Skein will create at spawn (its own, before first use). */
	exists: boolean;
	skeinEntry: SkeinEntryState;
	/** Why the entry cannot be added automatically, when `refused`. */
	reason: string | null;
	/** Skein's status hooks can be added and approved here and are not yet (#625). */
	hooksPending?: boolean;
}

export interface HermesProfileListing {
	root: string | null;
	profiles: HermesProfileInfo[];
	error: string | null;
}

export interface EntryPreview {
	configPath: string;
	lines: string[];
	/** The preview includes Skein's MCP entry. */
	offersMcp: boolean;
	/** The preview includes Skein's status hooks and their approval. */
	offersHooks: boolean;
}

export type InjectResult = "added" | "alreadyPresent";

export const DEFAULT_HERMES_PROFILE = "default";

/** skein first, default second, then the rest alphabetically. Always offers
 *  both `skein` and `default`, so a failed or empty listing never blocks
 *  creation. A synthesized row claims nothing about the config. */
export const orderProfiles = (profiles: readonly HermesProfileInfo[]): HermesProfileInfo[] => {
	const byName = new Map(profiles.map((p) => [p.name, p]));
	const synth = (name: string): HermesProfileInfo => ({
		name,
		exists: false,
		skeinEntry: name === SKEIN_HERMES_PROFILE ? "present" : "noConfig",
		reason: null,
	});
	const rest = profiles
		.filter((p) => p.name !== SKEIN_HERMES_PROFILE && p.name !== DEFAULT_HERMES_PROFILE)
		.sort((a, b) => a.name.localeCompare(b.name));
	return [
		byName.get(SKEIN_HERMES_PROFILE) ?? synth(SKEIN_HERMES_PROFILE),
		byName.get(DEFAULT_HERMES_PROFILE) ?? synth(DEFAULT_HERMES_PROFILE),
		...rest,
	];
};

/** The list the step shows when the listing call itself failed. */
export const fallbackListing = (error: string): HermesProfileListing => ({
	root: null,
	profiles: orderProfiles([]),
	error,
});

export const profileLabel = (name: string): string => {
	if (name === SKEIN_HERMES_PROFILE) return "skein";
	if (name === DEFAULT_HERMES_PROFILE) return "default";
	return name;
};

export const profileBlurb = (p: HermesProfileInfo): string => {
	if (p.name === SKEIN_HERMES_PROFILE) {
		return "Skein's own profile, with the review tools. Created on first use.";
	}
	if (p.name === DEFAULT_HERMES_PROFILE) return "Hermes' root profile — your own config.";
	return "One of your Hermes profiles.";
};

/** Short status tag for a row; null when there is nothing to say. */
export const profileTag = (p: HermesProfileInfo): string | null => {
	if (p.name === SKEIN_HERMES_PROFILE) return null;
	switch (p.skeinEntry) {
		case "present":
			return p.hooksPending ? "review tools on · hooks not approved" : "review tools on";
		case "absent":
			return "no review tools yet";
		case "refused":
			return "review tools: manual";
		case "noConfig":
			return null;
	}
};

export type Consent = "none" | "ask" | "manual";

/** Whether choosing `profile` needs the consent dialog first. Skein's own
 *  profile never does; an `absent` entry, or a present one whose status
 *  hooks are still unapproved (#625), is offered for adding; a `refused`
 *  one is shown for the user to add by hand. `noConfig` runs as-is — there
 *  is no config to edit. Consent persists in the file itself. */
export const consentNeeded = (
	profile: string,
	entry: SkeinEntryState | undefined,
	hooksPending = false,
): Consent => {
	if (profile === SKEIN_HERMES_PROFILE) return "none";
	if (entry === "absent") return "ask";
	if (entry === "present" && hooksPending) return "ask";
	if (entry === "refused") return "manual";
	return "none";
};

export const entryOf = (
	listing: HermesProfileListing | null,
	profile: string,
): HermesProfileInfo | undefined => listing?.profiles.find((p) => p.name === profile);

const errText = (e: unknown): string => (e instanceof Error ? e.message : String(e));

export const listHermesProfiles = async (): Promise<HermesProfileListing> => {
	try {
		const l = await invoke<HermesProfileListing>("hermes_list_profiles");
		return { ...l, profiles: orderProfiles(l.profiles) };
	} catch (e) {
		return fallbackListing(errText(e));
	}
};

export const previewProfileEntry = (profile: string): Promise<EntryPreview> =>
	invoke<EntryPreview>("hermes_profile_entry_preview", { profile });

export const injectProfileEntry = (profile: string): Promise<InjectResult> =>
	invoke<InjectResult>("hermes_profile_inject", { profile });

export { errText as hermesErrorText };
