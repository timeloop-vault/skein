import { useEffect, useState } from "react";
import { type HermesProfileListing, listHermesProfiles } from "./hermesProfiles.ts";

/** The Hermes profile listing while `enabled`; null until it arrives.
 *  Re-fetched each time it is enabled — profiles are files the user can
 *  add while Skein is open. */
export const useHermesProfiles = (enabled: boolean): HermesProfileListing | null => {
	const [listing, setListing] = useState<HermesProfileListing | null>(null);
	useEffect(() => {
		if (!enabled) {
			setListing(null);
			return undefined;
		}
		let cancelled = false;
		void listHermesProfiles().then((l) => {
			if (!cancelled) setListing(l);
		});
		return () => {
			cancelled = true;
		};
	}, [enabled]);
	return listing;
};
