// design.show_changes (#547): the pure half. Match the changed lines Rust
// sends against the source sites the preview frame reports, and shape the
// answer. No DOM, no React: the hook (useDesignChanges.ts) does the posting.

import { asRecord, isOmitted, type RequestResult } from "./agentRequestsShared.ts";
import { stripSourcePrefix } from "./designPreview.ts";

export const LIMITS =
	"Highlights elements whose JSX opening tag (including its attribute lines) is on a changed line, every rendered instance. It cannot show changes to logic, styles defined outside the tag, tokens, CSS or plain .js, or elements not rendered right now; those are listed as unmapped. It shows where changed lines render, not everything that looks different.";

/** Most elements the frame is asked to outline. */
export const MAX_OUTLINED = 200;

export type ChangedFile = { path: string; lines: number[]; deleted: boolean };

export type ShowChangesRequest = {
	files: ChangedFile[];
	truncated?: true;
	reveal?: true;
};

/** One distinct (fileName, line) the frame rendered. */
export type SourceSite = {
	fileName: string;
	line: number;
	endLine: number;
	count: number;
	onScreen: number;
};

export type MappedSite = {
	path: string;
	line: number;
	endLine: number;
	changedLines: number[];
	elements: number;
	onScreen: number;
};

export type UnmappedEntry = {
	path: string;
	lines?: number[];
	reason: "deleted" | "file_not_rendered" | "no_rendered_tag";
};

export type ChangesMatch = {
	/** Raw sites to hand back to the frame. */
	matchedSites: { fileName: string; line: number; endLine: number }[];
	mapped: MappedSite[];
	unmapped: UnmappedEntry[];
	noSourceInfo: boolean;
	/** The frame's site list was cut at its cap, so it may be incomplete. */
	sitesCapped?: true;
};

export type ShowChangesResult = {
	highlighted: number;
	capped?: true;
	/** The rendered-site list was cut, so `file_not_rendered` may be wrong. */
	sitesCapped?: true;
	mapped: MappedSite[];
	unmapped: UnmappedEntry[];
	noSourceInfo?: true;
	truncated?: true;
	limits: string;
};

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

/** Separators unified and a leading `./` dropped, for comparing paths. */
export const normPath = (p: string): string => {
	let out = p.replace(/\\/g, "/");
	while (out.startsWith("./")) out = out.slice(2);
	return out;
};

export function parseShowChangesArgs(
	raw: unknown,
): RequestResult<{ roomId: string; harnessId: string } & ShowChangesRequest> {
	const r = asRecord(raw);
	if (!r) return { ok: false, error: "bad_arguments: args must be an object" };
	if (typeof r.roomId !== "string" || !r.roomId) {
		return { ok: false, error: "bad_arguments: roomId is required" };
	}
	if (typeof r.harnessId !== "string" || !r.harnessId) {
		return { ok: false, error: "bad_arguments: harnessId is required" };
	}
	if (!Array.isArray(r.files)) {
		return { ok: false, error: "bad_arguments: files must be an array" };
	}
	const files: ChangedFile[] = [];
	for (const f of r.files) {
		const fr = asRecord(f);
		if (!fr || typeof fr.path !== "string" || !fr.path) {
			return { ok: false, error: "bad_arguments: each file needs a path string" };
		}
		if (!Array.isArray(fr.lines) || !fr.lines.every((n) => isNum(n) && Number.isInteger(n))) {
			return { ok: false, error: "bad_arguments: each file needs a lines array of integers" };
		}
		if (!isOmitted(fr.deleted) && typeof fr.deleted !== "boolean") {
			return { ok: false, error: "bad_arguments: deleted must be a boolean" };
		}
		files.push({ path: fr.path, lines: fr.lines as number[], deleted: fr.deleted === true });
	}
	if (!isOmitted(r.reveal) && typeof r.reveal !== "boolean") {
		return { ok: false, error: "bad_arguments: reveal must be a boolean" };
	}
	return {
		ok: true,
		value: {
			roomId: r.roomId,
			harnessId: r.harnessId,
			files,
			...(r.truncated === true ? { truncated: true as const } : {}),
			...(r.reveal === true ? { reveal: true as const } : {}),
		},
	};
}

const uniqSorted = (xs: number[]): number[] => [...new Set(xs)].sort((a, b) => a - b);

/** Which changed lines land inside which rendered opening tags. */
export function matchChanges(
	sites: readonly SourceSite[],
	files: readonly ChangedFile[],
	sitesCapped = false,
): ChangesMatch {
	// Compared case-insensitively: prototypes are served from case-insensitive
	// filesystems on Windows and macOS.
	const bySite = sites
		.map((s) => ({ s, path: stripSourcePrefix(s.fileName) }))
		.flatMap(({ s, path }) =>
			path === undefined ? [] : [{ s, path: normPath(path).toLowerCase() }],
		);

	const matchedSites: ChangesMatch["matchedSites"] = [];
	const mapped: MappedSite[] = [];
	const unmapped: UnmappedEntry[] = [];

	for (const file of files) {
		if (file.deleted) {
			unmapped.push({ path: file.path, reason: "deleted" });
			continue;
		}
		const path = normPath(file.path).toLowerCase();
		const own = bySite.filter((x) => x.path === path);
		if (own.length === 0) {
			unmapped.push({
				path: file.path,
				...(file.lines.length > 0 ? { lines: uniqSorted(file.lines) } : {}),
				reason: "file_not_rendered",
			});
			continue;
		}
		const covered = new Set<number>();
		for (const { s } of own) {
			const hit = uniqSorted(file.lines.filter((n) => s.line <= n && n <= s.endLine));
			if (hit.length === 0) continue;
			for (const n of hit) covered.add(n);
			matchedSites.push({ fileName: s.fileName, line: s.line, endLine: s.endLine });
			mapped.push({
				path: file.path,
				line: s.line,
				endLine: s.endLine,
				changedLines: hit,
				elements: s.count,
				onScreen: s.onScreen,
			});
		}
		const rest = uniqSorted(file.lines.filter((n) => !covered.has(n)));
		if (rest.length > 0) unmapped.push({ path: file.path, lines: rest, reason: "no_rendered_tag" });
	}
	return {
		matchedSites,
		mapped,
		unmapped,
		noSourceInfo: sites.length === 0,
		...(sitesCapped ? { sitesCapped: true as const } : {}),
	};
}

/** The verb's answer from the match and what the frame reported outlining. */
export function changesAnswer(
	match: ChangesMatch,
	shown: { highlighted: number; capped?: boolean },
	truncated: boolean,
): ShowChangesResult {
	return {
		highlighted: shown.highlighted,
		...(shown.capped === true ? { capped: true as const } : {}),
		mapped: match.mapped,
		unmapped: match.unmapped,
		...(match.sitesCapped ? { sitesCapped: true as const } : {}),
		...(match.noSourceInfo ? { noSourceInfo: true as const } : {}),
		...(truncated ? { truncated: true as const } : {}),
		limits: LIMITS,
	};
}
