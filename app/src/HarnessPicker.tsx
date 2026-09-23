// The two-step "+ harness" picker (kind, then agent for kinds that
// take one). Split out of components.tsx (#19) — moved verbatim.

import { useEffect, useRef, useState } from "react";
import {
	type AgentInfo,
	type AgentListing,
	kindHasAgents,
	listHarnessAgents,
	validateAgent,
} from "./agents.ts";
import { HChip } from "./components.tsx";
import { HARNESS_KINDS } from "./data.tsx";
import { type DefaultAgents, defaultAgentFor } from "./prefs.ts";
import type { HarnessKind } from "./types.ts";

/** The ⚠ an agent earns by not being able to see the review tools.
 *
 *  Shared by both pickers because the thing it warns about is the same
 *  in both: an agent whose `tools` allowlist omits MCP cannot see
 *  Skein's review server *at all*, with a healthy connection and
 *  nothing in any log (#215). Nobody would find this out by using it —
 *  they would find out that the agent never answers a review comment. */
export const NO_REVIEW_TOOLS_TITLE =
	"This agent's tools list leaves out MCP, so it cannot see Skein's review tools. " +
	"Picking it switches off the review loop for this harness.";

export const NoReviewToolsBadge = () => (
	<span className="sk-agent-warn" title={NO_REVIEW_TOOLS_TITLE}>
		⚠ no review tools
	</span>
);

/** Fetch the agents a kind accepts in `cwd`, for as long as a picker
 *  wants them. `null` kind = nothing to fetch yet.
 *
 *  Re-fetches on every (kind, cwd) change rather than caching: the
 *  whole point of asking the CLI is that the answer moves — an agent
 *  file added while Skein was open must show up on the next open of
 *  the picker, not on the next restart. */
export const useAgentListing = (kind: HarnessKind | null, cwd: string) => {
	const [listing, setListing] = useState<AgentListing | null>(null);
	useEffect(() => {
		if (!kind || !kindHasAgents(kind)) {
			setListing(null);
			return undefined;
		}
		if (!cwd) {
			// No folder to ask about. A degraded listing rather than
			// `null`, which reads as "still loading" and would leave the
			// step spinning forever on a room that has no cwd.
			setListing({
				agents: [],
				degraded: `no folder to ask ${HARNESS_KINDS[kind].name} about`,
				unsupported: false,
			});
			return undefined;
		}

		let cancelled = false;
		setListing(null);
		void listHarnessAgents(kind, cwd).then((l) => {
			if (!cancelled) setListing(l);
		});
		return () => {
			cancelled = true;
		};
	}, [kind, cwd]);
	return listing;
};

/** Second step of the picker: which agent this harness runs as.
 *
 *  `(tool default)` is the first row and not a decoration — it is what
 *  every harness before #247 did, and it means "whatever the tool's own
 *  `agent` setting says", which no named row can express. Not labelled
 *  plain "(default)" since #248: the Settings default is a *name*, and
 *  the two must not read as the same thing.
 *
 *  #248: the kind's Settings default arrives preselected and Enter takes
 *  it, so a settled-in user spends one keystroke rather than a hunt. A
 *  default the CLI has since dropped is not preselected — the row it
 *  would have highlighted is gone, and quietly highlighting something
 *  else would spawn an agent nobody chose — so the step falls back to
 *  `(tool default)` and says why. */
const AgentStep = ({
	kind,
	listing,
	preferred,
	active,
	onPick,
	onBack,
}: {
	kind: HarnessKind;
	listing: AgentListing | null;
	/** The kind's default from Settings, if any. */
	preferred: string | undefined;
	/** Whether this picker's room is the one on screen. */
	active: boolean;
	onPick: (agent: string | undefined) => void;
	onBack: () => void;
}) => {
	const tool = HARNESS_KINDS[kind].name;
	// Before the list arrives the default cannot be disproved, so it is
	// preselected; the spawn re-checks the name anyway (#247).
	const gone = listing !== null && validateAgent(preferred, listing).kind === "unknown";
	const selected = gone ? undefined : preferred;

	// Enter *spawns*, so the listener is held to the one picker the user
	// is looking at. Every room's column stays mounted and a picker can
	// be left open in a room the user switched away from; unscoped, an
	// Enter typed into another room's terminal would spawn a harness here,
	// out of sight. Same for an Enter aimed at a field or a modal.
	useEffect(() => {
		if (!active) return undefined;
		const onKey = (e: KeyboardEvent) => {
			if (e.key !== "Enter" || e.repeat || e.defaultPrevented) return;
			const t = e.target instanceof HTMLElement ? e.target : null;
			if (
				t?.closest("input, textarea, select, [contenteditable], .sk-modal, .sk-palette, .xterm")
			) {
				return;
			}
			e.preventDefault();
			onPick(selected);
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [active, onPick, selected]);

	// The highlighted row is what Enter takes, so it has to be on screen:
	// a default near the bottom of a long plugin list would otherwise sit
	// out of view while Enter spawns it. Re-run when the list lands, since
	// the row does not exist until then. Scrolls the list only —
	// `scrollIntoView` would also scroll the panes around the picker.
	const listRef = useRef<HTMLDivElement | null>(null);
	// biome-ignore lint/correctness/useExhaustiveDependencies: selected + listing are deliberate triggers — the highlighted row moves or first appears without the effect reading either
	useEffect(() => {
		const list = listRef.current;
		const row = list?.querySelector<HTMLElement>(".sk-agent-row.selected");
		if (!list || !row) return;
		const l = list.getBoundingClientRect();
		const r = row.getBoundingClientRect();
		if (r.top >= l.top && r.bottom <= l.bottom) return;
		list.scrollTop += r.top - l.top - (l.height - r.height) / 2;
	}, [selected, listing]);

	return (
		<>
			<h3>{tool} — which agent?</h3>
			<p>
				Bound at launch and fixed for the life of the conversation. Skein re-passes it on every
				resume. Enter picks the highlighted row.
			</p>
			<div className="sk-agent-list" ref={listRef}>
				<div
					className={`sk-agent-row ${selected === undefined ? "selected" : ""}`}
					onClick={() => onPick(undefined)}
				>
					<div className="head">
						<span className="a-name">(tool default)</span>
					</div>
					<div className="a-desc">No agent named — whatever {tool} is configured to use.</div>
				</div>
				{gone && preferred && (
					<div className="sk-agent-note warn">
						Your default "{preferred}" is not one {tool} offers here any more.
					</div>
				)}
				{listing === null ? (
					<div className="sk-agent-note">asking {tool}…</div>
				) : (
					<>
						{listing.degraded && (
							<div className="sk-agent-note warn">
								This list may be incomplete — {listing.degraded}
							</div>
						)}
						{listing.agents.map((a) => (
							<AgentRow
								key={a.name}
								agent={a}
								selected={a.name === selected}
								isDefault={a.name === preferred}
								onClick={() => onPick(a.name)}
							/>
						))}
					</>
				)}
			</div>
			<button className="sk-btn" type="button" onClick={onBack}>
				← Back
			</button>
		</>
	);
};

const AgentRow = ({
	agent,
	selected,
	isDefault,
	onClick,
}: {
	agent: AgentInfo;
	selected: boolean;
	isDefault: boolean;
	onClick: () => void;
}) => (
	<div
		className={`sk-agent-row ${selected ? "selected" : ""}`}
		onClick={onClick}
		title={agent.name}
	>
		<div className="head">
			<span className="a-name">{agent.name}</span>
			{isDefault && <span className="sk-agent-tag">your default</span>}
			{!agent.allowsReviewTools && <NoReviewToolsBadge />}
		</div>
		{agent.description && <div className="a-desc">{agent.description}</div>}
	</div>
);

// The full-pane harness picker: `+ harness` swaps the body slot for
// this card grid until a kind is picked. (The files-pillar design
// proposed an anchored dropdown here instead; the owner prefers the
// pane — 2026-07-14.) Iterating the registry means new kinds (Files)
// appear as cards automatically.
//
// #247 makes it two-step for the kinds that take `--agent`: the grid
// swaps for an agent list instead of spawning, because the agent is
// bound at launch and Claude cannot change it afterwards — "pick it
// later" is not on offer. Kinds without the capability still spawn on
// click, so the extra step only appears where it buys something.
export const HarnessPicker = ({
	cwd,
	defaultAgents,
	active,
	onPick,
	onCancel,
}: {
	/** The room's worktree — project agents resolve relative to it. */
	cwd: string;
	/** Settings' per-kind defaults (#248), preselected in the agent step. */
	defaultAgents: DefaultAgents;
	/** The picker's room is the active one — gates the agent step's Enter. */
	active: boolean;
	onPick: (kind: HarnessKind, agent?: string) => void;
	onCancel: () => void;
}) => {
	const [step, setStep] = useState<HarnessKind | null>(null);
	// Fetched as soon as the kind is chosen, not on hover or on mount:
	// the probe runs the harness CLI, and a grid that shells out five
	// times just to be looked at would be a poor trade for a subtitle.
	const listing = useAgentListing(step, cwd);
	return (
		<div className="sk-empty-harness">
			{/* #189: the picker needs a way out that isn't picking. Esc still
			    cancels the whole picker from the agent step too — Back is for
			    changing your mind about the kind, not for leaving. */}
			<span className="sk-empty-harness-x" title="Cancel (Esc)" onClick={onCancel}>
				×
			</span>
			{step ? (
				<AgentStep
					kind={step}
					listing={listing}
					preferred={defaultAgentFor(defaultAgents, step)}
					active={active}
					onPick={(agent) => onPick(step, agent)}
					onBack={() => setStep(null)}
				/>
			) : (
				<>
					<h3>Add a harness</h3>
					<p>Pick an agent for this workspace. All harnesses see the same worktree.</p>
					<div className="sk-harness-grid">
						{(
							Object.values(HARNESS_KINDS) as { id: HarnessKind; name: string; desc: string }[]
						).map((k) => (
							<div
								key={k.id}
								className="sk-harness-card"
								onClick={() => (kindHasAgents(k.id) ? setStep(k.id) : onPick(k.id))}
							>
								<div className="head">
									<HChip kind={k.id} /> <span className="h-name">{k.name}</span>
								</div>
								<div className="h-desc">{k.desc}</div>
							</div>
						))}
					</div>
				</>
			)}
		</div>
	);
};
