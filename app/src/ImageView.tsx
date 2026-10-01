// ImageView — view-only image preview (#409), shared by the Files
// editor and (next) the review pane's diff view. Loads raw bytes via
// `load`, turns them into a `Blob` + object URL, and renders an `<img>`
// — never inline SVG markup into the DOM, since an SVG can carry a
// `<script>` and this pane has no sandboxing for that.

import { type SyntheticEvent, useEffect, useId, useState } from "react";
import { describeImageError, formatBytes } from "./imageFiles.ts";
import { useImageBytes } from "./imageLoad.ts";
import "./ImageView.css";

interface ImageViewProps {
	/** Fetches the raw bytes; rejects with the same error strings
	 *  `read_image_bytes` returns (see `parseImageError`). */
	load: () => Promise<ArrayBuffer>;
	/** Absolute path — used only to pick the MIME type and for the
	 *  `<img>` alt text. */
	path: string;
	/** Reload when this changes (e.g. path + an open counter). */
	loadKey: string;
}

/** Fit-to-pane, never upscaled past 1:1 vs. natural size in a
 *  scrollable area — both on a checkerboard so transparency shows. */
export const ImageFrame = ({
	url,
	alt,
	fit,
	onLoad,
	onError,
}: {
	url: string;
	alt: string;
	fit: boolean;
	onLoad?: (e: SyntheticEvent<HTMLImageElement>) => void;
	onError?: () => void;
}) => (
	<div className={`iv-frame ${fit ? "fit" : "actual"}`}>
		<img src={url} alt={alt} onLoad={onLoad} onError={onError} draggable={false} />
	</div>
);

export const ImageView = ({ load, path, loadKey }: ImageViewProps) => {
	const loadState = useImageBytes(load, path, loadKey);
	const [fit, setFit] = useState(true);
	const [natural, setNatural] = useState<{ w: number; h: number } | null>(null);
	// The bytes can load fine and still fail to decode as an image (a
	// truncated file, say) — that only shows up once the `<img>` itself
	// fires `onError`, which `useImageBytes` has no way to know about.
	const [decodeFailed, setDecodeFailed] = useState(false);
	const name = path.split(/[/\\]/).pop() ?? path;
	const titleId = useId();

	// A fresh load clears the previous image's dimensions and any decode
	// failure recorded against it — `useImageBytes` resets its own state
	// on the same trigger.
	// biome-ignore lint/correctness/useExhaustiveDependencies: `loadKey` is the deliberate trigger, matching what it's for.
	useEffect(() => {
		setNatural(null);
		setDecodeFailed(false);
	}, [loadKey]);

	const showImage = loadState.kind === "ready" && !decodeFailed;
	const errorMessage =
		loadState.kind === "error"
			? describeImageError(loadState.message)
			: decodeFailed
				? "can't display this image"
				: null;

	return (
		<div className="iv-view" aria-labelledby={titleId}>
			<div className="iv-toolbar" id={titleId}>
				<button
					type="button"
					className="sk-btn ghost"
					onClick={() => setFit((f) => !f)}
					disabled={!showImage}
				>
					{fit ? "Fit" : "1:1"}
				</button>
				<span className="iv-dim">
					{natural ? (natural.w && natural.h ? `${natural.w} × ${natural.h}` : "—") : "—"}
				</span>
				<span className="iv-size">
					{loadState.kind === "ready" ? formatBytes(loadState.byteLength) : ""}
				</span>
			</div>
			<div className="iv-body">
				{loadState.kind === "loading" && <div className="iv-status">loading…</div>}
				{errorMessage != null && <div className="iv-status error">{errorMessage}</div>}
				{showImage && loadState.kind === "ready" && (
					<ImageFrame
						url={loadState.url}
						alt={name}
						fit={fit}
						onLoad={(e) => {
							const img = e.currentTarget;
							setNatural({ w: img.naturalWidth, h: img.naturalHeight });
						}}
						onError={() => setDecodeFailed(true)}
					/>
				)}
			</div>
		</div>
	);
};
