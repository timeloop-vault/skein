// Settings → agent defaults and the worktree branch template.

import { kindHasAgents } from "./agents.ts";
import { HChip, NO_REVIEW_TOOLS_TITLE, useAgentListing } from "./components.tsx";
import { HARNESS_KINDS, HARNESS_ORDER } from "./data.tsx";
import { type DefaultAgents, defaultAgentFor } from "./prefs.ts";
import type { HarnessKind } from "./types.ts";

/** One kind's default-agent field. Its own component so each kind runs
 *  its own `useAgentListing` probe. */
const DefaultAgentField = ({
	kind,
	cwd,
	value,
	onChange,
}: {
	kind: HarnessKind;
	cwd: string;
	value: string | undefined;
	onChange: (agent: string | undefined) => void;
}) => {
	const listing = useAgentListing(kind, cwd);
	const listed = listing?.agents.some((a) => a.name === value) ?? false;
	// Same rule as New room: only an authoritative list proves a name gone.
	const missing = value !== undefined && listing !== null && listing.degraded === null && !listed;
	const picked = value ? listing?.agents.find((a) => a.name === value) : undefined;
	const note = missing
		? {
				cls: "err",
				text: `${HARNESS_KINDS[kind].name} does not offer "${value}" in this folder. New harnesses fall back to the tool default until you pick another.`,
			}
		: picked && !picked.allowsReviewTools
			? { cls: "warn", text: NO_REVIEW_TOOLS_TITLE }
			: listing?.degraded
				? { cls: "warn", text: `This list may be incomplete — ${listing.degraded}` }
				: null;
	return (
		<div className="sk-default-agent">
			<label htmlFor={`sk-default-agent-${kind}`}>
				<HChip kind={kind} /> {HARNESS_KINDS[kind].name}
			</label>
			<select
				id={`sk-default-agent-${kind}`}
				className="sk-select"
				value={value ?? ""}
				onChange={(e) => onChange(e.target.value || undefined)}
			>
				<option value="">(tool default) — no agent named</option>
				{/* The stored name always renders, even before the list lands
				    or after it stops being offered, so the field shows what it
				    is set to instead of sliding to (tool default). */}
				{value !== undefined && !listed && (
					<option value={value}>
						{value}
						{missing ? " — not found" : ""}
					</option>
				)}
				{(listing?.agents ?? []).map((a) => (
					<option key={a.name} value={a.name}>
						{a.name}
						{a.allowsReviewTools ? "" : "  ⚠ no review tools"}
					</option>
				))}
			</select>
			{note && <div className={`sk-default-agent-note ${note.cls}`}>{note.text}</div>}
		</div>
	);
};

export const AgentSettings = ({
	defaultAgents,
	onDefaultAgent,
	agentCwd,
	branchTemplate,
	onBranchTemplate,
}: {
	defaultAgents: DefaultAgents;
	onDefaultAgent: (kind: HarnessKind, agent: string | undefined) => void;
	agentCwd: string;
	branchTemplate: string;
	onBranchTemplate: (v: string) => void;
}) => (
	<>
		<div className="sk-field">
			<label>Default agents</label>
			<div className="sk-help">
				The agent a new harness of each kind starts as. The <code>+ harness</code> picker highlights
				it, so Enter takes it, and New room fills it in unless the folder remembers its own. (tool
				default) names no agent and leaves it to the tool's own setting.
			</div>
			{HARNESS_ORDER.filter(kindHasAgents).map((kind) => (
				<DefaultAgentField
					key={kind}
					kind={kind}
					cwd={agentCwd}
					value={defaultAgentFor(defaultAgents, kind)}
					onChange={(agent) => onDefaultAgent(kind, agent)}
				/>
			))}
		</div>

		<div className="sk-field">
			<label htmlFor="sk-branch-template">Branch template</label>
			<div className="sk-help">
				{"{slug}"} is the task; a folder remembers the last prefix you used.
			</div>
			<input
				id="sk-branch-template"
				className="sk-input"
				value={branchTemplate}
				onChange={(e) => onBranchTemplate(e.target.value)}
			/>
		</div>
	</>
);
