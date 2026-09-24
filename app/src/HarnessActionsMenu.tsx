// The harness tab row's Actions ▾ button (#355 step 2), beside `+
// harness` in HarnessColumn.tsx. Lists the non-review nudges
// (`nudgeRegistry.ts`'s "actions" scope — empty until #358 adds the
// first) and pastes the chosen one into the room's ACTIVE harness,
// through the same `harnessInput.ts` seam #238's Nudge button uses.
//
// Thin on purpose: `actionsButtonState` (harnessActionsMenu.ts) is the
// pure "what should this button show" logic; this component only wires
// it to the live stores (activity, registration, overrides) and the
// menu's open/close chrome.

import { useEffect, useRef, useState } from "react";
import { HARNESS_KINDS } from "./data.tsx";
import { type ActionsMenuItem, actionsButtonState } from "./harnessActionsMenu.ts";
import { useHarnessActivity } from "./harnessActivity.ts";
import { canSendPrompt, harnessInput, sendPrompt } from "./harnessInput.ts";
import { actionNudges } from "./nudgeRegistry.ts";
import { useNudgeOverrides } from "./nudgeStore.ts";
import type { Harness } from "./types.ts";

export const HarnessActionsMenu = ({ activeHarness }: { activeHarness: Harness | undefined }) => {
	const [open, setOpen] = useState(false);
	const [error, setError] = useState<string | undefined>(undefined);
	const rootRef = useRef<HTMLDivElement | null>(null);

	const activity = useHarnessActivity(activeHarness?.id ?? null);
	const overrides = useNudgeOverrides();
	const capabilities = activeHarness ? HARNESS_KINDS[activeHarness.kind].capabilities : null;

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

	const state = actionsButtonState(true, actionNudges(), overrides, (body) =>
		activeHarness
			? canSendPrompt({
					capabilities,
					activity,
					registered: harnessInput.isRegistered(activeHarness.id),
					bracketedPasteOn: harnessInput.bracketedPaste(activeHarness.id),
					body,
				})
			: { ok: false, reason: "no active harness" },
	);

	const onChoose = (item: ActionsMenuItem) => {
		if (!activeHarness || !item.gate.ok) return;
		const result = sendPrompt(activeHarness.id, activeHarness.kind, item.body);
		if (!result.ok) setError(result.reason);
		setOpen(false);
	};

	return (
		<div className="sk-harness-actions" ref={rootRef}>
			<button
				type="button"
				className="sk-harness-actions-btn"
				disabled={state.kind === "disabled"}
				title={state.kind === "disabled" ? state.reason : undefined}
				onClick={() => setOpen((o) => !o)}
			>
				Actions ▾
			</button>
			{open && state.kind === "menu" && (
				<div className="sk-harness-actions-menu">
					{state.items.map((item) => (
						<button
							type="button"
							key={item.id}
							className="sk-harness-actions-item"
							disabled={!item.gate.ok}
							title={item.gate.ok ? item.title : item.gate.reason}
							onClick={() => onChoose(item)}
						>
							{item.label}
						</button>
					))}
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
