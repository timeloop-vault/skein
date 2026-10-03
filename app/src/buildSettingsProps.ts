import type { ComponentProps } from "react";
import type { VersionNoticeMode } from "./claudeVersion.ts";
import { versionNoticeModeFor, withDefaultAgent, withVersionNoticeMode } from "./prefs.ts";
import type { SettingsModal } from "./SettingsModal.tsx";
import type { HarnessKind, SpawnSettingsPayload } from "./types.ts";
import {
	CHROME_FONT_MAX,
	CHROME_FONT_MIN,
	FONT_MAX,
	FONT_MIN,
	type useAppSettings,
} from "./useAppSettings.ts";

// The Settings modal's props, assembled from the app-settings state, the
// spawn-env mirror (null before it loads) and the two values that come
// from elsewhere in App.
export function buildSettingsProps(
	s: ReturnType<typeof useAppSettings>,
	spawnEnv: SpawnSettingsPayload | null,
	saveSpawnSettings: ComponentProps<typeof SettingsModal>["onSpawnSettings"],
	agentCwd: string,
	onClose: () => void,
): ComponentProps<typeof SettingsModal> {
	return {
		theme: s.theme,
		density: s.density,
		fontSize: s.fontSize,
		fontMin: FONT_MIN,
		fontMax: FONT_MAX,
		chromeFontSize: s.chromeFontPt,
		chromeFontMin: CHROME_FONT_MIN,
		chromeFontMax: CHROME_FONT_MAX,
		copyOnSelect: s.copyOnSelect,
		onCopyOnSelect: s.setCopyOnSelect,
		onTheme: s.setTheme,
		onDensity: s.setDensity,
		onFontSize: s.setFontSize,
		onChromeFontSize: s.setChromeFontPt,
		notifyBadge: s.notifyBadge,
		notifyToast: s.notifyToast,
		notifyUrgent: s.notifyUrgent,
		notifyOs: s.notifyOs,
		onNotifyBadge: s.setNotifyBadge,
		onNotifyToast: s.setNotifyToast,
		onNotifyUrgent: s.setNotifyUrgent,
		onNotifyOs: s.setNotifyOs,
		spawnSettings: spawnEnv?.settings ?? null,
		spawnDegraded: spawnEnv?.degraded ?? null,
		spawnSettingsPath: spawnEnv?.settingsPath ?? "",
		onSpawnSettings: saveSpawnSettings,
		defaultAgents: s.defaultAgents,
		onDefaultAgent: (kind: HarnessKind, agent: string | undefined) =>
			s.setDefaultAgents((prev) => withDefaultAgent(prev, kind, agent)),
		versionNoticeMode: versionNoticeModeFor(s.versionNoticeModes, "claude"),
		onVersionNoticeMode: (mode: VersionNoticeMode) =>
			s.setVersionNoticeModes((prev) => withVersionNoticeMode(prev, "claude", mode)),
		branchTemplate: s.branchTemplate,
		onBranchTemplate: s.setBranchTemplate,
		agentCwd,
		onClose,
	};
}
