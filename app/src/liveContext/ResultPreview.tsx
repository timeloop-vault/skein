// Shared tool-result preview block — issue #80 D2c.
//
// A collapsed peek at a tool's output (bash stdout, read file content,
// an opencode todo plan, an api-error message) that escapes the row grid
// as a sibling via <Row>'s `extra` prop. The block is capped at ~6em and
// shows an "expand" affordance only when the content actually overflows
// that cap (measured, not guessed) — so short results render as a plain
// labelled block with no misleading toggle. Expanding swaps to the
// taller, scrollable `.tall` variant.
//
// The CSS (`.lc-row-preview` and friends) shipped with the D2a chrome;
// this is the component that finally drives it. `formatBytes`/`byteLen`
// are exported for the rows that compute a size pill.

import { createContext, useContext, useLayoutEffect, useRef, useState } from "react";

/// Per-row preview expansion, lifted out of ResultPreview so it survives
/// the row unmounting/remounting as the virtualized feed scrolls (D2g) —
/// local state would collapse every expanded preview the moment it
/// scrolls out of view. A row has at most one preview, so ActivityRow
/// provides a single bool + toggle for its own row. Absent provider →
/// ResultPreview falls back to local state (keeps it usable standalone).
export interface PreviewExpansion {
	expanded: boolean;
	toggle: () => void;
}
export const PreviewExpansionContext = createContext<PreviewExpansion | null>(null);

/// Human-readable byte size for a result body: "342 B" / "1.2 KB" / "3.4 MB".
export function formatBytes(n: number): string {
	if (n < 1024) return `${n} B`;
	if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
	return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/// UTF-8 byte length of a string (the size pill counts bytes, not code units).
export function byteLen(s: string): number {
	return new TextEncoder().encode(s).length;
}

export const ResultPreview = ({
	label,
	body,
	sizeLabel,
	variant,
}: {
	/** Uppercase block label, e.g. "output", "file", "plan". */
	label: string;
	/** The preview text. Rendered verbatim in a pre-wrap block. */
	body: string;
	/** Right-aligned meta (size pill). Omit when there's nothing to size. */
	sizeLabel?: string | undefined;
	/** `api-error` tints the block red (retry notices). */
	variant?: "api-error" | undefined;
}) => {
	const bodyRef = useRef<HTMLPreElement>(null);
	// Expansion comes from the parent (survives scroll unmount) when an
	// ActivityRow provides it; otherwise local. `overflowing` stays local
	// — it's a measurement of the rendered DOM, re-derived on mount.
	const ctx = useContext(PreviewExpansionContext);
	const [localExpanded, setLocalExpanded] = useState(false);
	const expanded = ctx ? ctx.expanded : localExpanded;
	const toggleExpanded = ctx ? ctx.toggle : () => setLocalExpanded((e) => !e);
	const [overflowing, setOverflowing] = useState(false);

	// The body (not the whole block) is the scroll region, so the head —
	// which holds the collapse control — stays pinned above it and never
	// scrolls out of reach. Measure the body's overflow against its cap to
	// decide whether to show the expand affordance. A ResizeObserver (not
	// a one-shot measure) is required because inactive room cards stay
	// mounted with display:none: a block that first lays out while hidden
	// measures 0×0 and must re-measure when shown (0 → real size fires the
	// observer); it also re-measures on expand/collapse, when the cap
	// itself changes.
	useLayoutEffect(() => {
		const el = bodyRef.current;
		if (!el) return;
		const measure = () => setOverflowing(el.scrollHeight > el.clientHeight + 2);
		measure();
		const ro = new ResizeObserver(measure);
		ro.observe(el);
		return () => ro.disconnect();
	}, []);

	const canToggle = overflowing || expanded;
	const cls = `lc-row-preview${expanded ? " tall" : ""}${variant ? ` ${variant}` : ""}`;

	return (
		<div className={cls}>
			<div className="head">
				<span>{label}</span>
				<span className="size">
					{sizeLabel}
					{canToggle && (
						<button type="button" className="expand-link" onClick={toggleExpanded}>
							{sizeLabel ? " · " : ""}
							{expanded ? "collapse" : "expand"}
						</button>
					)}
				</span>
			</div>
			<pre className="body" ref={bodyRef}>
				{body}
			</pre>
		</div>
	);
};
