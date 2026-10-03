// Pure logic for "this Claude harness runs an older Claude Code than the
// one installed" (#491). No React, no invoke. Unknown input never raises a
// notice: a false alarm costs more than a missed one.

import type { ActivityPhase } from "./harnessActivityTypes.ts";

export interface ParsedVersion {
	major: number;
	minor: number;
	patch: number;
	pre: string[];
}

export type VersionNoticeMode = "off" | "badge" | "auto";
export const DEFAULT_VERSION_NOTICE_MODE: VersionNoticeMode = "badge";

export interface VersionNotice {
	running: string;
	installed: string;
}

export type VersionAction = { kind: "none" } | { kind: "badge" } | { kind: "restart" };

const SEMVER = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/;

export function parseVersion(s: string | null | undefined): ParsedVersion | null {
	if (s == null) return null;
	const m = SEMVER.exec(s.trim());
	if (!m) return null;
	return {
		major: Number(m[1]),
		minor: Number(m[2]),
		patch: Number(m[3]),
		pre: m[4] ? m[4].split(".") : [],
	};
}

const cmp = (a: number, b: number): -1 | 0 | 1 => (a < b ? -1 : a > b ? 1 : 0);
const isNumeric = (s: string): boolean => /^\d+$/.test(s);

function comparePre(a: string[], b: string[]): -1 | 0 | 1 {
	if (a.length === 0 && b.length === 0) return 0;
	if (a.length === 0) return 1;
	if (b.length === 0) return -1;
	const n = Math.min(a.length, b.length);
	for (let i = 0; i < n; i++) {
		const x = a[i] as string;
		const y = b[i] as string;
		if (x === y) continue;
		const xn = isNumeric(x);
		const yn = isNumeric(y);
		if (xn && yn) return cmp(Number(x), Number(y));
		if (xn) return -1;
		if (yn) return 1;
		return x < y ? -1 : 1;
	}
	return cmp(a.length, b.length);
}

export function compareVersions(
	a: string | null | undefined,
	b: string | null | undefined,
): -1 | 0 | 1 | null {
	const pa = parseVersion(a);
	const pb = parseVersion(b);
	if (!pa || !pb) return null;
	return (
		cmp(pa.major, pb.major) ||
		cmp(pa.minor, pb.minor) ||
		cmp(pa.patch, pb.patch) ||
		comparePre(pa.pre, pb.pre)
	);
}

export function versionNotice(
	running: string | null | undefined,
	installed: string | null | undefined,
): VersionNotice | null {
	if (compareVersions(installed, running) !== 1) return null;
	return { running: (running as string).trim(), installed: (installed as string).trim() };
}

/// The harness must have sat in `waiting` continuously for this long before
/// an auto-restart, so a mail delivery or a just-submitted prompt (which
/// moves it to running) gets there first instead of being killed mid-flight.
export const AUTO_RESTART_SETTLE_MS = 15_000;

export function decideVersionAction(args: {
	mode: VersionNoticeMode;
	notice: VersionNotice | null;
	phase: ActivityPhase | null;
	autoRestartedFor: string | null;
	/// When the harness last entered `waiting`; null = unknown / not waiting.
	waitingSinceMs: number | null;
	/// A restart was already attempted (and refused) in this waiting
	/// episode; retry only in the next one, so a refusal cannot loop.
	triedThisEpisode?: boolean;
	nowMs: number;
}): VersionAction {
	const { mode, notice, phase, autoRestartedFor, waitingSinceMs, nowMs } = args;
	if (mode === "off" || !notice) return { kind: "none" };
	if (
		mode === "auto" &&
		phase === "waiting" &&
		!args.triedThisEpisode &&
		autoRestartedFor !== notice.installed &&
		waitingSinceMs !== null &&
		nowMs - waitingSinceMs >= AUTO_RESTART_SETTLE_MS
	) {
		return { kind: "restart" };
	}
	return { kind: "badge" };
}

export function noticeLabel(n: VersionNotice): string {
	return `Claude Code ${n.running} → ${n.installed} available`;
}

/// Installed-version probe throttle: one probe per window, never two at once.
export const PROBE_INTERVAL_MS = 5 * 60 * 1000;

export function shouldProbe(nowMs: number, lastProbeMs: number | null, inFlight: boolean): boolean {
	if (inFlight) return false;
	return lastProbeMs === null || nowMs - lastProbeMs >= PROBE_INTERVAL_MS;
}

export interface AutoRestartCandidate {
	roomId: string;
	harnessId: string;
	notice: VersionNotice | null;
	phase: ActivityPhase | null;
	autoRestartedFor: string | null;
	waitingSinceMs: number | null;
	triedThisEpisode: boolean;
}

export interface AutoRestartTarget {
	roomId: string;
	harnessId: string;
	installed: string;
}

/// Which harnesses the auto policy should restart now, over a snapshot of
/// the Claude harnesses. Pure; the hook only gathers the snapshot.
export function autoRestartTargets(
	mode: VersionNoticeMode,
	candidates: AutoRestartCandidate[],
	nowMs: number,
): AutoRestartTarget[] {
	const out: AutoRestartTarget[] = [];
	for (const c of candidates) {
		const action = decideVersionAction({
			mode,
			notice: c.notice,
			phase: c.phase,
			autoRestartedFor: c.autoRestartedFor,
			waitingSinceMs: c.waitingSinceMs,
			triedThisEpisode: c.triedThisEpisode,
			nowMs,
		});
		if (action.kind === "restart" && c.notice) {
			out.push({ roomId: c.roomId, harnessId: c.harnessId, installed: c.notice.installed });
		}
	}
	return out;
}
