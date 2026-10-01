// Settings → appearance and terminal controls: theme, density, the two
// font sizes and copy-on-select. All apply on change.

import { isMac } from "./shortcuts.ts";
import type { Density, Theme } from "./types.ts";
import "./Settings.css";

const DENSITY_OPTIONS: { value: Density; label: string; desc: string }[] = [
	{ value: "compact", label: "Compact", desc: "Tightest. More on screen at once." },
	{ value: "regular", label: "Regular", desc: "Default. Balanced spacing." },
	{ value: "comfy", label: "Comfy", desc: "Roomier. Easier to scan." },
];

export const AppearanceSettings = ({
	theme,
	density,
	fontSize,
	fontMin,
	fontMax,
	chromeFontSize,
	chromeFontMin,
	chromeFontMax,
	onTheme,
	onDensity,
	onFontSize,
	onChromeFontSize,
	copyOnSelect,
	onCopyOnSelect,
}: {
	theme: Theme;
	density: Density;
	fontSize: number;
	fontMin: number;
	fontMax: number;
	chromeFontSize: number;
	chromeFontMin: number;
	chromeFontMax: number;
	onTheme: (v: Theme) => void;
	onDensity: (v: Density) => void;
	onFontSize: (v: number) => void;
	onChromeFontSize: (v: number) => void;
	copyOnSelect: boolean;
	onCopyOnSelect: (v: boolean) => void;
}) => (
	<>
		<div className="sk-field">
			<label>Theme</label>
			<div className="sk-radio-row">
				<button
					type="button"
					className={`sk-radio-card ${theme === "dark" ? "selected" : ""}`}
					onClick={() => onTheme("dark")}
				>
					<div className="top">Dark</div>
					<div className="desc">Default. Easier on the eyes.</div>
				</button>
				<button
					type="button"
					className={`sk-radio-card ${theme === "light" ? "selected" : ""}`}
					onClick={() => onTheme("light")}
				>
					<div className="top">Light</div>
					<div className="desc">High-contrast for daylight work.</div>
				</button>
			</div>
		</div>

		<div className="sk-field">
			<label htmlFor="sk-density">Density</label>
			<select
				id="sk-density"
				className="sk-select"
				value={density}
				onChange={(e) => onDensity(e.target.value as Density)}
			>
				{DENSITY_OPTIONS.map((opt) => (
					<option key={opt.value} value={opt.value}>
						{opt.label} — {opt.desc}
					</option>
				))}
			</select>
		</div>

		<div className="sk-field">
			<label>Terminal font size</label>
			<div className="sk-stepper">
				<button
					type="button"
					className="sk-btn ghost"
					onClick={() => onFontSize(Math.max(fontMin, fontSize - 1))}
					disabled={fontSize <= fontMin}
				>
					−
				</button>
				<span className="sk-stepper-value">{fontSize} pt</span>
				<button
					type="button"
					className="sk-btn ghost"
					onClick={() => onFontSize(Math.min(fontMax, fontSize + 1))}
					disabled={fontSize >= fontMax}
				>
					+
				</button>
			</div>
		</div>

		<div className="sk-field">
			<div className="sk-toggles">
				<label className="sk-toggle">
					<input
						type="checkbox"
						checked={copyOnSelect}
						onChange={(e) => onCopyOnSelect(e.target.checked)}
					/>
					<span className="sk-toggle-label">
						<span className="sk-toggle-title">Copy on select</span>
						<span className="sk-toggle-sub">
							Finishing a mouse selection in a terminal copies it immediately — no Ctrl+C needed.{" "}
							{isMac ? "Option+drag" : "Shift+drag"} still forces a selection over an agent's TUI
							when it's capturing the mouse.
						</span>
					</span>
				</label>
			</div>
		</div>

		<div className="sk-field">
			<label>Chrome font size</label>
			<div className="sk-help">
				UI text — tabs, cards, the activity feed, the status bar. The terminal is unaffected.
			</div>
			<div className="sk-stepper">
				<button
					type="button"
					className="sk-btn ghost"
					onClick={() => onChromeFontSize(Math.max(chromeFontMin, chromeFontSize - 1))}
					disabled={chromeFontSize <= chromeFontMin}
				>
					−
				</button>
				<span className="sk-stepper-value">{chromeFontSize} pt</span>
				<button
					type="button"
					className="sk-btn ghost"
					onClick={() => onChromeFontSize(Math.min(chromeFontMax, chromeFontSize + 1))}
					disabled={chromeFontSize >= chromeFontMax}
				>
					+
				</button>
			</div>
		</div>
	</>
);
