import type { ReactNode } from "react";
import { Titlebar, type TitlebarProps } from "./AppChrome.tsx";
import { isMac } from "./shortcuts.ts";

// The `.sk-app` root every render path of App shares: theme, density,
// platform marker and the chrome font-size variable.
export const AppShell = ({
	theme,
	density,
	chromeFontPt,
	children,
}: {
	theme: string;
	density: string;
	chromeFontPt: number;
	children: ReactNode;
}) => (
	<div
		className={`sk-app sk-${theme} density-${density}`}
		data-platform={isMac ? "mac" : "other"}
		style={{ ["--cfs" as string]: `${chromeFontPt}px` }}
	>
		{children}
	</div>
);

// Pre-hydration body: rooms haven't loaded from sqlite yet, so render a
// quiet titlebar + blank pane, not the EmptyState — otherwise users with
// existing rooms get a flash of new-user onboarding every boot (#39).
export const BootBody = ({
	titlebarProps,
	loadFailed,
	onRetry,
}: {
	titlebarProps: TitlebarProps;
	loadFailed: string | null;
	onRetry: () => void;
}) => (
	<>
		<Titlebar {...titlebarProps} />
		{loadFailed !== null ? (
			// #167: the load failed wholesale. The autosave is
			// parked (loaded stays false) so the DB is untouched;
			// offer retry instead of silently starting empty.
			<div className="sk-boot-error">
				<div className="sk-boot-error-card">
					<div className="sk-boot-error-title">Couldn't load your rooms</div>
					<div className="sk-boot-error-msg">{loadFailed}</div>
					<div className="sk-boot-error-hint">
						No saved rooms have been deleted (unreadable rows may have been moved to the
						sessions_quarantine table inside skein.db), and nothing will be saved until loading
						succeeds. Last-known-good snapshots sit next to it as skein.db.bak and .bak.1 — if you
						restore one manually, also delete skein.db-wal and skein.db-shm.
					</div>
					<div className="sk-boot-error-actions">
						<button type="button" className="sk-btn primary" onClick={onRetry}>
							Retry
						</button>
					</div>
				</div>
			</div>
		) : (
			<div className="sk-boot" />
		)}
	</>
);
