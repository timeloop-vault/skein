// Cross-harness activity feed — epic #50 L7c.
//
// Per-room view of recent phase transitions: when each harness in
// this room moved from running → waiting, idle → running, etc., and
// which detection strategy (L2c-1 Claude end_turn, L2c-2 opencode
// idle, L2a 8 s silence, L2b prompt-match, …) fired it.
//
// Data comes from two sources merged in the UI:
//   * Historical — one-shot query against the `harness_events`
//     sqlite table (L6) on mount / room change. Returns newest-first
//     up to a fixed cap.
//   * Live — `harnessActivity.subscribeTransitions` callback prepends
//     a synthetic event the moment a real transition fires. The L6
//     DB writer hook in App.tsx persists the same transition
//     asynchronously; we don't wait for that round-trip before
//     showing it to the user.
//
// Time labels are relative ("just now", "12s ago", "3m ago"). A
// background tick re-renders every 5 s so the labels stay live
// without per-row timers.

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import { HChip } from "./components.tsx";
import { type ActivityPhase, harnessActivity } from "./harnessActivity.ts";
import type { Harness } from "./types.ts";

/// Mirrors the Rust `HarnessEvent` DTO from `app/src-tauri/src/db.rs`.
/// Tauri serializes with camelCase (rename_all = "camelCase") so the
/// TS shape matches one-to-one.
interface HarnessEvent {
	id: number;
	harnessId: string;
	roomId: string;
	fromPhase: ActivityPhase;
	toPhase: ActivityPhase;
	timestampMs: number;
	hasUserInput: boolean;
	source: string | null;
}

interface ActivityFeedProps {
	roomId: string;
	/// Harness list for this room — used to look up chip kind +
	/// display name for each event row. Passing the full list (not
	/// just ids) avoids a per-row lookup in parent state and lets
	/// `ActivityFeed` survive harness ordering changes.
	harnesses: readonly Harness[];
}

/// How many historical events to load on mount. 200 covers the
/// typical "what happened recently" window without paging; for a
/// chatty session this is ~30-60 minutes of history. Older rows
/// stay in the DB but aren't displayed in v1.
const HISTORY_LIMIT = 200;
/// Re-render cadence for the relative-time labels. 5 s is fast
/// enough that "12s ago" doesn't get stale for long and slow
/// enough that we're not thrashing the React tree.
const TIME_REFRESH_MS = 5_000;

export const ActivityFeed = ({ roomId, harnesses }: ActivityFeedProps) => {
	const [events, setEvents] = useState<HarnessEvent[] | null>(null);
	const [error, setError] = useState<string | null>(null);

	// Tick state purely to bust the render cache for time labels —
	// the value is ignored; setTick incrementing is the whole point.
	const [, setTick] = useState(0);
	useEffect(() => {
		const id = setInterval(() => setTick((t) => t + 1), TIME_REFRESH_MS);
		return () => clearInterval(id);
	}, []);

	// Harness id → harness lookup so each row can render the chip
	// and a sensible label even if the parent re-orders the array.
	const harnessById = useMemo(() => {
		const m = new Map<string, Harness>();
		for (const h of harnesses) m.set(h.id, h);
		return m;
	}, [harnesses]);

	// Historical load. Runs on mount + whenever the room id changes.
	// We don't paginate yet — the cap is the cap. If the user wants
	// older history, "load more" lands in a follow-up.
	useEffect(() => {
		let cancelled = false;
		setError(null);
		invoke<HarnessEvent[]>("db_recent_harness_events_by_room", {
			roomId,
			sinceMs: 0,
			limit: HISTORY_LIMIT,
		})
			.then((rows) => {
				if (!cancelled) setEvents(rows);
			})
			.catch((err: unknown) => {
				if (cancelled) return;
				const msg = err instanceof Error ? err.message : String(err);
				console.warn(`[skein] db_recent_harness_events_by_room failed for ${roomId}:`, msg);
				setError(msg);
			});
		return () => {
			cancelled = true;
		};
	}, [roomId]);

	// Live updates. The L6 writer in App.tsx is also subscribed to
	// the same source; it persists the transition asynchronously.
	// We're optimistic — render immediately, the DB row arrives
	// soon enough that the user never sees a discrepancy.
	useEffect(() => {
		const harnessIds = new Set(harnesses.map((h) => h.id));
		const unsub = harnessActivity.subscribeTransitions((harnessId, from, to, source) => {
			if (!harnessIds.has(harnessId)) return;
			const activity = harnessActivity.get(harnessId);
			const synthetic: HarnessEvent = {
				// Negative ids signal "not in DB yet" — collisions are
				// avoided because the real `harness_events.id` autoincrements
				// from 1 and we use `Math.random()` to keep React keys
				// unique across multiple synthetic events in one render.
				id: -Math.floor(Math.random() * 1_000_000_000) - 1,
				harnessId,
				roomId,
				fromPhase: from,
				toPhase: to,
				timestampMs: Date.now(),
				hasUserInput: activity?.hasUserInput ?? false,
				source,
			};
			setEvents((prev) => (prev === null ? [synthetic] : [synthetic, ...prev]));
		});
		return unsub;
	}, [roomId, harnesses]);

	if (error) {
		return (
			<div
				style={{
					margin: "10px 14px",
					padding: "8px 10px",
					color: "var(--err)",
					fontFamily: "var(--sk-mono)",
					fontSize: 11,
					background: "color-mix(in srgb, var(--err) 8%, var(--bg-2))",
					border: "1px solid color-mix(in srgb, var(--err) 35%, var(--line))",
					borderRadius: 5,
				}}
			>
				{error}
			</div>
		);
	}

	if (events === null) {
		return <div style={{ padding: "10px 14px", color: "var(--fg-3)" }}>loading…</div>;
	}

	if (events.length === 0) {
		return (
			<div
				style={{
					display: "flex",
					flexDirection: "column",
					alignItems: "center",
					justifyContent: "center",
					gap: 6,
					padding: "32px 14px",
					color: "var(--fg-2)",
					textAlign: "center",
				}}
			>
				<span style={{ color: "var(--fg-1)" }}>no activity yet</span>
				<span style={{ fontSize: 10, color: "var(--fg-3)" }}>
					transitions will appear here as harnesses run
				</span>
			</div>
		);
	}

	return (
		<div
			style={{
				flex: 1,
				overflowY: "auto",
				padding: "6px 0",
				fontFamily: "var(--sk-mono)",
				fontSize: 11,
				minHeight: 0,
			}}
		>
			{events.map((e) => (
				<EventRow key={e.id} event={e} harness={harnessById.get(e.harnessId)} />
			))}
		</div>
	);
};

interface EventRowProps {
	event: HarnessEvent;
	/// `undefined` if the event references a harness no longer in the
	/// room (e.g. the harness was removed but the historical event
	/// row survives). We still render the row — just with the id as
	/// a fallback label.
	harness: Harness | undefined;
}

const EventRow = ({ event, harness }: EventRowProps) => (
	<div
		style={{
			display: "flex",
			alignItems: "center",
			gap: 10,
			padding: "4px 14px",
			color: "var(--fg-1)",
			minWidth: 0,
		}}
		title={new Date(event.timestampMs).toLocaleString()}
	>
		{harness ? (
			<HChip kind={harness.kind} size={11} />
		) : (
			// Placeholder slot keeps row alignment stable for events
			// from harnesses that have since been removed.
			<span style={{ width: 11, height: 11, flex: "0 0 11px" }} />
		)}
		<span
			style={{
				color: "var(--fg-0)",
				flex: "0 0 auto",
				overflow: "hidden",
				textOverflow: "ellipsis",
				whiteSpace: "nowrap",
				maxWidth: 100,
			}}
		>
			{harness?.name ?? event.harnessId.slice(0, 6)}
		</span>
		<span style={{ color: "var(--fg-3)", flex: "0 0 auto" }}>
			<PhaseToken phase={event.fromPhase} /> → <PhaseToken phase={event.toPhase} />
		</span>
		<span style={{ flex: 1 }} />
		{event.source && <SourceChip source={event.source} />}
		<span
			style={{
				color: "var(--fg-3)",
				flex: "0 0 auto",
				fontSize: 10,
				minWidth: 60,
				textAlign: "right",
			}}
		>
			{relativeTime(event.timestampMs)}
		</span>
	</div>
);

/// Colored phase keyword. Mirrors the StatusDot color choices so the
/// reader can scan transitions by hue without parsing the words.
const PhaseToken = ({ phase }: { phase: ActivityPhase }) => (
	<span style={{ color: PHASE_COLOR[phase] }}>{phase}</span>
);

const PHASE_COLOR: Record<ActivityPhase, string> = {
	spawning: "var(--ok)",
	running: "var(--ok)",
	idle: "var(--fg-3)",
	waiting: "var(--waiting)",
	exited: "var(--fg-2)",
};

/// Compact chip showing which detection strategy fired this
/// transition. Sources are grouped by family for color:
///   * `l2c1-*` (Claude adapter) — orange
///   * `l2c2-*` (opencode adapter) — purple
///   * `l2a-*` / `l2b-*` (idle / pattern heuristics) — neutral
///   * `pty-*` (lifecycle: spawn-output, exit) — green / red
const SourceChip = ({ source }: { source: string }) => (
	<span
		style={{
			color: SOURCE_COLOR(source),
			background: "var(--bg-3)",
			padding: "1px 6px",
			borderRadius: 3,
			fontSize: 10,
			flex: "0 0 auto",
		}}
	>
		{source}
	</span>
);

const SOURCE_COLOR = (source: string): string => {
	if (source.startsWith("l2c1-")) return "var(--accent)";
	if (source.startsWith("l2c2-")) return "var(--waiting)";
	if (source === "pty-exit") return "var(--err)";
	if (source === "pty-output") return "var(--ok)";
	return "var(--fg-2)";
};

/// "just now" / "12s ago" / "3m ago" / "2h ago" / "5d ago". Coarse
/// granularity is fine — exact timestamps are accessible via the
/// row's `title` attribute on hover.
const relativeTime = (timestampMs: number): string => {
	const diff = Date.now() - timestampMs;
	if (diff < 5_000) return "just now";
	if (diff < 60_000) return `${Math.floor(diff / 1_000)}s ago`;
	if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
	if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
	return `${Math.floor(diff / 86_400_000)}d ago`;
};
