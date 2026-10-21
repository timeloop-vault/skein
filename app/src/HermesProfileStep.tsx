// #627: the picker's second step for a `hermes` harness — which profile it
// runs in. Its own step rather than the agent step: a profile is a whole
// config home, not an `--agent` name. Reuses the agent list's row styling.

import { useEffect, useState } from "react";
import { HermesConsentDialog } from "./HermesConsentDialog.tsx";
import { SKEIN_HERMES_PROFILE } from "./harnessCmd.ts";
import {
	consentNeeded,
	entryOf,
	type HermesProfileListing,
	profileBlurb,
	profileLabel,
	profileTag,
} from "./hermesProfiles.ts";
import { useHermesProfiles } from "./useHermesProfiles.ts";

export const HermesProfileStep = ({
	active,
	onPick,
	onBack,
}: {
	/** Whether this picker's room is the one on screen — gates Enter. */
	active: boolean;
	onPick: (profile: string) => void;
	onBack: () => void;
}) => {
	const listing: HermesProfileListing | null = useHermesProfiles(true);
	const [selected, setSelected] = useState(SKEIN_HERMES_PROFILE);
	const [asking, setAsking] = useState<string | null>(null);

	const choose = (name: string) => {
		const entry = entryOf(listing, name);
		if (consentNeeded(name, entry?.skeinEntry, entry?.hooksPending) === "none") onPick(name);
		else setAsking(name);
	};

	// Same Enter contract as the agent step: only the picker on screen, and
	// never an Enter aimed at a field or a modal (the consent dialog).
	useEffect(() => {
		if (!active || asking !== null) return undefined;
		const onKey = (e: KeyboardEvent) => {
			const arrow = e.key === "ArrowDown" ? 1 : e.key === "ArrowUp" ? -1 : 0;
			if ((e.key !== "Enter" && arrow === 0) || e.repeat || e.defaultPrevented) return;
			const t = e.target instanceof HTMLElement ? e.target : null;
			if (
				t?.closest("input, textarea, select, [contenteditable], .sk-modal, .sk-palette, .xterm")
			) {
				return;
			}
			e.preventDefault();
			if (arrow === 0) {
				choose(selected);
				return;
			}
			const names = listing?.profiles.map((p) => p.name) ?? [];
			const at = names.indexOf(selected);
			const next = names[Math.min(names.length - 1, Math.max(0, at + arrow))];
			if (next !== undefined) setSelected(next);
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	});

	const askingEntry = asking === null ? undefined : entryOf(listing, asking);

	return (
		<>
			<h3>hermes — which profile?</h3>
			<p>
				Bound at launch. Skein's own profile carries the review tools; in one of yours Skein asks
				before adding its entry. Up/Down moves the highlight, Enter picks it.
			</p>
			<div className="sk-agent-list">
				{listing === null ? (
					<div className="sk-agent-note">asking hermes…</div>
				) : (
					<>
						{listing.error && (
							<div className="sk-agent-note warn">
								This list may be incomplete — {listing.error}
							</div>
						)}
						{listing.profiles.map((p) => {
							const tag = profileTag(p);
							return (
								<div
									key={p.name}
									className={`sk-agent-row ${p.name === selected ? "selected" : ""}`}
									onClick={() => {
										setSelected(p.name);
										choose(p.name);
									}}
									title={p.name}
								>
									<div className="head">
										<span className="a-name">{profileLabel(p.name)}</span>
										{tag && <span className="sk-agent-tag">{tag}</span>}
									</div>
									<div className="a-desc">{profileBlurb(p)}</div>
								</div>
							);
						})}
					</>
				)}
			</div>
			<button className="sk-btn" type="button" onClick={onBack}>
				← Back
			</button>
			{asking !== null && (
				<HermesConsentDialog
					profile={asking}
					mode={
						consentNeeded(asking, askingEntry?.skeinEntry, askingEntry?.hooksPending) === "manual"
							? "manual"
							: "ask"
					}
					reason={askingEntry?.reason ?? null}
					onProceed={() => {
						const name = asking;
						setAsking(null);
						onPick(name);
					}}
					onCancel={() => setAsking(null)}
				/>
			)}
		</>
	);
};
