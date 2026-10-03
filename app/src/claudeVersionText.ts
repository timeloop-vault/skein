// Wording for the Claude Code update notice's three surfaces (#491): the
// tab badge's tooltip, the hover popover segment and the Actions menu item.
// Pure, so the strings are table-tested.

import { noticeLabel, type VersionNotice } from "./claudeVersion.ts";

/// The popover's `update` segment value.
export function updateSegmentText(notice: VersionNotice, refusal: string | null): string {
	const base = noticeLabel(notice);
	return refusal ? `${base} · not restarted: ${refusal}` : base;
}

/// The badge's native tooltip: two lines when a restart was refused.
export function badgeTitle(notice: VersionNotice, refusal: string | null): string {
	const first = `${noticeLabel(notice)} — click to restart`;
	return refusal ? `${first}\nNot restarted: ${refusal}` : first;
}

export function badgeAriaLabel(notice: VersionNotice): string {
	return `${noticeLabel(notice)}; restart harness`;
}

export function restartMenuLabel(notice: VersionNotice | null): string {
	return notice ? `Restart to update (${notice.running} → ${notice.installed})` : "Restart harness";
}
