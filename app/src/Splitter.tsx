// Splitter — flex container with a draggable divider between two panes.
//
// `direction="row"` lays panes out side-by-side; the divider drags
// horizontally. `direction="column"` stacks them; the divider drags
// vertically. `size` is the pixel size of the *first* pane (width for
// row, height for column); the second pane absorbs the remainder via
// flex: 1.

import { type ReactNode, useRef } from "react";

interface SplitterProps {
	direction: "row" | "column";
	size: number;
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
	onResize,
	minFirst,
	minSecond,
	first,
	second,
	className,
}: SplitterProps) => {
	const containerRef = useRef<HTMLDivElement>(null);
	const isRow = direction === "row";

	const onMouseDown = (e: React.MouseEvent) => {
		e.preventDefault();
		const container = containerRef.current;
		if (!container) return;
		const startCoord = isRow ? e.clientX : e.clientY;
		const startSize = size;
		// Read container size at drag start so the max bound respects
		// the current window dimensions; the user may have resized.
		const containerSize = isRow ? container.clientWidth : container.clientHeight;
		const max = Math.max(minFirst, containerSize - minSecond);
		const onMove = (m: MouseEvent) => {
			const delta = (isRow ? m.clientX : m.clientY) - startCoord;
			const next = Math.min(max, Math.max(minFirst, startSize + delta));
			onResize(next);
		};
		const onUp = () => {
			window.removeEventListener("mousemove", onMove);
			window.removeEventListener("mouseup", onUp);
			document.body.style.cursor = "";
			document.body.style.userSelect = "";
		};
		document.body.style.cursor = isRow ? "col-resize" : "row-resize";
		document.body.style.userSelect = "none";
		window.addEventListener("mousemove", onMove);
		window.addEventListener("mouseup", onUp);
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
					[sizeKey]: size,
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
			<div className={isRow ? "sk-splitter-x" : "sk-splitter-y"} onMouseDown={onMouseDown} />
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
