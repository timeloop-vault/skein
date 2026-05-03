// Shared, low-level components used across the app.

import type { CSSProperties } from "react";
import { HARNESS_KINDS } from "./data.tsx";
import type { Harness, HarnessKind, Session, Status } from "./types.ts";

export const HChip = ({ kind, size = 14 }: { kind: HarnessKind; size?: number }) => {
	const k = HARNESS_KINDS[kind];
	const style: CSSProperties = {
		width: size,
		height: size,
		fontSize: Math.round(size * 0.62),
		borderRadius: Math.max(2, size / 4),
	};
	return (
		<span className={`h-chip ${k.chip}`} style={style} title={k.name}>
			{k.label}
		</span>
	);
};

export const StatusDot = ({ status, size = 6 }: { status: Status; size?: number }) => (
	<span className={`tab-status st-${status}`} style={{ width: size, height: size }} />
);

// ── Tabs / chrome ──────────────────────────────────────────────────

export const SessionTab = ({
	s,
	active,
	onClick,
	onClose,
}: {
	s: Session;
	active: boolean;
	onClick: () => void;
	onClose: () => void;
}) => (
	<div className={`sk-tab ${active ? "active" : ""}`} onClick={onClick} title={s.task}>
		<div className="row-1">
			<StatusDot status={s.status} />
			<span className="name">{s.name}</span>
			{s.badge > 0 && <span className="tab-badge">{s.badge}</span>}
			<span
				className="sk-tab-close"
				title="Close session"
				onClick={(e) => {
					e.stopPropagation();
					onClose();
				}}
			>
				×
			</span>
		</div>
		<div className="row-2">
			<span>{s.branch}</span>
			<span>·</span>
			<span style={{ display: "flex", gap: 2 }}>
				{s.harnesses.map((h) => (
					<HChip key={h.id} kind={h.kind} size={9} />
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
}: {
	h: Harness;
	active: boolean;
	closable: boolean;
	onClick: () => void;
	onClose: () => void;
}) => (
	<div className={`sk-harness-tab ${active ? "active" : ""}`} onClick={onClick}>
		<StatusDot status={h.status} size={5} />
		<HChip kind={h.kind} size={11} />
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
							<HChip kind={k.id} size={16} /> <span className="h-name">{k.name}</span>
						</div>
						<div className="h-desc">{k.desc}</div>
					</div>
				),
			)}
		</div>
	</div>
);
