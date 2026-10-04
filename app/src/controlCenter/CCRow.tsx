import { StatusDot } from "../components.tsx";
import type { Status } from "../types.ts";
import type { CCRow as CCRowData, HarnessSnapshot } from "./model.ts";
import { relativeTime } from "./relativeTime.ts";

function dotStatus(h: HarnessSnapshot): Status {
	return h.status === "spawning" ? "running" : h.status;
}

// "idle · idle" and "review · ready for review" say the same thing twice.
function chipText(rank: string, reason: string): string {
	return reason.toLowerCase().includes(rank) ? reason : `${rank} · ${reason}`;
}

export function CCRowView({
	row,
	active,
	now,
	onFocus,
}: {
	row: CCRowData;
	active: boolean;
	now: number;
	onFocus: (roomId: string, harnessId?: string) => void;
}) {
	const { room, snap, attention } = row;
	const lead = snap.harnesses.find((h) => h.id === attention.harnessId) ?? snap.harnesses[0];
	const since = attention.since;
	const lastActive = snap.harnesses.reduce(
		(best, h) => Math.max(best, h.lastActivityAt ?? 0),
		snap.lastStatus?.createdMs ?? 0,
	);
	const so = snap.signoff;
	const drivingText = lead ? `${lead.name} · ${lead.label}` : "no harness";
	return (
		<div
			className={`cc-row${active ? " cc-active" : ""}`}
			data-rank={attention.rank}
			data-room-id={room.id}
		>
			<button
				type="button"
				className="cc-row-main"
				onClick={() => onFocus(room.id)}
				aria-label={`Focus room ${room.name}`}
			>
				<span className="cc-cell">
					<span className="cc-name" title={room.name}>
						{room.name}
					</span>
					{snap.branch ? (
						<span className="cc-sub" title={snap.branch}>
							{snap.branch}
						</span>
					) : null}
				</span>
				<span className="cc-cell">
					<span className="cc-chip" data-rank={attention.rank}>
						{chipText(attention.rank, attention.reason)}
					</span>
					{since !== null ? <span className="cc-sub">{relativeTime(since, now)}</span> : null}
				</span>
				<span className="cc-cell">
					<span className="cc-phase" title={drivingText}>
						{drivingText}
					</span>
				</span>
				<span className="cc-cell" title={snap.lastStatus?.body ?? ""}>
					{snap.lastStatus ? (
						<>
							<span className="cc-status-text">{snap.lastStatus.body.split(/\r?\n/, 1)[0]}</span>
							<span className="cc-sub">{relativeTime(snap.lastStatus.createdMs, now)}</span>
						</>
					) : (
						<span className="cc-none">no status reported</span>
					)}
				</span>
				<span className="cc-cell cc-review">
					{so && so.unresolvedCount > 0 ? (
						<span className="cc-threads">{so.unresolvedCount} open</span>
					) : null}
					{so?.approved ? <span className="cc-badge cc-signed">signed off</span> : null}
					{so?.stale ? <span className="cc-badge cc-stale">stale</span> : null}
				</span>
				<span className="cc-cell">
					{lastActive > 0 ? (
						<span className="cc-active-ago">{relativeTime(lastActive, now)}</span>
					) : null}
				</span>
			</button>
			<span className="cc-harnesses">
				{snap.harnesses.map((h) => (
					<button
						type="button"
						key={h.id}
						className="cc-harness"
						title={`${h.name}: ${h.label}`}
						onClick={() => onFocus(room.id, h.id)}
					>
						<StatusDot status={dotStatus(h)} />
						<span>{h.name}</span>
					</button>
				))}
			</span>
		</div>
	);
}
