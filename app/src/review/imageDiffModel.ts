// Pure decision logic for the review pane's image handling (#409).
//
// Kept out of ReviewPane.tsx / ImageDiff.tsx so the two questions they
// answer — which body to render for a file, and which before/after
// modes make sense given what actually loaded — are each one small,
// table-tested function rather than JSX conditionals nobody can unit
// test on their own.

import { imageMime, isImagePath } from "../imageFiles.ts";

/// SVG is the one image type that is *also* text: git diffs it line by
/// line like source, so unlike a raster format it can offer a real text
/// view alongside the image one.
export const isSvgPath = (path: string): boolean => imageMime(path) === "image/svg+xml";

export type SvgViewMode = "image" | "text";

export type ReviewBody =
	| { kind: "no-file" }
	| { kind: "image" }
	| { kind: "blocked"; blocked: string }
	| { kind: "no-diff" }
	| { kind: "diff" };

/// What the review pane's body should render for the open file.
///
/// A non-SVG image always renders as an image, whether or not the
/// backend reports it `blocked` — a `blocked: "binary"` raster file is
/// exactly the case #409 replaces. An SVG defaults to the image view
/// too, but `svgMode: "text"` falls through to the ordinary diff logic
/// (blocked / no-diff / diff) since SVG has real hunks to show.
export function chooseReviewBody(args: {
	hasFile: boolean;
	path: string;
	blocked: string | undefined;
	hunksLength: number;
	svgMode: SvgViewMode;
}): ReviewBody {
	if (!args.hasFile) return { kind: "no-file" };
	const image = isImagePath(args.path);
	const svg = image && isSvgPath(args.path);
	if (image && (!svg || args.svgMode === "image")) return { kind: "image" };
	if (args.blocked) return { kind: "blocked", blocked: args.blocked };
	if (args.hunksLength === 0) return { kind: "no-diff" };
	return { kind: "diff" };
}

/// Which side(s) of a before/after pair actually exist, from the
/// file's change kind. An added/untracked file has no "before"; a
/// deleted one has no "after". Everything else — including a rename,
/// where the *path* changes but content exists on both sides — has
/// both. Used to skip loading a side that cannot exist rather than
/// waiting on a call that can only come back "missing".
export function sidesForChange(change: string): { old: boolean; new: boolean } {
	if (change === "added" || change === "untracked") return { old: false, new: true };
	if (change === "deleted") return { old: true, new: false };
	return { old: true, new: true };
}

export type SideStatus = "loading" | "ready" | "absent" | "error";

/// Swap and onion both need a real image on *each* side to mean
/// anything — flipping between one photo and a placeholder, or fading
/// a placeholder over a photo, is not a mode. Side-by-side is always
/// available since it renders whatever each side has, placeholder or
/// not.
export function modesAvailable(
	oldStatus: SideStatus,
	newStatus: SideStatus,
): { swap: boolean; onion: boolean } {
	const bothReady = oldStatus === "ready" && newStatus === "ready";
	return { swap: bothReady, onion: bothReady };
}
