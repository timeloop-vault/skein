// Splitter — flex container with a draggable divider between two panes.
//
// `direction="row"` lays panes out side-by-side; the divider drags
// horizontally. `direction="column"` stacks them; the divider drags
// vertically. `size` is the pixel size of the *first* pane (width for
// row, height for column); the second pane absorbs the remainder via
// flex: 1. The first pane is rendered clamped to the container
// (splitterClamp.ts) so a saved size wider than the window never pushes the
// divider off-screen; double-clicking the divider resets to `defaultSize`.

import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import "./Splitter.css";
import { clampSplit } from "./splitterClamp.ts";

// Divider thickness in px; keep in sync with .sk-splitter-x/-y in Splitter.css.
const DIVIDER_PX = 4;

// Set on <body> for the length of a drag; Splitter.css turns iframe pointer
// events off under it so the drag keeps following the pointer.
const DRAGGING_CLASS = "sk-dragging";

interface SplitterProps {
	direction: "row" | "column";
	size: number;
	defaultSize?: number;
	onResize: (next: number) => void;
	minFirst: number;
	minSecond: number;
	first: ReactNode;
	second: ReactNode;
	className?: string;
}

export const Splitter = ({
	direction,
	size,
	defaultSize,
	onResize,
	minFirst,
	minSecond,
	first,
	second,
	className,
}: SplitterProps) => {
	const containerRef = useRef<HTMLDivElement>(null);
	const isRow = direction === "row";
	const [containerSize, setContainerSize] = useState(0);
	// Layout effect: the first paint already uses the measured size.
	useLayoutEffect(() => {
		const el = containerRef.current;
		if (!el) return;
		const measure = () => setContainerSize(isRow ? el.clientWidth : el.clientHeight);
		measure();
		const ro = new ResizeObserver(measure);
		ro.observe(el);
		return () => ro.disconnect();
	}, [isRow]);
	// Render-only clamp: onResize never gets the clamped value, so the saved
	// preference survives a transient small window.
	const rendered = clampSplit(size, containerSize, minFirst, minSecond, DIVIDER_PX);
	// Ends a drag in flight if the splitter unmounts mid-drag.
	const stopUnmount = useRef<(() => void) | null>(null);
	useEffect(() => () => stopUnmount.current?.(), []);

	const onMouseDown = (e: React.MouseEvent) => {
		e.preventDefault();
		const container = containerRef.current;
		if (!container) return;
		const startCoord = isRow ? e.clientX : e.clientY;
		// Start from the rendered (clamped) size so the divider doesn't jump.
		const startSize = rendered;
		// Read container size at drag start so the max bound respects
		// the current window dimensions; the user may have resized.
		const liveSize = isRow ? container.clientWidth : container.clientHeight;
		const onMove = (m: MouseEvent) => {
			const delta = (isRow ? m.clientX : m.clientY) - startCoord;
			onResize(clampSplit(startSize + delta, liveSize, minFirst, minSecond, DIVIDER_PX));
		};
		const onUp = () => {
			window.removeEventListener("mousemove", onMove);
			window.removeEventListener("mouseup", onUp);
			window.removeEventListener("blur", onUp);
			stopUnmount.current = null;
			document.body.style.cursor = "";
			document.body.style.userSelect = "";
			document.body.classList.remove(DRAGGING_CLASS);
		};
		document.body.style.cursor = isRow ? "col-resize" : "row-resize";
		document.body.style.userSelect = "none";
		// An iframe under the pointer swallows mousemove/mouseup.
		document.body.classList.add(DRAGGING_CLASS);
		window.addEventListener("mousemove", onMove);
		window.addEventListener("mouseup", onUp);
		window.addEventListener("blur", onUp);
		stopUnmount.current = onUp;
	};

	const sizeKey = isRow ? "width" : "height";

	return (
		<div
			ref={containerRef}
			className={className}
			style={{
				flex: 1,
				display: "flex",
				flexDirection: direction,
				minWidth: 0,
				minHeight: 0,
			}}
		>
			<div
				style={{
					[sizeKey]: rendered,
					flex: "0 0 auto",
					display: "flex",
					flexDirection: "column",
					minWidth: 0,
					minHeight: 0,
					overflow: "hidden",
				}}
			>
				{first}
			</div>
			<div
				className={isRow ? "sk-splitter-x" : "sk-splitter-y"}
				onMouseDown={onMouseDown}
				onDoubleClick={defaultSize === undefined ? undefined : () => onResize(defaultSize)}
			/>
			<div
				style={{
					flex: 1,
					display: "flex",
					flexDirection: "column",
					minWidth: 0,
					minHeight: 0,
					overflow: "hidden",
				}}
			>
				{second}
			</div>
		</div>
	);
};
