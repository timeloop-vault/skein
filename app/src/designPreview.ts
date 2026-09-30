// designPreview — the pure half of the `design` harness (#433): the
// preview URL and the beacon messages the injected script posts from
// inside the iframe. The page is arbitrary worktree code, so every
// message is UNTRUSTED input: the caller must already have checked
// `e.source === iframe.contentWindow`; this module checks the shape,
// caps every string and never yields anything but plain text.

export const MAX_BEACON_TEXT = 500;
export const MAX_BEACONS = 50;

export type Beacon =
	| { type: "ready"; href: string }
	| { type: "resource-error"; tag: string; url: string }
	| { type: "script-error"; message: string; url?: string; line?: number };

const cap = (s: string): string => (s.length > MAX_BEACON_TEXT ? s.slice(0, MAX_BEACON_TEXT) : s);

/** Validate one `message` event payload from the preview iframe.
 *  Returns null for anything that is not a well-formed v1 beacon. */
export const parseBeacon = (data: unknown): Beacon | null => {
	if (typeof data !== "object" || data === null) return null;
	const d = data as Partial<
		Record<"source" | "v" | "type" | "href" | "tag" | "url" | "message" | "line", unknown>
	>;
	if (d.source !== "skein-design" || d.v !== 1) return null;
	switch (d.type) {
		case "ready":
			return typeof d.href === "string" ? { type: "ready", href: cap(d.href) } : null;
		case "resource-error":
			return typeof d.tag === "string" && typeof d.url === "string"
				? { type: "resource-error", tag: cap(d.tag), url: cap(d.url) }
				: null;
		case "script-error": {
			if (typeof d.message !== "string") return null;
			const out: Beacon = { type: "script-error", message: cap(d.message) };
			if (typeof d.url === "string") out.url = cap(d.url);
			if (typeof d.line === "number" && Number.isFinite(d.line)) out.line = d.line;
			return out;
		}
		default:
			return null;
	}
};

/** Append a beacon, keeping at most MAX_BEACONS (the oldest win). */
export const pushBeacon = (list: readonly Beacon[], b: Beacon): Beacon[] =>
	list.length >= MAX_BEACONS ? [...list] : [...list, b];

/** `base` (ends with `/`) + the worktree-relative `entry`, each path
 *  segment percent-encoded, plus a `?v=` cache-buster. */
export const previewUrl = (base: string, entry: string, version: number): string =>
	`${base}${entry.split("/").map(encodeURIComponent).join("/")}?v=${version}`;
