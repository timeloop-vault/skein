// Settings modal — appearance + terminal preferences.
//
// Replaces the in-titlebar settings cluster (theme, density, UI scale,
// terminal font size). Triggered by the cog icon in the titlebar, by
// Mod+, anywhere in the app, and by macOS's Skein → Preferences… menu
// item (which emits skein://open-settings on the Rust side).
//
// The actual state lives in App.tsx via usePersistedState; this is a
// dumb component that takes values + setters.

import { getVersion } from "@tauri-apps/api/app";
import { check } from "@tauri-apps/plugin-updater";
import { useCallback, useEffect, useState } from "react";
import type { Density, Theme } from "./types.ts";

type UpdateState =
	| { status: "idle" }
	| { status: "checking" }
	| { status: "current" }
	| { status: "available"; version: string; notes: string | undefined }
	| { status: "downloading"; downloaded: number; total: number | undefined }
	| { status: "ready" }
	| { status: "error"; message: string };

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

	// Updater UI: pulled into the modal so the user can check + install
	// updates from a discoverable surface. Tauri's updater plugin
	// handles the cryptographic verification (against the pubkey in
	// tauri.conf.json) and writes the new bundle in place.
	const [version, setVersion] = useState<string>("");
	useEffect(() => {
		void getVersion().then(setVersion);
	}, []);

	const [update, setUpdate] = useState<UpdateState>({ status: "idle" });
	const checkForUpdate = useCallback(async () => {
		setUpdate({ status: "checking" });
		try {
			const found = await check();
			if (!found) {
				setUpdate({ status: "current" });
				return;
			}
			setUpdate({ status: "available", version: found.version, notes: found.body });
		} catch (err: unknown) {
			const message = err instanceof Error ? err.message : String(err);
			setUpdate({ status: "error", message });
		}
	}, []);

	const downloadAndInstall = useCallback(async () => {
		setUpdate({ status: "downloading", downloaded: 0, total: undefined });
		try {
			const found = await check();
			if (!found) {
				setUpdate({ status: "current" });
				return;
			}
			let downloaded = 0;
			let total: number | undefined;
			await found.downloadAndInstall((event) => {
				if (event.event === "Started") {
					total = event.data.contentLength;
					setUpdate({ status: "downloading", downloaded: 0, total });
				} else if (event.event === "Progress") {
					downloaded += event.data.chunkLength;
					setUpdate({ status: "downloading", downloaded, total });
				} else if (event.event === "Finished") {
					setUpdate({ status: "ready" });
				}
			});
		} catch (err: unknown) {
			const message = err instanceof Error ? err.message : String(err);
			setUpdate({ status: "error", message });
		}
	}, []);

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

					<div className="sk-field">
						<label>About</label>
						<div className="sk-update">
							<div className="sk-update-row">
								<span className="sk-update-version">Skein {version || "—"}</span>
								<button
									type="button"
									className="sk-btn"
									onClick={checkForUpdate}
									disabled={update.status === "checking" || update.status === "downloading"}
								>
									{update.status === "checking" ? "Checking…" : "Check for updates"}
								</button>
							</div>
							{update.status === "current" && (
								<div className="sk-update-msg ok">You're on the latest version.</div>
							)}
							{update.status === "available" && (
								<div className="sk-update-msg">
									<div>
										Update available: <strong>{update.version}</strong>
									</div>
									{update.notes && <pre className="sk-update-notes">{update.notes}</pre>}
									<button
										type="button"
										className="sk-btn primary"
										onClick={downloadAndInstall}
										style={{ marginTop: 6 }}
									>
										Download & install
									</button>
								</div>
							)}
							{update.status === "downloading" && (
								<div className="sk-update-msg">
									Downloading…{" "}
									{update.total
										? `${Math.round((update.downloaded / update.total) * 100)}%`
										: `${(update.downloaded / 1024).toFixed(0)} KB`}
								</div>
							)}
							{update.status === "ready" && (
								<div className="sk-update-msg ok">
									Update installed. Restart Skein to use the new version.
								</div>
							)}
							{update.status === "error" && (
								<div className="sk-update-msg err">Update failed: {update.message}</div>
							)}
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
