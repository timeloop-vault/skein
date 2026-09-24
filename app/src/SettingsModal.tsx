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
import { invoke } from "@tauri-apps/api/core";
import { check } from "@tauri-apps/plugin-updater";
import { useCallback, useEffect, useState } from "react";
import { NudgesPanel } from "./NudgesPanel.tsx";
import { SpawnEnvPanel } from "./SpawnEnvPanel.tsx";
import { kindHasAgents } from "./agents.ts";
import { HChip, NO_REVIEW_TOOLS_TITLE, useAgentListing } from "./components.tsx";
import { HARNESS_KINDS, HARNESS_ORDER } from "./data.tsx";
import { type DefaultAgents, defaultAgentFor } from "./prefs.ts";
import { isMac } from "./shortcuts.ts";
import type { Density, HarnessKind, SpawnSettings, Theme } from "./types.ts";
import { useFocusRestore } from "./useFocusRestore.ts";

type UpdateState =
	| { status: "idle" }
	| { status: "checking" }
	| { status: "current" }
	| { status: "available"; version: string; notes: string | undefined }
	| { status: "downloading"; downloaded: number; total: number | undefined }
	| { status: "ready" }
	| { status: "error"; message: string };

/// Mirror of `agent_api::state::AgentApiStatus` (#213).
interface AgentApiStatus {
	/** The bound port, or null when the listener never came up. */
	port: number | null;
	error: string | null;
}

interface SettingsModalProps {
	theme: Theme;
	density: Density;
	fontSize: number;
	fontMin: number;
	fontMax: number;
	// #17: chrome font size — separate lever from the terminal font.
	chromeFontSize: number;
	chromeFontMin: number;
	chromeFontMax: number;
	onTheme: (v: Theme) => void;
	onDensity: (v: Density) => void;
	onFontSize: (v: number) => void;
	onChromeFontSize: (v: number) => void;
	// #158: copy a mouse selection to the clipboard as soon as it's made.
	// Default true — see App.tsx's `copyOnSelect` state for the rest.
	copyOnSelect: boolean;
	onCopyOnSelect: (v: boolean) => void;
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
	// Shell / PATH environment (#72, #3, #1). These come from Rust, not
	// localStorage — the spawn path reads them and the shell probe runs
	// before a webview exists. `null` while the first load is in flight.
	spawnSettings: SpawnSettings | null;
	spawnDegraded: string | null;
	spawnSettingsPath: string;
	onSpawnSettings: (next: SpawnSettings) => Promise<void>;
	// #248: default agent per kind. localStorage (see prefs.ts), because
	// the frontend builds the argv the agent goes into.
	defaultAgents: DefaultAgents;
	onDefaultAgent: (kind: HarnessKind, agent: string | undefined) => void;
	/** The folder the agent lists are asked about — the active room's.
	 *  Project agents differ per repo; user and plugin agents do not. */
	agentCwd: string;
	// #227: the app-wide worktree branch template New Room proposes for a
	// folder it has never seen. A folder's own remembered template
	// (`FolderDefaults.branchTemplate`) still wins.
	branchTemplate: string;
	onBranchTemplate: (v: string) => void;
	onClose: () => void;
}

/** One kind's default-agent field. Its own component so each kind runs
 *  its own `useAgentListing` probe. */
const DefaultAgentField = ({
	kind,
	cwd,
	value,
	onChange,
}: {
	kind: HarnessKind;
	cwd: string;
	value: string | undefined;
	onChange: (agent: string | undefined) => void;
}) => {
	const listing = useAgentListing(kind, cwd);
	const listed = listing?.agents.some((a) => a.name === value) ?? false;
	// Same rule as New room: only an authoritative list proves a name gone.
	const missing = value !== undefined && listing !== null && listing.degraded === null && !listed;
	const picked = value ? listing?.agents.find((a) => a.name === value) : undefined;
	const note = missing
		? {
				cls: "err",
				text: `${HARNESS_KINDS[kind].name} does not offer "${value}" in this folder. New harnesses fall back to the tool default until you pick another.`,
			}
		: picked && !picked.allowsReviewTools
			? { cls: "warn", text: NO_REVIEW_TOOLS_TITLE }
			: listing?.degraded
				? { cls: "warn", text: `This list may be incomplete — ${listing.degraded}` }
				: null;
	return (
		<div className="sk-default-agent">
			<label htmlFor={`sk-default-agent-${kind}`}>
				<HChip kind={kind} /> {HARNESS_KINDS[kind].name}
			</label>
			<select
				id={`sk-default-agent-${kind}`}
				className="sk-select"
				value={value ?? ""}
				onChange={(e) => onChange(e.target.value || undefined)}
			>
				<option value="">(tool default) — no agent named</option>
				{/* The stored name always renders, even before the list lands
				    or after it stops being offered, so the field shows what it
				    is set to instead of sliding to (tool default). */}
				{value !== undefined && !listed && (
					<option value={value}>
						{value}
						{missing ? " — not found" : ""}
					</option>
				)}
				{(listing?.agents ?? []).map((a) => (
					<option key={a.name} value={a.name}>
						{a.name}
						{a.allowsReviewTools ? "" : "  ⚠ no review tools"}
					</option>
				))}
			</select>
			{note && <div className={`sk-default-agent-note ${note.cls}`}>{note.text}</div>}
		</div>
	);
};

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
	chromeFontSize,
	chromeFontMin,
	chromeFontMax,
	onTheme,
	onDensity,
	onFontSize,
	onChromeFontSize,
	copyOnSelect,
	onCopyOnSelect,
	notifyBadge,
	notifyToast,
	notifyUrgent,
	notifyOs,
	onNotifyBadge,
	onNotifyToast,
	onNotifyUrgent,
	onNotifyOs,
	spawnSettings,
	spawnDegraded,
	spawnSettingsPath,
	onSpawnSettings,
	defaultAgents,
	onDefaultAgent,
	agentCwd,
	branchTemplate,
	onBranchTemplate,
	onClose,
}: SettingsModalProps) => {
	useFocusRestore();

	// The environment panel is the only save-required form in Settings —
	// every other control applies on change, so dismissing the modal used
	// to be lossless. Escape and backdrop-click would otherwise discard a
	// half-typed PATH with no warning. First attempt warns; a second one
	// discards, so the modal never becomes a trap.
	const [envDirty, setEnvDirty] = useState(false);
	const [envWarned, setEnvWarned] = useState(false);
	useEffect(() => {
		if (!envDirty) setEnvWarned(false);
	}, [envDirty]);
	const closeGuarded = useCallback(() => {
		if (envDirty && !envWarned) {
			setEnvWarned(true);
			return;
		}
		onClose();
	}, [envDirty, envWarned, onClose]);

	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (e.key === "Escape") {
				e.preventDefault();
				closeGuarded();
			}
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [closeGuarded]);

	// Updater UI: pulled into the modal so the user can check + install
	// updates from a discoverable surface. Tauri's updater plugin
	// handles the cryptographic verification (against the pubkey in
	// tauri.conf.json) and writes the new bundle in place.
	const [version, setVersion] = useState<string>("");
	useEffect(() => {
		void getVersion().then(setVersion);
	}, []);

	// #213: whether the agent-facing review API came up. Shown because
	// the alternative is an agent whose review tools silently do not
	// exist, which looks like a broken harness rather than a bound port
	// that never bound (#176).
	const [agentApi, setAgentApi] = useState<AgentApiStatus | undefined>(undefined);
	useEffect(() => {
		void invoke<AgentApiStatus>("agent_api_status")
			.then(setAgentApi)
			.catch((err: unknown) => {
				setAgentApi({ port: null, error: err instanceof Error ? err.message : String(err) });
			});
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
		<div className="sk-modal-bg" onClick={closeGuarded}>
			<div className="sk-modal" onClick={(e) => e.stopPropagation()}>
				<div className="sk-modal-head">
					<h2>Settings</h2>
					<div className="sub">
						Appearance, environment and terminal preferences. Persisted across restarts.
					</div>
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
										Finishing a mouse selection in a terminal copies it immediately — no Ctrl+C
										needed. {isMac ? "Option+drag" : "Shift+drag"} still forces a selection over an
										agent's TUI when it's capturing the mouse.
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
						<label>Default agents</label>
						<div className="sk-help">
							The agent a new harness of each kind starts as. The <code>+ harness</code> picker
							highlights it, so Enter takes it, and New room fills it in unless the folder remembers
							its own. (tool default) names no agent and leaves it to the tool's own setting.
						</div>
						{HARNESS_ORDER.filter(kindHasAgents).map((kind) => (
							<DefaultAgentField
								key={kind}
								kind={kind}
								cwd={agentCwd}
								value={defaultAgentFor(defaultAgents, kind)}
								onChange={(agent) => onDefaultAgent(kind, agent)}
							/>
						))}
					</div>

					<div className="sk-field">
						<label htmlFor="sk-branch-template">Branch template</label>
						<div className="sk-help">
							{"{slug}"} is the task; a folder remembers the last prefix you used.
						</div>
						<input
							id="sk-branch-template"
							className="sk-input"
							value={branchTemplate}
							onChange={(e) => onBranchTemplate(e.target.value)}
						/>
					</div>

					<div className="sk-field">
						<label>Nudges</label>
						<NudgesPanel />
					</div>

					<div className="sk-field">
						<label>Shell &amp; environment</label>
						{envWarned && (
							<div className="sk-env-banner sk-env-warn">
								You have unsaved environment changes. Save or revert them, or press Escape again to
								discard.
							</div>
						)}
						<SpawnEnvPanel
							settings={spawnSettings}
							degraded={spawnDegraded}
							settingsPath={spawnSettingsPath}
							onSave={onSpawnSettings}
							onDirtyChange={setEnvDirty}
						/>
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
							{agentApi &&
								(agentApi.port != null ? (
									<div className="sk-update-msg">
										Agent review API on 127.0.0.1:{agentApi.port} — harnesses get{" "}
										<code>SKEIN_REVIEW_URL</code> and <code>SKEIN_REVIEW_TOKEN</code>.
									</div>
								) : (
									<div className="sk-update-msg err">
										Agent review API is down: {agentApi.error ?? "unknown reason"}. Agents in this
										session cannot read review comments.
									</div>
								))}
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
