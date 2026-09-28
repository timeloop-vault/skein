// imageFiles — pure helpers for the Files editor's image preview
// (#409): which paths are images, what MIME type they render as, and
// how to read the error strings `read_image_bytes` (app/src-tauri/src/
// fs.rs) returns. Mirrors `skein_review::image` (Rust) so the frontend
// never has to round-trip to find out a path is an image before
// deciding whether to call `read_file_text` or `read_image_bytes`.

/** Files above this are refused by `read_image_bytes` — kept in sync
 *  with `skein_review::MAX_IMAGE_BYTES`. */
export const MAX_IMAGE_BYTES = 16 * 1024 * 1024;

/** The MIME type for `path`'s extension, or `null` if it is not one of
 *  the known image types. Case-insensitive; works on either `/` or `\`
 *  separators. A dotfile like ".png" has no stem before its one dot —
 *  that's a hidden file named "png", not a PNG with an empty name — so
 *  it is treated as extension-less, matching the Rust side. */
export const imageMime = (path: string): string | null => {
	const name = path.split(/[/\\]/).pop() ?? path;
	const dot = name.lastIndexOf(".");
	if (dot <= 0 || dot === name.length - 1) return null;
	const ext = name.slice(dot + 1).toLowerCase();
	switch (ext) {
		case "png":
			return "image/png";
		case "jpg":
		case "jpeg":
			return "image/jpeg";
		case "gif":
			return "image/gif";
		case "webp":
			return "image/webp";
		case "svg":
			return "image/svg+xml";
		default:
			return null;
	}
};

/** Whether `path` names an image by the same rule as {@link imageMime}. */
export const isImagePath = (path: string): boolean => imageMime(path) != null;

export type ImageError =
	| { kind: "toolarge"; len: number }
	| { kind: "notimage" }
	| { kind: "missing" }
	| { kind: "unavailable" }
	| { kind: "other"; msg: string };

/** Parse a `read_image_bytes` (or future review-pane image read)
 *  rejection into a typed shape the UI can switch on. Unrecognized
 *  messages fall through to `other`, carrying the raw text. */
export const parseImageError = (msg: string): ImageError => {
	if (msg === "notimage") return { kind: "notimage" };
	if (msg === "missing") return { kind: "missing" };
	if (msg === "unavailable") return { kind: "unavailable" };
	if (msg.startsWith("toolarge:")) {
		const len = Number(msg.slice("toolarge:".length));
		return { kind: "toolarge", len: Number.isFinite(len) ? len : 0 };
	}
	return { kind: "other", msg };
};

/** Human-readable byte size: "512 B", "3.4 KB", "16.0 MB". One decimal
 *  past the first unit; bytes are shown as a bare integer. */
export const formatBytes = (n: number): string => {
	if (n < 1024) return `${n} B`;
	if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
	return `${(n / (1024 * 1024)).toFixed(1)} MB`;
};

/** Render a `parseImageError` rejection as prose, for the Files editor's
 *  single-image view. `unavailable` and `missing` read the same here —
 *  from a plain file read they're indistinguishable — but the review
 *  pane's before/after diff (`ImageDiff.tsx`) gives `missing` its own
 *  "not in this version" wording instead, since there the side legitimately
 *  not existing at that revision is the common case, not a failure. */
export const describeImageError = (msg: string): string => {
	const err = parseImageError(msg);
	switch (err.kind) {
		case "toolarge": {
			const limitMb = MAX_IMAGE_BYTES / (1024 * 1024);
			return `${formatBytes(err.len)} — too large to preview (limit ${limitMb} MB)`;
		}
		case "notimage":
			return "not a recognized image type";
		case "missing":
			return "file no longer exists";
		case "unavailable":
			return "can't read this file right now";
		case "other":
			return err.msg;
		default:
			return msg;
	}
};
