// Shared, low-level components used across the app.

import type { DragEvent } from "react";
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

export const HarnessPicker = ({ onPick }: { onPick: (kind: HarnessKind) => void }) => (
	<div className="sk-empty-harness">
		<h3>Add a harness</h3>
		<p>Pick an agent for this workspace. All harnesses see the same worktree.</p>
		<div className="sk-harness-grid">
			{(Object.values(HARNESS_KINDS) as { id: HarnessKind; name: string; desc: string }[]).map(
				(k) => (
					<div key={k.id} className="sk-harness-card" onClick={() => onPick(k.id)}>
						<div className="head">
							<HChip kind={k.id} /> <span className="h-name">{k.name}</span>
						</div>
						<div className="h-desc">{k.desc}</div>
					</div>
				),
			)}
		</div>
	</div>
);
