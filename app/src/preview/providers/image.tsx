// Image provider — renders raster + vector image formats. Streams
// over the tauri `asset:` protocol (`convertFileSrc`) instead of the
// old base64 IPC round-trip, so there is no size cap and no #46-style
// breakage on large images. The Rust side grants the protocol scope
// per room cwd when rooms load/save (see `sync_asset_scope`).
//
// Vector (svg) is also handled here so users get the rendered image
// by default; a "show source" toggle is a future improvement.

import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { registerPreviewProvider } from "../registry.ts";

const IMAGE_EXTS = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico", ".svg", ".avif"];

const ImagePreview = ({ path }: { path: string }) => {
	const [failed, setFailed] = useState(false);
	// A new file resets the error state (the component is reused when
	// the user clicks from one image to the next).
	useEffect(() => setFailed(false), []);
	if (failed) {
		return (
			<div style={{ color: "var(--fg-3)" }}>
				image failed to load — if the room was just created, close and reopen the browser
			</div>
		);
	}
	return (
		<div
			style={{
				display: "flex",
				flexDirection: "column",
				alignItems: "center",
				gap: 8,
				padding: 12,
			}}
		>
			<img
				src={convertFileSrc(path)}
				alt={path}
				onError={() => setFailed(true)}
				style={{
					maxWidth: "100%",
					maxHeight: "100%",
					objectFit: "contain",
					background:
						"repeating-conic-gradient(var(--bg-2) 0% 25%, var(--bg-1) 0% 50%) 50% / 16px 16px",
					borderRadius: 4,
				}}
			/>
		</div>
	);
};

registerPreviewProvider({
	id: "image",
	patterns: IMAGE_EXTS.map((ext) => `*${ext}`),
	priority: 100,
	// "path" — no IPC read; the asset protocol streams the file itself.
	needs: "path",
	render: ({ path }) => <ImagePreview key={path} path={path} />,
});
