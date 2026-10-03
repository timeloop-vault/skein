// The harness tab row's Actions ▾ button (#355 step 2), beside `+
// harness` in HarnessColumn.tsx. Lists the non-review nudges
// (`nudgeRegistry.ts`'s "actions" scope — #358's worktree sweep is the
// first), the room's own repo skills (#359 — `repoSkills.ts`), and
// pastes the chosen one into the room's ACTIVE harness, through the
// same `harnessInput.ts` seam #238's Nudge button uses.
//
// Thin on purpose: `actionsButtonState` (harnessActionsMenu.ts) is the
// pure "what should this button show" logic; this component only wires
// it to the live stores (activity, registration, overrides), the
// `.claude/skills` load, and the menu's open/close chrome.

import { invoke } from "@tauri-apps/api/core";
import { Fragment, useEffect, useRef, useState } from "react";
import { HARNESS_KINDS } from "./data.tsx";
import { type ActionsMenuItem, actionsButtonState } from "./harnessActionsMenu.ts";
import { useHarnessActivity } from "./harnessActivity.ts";
import { hasClaudeTranscriptTail } from "./harnessEvents.ts";
import { canSendPrompt, type GateResult, harnessInput, sendPrompt } from "./harnessInput.ts";
import { canRestart } from "./harnessRestart.ts";
import { useMailHold } from "./mailHold.ts";
import { actionNudges } from "./nudgeRegistry.ts";
import { useNudgeOverrides } from "./nudgeStore.ts";
import { loadRepoSkills, type RepoSkill } from "./repoSkills.ts";
import type { Harness } from "./types.ts";
import "./HarnessActionsMenu.css";

interface TextDto {
	content: string;
}

export const HarnessActionsMenu = ({
	activeHarness,
	cwd,
	onReattachTelemetry,
	onRestart,
}: {
	activeHarness: Harness | undefined;
	cwd: string | undefined;
	// #490: re-checks its own gate against live state and refuses with a
	// reason, so a stale enabled item is harmless.
	onRestart: (harnessId: string) => Promise<GateResult>;
	// #410: manual recovery for a Claude harness whose Rust-side JSONL
	// tail died — only ever called for a harness that passes
	// `hasClaudeTranscriptTail` below, same as the automatic attach in
	// `useTerminalSpawn.ts`.
	onReattachTelemetry: (harnessId: string) => void;
}) => {
	const [open, setOpen] = useState(false);
	const [error, setError] = useState<string | undefined>(undefined);
	const [skills, setSkills] = useState<readonly RepoSkill[]>([]);
	const skillsSeq = useRef(0);
	const rootRef = useRef<HTMLDivElement | null>(null);

	const activity = useHarnessActivity(activeHarness?.id ?? null);
	const mailHeld = useMailHold(activeHarness?.id ?? "").held;
	// The composer draft has no change subscription (only "cleared"), so
	// while the menu is open re-render on a short tick to keep the restart
	// gate's draft check live. Closed menu = no ticking.
	const [, setTick] = useState(0);
	useEffect(() => {
		if (!open) return undefined;
		const t = setInterval(() => setTick((n) => n + 1), 400);
		return () => clearInterval(t);
	}, [open]);
	const overrides = useNudgeOverrides();
	const capabilities = activeHarness ? HARNESS_KINDS[activeHarness.kind].capabilities : null;
	const skillInvocation = activeHarness ? HARNESS_KINDS[activeHarness.kind].skillInvocation : null;

	// Reload the room's repo skills fresh every time the menu opens — no
	// caching, since a skill directory can appear or change between
	// opens. A sequence guard means a slow load for an earlier
	// cwd/kind can't clobber a newer one that already resolved. Skipped
	// entirely (and reset to empty) when there's no cwd yet or the
	// active kind has no slash convention at all.
	useEffect(() => {
		if (!open) return;
		const seq = ++skillsSeq.current;
		if (!cwd || skillInvocation === null) {
			setSkills([]);
			return;
		}
		loadRepoSkills(cwd, {
			listDir: (path) => invoke("list_dir", { path }),
			readFile: async (path) => (await invoke<TextDto>("read_file_text", { path })).content,
		}).then((loaded) => {
			if (seq === skillsSeq.current) setSkills(loaded);
		});
	}, [open, cwd, skillInvocation]);

	// Close on outside click / Escape, same affordance as any other
	// lightweight popover in this app.
	useEffect(() => {
		if (!open) return undefined;
		const onPointerDown = (e: MouseEvent) => {
			if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
		};
		const onKey = (e: KeyboardEvent) => {
			if (e.key === "Escape") setOpen(false);
		};
		document.addEventListener("mousedown", onPointerDown);
		document.addEventListener("keydown", onKey);
		return () => {
			document.removeEventListener("mousedown", onPointerDown);
			document.removeEventListener("keydown", onKey);
		};
	}, [open]);

	// No terminal at all (e.g. `files`) — same rule the #238 Nudge
	// button uses: hidden entirely, not disabled-and-unexplained.
	if (!capabilities?.pty) return null;

	const state = actionsButtonState(
		true,
		actionNudges(),
		overrides,
		(body) =>
			activeHarness
				? canSendPrompt({
						capabilities,
						activity,
						registered: harnessInput.isRegistered(activeHarness.id),
						bracketedPasteOn: harnessInput.bracketedPaste(activeHarness.id),
						body,
					})
				: { ok: false, reason: "no active harness" },
		skills,
		skillInvocation,
		activeHarness && capabilities.pty && capabilities.resume
			? canRestart({
					kind: activeHarness.kind,
					capabilities,
					phase: activity?.phase ?? null,
					mailHeld,
					draft: harnessInput.draft(activeHarness.id),
				})
			: undefined,
	);

	const onRestartClick = async () => {
		if (!activeHarness) return;
		setOpen(false);
		const result = await onRestart(activeHarness.id);
		setError(result.ok ? undefined : result.reason);
	};

	const onChoose = (item: ActionsMenuItem) => {
		if (!activeHarness || !item.gate.ok) return;
		const result = sendPrompt(activeHarness.id, activeHarness.kind, item.body);
		if (!result.ok) setError(result.reason);
		setOpen(false);
	};

	// #410: independent of the nudge/skill items above — this doesn't
	// paste a prompt into the terminal, it calls Rust directly, so it
	// carries no `canSendPrompt` gate of its own. Shown only for the
	// harnesses `attachClaudeEvents` would ever attach to.
	const canReattach = Boolean(
		activeHarness && hasClaudeTranscriptTail(activeHarness.kind, activeHarness.sessionId),
	);
	const onReattach = () => {
		if (!activeHarness) return;
		onReattachTelemetry(activeHarness.id);
		setOpen(false);
	};

	return (
		<div className="sk-harness-actions" ref={rootRef}>
			<button
				type="button"
				className="sk-harness-actions-btn"
				disabled={state.kind === "disabled" && !canReattach}
				title={state.kind === "disabled" && !canReattach ? state.reason : undefined}
				onClick={() => setOpen((o) => !o)}
			>
				Actions ▾
			</button>
			{open && (state.kind === "menu" || canReattach) && (
				<div className="sk-harness-actions-menu">
					{state.kind === "menu" &&
						state.items.map((item, i) => (
							<Fragment key={item.id}>
								{item.source === "skill" && state.items[i - 1]?.source !== "skill" && (
									<div className="sk-harness-actions-divider">Repo skills</div>
								)}
								<button
									type="button"
									className="sk-harness-actions-item"
									disabled={!item.gate.ok}
									title={item.gate.ok ? item.title : item.gate.reason}
									onClick={() => onChoose(item)}
								>
									{item.label}
								</button>
							</Fragment>
						))}
					{state.kind === "menu" && state.restart && (
						<>
							{(state.items.length > 0 || canReattach) && (
								<div className="sk-harness-actions-divider" />
							)}
							<button
								type="button"
								className="sk-harness-actions-item"
								disabled={!state.restart.gate.ok}
								title={
									state.restart.gate.ok
										? "Kill and respawn this harness, resuming its conversation"
										: state.restart.gate.reason
								}
								onClick={() => void onRestartClick()}
							>
								Restart harness
							</button>
						</>
					)}
					{canReattach && (
						<>
							{state.kind === "menu" && state.items.length > 0 && (
								<div className="sk-harness-actions-divider">Telemetry</div>
							)}
							<button
								type="button"
								className="sk-harness-actions-item"
								title="Re-attach the Claude transcript tail if it died"
								onClick={onReattach}
							>
								Reattach telemetry
							</button>
						</>
					)}
				</div>
			)}
			{error && (
				<div className="sk-harness-actions-error" title={error}>
					{error}
				</div>
			)}
		</div>
	);
};
