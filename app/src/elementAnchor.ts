// Element-comment re-anchoring (#434, docs/design-surface-recon.md §3.3).
//
// The DOM counterpart of crates/skein-review/src/anchor.rs, same contract:
// a comment is NEVER silently moved and never silently dropped. Pure — the
// iframe gathers candidates, this decides which one (if any) the anchor sits
// on now. A tie is ambiguous and ambiguity is `lost`, never a mis-point.

export type ElementRect = { x: number; y: number; w: number; h: number };
export type ElementSource = { file: string; line: number; column?: number };
export type ElementDescriptor = {
	odId?: string;
	selector: string;
	tag: string;
	text: string;
	attrs: Record<string, string>;
	source?: ElementSource;
	rect: ElementRect;
};
/** The stored evidence: what the element looked like when commented on. */
export type ElementAnchor = ElementDescriptor & { entry: string };

export type LocateResult = {
	/** Element currently at anchor.selector. */
	bySelector: ElementDescriptor | null;
	/** Elements whose odId === anchor.odId (at most 10). */
	byOdId: ElementDescriptor[];
	/** Same tag and same normalized text (at most 10). */
	byText: ElementDescriptor[];
	/** Every element with anchor.tag (at most 200), for overlap scoring. */
	sameTag: ElementDescriptor[];
};

export type AnchorState = "anchored" | "reanchored" | "stale" | "lost";
export type Placement = {
	state: AnchorState;
	at: ElementDescriptor | null;
	score?: number;
};

/** Best overlap must reach this to be offered as a guess. */
export const STALE_THRESHOLD = 0.5;
const W_TEXT = 0.5;
const W_ATTRS = 0.3;
const W_SOURCE_LINE = 0.2;
const W_SOURCE_FILE = 0.1;
const EPS = 1e-9;

export function normalizeText(s: string): string {
	return s.replace(/\s+/g, " ").trim();
}

function sameText(a: string, b: string): boolean {
	return normalizeText(a) === normalizeText(b);
}

/** Distinct by selector: candidate lists overlap. */
function distinct(list: ElementDescriptor[]): ElementDescriptor[] {
	const seen = new Set<string>();
	const out: ElementDescriptor[] = [];
	for (const c of list) {
		if (!seen.has(c.selector)) {
			seen.add(c.selector);
			out.push(c);
		}
	}
	return out;
}

function only(list: ElementDescriptor[]): ElementDescriptor | null {
	const d = distinct(list);
	return d.length === 1 ? (d[0] ?? null) : null;
}

function nonEmptyAttrs(attrs: Record<string, string>): [string, string][] {
	return Object.entries(attrs).filter(([, v]) => v !== "");
}

function attrsEqual(want: [string, string][], have: Record<string, string>): boolean {
	const got = nonEmptyAttrs(have);
	return got.length === want.length && want.every(([k, v]) => have[k] === v);
}

function jaccard(a: Set<string>, b: Set<string>): number {
	if (a.size === 0 && b.size === 0) return 0;
	let inter = 0;
	for (const x of a) if (b.has(x)) inter++;
	return inter / (a.size + b.size - inter);
}

function tokens(s: string): Set<string> {
	const n = normalizeText(s).toLowerCase();
	return new Set(n === "" ? [] : n.split(" "));
}

function attrPairs(attrs: Record<string, string>): Set<string> {
	return new Set(nonEmptyAttrs(attrs).map(([k, v]) => `${k}=${v}`));
}

function overlap(anchor: ElementAnchor, c: ElementDescriptor): number {
	let score =
		jaccard(tokens(anchor.text), tokens(c.text)) * W_TEXT +
		jaccard(attrPairs(anchor.attrs), attrPairs(c.attrs)) * W_ATTRS;
	if (anchor.source && c.source && anchor.source.file === c.source.file) {
		score += anchor.source.line === c.source.line ? W_SOURCE_LINE : W_SOURCE_FILE;
	}
	return score;
}

export function matchElement(anchor: ElementAnchor, found: LocateResult): Placement {
	const text = normalizeText(anchor.text);
	const sel = found.bySelector;

	// 1. anchored
	if (anchor.odId) {
		const hits = distinct(found.byOdId.filter((c) => c.tag === anchor.tag));
		if (hits.length === 1 && hits[0]) {
			return { state: "anchored", at: hits[0] };
		}
	}
	if (
		sel &&
		(!anchor.odId || sel.odId === anchor.odId) &&
		sel.tag === anchor.tag &&
		sameText(sel.text, anchor.text)
	) {
		return { state: "anchored", at: sel };
	}

	// 2. reanchored: found elsewhere, and unambiguous
	const re = (at: ElementDescriptor | null): Placement | null =>
		at ? { state: "reanchored", at } : null;
	const sameTag = found.sameTag.filter((c) => c.tag === anchor.tag);
	if (anchor.odId) {
		const byId = found.byOdId.filter((c) => c.tag === anchor.tag && sameText(c.text, anchor.text));
		const hit = re(only(byId));
		if (hit && text !== "") return hit;
	}
	if (text !== "") {
		const hit = re(
			only(found.byText.filter((c) => c.tag === anchor.tag && sameText(c.text, anchor.text))),
		);
		if (hit) return hit;
	}
	const want = nonEmptyAttrs(anchor.attrs);
	if (want.length > 0) {
		const hit = re(only(sameTag.filter((c) => attrsEqual(want, c.attrs))));
		if (hit) return hit;
	}
	if (anchor.source) {
		const { file, line } = anchor.source;
		const hit = re(only(sameTag.filter((c) => c.source?.file === file && c.source.line === line)));
		if (hit) return hit;
	}

	// 3. stale: best structural overlap, strictly ahead of the runner-up
	const pool = distinct([...sameTag, ...(sel ? [sel] : []), ...found.byOdId]).filter(
		(c) => c.tag === anchor.tag,
	);
	const scored = pool
		.map((c) => ({ c, score: overlap(anchor, c) }))
		.sort((a, b) => b.score - a.score);
	const best = scored[0];
	const next = scored[1];
	if (best && best.score >= STALE_THRESHOLD - EPS && (!next || best.score - next.score > EPS)) {
		return { state: "stale", at: best.c, score: best.score };
	}

	// 4. lost
	return { state: "lost", at: null };
}
