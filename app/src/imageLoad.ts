// useImageBytes — shared blob-URL loading for an image preview (#409).
//
// Extracted out of ImageView so the review pane's before/after diff
// (ImageDiff.tsx) can load two sides with the exact same lifecycle
// rather than re-deriving it: fetch raw bytes, mint an object URL for
// the path's MIME type, revoke the previous one, and drop a result that
// arrives after a newer `loadKey` has already superseded it.

import { useEffect, useRef, useState } from "react";
import { imageMime } from "./imageFiles.ts";

export type ImageLoadState =
	| { kind: "loading" }
	| { kind: "error"; message: string }
	| { kind: "ready"; url: string; byteLength: number };

/** Fetches `load()`'s bytes and turns them into an object URL for
 *  `path`'s MIME type. Reloads whenever `loadKey` changes; a fetch that
 *  resolves after a later `loadKey` has already started a new one is
 *  discarded rather than clobbering the newer state. `message` on the
 *  error state is the raw rejection text (the same error strings
 *  `read_image_bytes` / `review_image_bytes` return) — callers format it
 *  for display via `parseImageError`. */
export function useImageBytes(
	load: () => Promise<ArrayBuffer>,
	path: string,
	loadKey: string,
): ImageLoadState {
	const [state, setState] = useState<ImageLoadState>({ kind: "loading" });
	const genRef = useRef(0);
	const urlRef = useRef<string | null>(null);

	// biome-ignore lint/correctness/useExhaustiveDependencies: `load`/`path` are re-created per render by the caller — `loadKey` is the deliberate trigger, matching what it's for.
	useEffect(() => {
		const gen = ++genRef.current;
		setState({ kind: "loading" });
		void load()
			.then((buf) => {
				if (genRef.current !== gen) return; // superseded by a later load
				const mime = imageMime(path) ?? "application/octet-stream";
				const url = URL.createObjectURL(new Blob([buf], { type: mime }));
				if (urlRef.current) URL.revokeObjectURL(urlRef.current);
				urlRef.current = url;
				setState({ kind: "ready", url, byteLength: buf.byteLength });
			})
			.catch((err: unknown) => {
				if (genRef.current !== gen) return;
				const msg = err instanceof Error ? err.message : String(err);
				setState({ kind: "error", message: msg });
			});
	}, [loadKey]);

	// Revoke whatever URL is live when the caller unmounts (a change is
	// already handled above, right before a new URL is minted).
	useEffect(
		() => () => {
			if (urlRef.current) URL.revokeObjectURL(urlRef.current);
			urlRef.current = null;
		},
		[],
	);

	return state;
}
