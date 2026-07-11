// Fallback text provider — handles whatever no specific provider
// claimed. UTF-8 lossy decode happens in Rust (`read_file_text`),
// then we render as monospace pre — plus find-in-file (#49 stage 1):
// a search box above the content, case-insensitive plain-string
// matches highlighted with <mark>, Enter / Shift+Enter cycling the
// active match into view. Regex find waits for the stage-2 editor.

import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { registerPreviewProvider } from "../registry.ts";

interface Segment {
	text: string;
	hit: boolean;
	/** Char offset in the source — stable React key. */
	at: number;
}

/** Queries shorter than this don't search — a 1-char query on a
 *  256 KB file means tens of thousands of `<mark>`s per keystroke. */
const MIN_QUERY_LEN = 2;
/** Hard cap on highlighted matches for the same reason. */
const MAX_MATCHES = 1000;

const segment = (
	text: string,
	query: string,
): { parts: Segment[]; count: number; capped: boolean } => {
	const parts: Segment[] = [];
	// Case folding can change string length (Turkish İ → two code
	// units), which would shift every offset we compute on `lower`
	// but apply to `text`. Fall back to case-sensitive search when
	// the lengths disagree — rare, and correct beats clever here.
	let lower = text.toLowerCase();
	let q = query.toLowerCase();
	if (lower.length !== text.length) {
		lower = text;
		q = query;
	}
	let i = 0;
	let count = 0;
	let capped = false;
	while (i <= text.length) {
		const at = count < MAX_MATCHES ? lower.indexOf(q, i) : -1;
		if (at === -1) {
			capped = count >= MAX_MATCHES && lower.indexOf(q, i) !== -1;
			if (i < text.length) parts.push({ text: text.slice(i), hit: false, at: i });
			break;
		}
		if (at > i) parts.push({ text: text.slice(i, at), hit: false, at: i });
		parts.push({ text: text.slice(at, at + query.length), hit: true, at });
		count++;
		i = at + query.length;
	}
	return { parts, count, capped };
};

const TextPreview = ({ text, truncated }: { text: string; truncated: boolean }) => {
	const [query, setQuery] = useState("");
	const [active, setActive] = useState(0);
	const activeRef = useRef<HTMLElement | null>(null);

	const segments = useMemo(
		() => (query.length >= MIN_QUERY_LEN ? segment(text, query) : null),
		[text, query],
	);
	const count = segments?.count ?? 0;
	const norm = count > 0 ? ((active % count) + count) % count : 0;

	// biome-ignore lint/correctness/useExhaustiveDependencies: reset the cursor when the query changes
	useEffect(() => setActive(0), [query]);
	// biome-ignore lint/correctness/useExhaustiveDependencies: norm/segments are deliberate triggers — re-scroll whenever the active match moves
	useEffect(() => {
		activeRef.current?.scrollIntoView({ block: "center" });
	}, [norm, segments]);

	let seen = -1;
	const body: ReactNode = segments
		? segments.parts.map((p) => {
				if (!p.hit) return <span key={p.at}>{p.text}</span>;
				seen += 1;
				const isActive = seen === norm;
				return (
					<mark
						key={p.at}
						className={isActive ? "sk-find-hit active" : "sk-find-hit"}
						ref={
							isActive
								? (el) => {
										activeRef.current = el;
									}
								: undefined
						}
					>
						{p.text}
					</mark>
				);
			})
		: text;

	return (
		<>
			<div className="sk-find-bar">
				<input
					value={query}
					placeholder="find in file"
					spellCheck={false}
					onChange={(e) => setQuery(e.target.value)}
					onKeyDown={(e) => {
						if (e.key === "Enter") {
							e.preventDefault();
							setActive((a) => a + (e.shiftKey ? -1 : 1));
						} else if (e.key === "Escape" && query) {
							// Clear the query on first Esc; a second Esc
							// bubbles up and closes the overlay.
							e.stopPropagation();
							setQuery("");
						}
					}}
				/>
				<span className="sk-find-count">
					{segments?.capped
						? `${norm + 1}/${count}+ — refine query`
						: count > 0
							? `${norm + 1}/${count}`
							: query.length >= MIN_QUERY_LEN
								? "0 matches"
								: query
									? `${MIN_QUERY_LEN}+ chars`
									: ""}
				</span>
			</div>
			{truncated && (
				<div style={{ color: "var(--warn)", marginBottom: 8 }}>truncated to first 256 KB</div>
			)}
			<div>{body}</div>
		</>
	);
};

registerPreviewProvider({
	id: "text",
	// `*` = catch-all fallback. Lower priority than every specific
	// provider so it only fires when nothing else matched.
	patterns: ["*"],
	priority: 0,
	needs: "text",
	render: ({ text, truncated }) => {
		if (text === undefined) return null;
		return <TextPreview text={text} truncated={truncated} />;
	},
});
