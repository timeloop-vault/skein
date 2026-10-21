import { useState } from "react";
import { consentNeeded, entryOf } from "./hermesProfiles.ts";
import { type FolderDefaults, startingHermesProfile } from "./prefs.ts";
import type { HarnessKind } from "./types.ts";
import { useHermesProfiles } from "./useHermesProfiles.ts";

/** #627: the New Room form's Hermes profile choice and its consent gate.
 *  The profile is remembered per folder exactly like the agent
 *  (FolderDefaults). */
export const useNewRoomHermesProfile = (
	harness: HarnessKind,
	initialDefaults: FolderDefaults | undefined,
) => {
	const [hermesProfile, setHermesProfile] = useState(() => startingHermesProfile(initialDefaults));
	// Set while the consent dialog is up for the chosen profile.
	const [consent, setConsent] = useState<{ mode: "ask" | "manual"; reason: string | null } | null>(
		null,
	);
	const hermesListing = useHermesProfiles(harness === "hermes");

	/** True (and the dialog raised) when the chosen profile of the user's own
	 *  needs consent before the room is created; the dialog calls `create`
	 *  when it resolves. */
	const gateOnConsent = (): boolean => {
		if (harness !== "hermes") return false;
		const entry = entryOf(hermesListing, hermesProfile);
		const need = consentNeeded(hermesProfile, entry?.skeinEntry, entry?.hooksPending);
		if (need === "none") return false;
		setConsent({ mode: need, reason: entry?.reason ?? null });
		return true;
	};

	return { hermesProfile, setHermesProfile, hermesListing, consent, setConsent, gateOnConsent };
};
