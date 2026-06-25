// The shared Activity-row grid + atoms. Every kind-specific row is a
// thin wrapper around <Row>; only <Row> owns the
// [time][chip][glyph][gist][right] grid, so the per-kind gist stays
// clean. Issue #80 D2a. Lifted from the design canvas
// (docs/design/skein/project/live-context-rows.jsx).

import type { CSSProperties, ReactNode } from "react";
import { HChip } from "../components.tsx";
import type { HarnessKind } from "../types.ts";

/// Default glyph per displayKind (the CSS class suffix). An explicit
/// `glyph` prop on <Row> wins; otherwise this map; otherwise "·".
export const GLYPH: Record<string, string> = {
	edit: "✎",
	write: "✎",
	read: "◌",
	grep: "⌕",
	glob: "⌕",
	bash: "$",
	task: "◇",
	todowrite: "☰",
	ask: "?",
	agent: "✦",
	pr: "⤴",
	error: "✕",
	queue: "⏵",
	userfile: "✋",
	slash: "/",
	compact: "⤓",
	cost: "$",
	perm: "⏵",
	burst: "▸",
	user: "›",
	title: "❝",
	"perm-mode": "⏵",
	bridge: "⇄",
};

const GLYPH_STYLE: CSSProperties = {
	display: "inline-block",
	width: 12,
	textAlign: "center",
	marginRight: 6,
};

/// Epoch-ms → "HH:MM:SS" (24h). Empty string for missing/zero
/// timestamps (timestamp-less Claude rows are stamped 0; the store
/// keeps them but they have no real clock).
export function formatClock(ms: number): string {
	if (!ms || ms <= 0) return "";
	const d = new Date(ms);
	const p = (n: number) => String(n).padStart(2, "0");
	return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

interface RowProps {
	/** displayKind — drives the `k-<kind>` CSS class and the glyph. */
	kind: string;
	harness?: HarnessKind | undefined;
	timestampMs: number;
	children: ReactNode;
	right?: ReactNode;
	onClick?: (() => void) | undefined;
	/** Optional preview block; escapes the grid as a sibling. */
	extra?: ReactNode;
	/** Explicit glyph override (wins over the GLYPH map). */
	glyph?: string | undefined;
	/** Extra modifier class on the row element itself (e.g. the burst
	 *  shimmer's `live` — its selector needs it on `.lc-row`, where the
	 *  slide-in wrapper class can't reach). */
	className?: string | undefined;
}

export const Row = ({
	kind,
	harness,
	timestampMs,
	children,
	right,
	onClick,
	extra,
	glyph,
	className,
}: RowProps) => (
	<div
		className={`lc-row k-${kind}${className ? ` ${className}` : ""}`}
		onClick={onClick}
		style={onClick ? { cursor: "pointer" } : undefined}
	>
		<span className="time">{formatClock(timestampMs)}</span>
		<span className="by">{harness && <HChip kind={harness} />}</span>
		<span className="gist">
			<span className="glyph" aria-hidden="true" style={GLYPH_STYLE}>
				{glyph ?? GLYPH[kind] ?? "·"}
			</span>
			{children}
		</span>
		<span className="right">{right}</span>
		{extra}
	</div>
);

/// Last path segment. Splits on either separator — harnesses emit
/// native paths, so Windows rows carry backslashes. Tolerates trailing
/// separators and empty input.
export function basename(path: string | null | undefined): string {
	if (!path) return "";
	const trimmed = path.replace(/[\\/]+$/, "");
	const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
	return i === -1 ? trimmed : trimmed.slice(i + 1);
}

/// ms → "850ms" / "4.2s" / "3m". Shared by tool rows (durations) and the
/// turn separator (turn length).
export function formatDuration(ms: number): string {
	if (ms < 1000) return `${ms}ms`;
	if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
	return `${Math.round(ms / 60_000)}m`;
}
