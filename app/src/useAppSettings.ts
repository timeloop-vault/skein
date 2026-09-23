// App-wide settings/prefs state, extracted out of App.tsx (#19) — pure
// move, no behaviour change. Owns theme/density/font-size, the copy-
// on-select and notification toggles, per-turn cost display, default
// agents, the branch template, the right-pane-tab-per-room map, the
// harness column width, and the live-HEAD-branch-per-room map — plus
// the stable callbacks that wrap the latter three
// (handleToggleTurnCosts/setRightPaneTab/handleBranchChange). App.tsx
// calls this hook at the point this state used to be declared and
// destructures the return value the same way it does `useRoomsStore`/
// `useHarnessCreation`.
//
// NOT here: `permissionHarnessIds` (`usePermissionHarnessIds()`) —
// it sat textually between `notifyOs` and `showTurnCosts` in the old
// App.tsx but depends on none of this state, so it stays in App,
// called right after this hook.

import { useCallback, useState } from "react";
import type { RightPaneTab } from "./RightPane.tsx";
import { DEFAULT_BRANCH_TEMPLATE } from "./branchName.ts";
import { type DefaultAgents, usePersistedState } from "./prefs.ts";
import type { Density, Theme } from "./types.ts";

// xterm font size range. Outside this band the terminal looks either
// unreadable (sub-12) or comically large (above 18) on a 1320x820 window.
export const FONT_MIN = 12;
export const FONT_MAX = 18;
const FONT_DEFAULT = 13;
// #17: chrome font size — scales the UI chrome (tabs, cards, feed,
// status bar) via the --cfs variable, independent of the terminal font.
export const CHROME_FONT_MIN = 10;
export const CHROME_FONT_MAX = 20;
const CHROME_FONT_DEFAULT = 12;

export function useAppSettings() {
	const [theme, setTheme] = usePersistedState<Theme>("theme", "dark");
	const [density, setDensity] = usePersistedState<Density>("density", "regular");
	const [fontSize, setFontSize] = usePersistedState<number>("fontSize", FONT_DEFAULT);
	const [chromeFontPt, setChromeFontPt] = usePersistedState<number>(
		"chromeFontPt",
		CHROME_FONT_DEFAULT,
	);
	// #158: copy a mouse selection to the clipboard the moment it's made
	// (drag, Shift/Option+drag over an agent's TUI, double/triple-click),
	// on top of the explicit Ctrl+C/Cmd+C bindings. Default true on every
	// platform — most terminal apps do this — with a toggle in Settings.
	// Absent key (every install before this setting shipped) reads as
	// true via `usePersistedState`'s own initial-value fallback.
	const [copyOnSelect, setCopyOnSelect] = usePersistedState<boolean>("copyOnSelect", true);
	// L5e — per-surface notification toggles. Defaults: in-app on,
	// OS off (less surprising on first run; user opts in to OS
	// banners when they want them).
	const [notifyBadge, setNotifyBadge] = usePersistedState<boolean>("notifyBadge", true);
	const [notifyToast, setNotifyToast] = usePersistedState<boolean>("notifyToast", true);
	const [notifyUrgent, setNotifyUrgent] = usePersistedState<boolean>("notifyUrgent", true);
	const [notifyOs, setNotifyOs] = usePersistedState<boolean>("notifyOs", false);
	// Per-turn cost hair-lines in the Activity feed (issue #80 D2d-2).
	// Off by default; toggled from the Activity card head. App-owned so
	// every room's mounted LiveContext sees the same value.
	const [showTurnCosts, setShowTurnCosts] = usePersistedState<boolean>("showTurnCosts", false);
	// #248: default agent per harness kind, set in Settings. Read by the
	// `+ harness` picker (preselected) and New room (prefilled).
	const [defaultAgents, setDefaultAgents] = usePersistedState<DefaultAgents>("defaultAgents", {});
	// #227: the app-wide worktree branch template, set in Settings. A
	// folder's own remembered template still wins — see `branchTemplateFor`.
	const [branchTemplate, setBranchTemplate] = usePersistedState<string>(
		"branchTemplate",
		DEFAULT_BRANCH_TEMPLATE,
	);
	// Which right-pane tab each room is showing (#212). Per room rather
	// than global: a room mid-task wants the activity feed and a room
	// whose agent has just finished wants the review, and that is a
	// property of the room, not of the user. One map rather than a key
	// per room so App can flip the active room's tab from Mod+R.
	const [rightPaneTabs, setRightPaneTabs] = usePersistedState<Record<string, RightPaneTab>>(
		"rightPaneTabs",
		{},
	);
	// Width of the harness column in px. Right pane absorbs the remainder
	// via flex:1. Splitter clamps against window size at drag time.
	const [harnessColWidth, setHarnessColWidth] = usePersistedState<number>("harnessColWidth", 640);
	// Live HEAD branch per room, populated by LiveStatus on every watcher
	// tick. `room.branch` is the *creation* branch (worktree identity);
	// this is what's actually checked out right now. The status bar reads
	// from here first so a `git checkout` inside a harness is visible.
	// Issue #18.
	const [liveBranches, setLiveBranches] = useState<Record<string, string | null>>({});
	// Stable callbacks for the per-room LiveContext (memoized below): a
	// room switch re-renders App, and without stable props React.memo
	// can't skip the rooms whose `visible` didn't change — every mounted
	// room would reconcile its whole feed. setShowTurnCosts is stable;
	// onBranchChange takes the roomId so one callback serves all rooms.
	const handleToggleTurnCosts = useCallback(() => setShowTurnCosts((v) => !v), [setShowTurnCosts]);
	const setRightPaneTab = useCallback(
		(roomId: string, tab: RightPaneTab) => {
			setRightPaneTabs((prev) => (prev[roomId] === tab ? prev : { ...prev, [roomId]: tab }));
		},
		[setRightPaneTabs],
	);
	const handleBranchChange = useCallback((roomId: string, branch: string | null) => {
		setLiveBranches((prev) => (prev[roomId] === branch ? prev : { ...prev, [roomId]: branch }));
	}, []);

	return {
		theme,
		setTheme,
		density,
		setDensity,
		fontSize,
		setFontSize,
		chromeFontPt,
		setChromeFontPt,
		copyOnSelect,
		setCopyOnSelect,
		notifyBadge,
		setNotifyBadge,
		notifyToast,
		setNotifyToast,
		notifyUrgent,
		setNotifyUrgent,
		notifyOs,
		setNotifyOs,
		showTurnCosts,
		handleToggleTurnCosts,
		defaultAgents,
		setDefaultAgents,
		branchTemplate,
		setBranchTemplate,
		rightPaneTabs,
		setRightPaneTab,
		harnessColWidth,
		setHarnessColWidth,
		liveBranches,
		setLiveBranches,
		handleBranchChange,
	};
}
