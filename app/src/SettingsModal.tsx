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
import { useFocusRestore } from "./useFocusRestore.ts";

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
	fontMin: number;
	fontMax: number;
	onTheme: (v: Theme) => void;
	onDensity: (v: Density) => void;
	onFontSize: (v: number) => void;
	// Notification toggles (#12 L5e). Each controls one surface
	// independently; defaults are in App.tsx (in-app on, OS off).
	notifyBadge: boolean;
	notifyToast: boolean;
	notifyUrgent: boolean;
	notifyOs: boolean;
	onNotifyBadge: (v: boolean) => void;
	onNotifyToast: (v: boolean) => void;
	onNotifyUrgent: (v: boolean) => void;
	onNotifyOs: (v: boolean) => void;
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
	fontMin,
	fontMax,
	onTheme,
	onDensity,
	onFontSize,
	notifyBadge,
	notifyToast,
	notifyUrgent,
	notifyOs,
	onNotifyBadge,
	onNotifyToast,
	onNotifyUrgent,
	onNotifyOs,
	onClose,
}: SettingsModalProps) => {
	useFocusRestore();

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
						<label>Notifications</label>
						<div className="sk-help">
							When an inactive harness goes idle or exits, Skein can surface that several ways. Each
							is independent — leave on what helps and turn off what distracts. Defaults: in-app on,
							OS notification off until you opt in.
						</div>
						<div className="sk-toggles">
							<label className="sk-toggle">
								<input
									type="checkbox"
									checked={notifyBadge}
									onChange={(e) => onNotifyBadge(e.target.checked)}
								/>
								<span className="sk-toggle-label">
									<span className="sk-toggle-title">Tab badges</span>
									<span className="sk-toggle-sub">
										Count on the room tab. Passive; only visible if you look up.
									</span>
								</span>
							</label>
							<label className="sk-toggle">
								<input
									type="checkbox"
									checked={notifyToast}
									onChange={(e) => onNotifyToast(e.target.checked)}
								/>
								<span className="sk-toggle-label">
									<span className="sk-toggle-title">Toast pop-up</span>
									<span className="sk-toggle-sub">
										Bottom-right card that slides in for a few seconds. Click to jump.
									</span>
								</span>
							</label>
							<label className="sk-toggle">
								<input
									type="checkbox"
									checked={notifyUrgent}
									onChange={(e) => onNotifyUrgent(e.target.checked)}
								/>
								<span className="sk-toggle-label">
									<span className="sk-toggle-title">Status-bar urgent indicator</span>
									<span className="sk-toggle-sub">
										Persistent indicator in the bottom bar pointing to whichever room has the most
										unattended events.
									</span>
								</span>
							</label>
							<label className="sk-toggle">
								<input
									type="checkbox"
									checked={notifyOs}
									onChange={(e) => onNotifyOs(e.target.checked)}
								/>
								<span className="sk-toggle-label">
									<span className="sk-toggle-title">OS notification</span>
									<span className="sk-toggle-sub">
										System banner when Skein isn't focused (you've alt+tabbed away). First enable
										here triggers the macOS permission prompt.
									</span>
								</span>
							</label>
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
