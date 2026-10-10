import type { Dispatch, SetStateAction } from "react";
import { type AgentListing, kindHasAgents } from "./agents.ts";
import { HChip } from "./components.tsx";
import { HARNESS_KINDS, HARNESS_ORDER } from "./data.tsx";
import { AgentFieldNote } from "./NewRoomAgentFieldNote.tsx";
import { type DefaultAgents, defaultAgentFor } from "./prefs.ts";
import type { HarnessKind } from "./types.ts";

// Split out of `NewRoomDialog.tsx` (#461): the Starting harness cards and
// the #247 Agent field for kinds that take one.

export const HarnessSection = ({
	harness,
	setHarness,
	agent,
	setAgent,
	agentListing,
	agentMissing,
	defaultAgents,
}: {
	harness: HarnessKind;
	setHarness: Dispatch<SetStateAction<HarnessKind>>;
	agent: string | undefined;
	setAgent: Dispatch<SetStateAction<string | undefined>>;
	agentListing: AgentListing | null;
	agentMissing: boolean;
	defaultAgents: DefaultAgents;
}) => (
	<>
		<div className="sk-field">
			<label>Starting harness</label>
			<div className="sk-radio-row sk-harness-kinds">
				{HARNESS_ORDER.filter((id) => id !== "remote").map((id) => {
					const k = HARNESS_KINDS[id];
					return (
						<div
							key={id}
							className={`sk-radio-card ${harness === id ? "selected" : ""}`}
							onClick={() => {
								if (id === harness) return;
								setHarness(id);
								// The agent belongs to the kind it was picked for —
								// leaving it set would hand Claude's `--agent coder`
								// to opencode. Switching kind takes that kind's
								// Settings default instead (#248), which is
								// undefined for a kind with no agents at all.
								setAgent(defaultAgentFor(defaultAgents, id));
							}}
						>
							<div className="top">
								<HChip kind={id} /> {k.name}
							</div>
							<div className="desc">{k.desc}</div>
						</div>
					);
				})}
			</div>
		</div>

		{/* #247: only for kinds that bind `--agent` at launch. The
					    field is a <select> rather than the picker's row list
					    because the modal has one column and four fields above
					    it; the full list with descriptions is what the `+
					    harness` picker is for. */}
		{kindHasAgents(harness) && (
			<div className="sk-field">
				<label htmlFor="sk-agent">Agent</label>
				<select
					id="sk-agent"
					className="sk-select"
					value={agent ?? ""}
					onChange={(e) => setAgent(e.target.value || undefined)}
				>
					<option value="">(tool default) — no agent named</option>
					{/* A remembered name the CLI no longer offers still
								    renders, so the field shows what it is actually set
								    to instead of silently sliding to (default). */}
					{agentMissing && agent !== undefined && (
						<option value={agent}>{agent} — not found</option>
					)}
					{(agentListing?.agents ?? []).map((a) => (
						<option key={a.name} value={a.name}>
							{a.name}
							{a.allowsReviewTools ? "" : "  ⚠ no review tools"}
						</option>
					))}
				</select>
				<AgentFieldNote
					agent={agent}
					listing={agentListing}
					missing={agentMissing}
					kind={harness}
				/>
			</div>
		)}
	</>
);
