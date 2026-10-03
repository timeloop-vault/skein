// Glue for the Claude Code update notice (#491): keeps the installed
// version fresh (throttled) and runs the auto-restart policy. All decisions
// live in claudeVersion.ts; this only gathers snapshots and acts.

import { getCurrentWindow } from "@tauri-apps/api/window";
import { type MutableRefObject, useEffect, useRef } from "react";
import { installedClaudeVersion } from "./claudeCli.ts";
import type { VersionNoticeMode } from "./claudeVersion.ts";
import {
	AUTO_RESTART_SETTLE_MS,
	type AutoRestartCandidate,
	autoRestartTargets,
	shouldProbe,
} from "./claudeVersion.ts";
import { claudeVersionStore } from "./claudeVersionStore.ts";
import { harnessActivity } from "./harnessActivity.ts";
import { TRANSITION_SOURCE } from "./harnessActivityTypes.ts";
import type { GateResult } from "./harnessInputGate.ts";
import type { Room } from "./types.ts";

type RestartFn = (roomId: string, harnessId: string, source?: string) => Promise<GateResult>;

export function useClaudeVersionWatch(
	roomsRef: MutableRefObject<Room[]>,
	restartHarness: RestartFn,
	mode: VersionNoticeMode,
): void {
	const restartRef = useRef(restartHarness);
	restartRef.current = restartHarness;
	const modeRef = useRef(mode);
	modeRef.current = mode;

	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef is a stable ref; restartRef/modeRef are refs.
	useEffect(() => {
		const claudeHarnesses = () =>
			roomsRef.current.flatMap((r) =>
				r.harnesses.filter((h) => h.kind === "claude").map((h) => ({ roomId: r.id, id: h.id })),
			);

		let lastProbe: number | null = null;
		let inFlight = false;
		const probe = (): void => {
			if (modeRef.current === "off" || claudeHarnesses().length === 0) return;
			const now = Date.now();
			if (!shouldProbe(now, lastProbe, inFlight)) return;
			lastProbe = now;
			inFlight = true;
			void installedClaudeVersion()
				.then((v) => claudeVersionStore.setInstalled(v))
				.finally(() => {
					inFlight = false;
				});
		};

		// When each harness last entered `waiting`. A harness already waiting
		// with no entry (hook mount, new harness) is stamped on first sight:
		// conservative, it still gets a full settle window.
		const waitingSince = new Map<string, number>();
		// Harnesses whose restart was attempted (and refused) in the current
		// waiting episode; an episode ends on any transition away from waiting.
		const triedThisEpisode = new Set<string>();
		let timer: ReturnType<typeof setTimeout> | null = null;

		const evaluate = (): void => {
			if (timer !== null) {
				clearTimeout(timer);
				timer = null;
			}
			const now = Date.now();
			const candidates: AutoRestartCandidate[] = claudeHarnesses().map(({ roomId, id }) => {
				const phase = harnessActivity.get(id)?.phase ?? null;
				if (phase === "waiting") {
					if (!waitingSince.has(id)) waitingSince.set(id, now);
				} else {
					waitingSince.delete(id);
					triedThisEpisode.delete(id);
				}
				return {
					roomId,
					harnessId: id,
					notice: claudeVersionStore.notice(id),
					phase,
					autoRestartedFor: claudeVersionStore.autoRestartedFor(id),
					waitingSinceMs: waitingSince.get(id) ?? null,
					triedThisEpisode: triedThisEpisode.has(id),
				};
			});
			// One timer for the earliest settle deadline still pending.
			let next: number | null = null;
			for (const c of candidates) {
				if (modeRef.current !== "auto" || !c.notice || c.waitingSinceMs === null) continue;
				if (c.autoRestartedFor === c.notice.installed || c.triedThisEpisode) continue;
				const due = c.waitingSinceMs + AUTO_RESTART_SETTLE_MS;
				if (due > now && (next === null || due < next)) next = due;
			}
			if (next !== null) timer = setTimeout(evaluate, next - now);
			for (const t of autoRestartTargets(modeRef.current, candidates, now)) {
				// Mark synchronously (re-entrancy guard). A refusal or error clears
				// the mark but stays tried for this episode: retried only on the
				// next `-> waiting`, never by timer.
				claudeVersionStore.markAutoRestarted(t.harnessId, t.installed);
				const giveUp = (reason: string): void => {
					triedThisEpisode.add(t.harnessId);
					claudeVersionStore.clearAutoRestarted(t.harnessId);
					claudeVersionStore.setRefusal(t.harnessId, reason);
				};
				void restartRef
					.current(t.roomId, t.harnessId, TRANSITION_SOURCE.VersionAutoRestart)
					.then((res) => {
						if (res.ok) claudeVersionStore.setRefusal(t.harnessId, null);
						else giveUp(res.reason);
					})
					.catch((err: unknown) => {
						console.warn(`[skein] version auto-restart failed for ${t.harnessId}:`, err);
						giveUp(err instanceof Error ? err.message : String(err));
					});
			}
		};

		probe();
		evaluate();
		const unsubStore = claudeVersionStore.subscribe(evaluate);
		const unsubTransitions = harnessActivity.subscribeTransitions((id, _from, to) => {
			if (to !== "waiting") {
				waitingSince.delete(id);
				triedThisEpisode.delete(id);
				return;
			}
			waitingSince.set(id, Date.now());
			probe();
			evaluate();
		});
		const unFocus = getCurrentWindow().onFocusChanged(({ payload }) => {
			if (payload) probe();
		});
		return () => {
			if (timer !== null) clearTimeout(timer);
			unsubStore();
			unsubTransitions();
			void unFocus.then((un) => un());
		};
	}, [mode]);
}
