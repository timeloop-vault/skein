// Shared, low-level components used across the app.

import { type DragEvent, useEffect, useState } from "react";
import { type AgentInfo, type AgentListing, kindHasAgents, listHarnessAgents } from "./agents.ts";
import { HARNESS_KINDS } from "./data.tsx";
import type { Harness, HarnessKind, Room, Status } from "./types.ts";

// Drop indicator side relative to a tab. The dragOver handler picks
// "before" if the cursor is left of the tab's horizontal midpoint,
// "after" if right. CSS pseudo-elements render an accent-colored bar
// on the matching edge so the user can see where the drop will land.
// Issue #26.
export type DropSide = "before" | "after" | null;

// Drag-related props that both tab kinds share. All optional so the
// presentational tab can render without drag wiring (e.g. in tests).
export interface DragProps {
	draggable?: boolean;
	dragging?: boolean;
	dropSide?: DropSide;
	onDragStart?: (e: DragEvent<HTMLDivElement>) => void;
	onDragOver?: (e: DragEvent<HTMLDivElement>) => void;
	onDrop?: (e: DragEvent<HTMLDivElement>) => void;
	onDragEnd?: (e: DragEvent<HTMLDivElement>) => void;
}

// #68: size is owned by CSS (density --chip / --dot tokens + context
// overrides in styles.css), not per-call-site numbers.
// #132: data-kind / data-status feed the shared hover popover
// (statusPopover.ts), which replaces the native title= (slow, unstyled,
// and it couldn't show state). aria-label keeps the info available to
// screen readers.
export const HChip = ({ kind, harnessId }: { kind: HarnessKind; harnessId?: string }) => {
	const k = HARNESS_KINDS[kind];
	// #141: harnessId lets the popover read this harness's OWN live state
	// (so a room-tab summary chip shows its real state, not the room
	// aggregate). Omitted where there's no single harness behind the chip.
	return (
		<span
			className={`h-chip ${k.chip}`}
			data-kind={kind}
			data-harness-id={harnessId}
			aria-label={k.name}
		>
			{k.label}
		</span>
	);
};

export const StatusDot = ({ status }: { status: Status }) => (
	<span className={`tab-status st-${status}`} data-status={status} aria-label={status} />
);

// ── Tabs / chrome ──────────────────────────────────────────────────

export const RoomTab = ({
	r,
	active,
	onClick,
	onClose,
	draggable,
	dragging,
	dropSide,
	onDragStart,
	onDragOver,
	onDrop,
	onDragEnd,
}: {
	r: Room;
	active: boolean;
	onClick: () => void;
	onClose: () => void;
} & DragProps) => (
	<div
		className={`sk-tab ${active ? "active" : ""} ${dragging ? "dragging" : ""} ${dropSide ? `drop-${dropSide}` : ""}`}
		onClick={onClick}
		draggable={draggable}
		onDragStart={onDragStart}
		onDragOver={onDragOver}
		onDrop={onDrop}
		onDragEnd={onDragEnd}
	>
		<div className="row-1">
			<StatusDot status={r.status} />
			{/* #132: task tooltip lives on the name, not the whole tab, so
			    hovering a dot/chip shows only the status popover (not the
			    native tooltip on top of it). */}
			<span className="name" title={r.task}>
				{r.name}
			</span>
			{r.badge > 0 && <span className="tab-badge">{r.badge}</span>}
			<span
				className="sk-tab-close"
				title="Close room"
				onClick={(e) => {
					e.stopPropagation();
					onClose();
				}}
			>
				×
			</span>
		</div>
		<div className="row-2">
			{r.branch && (
				<>
					<span>{r.branch}</span>
					<span>·</span>
				</>
			)}
			<span style={{ display: "flex", gap: 2 }}>
				{r.harnesses.map((h) => (
					<HChip key={h.id} kind={h.kind} harnessId={h.id} />
				))}
			</span>
		</div>
	</div>
);

export const HarnessTab = ({
	h,
	active,
	closable,
	onClick,
	onClose,
	draggable,
	dragging,
	dropSide,
	onDragStart,
	onDragOver,
	onDrop,
	onDragEnd,
}: {
	h: Harness;
	active: boolean;
	closable: boolean;
	onClick: () => void;
	onClose: () => void;
} & DragProps) => (
	<div
		className={`sk-harness-tab ${active ? "active" : ""} ${dragging ? "dragging" : ""} ${dropSide ? `drop-${dropSide}` : ""}`}
		data-htab={h.id}
		onClick={onClick}
		draggable={draggable}
		onDragStart={onDragStart}
		onDragOver={onDragOver}
		onDrop={onDrop}
		onDragEnd={onDragEnd}
	>
		<StatusDot status={h.status} />
		<HChip kind={h.kind} harnessId={h.id} />
		<span className="ht-name">{h.name}</span>
		{closable && (
			<span
				className="ht-x"
				onClick={(e) => {
					e.stopPropagation();
					onClose();
				}}
			>
				×
			</span>
		)}
	</div>
);

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
 *  `(default)` is the first row and not a decoration — it is what every
 *  harness before #247 did, and it means "whatever the tool's own
 *  `agent` setting says", which no named row can express. */
const AgentStep = ({
	kind,
	listing,
	onPick,
	onBack,
}: {
	kind: HarnessKind;
	listing: AgentListing | null;
	onPick: (agent: string | undefined) => void;
	onBack: () => void;
}) => (
	<>
		<h3>{HARNESS_KINDS[kind].name} — which agent?</h3>
		<p>
			Bound at launch and fixed for the life of the conversation. Skein re-passes it on every
			resume.
		</p>
		<div className="sk-agent-list">
			<div className="sk-agent-row" onClick={() => onPick(undefined)}>
				<div className="head">
					<span className="a-name">(default)</span>
				</div>
				<div className="a-desc">Whatever {HARNESS_KINDS[kind].name} is configured to use.</div>
			</div>
			{listing === null ? (
				<div className="sk-agent-note">asking {HARNESS_KINDS[kind].name}…</div>
			) : (
				<>
					{listing.degraded && (
						<div className="sk-agent-note warn">
							This list may be incomplete — {listing.degraded}
						</div>
					)}
					{listing.agents.map((a) => (
						<AgentRow key={a.name} agent={a} onClick={() => onPick(a.name)} />
					))}
				</>
			)}
		</div>
		<button className="sk-btn" type="button" onClick={onBack}>
			← Back
		</button>
	</>
);

const AgentRow = ({ agent, onClick }: { agent: AgentInfo; onClick: () => void }) => (
	<div className="sk-agent-row" onClick={onClick} title={agent.name}>
		<div className="head">
			<span className="a-name">{agent.name}</span>
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
	onPick,
	onCancel,
}: {
	/** The room's worktree — project agents resolve relative to it. */
	cwd: string;
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
