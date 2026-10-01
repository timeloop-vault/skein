// ImageDiff — before/after image preview for the review pane (#409).
//
// Replaces the "{blocked} — no line diff to show" message for an image
// file: an image has no meaningful line diff, but it has an obvious
// before/after, so this renders that instead. Three modes over the same
// two loaded sides — side by side (default), swap (one frame, flips),
// onion (after faded over before) — because none of the three alone is
// right for every kind of change: side by side is best for a resize or
// a crop, swap for a subtle recolour, onion for pixel-level drift.
//
// Reuses `ImageFrame`/`useImageBytes` from the Files editor's image
// preview rather than reimplementing blob-URL handling a second time.

import { type SyntheticEvent, useEffect, useState } from "react";
import { ImageFrame } from "../ImageView.tsx";
import { describeImageError, formatBytes, parseImageError } from "../imageFiles.ts";
import { type ImageLoadState, useImageBytes } from "../imageLoad.ts";
import { type ReviewScope, type ImageDiffSide as Side, fetchImageBytes } from "./api.ts";
import { type SideStatus, modesAvailable, sidesForChange } from "./imageDiffModel.ts";
import "./ImageDiff.css";

type Mode = "side-by-side" | "swap" | "onion";

interface ImageDiffProps {
	roomId: string;
	cwd: string;
	scope: ReviewScope;
	commitSha: string | undefined;
	path: string;
	/** `ReviewFileDetail.change` — decides which side(s) can exist. */
	change: string;
	/** Bumped whenever the reviewed content actually changes; part of
	 *  the reload key alongside path/scope/commit. */
	contentHash: string;
}

const statusOf = (state: ImageLoadState): SideStatus => {
	if (state.kind === "loading") return "loading";
	if (state.kind === "ready") return "ready";
	return parseImageError(state.message).kind === "missing" ? "absent" : "error";
};

/// A side legitimately not existing at this revision reads as "not in
/// this version" rather than as a missing-file error — the common case
/// for an added/deleted image, not a failure.
const describeSideError = (msg: string): string =>
	parseImageError(msg).kind === "missing" ? "not in this version" : describeImageError(msg);

export const ImageDiff = ({
	roomId,
	cwd,
	scope,
	commitSha,
	path,
	change,
	contentHash,
}: ImageDiffProps) => {
	const sides = sidesForChange(change);
	const loadKey = `${scope}:${commitSha ?? ""}:${path}:${contentHash}`;

	const oldState = useImageBytes(
		() =>
			sides.old
				? fetchImageBytes(roomId, cwd, path, scope, "old", commitSha)
				: Promise.reject(new Error("missing")),
		path,
		loadKey,
	);
	const newState = useImageBytes(
		() =>
			sides.new
				? fetchImageBytes(roomId, cwd, path, scope, "new", commitSha)
				: Promise.reject(new Error("missing")),
		path,
		loadKey,
	);

	const oldStatus = statusOf(oldState);
	const newStatus = statusOf(newState);
	const modes = modesAvailable(oldStatus, newStatus);

	const [mode, setMode] = useState<Mode>("side-by-side");
	const [fit, setFit] = useState(true);
	const [swapShowing, setSwapShowing] = useState<Side>("new");
	const [onionOpacity, setOnionOpacity] = useState(50);

	// A different file (or a real change to this one) starts over: back
	// to side-by-side rather than carrying a mode that made sense for
	// the last image but not this one.
	// biome-ignore lint/correctness/useExhaustiveDependencies: `loadKey` is the deliberate trigger, matching what it's for.
	useEffect(() => {
		setMode("side-by-side");
		setSwapShowing("new");
		setOnionOpacity(50);
	}, [loadKey]);

	// A mode can also become unavailable mid-view — a side that was
	// loading resolves to an error — and there is no sense leaving the
	// pane on a mode whose controls just went inert.
	useEffect(() => {
		if (mode === "swap" && !modes.swap) setMode("side-by-side");
		if (mode === "onion" && !modes.onion) setMode("side-by-side");
	}, [mode, modes.swap, modes.onion]);

	const name = path.split(/[/\\]/).pop() ?? path;

	return (
		<div className="rv-imgdiff">
			<div className="rv-imgdiff-toolbar">
				<span className="rv-imgdiff-modes">
					<button
						type="button"
						className={`rv-btn${mode === "side-by-side" ? " primary" : ""}`}
						onClick={() => setMode("side-by-side")}
					>
						side by side
					</button>
					<button
						type="button"
						className={`rv-btn${mode === "swap" ? " primary" : ""}`}
						disabled={!modes.swap}
						title={modes.swap ? "flip between before and after" : "needs both sides loaded"}
						onClick={() => setMode("swap")}
					>
						swap
					</button>
					<button
						type="button"
						className={`rv-btn${mode === "onion" ? " primary" : ""}`}
						disabled={!modes.onion}
						title={modes.onion ? "fade between before and after" : "needs both sides loaded"}
						onClick={() => setMode("onion")}
					>
						onion
					</button>
				</span>
				<button type="button" className="rv-btn" onClick={() => setFit((f) => !f)}>
					{fit ? "Fit" : "1:1"}
				</button>
			</div>
			<div className="rv-imgdiff-body">
				{mode === "side-by-side" && (
					<div className="rv-imgdiff-pair">
						<ImageSidePanel
							key={`old:${loadKey}`}
							label="before"
							state={oldState}
							fit={fit}
							name={name}
						/>
						<ImageSidePanel
							key={`new:${loadKey}`}
							label="after"
							state={newState}
							fit={fit}
							name={name}
						/>
					</div>
				)}
				{mode === "swap" && (
					<SwapPanel
						key={loadKey}
						oldState={oldState}
						newState={newState}
						showing={swapShowing}
						onFlip={() => setSwapShowing((s) => (s === "old" ? "new" : "old"))}
						fit={fit}
						name={name}
					/>
				)}
				{mode === "onion" && oldState.kind === "ready" && newState.kind === "ready" && (
					<div className="rv-imgdiff-onion">
						<div className={`iv-frame ${fit ? "fit" : "actual"}`}>
							<img src={oldState.url} alt="" draggable={false} />
							<img
								src={newState.url}
								alt={name}
								draggable={false}
								style={{ opacity: onionOpacity / 100 }}
							/>
						</div>
						<input
							type="range"
							className="rv-imgdiff-onionslider"
							min={0}
							max={100}
							value={onionOpacity}
							onChange={(e) => setOnionOpacity(Number(e.target.value))}
							aria-label="onion blend — before to after"
						/>
					</div>
				)}
			</div>
		</div>
	);
};

/// One labelled frame of the side-by-side pair: dims + byte size when
/// loaded, a "not in this version" placeholder when the side is absent,
/// worded errors otherwise. Given a fresh `key` per load by the caller,
/// so its own dims/decode state never carries over from a previous file.
const ImageSidePanel = ({
	label,
	state,
	fit,
	name,
}: {
	label: string;
	state: ImageLoadState;
	fit: boolean;
	name: string;
}) => {
	const [natural, setNatural] = useState<{ w: number; h: number } | null>(null);
	const [decodeFailed, setDecodeFailed] = useState(false);
	const showImage = state.kind === "ready" && !decodeFailed;
	const errorMessage =
		state.kind === "error"
			? describeSideError(state.message)
			: decodeFailed
				? "can't display this image"
				: null;

	return (
		<div className="rv-imgdiff-side">
			<div className="rv-imgdiff-sidebar">
				<span className="rv-imgdiff-sidelabel">{label}</span>
				<span className="iv-dim">
					{natural ? (natural.w && natural.h ? `${natural.w} × ${natural.h}` : "—") : "—"}
				</span>
				<span className="iv-size">
					{state.kind === "ready" ? formatBytes(state.byteLength) : ""}
				</span>
			</div>
			<div className="rv-imgdiff-sidebody">
				{state.kind === "loading" && <div className="iv-status">loading…</div>}
				{errorMessage != null && <div className="iv-status">{errorMessage}</div>}
				{showImage && state.kind === "ready" && (
					<ImageFrame
						url={state.url}
						alt={`${name} (${label})`}
						fit={fit}
						onLoad={(e: SyntheticEvent<HTMLImageElement>) => {
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

/// One frame showing either side, flipped by a click on its own label
/// or the toggle. Only ever mounted once both sides are loaded (`modes`
/// gates entry into this mode), so there is no error/placeholder case
/// to render here beyond a decode failure.
const SwapPanel = ({
	oldState,
	newState,
	showing,
	onFlip,
	fit,
	name,
}: {
	oldState: ImageLoadState;
	newState: ImageLoadState;
	showing: Side;
	onFlip: () => void;
	fit: boolean;
	name: string;
}) => {
	const [decodeFailed, setDecodeFailed] = useState(false);
	// A decode failure belongs to whichever side produced it — flipping
	// to the other side must not carry it along.
	// biome-ignore lint/correctness/useExhaustiveDependencies: `showing` is the deliberate trigger — `setDecodeFailed` is stable and not a real dependency.
	useEffect(() => setDecodeFailed(false), [showing]);
	const state = showing === "old" ? oldState : newState;
	if (state.kind !== "ready") return null;
	return (
		<div className="rv-imgdiff-swap">
			<button type="button" className="rv-imgdiff-swaplabel" onClick={onFlip} title="click to flip">
				{showing === "old" ? "before" : "after"}
			</button>
			{decodeFailed ? (
				<div className="iv-status">can't display this image</div>
			) : (
				<ImageFrame
					url={state.url}
					alt={`${name} (${showing === "old" ? "before" : "after"})`}
					fit={fit}
					onError={() => setDecodeFailed(true)}
				/>
			)}
		</div>
	);
};
