// Settings modal — appearance + terminal preferences.
//
// Replaces the in-titlebar settings cluster (theme, density, UI scale,
// terminal font size). Triggered by the cog icon in the titlebar, by
// Mod+, anywhere in the app, and by macOS's Skein → Preferences… menu
// item (which emits skein://open-settings on the Rust side).
//
// The actual state lives in App.tsx via usePersistedState; this is a
// dumb component that takes values + setters.

import { useEffect } from "react";
import type { Density, Theme } from "./types.ts";

interface SettingsModalProps {
	theme: Theme;
	density: Density;
	fontSize: number;
	uiScale: number;
	fontMin: number;
	fontMax: number;
	uiScaleMin: number;
	uiScaleMax: number;
	uiScaleStep: number;
	onTheme: (v: Theme) => void;
	onDensity: (v: Density) => void;
	onFontSize: (v: number) => void;
	onUiScale: (v: number) => void;
	onClose: () => void;
}

const DENSITY_OPTIONS: { value: Density; label: string; desc: string }[] = [
	{ value: "compact", label: "Compact", desc: "Tightest. More on screen at once." },
	{ value: "regular", label: "Regular", desc: "Default. Balanced spacing." },
	{ value: "comfy", label: "Comfy", desc: "Roomier. Easier to scan." },
];

export const SettingsModal = ({
	theme,
	density,
	fontSize,
	uiScale,
	fontMin,
	fontMax,
	uiScaleMin,
	uiScaleMax,
	uiScaleStep,
	onTheme,
	onDensity,
	onFontSize,
	onUiScale,
	onClose,
}: SettingsModalProps) => {
	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (e.key === "Escape") {
				e.preventDefault();
				onClose();
			}
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [onClose]);

	const clampScale = (n: number): number =>
		Math.min(uiScaleMax, Math.max(uiScaleMin, Math.round(n * 100) / 100));

	return (
		<div className="sk-modal-bg" onClick={onClose}>
			<div className="sk-modal" onClick={(e) => e.stopPropagation()}>
				<div className="sk-modal-head">
					<h2>Settings</h2>
					<div className="sub">Appearance and terminal preferences. Persisted across restarts.</div>
				</div>
				<div className="sk-modal-body">
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
						<label>UI scale</label>
						<div className="sk-stepper">
							<button
								type="button"
								className="sk-btn ghost"
								onClick={() => onUiScale(clampScale(uiScale - uiScaleStep))}
								disabled={uiScale <= uiScaleMin + 0.001}
							>
								−
							</button>
							<span className="sk-stepper-value">{Math.round(uiScale * 100)}%</span>
							<button
								type="button"
								className="sk-btn ghost"
								onClick={() => onUiScale(clampScale(uiScale + uiScaleStep))}
								disabled={uiScale >= uiScaleMax - 0.001}
							>
								+
							</button>
							<span className="sk-stepper-hint">
								Chrome only — terminal stays at the size below.
							</span>
						</div>
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
				</div>
				<div className="sk-modal-foot">
					<button type="button" className="sk-btn" onClick={onClose}>
						Close
					</button>
				</div>
			</div>
		</div>
	);
};
