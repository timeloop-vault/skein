import { getCurrentWindow } from "@tauri-apps/api/window";
import { hints, isMac, modLabel } from "./shortcuts.ts";

// ── Empty state ────────────────────────────────────────────────────

interface EmptyStateProps {
	onNew: () => void;
	archivedCount: number;
	onReopen: () => void;
}

export const EmptyState = ({ onNew, archivedCount, onReopen }: EmptyStateProps) => (
	<div className="sk-empty">
		<div className="glyph">⊜</div>
		<h1>No rooms yet</h1>
		<div className="lede">
			A room pins a folder and a task. Open as many harnesses inside as you want — Claude Code and
			opencode on the same worktree, two Copilot runs on a fix, whatever shape the work takes.
		</div>
		<button className="start-btn" onClick={onNew}>
			Create your first room
		</button>
		{archivedCount > 0 && (
			<button type="button" className="sk-empty-reopen" onClick={onReopen}>
				Reopen recent ({archivedCount})…
			</button>
		)}
		<div className="hint-list">
			<div className="row">
				<span className="kbd">{hints.newRoom}</span>
				<span>New room</span>
			</div>
			<div className="row">
				<span className="kbd">{hints.addHarness}</span>
				<span>Add harness to current room</span>
			</div>
			<div className="row">
				<span className="kbd">{hints.nextRoom}</span>
				<span>{hints.roomNavDesc}</span>
			</div>
			<div className="row">
				<span className="kbd">{hints.nextHarness}</span>
				<span>{hints.harnessNavDesc}</span>
			</div>
			<div className="row">
				<span className="kbd">{hints.closeRoom}</span>
				<span>Close active room</span>
			</div>
		</div>
	</div>
);

// ── Titlebar ───────────────────────────────────────────────────────

// Tauri-driven minimize / toggle-maximize / close buttons. On macOS
// we don't render these — tauri.macos.conf.json sets titleBarStyle:
// "Overlay" and the OS draws real traffic lights at the upper-left.
// On Windows / Linux `decorations: false` means the OS draws nothing,
// so these are the only way to close the window from the UI.
const WindowControls = () => {
	const win = getCurrentWindow();
	return (
		<div className="sk-window-controls" data-tauri-drag-region="false">
			<button
				className="sk-wc-btn"
				onClick={() => void win.minimize()}
				title="Minimize"
				type="button"
			>
				<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
					<path d="M0 5h10" stroke="currentColor" strokeWidth="1" />
				</svg>
			</button>
			<button
				className="sk-wc-btn"
				onClick={() => void win.toggleMaximize()}
				title="Maximize"
				type="button"
			>
				<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
					<rect
						x="0.5"
						y="0.5"
						width="9"
						height="9"
						fill="none"
						stroke="currentColor"
						strokeWidth="1"
					/>
				</svg>
			</button>
			<button
				className="sk-wc-btn sk-wc-close"
				onClick={() => void win.close()}
				title="Close"
				type="button"
			>
				<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
					<path d="M1 1l8 8M9 1l-8 8" stroke="currentColor" strokeWidth="1" />
				</svg>
			</button>
		</div>
	);
};

export interface TitlebarProps {
	activeRoomLabel: string | null;
	onOpenSettings: () => void;
}

export const Titlebar = ({ activeRoomLabel, onOpenSettings }: TitlebarProps) => (
	<div className="sk-titlebar" data-tauri-drag-region>
		<span className="sk-app-name">
			<span className="dot">●</span> skein
		</span>
		{activeRoomLabel && <span className="sk-titlebar-session">{activeRoomLabel}</span>}
		<div className="sk-titlebar-actions" data-tauri-drag-region="false">
			<button
				type="button"
				className="sk-cog-btn"
				onClick={onOpenSettings}
				title={`Settings (${modLabel}+,)`}
				aria-label="Settings"
			>
				<svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
					<path
						d="M8 5.5a2.5 2.5 0 100 5 2.5 2.5 0 000-5zm5.6 2.5l1.4-.9-1.4-2.4-1.6.5a5.5 5.5 0 00-1.5-.9l-.3-1.7h-2.8l-.3 1.7c-.55.22-1.05.52-1.5.9l-1.6-.5-1.4 2.4 1.4.9c-.07.3-.1.6-.1.9s.03.6.1.9l-1.4.9 1.4 2.4 1.6-.5c.45.38.95.68 1.5.9l.3 1.7h2.8l.3-1.7c.55-.22 1.05-.52 1.5-.9l1.6.5 1.4-2.4-1.4-.9c.07-.3.1-.6.1-.9s-.03-.6-.1-.9z"
						fill="none"
						stroke="currentColor"
						strokeWidth="1.1"
						strokeLinejoin="round"
					/>
				</svg>
			</button>
		</div>
		{!isMac && <WindowControls />}
	</div>
);
