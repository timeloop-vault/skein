// #527 — fit the terminal without clipping its last column.
//
// The row width comes from canvas measureText (charW). xterm's DOM renderer
// sets the default letter-spacing to `cell.width - W`, where W is measured in
// the DOM by xterm's WidthCache and quantized by Chromium to 1/64 px. Every
// cell therefore renders up to 1/64 px wider than the grid assumes, so a row of `cols`
// cells overruns by up to cols/64 px. LiveTerminal.css stops the row from
// clipping that overrun; this module makes sure it never reaches the
// scrollbar, by fitting one column narrower when it would.

import type { FitAddon } from "@xterm/addon-fit";
import type { Terminal } from "@xterm/xterm";

/** FitAddon's own minimum. */
const MIN_COLS = 2;
/** Worst-case per-cell drift from the 1/64-px quantization of W. */
const DRIFT_PER_CELL = 1 / 64;
/** FitAddon's fallback scrollbar width. */
const DEFAULT_SCROLLBAR = 14;

/**
 * The largest cols <= proposedCols whose text, drift included
 * (`cols * cellWidth + cols / 64`), still fits in availableWidth.
 */
export function clipSafeCols(
	proposedCols: number,
	cellWidth: number,
	availableWidth: number,
): number {
	let cols = proposedCols;
	while (cols > MIN_COLS && cols * (cellWidth + DRIFT_PER_CELL) > availableWidth) {
		cols -= 1;
	}
	return Math.max(MIN_COLS, Math.min(cols, proposedCols));
}

/** The slice of xterm's private core FitAddon itself reads. */
interface FitCore {
	_renderService?: {
		dimensions?: { css?: { cell?: { width?: number } } };
		clear?: () => void;
	};
}

/** Drop-in replacement for `fit.fit()` that is clip-safe. */
export function fitTerminal(term: Terminal, fit: FitAddon): void {
	const dims = fit.proposeDimensions();
	if (!dims || Number.isNaN(dims.cols) || Number.isNaN(dims.rows)) return;

	// Private field, read the same way FitAddon reads it.
	const core = (term as unknown as { _core?: FitCore })._core;
	const cellWidth = core?._renderService?.dimensions?.css?.cell?.width;
	const parent = term.element?.parentElement;
	let cols = dims.cols;
	if (cellWidth && cellWidth > 0 && term.element && parent) {
		// Deliberately mirrors @xterm/addon-fit 0.11.0's proposeDimensions
		// (app/node_modules/@xterm/addon-fit/src/FitAddon.ts); recheck this
		// when addon-fit is bumped.
		const parentWidth = Math.max(
			0,
			Number.parseInt(window.getComputedStyle(parent).getPropertyValue("width"), 10),
		);
		const style = window.getComputedStyle(term.element);
		const padding =
			Number.parseInt(style.getPropertyValue("padding-left"), 10) +
			Number.parseInt(style.getPropertyValue("padding-right"), 10);
		const scrollbar =
			term.options.scrollback === 0 ? 0 : term.options.overviewRuler?.width || DEFAULT_SCROLLBAR;
		const available = parentWidth - padding - scrollbar;
		if (Number.isFinite(available)) cols = clipSafeCols(cols, cellWidth, available);
	}

	if (term.rows !== dims.rows || term.cols !== cols) {
		core?._renderService?.clear?.();
		term.resize(cols, dims.rows);
	}
}
