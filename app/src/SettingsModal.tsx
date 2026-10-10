// Settings modal — appearance + terminal preferences.
//
// Replaces the in-titlebar settings cluster (theme, density, UI scale,
// terminal font size). Triggered by the cog icon in the titlebar, by
// Mod+, anywhere in the app, and by macOS's Skein → Preferences… menu
// item (which emits skein://open-settings on the Rust side).
//
// The actual state lives in App.tsx via usePersistedState; this is a
// dumb component that takes values + setters.

import { useCallback, useEffect, useState } from "react";
import { CliShimPanel } from "./CliShimPanel.tsx";
import type { VersionNoticeMode } from "./claudeVersion.ts";
import { NudgesPanel } from "./NudgesPanel.tsx";
import type { DefaultAgents } from "./prefs.ts";
import { AboutSettings } from "./SettingsAbout.tsx";
import { AgentSettings } from "./SettingsAgents.tsx";
import { AppearanceSettings } from "./SettingsAppearance.tsx";
import { GlobalShortcutSettings } from "./SettingsGlobalShortcut.tsx";
import { NotificationSettings } from "./SettingsNotifications.tsx";
import { VersionNoticeSettings } from "./SettingsVersionNotice.tsx";
import { SpawnEnvPanel } from "./SpawnEnvPanel.tsx";
import type { Density, HarnessKind, SpawnSettings, Theme } from "./types.ts";
import { useFocusRestore } from "./useFocusRestore.ts";
import "./Settings.css";
import "./modal.css";
import "./SpawnEnvPanel.css";

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
	// #491: Claude Code update notice mode.
	versionNoticeMode: VersionNoticeMode;
	onVersionNoticeMode: (mode: VersionNoticeMode) => void;
	/** The folder the agent lists are asked about — the active room's.
	 *  Project agents differ per repo; user and plugin agents do not. */
	agentCwd: string;
	// #227: the app-wide worktree branch template New Room proposes for a
	// folder it has never seen. A folder's own remembered template
	// (`FolderDefaults.branchTemplate`) still wins.
	branchTemplate: string;
	onBranchTemplate: (v: string) => void;
	// #559: OS-wide shortcut that raises the Control Center.
	globalCcShortcut: boolean;
	onGlobalCcShortcut: (v: boolean) => void;
	globalCcShortcutFailed: boolean;
	onClose: () => void;
}

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
	versionNoticeMode,
	onVersionNoticeMode,
	agentCwd,
	branchTemplate,
	onBranchTemplate,
	globalCcShortcut,
	onGlobalCcShortcut,
	globalCcShortcutFailed,
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
					<AppearanceSettings
						theme={theme}
						density={density}
						fontSize={fontSize}
						fontMin={fontMin}
						fontMax={fontMax}
						chromeFontSize={chromeFontSize}
						chromeFontMin={chromeFontMin}
						chromeFontMax={chromeFontMax}
						onTheme={onTheme}
						onDensity={onDensity}
						onFontSize={onFontSize}
						onChromeFontSize={onChromeFontSize}
						copyOnSelect={copyOnSelect}
						onCopyOnSelect={onCopyOnSelect}
					/>

					<NotificationSettings
						notifyBadge={notifyBadge}
						notifyToast={notifyToast}
						notifyUrgent={notifyUrgent}
						notifyOs={notifyOs}
						onNotifyBadge={onNotifyBadge}
						onNotifyToast={onNotifyToast}
						onNotifyUrgent={onNotifyUrgent}
						onNotifyOs={onNotifyOs}
					/>

					<GlobalShortcutSettings
						enabled={globalCcShortcut}
						failed={globalCcShortcutFailed}
						onChange={onGlobalCcShortcut}
					/>

					<AgentSettings
						defaultAgents={defaultAgents}
						onDefaultAgent={onDefaultAgent}
						agentCwd={agentCwd}
						branchTemplate={branchTemplate}
						onBranchTemplate={onBranchTemplate}
					/>

					<VersionNoticeSettings mode={versionNoticeMode} onChange={onVersionNoticeMode} />

					<div className="sk-field">
						<label>Nudges</label>
						<NudgesPanel />
					</div>

					<div className="sk-field">
						<label>Command line</label>
						<CliShimPanel />
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

					<AboutSettings />
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
