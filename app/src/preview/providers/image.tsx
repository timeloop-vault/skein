// Image provider — renders raster + vector image formats via a
// data URL. Vector (svg) is also handled here so users get the
// rendered image by default; a "show source" toggle is a future
// improvement (issue #11 follow-up).

import { registerPreviewProvider } from "../registry.ts";

const MIME_BY_EXT: Record<string, string> = {
	".png": "image/png",
	".jpg": "image/jpeg",
	".jpeg": "image/jpeg",
	".gif": "image/gif",
	".webp": "image/webp",
	".bmp": "image/bmp",
	".ico": "image/x-icon",
	".svg": "image/svg+xml",
	".avif": "image/avif",
};

const mimeFor = (path: string): string | null => {
	const lower = path.toLowerCase();
	for (const [ext, mime] of Object.entries(MIME_BY_EXT)) {
		if (lower.endsWith(ext)) return mime;
	}
	return null;
};

registerPreviewProvider({
	id: "image",
	patterns: Object.keys(MIME_BY_EXT).map((ext) => `*${ext}`),
	priority: 100,
	needs: "bytes",
	render: ({ path, bytesBase64, truncated }) => {
		if (!bytesBase64) return null;
		const mime = mimeFor(path) ?? "application/octet-stream";
		const url = `data:${mime};base64,${bytesBase64}`;
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
				{truncated && (
					<div style={{ color: "var(--warn)", fontSize: 11, alignSelf: "stretch" }}>
						truncated — file larger than 5 MB; preview may be incomplete
					</div>
				)}
				<img
					src={url}
					alt={path}
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
	},
});
