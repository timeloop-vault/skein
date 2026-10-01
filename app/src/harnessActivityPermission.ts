// harnessActivity.ts split (#458): the #86 permission phase. Methods are spread into the
// `harnessActivity` store object there; shared state lives in harnessActivityCore.ts.

import { setPhase, store } from "./harnessActivityCore.ts";
import type { TransitionSource } from "./harnessActivityTypes.ts";

export const permissionMethods = {
	/// L2c adapter reports a permission dialog is on screen — Claude's
	/// PermissionRequest hook fired, or opencode's permission-asked SSE
	/// event landed. `toolName` is `null` when the adapter can't say
	/// (opencode's event carries no tool name); every notification
	/// surface shows it when present. `agentType` names the subagent
	/// the dialog belongs to when it isn't the main session (#298);
	/// omitted or `null` for the main session or when the adapter
	/// can't say. `agentId` is that same subagent's id — what
	/// `clearPermission` correlates on — omitted or `null` likewise.
	/// No-op once exited. Always goes through `setPhase` even when
	/// already in `permission`, so a second request with a different
	/// tool name still updates `.permissionTool` for anything reading
	/// `.get()` live. #86.
	setPermissionFromAdapter(
		id: string,
		source: TransitionSource,
		toolName: string | null,
		agentType?: string | null,
		agentId?: string | null,
	): void {
		const cur = store.get(id);
		if (!cur || cur.phase === "exited") return;
		setPhase(id, "permission", source, {
			permissionTool: toolName,
			permissionAgentType: agentType ?? null,
			permissionAgentId: agentId ?? null,
			permissionAt: Date.now(),
		});
	},
	/// Leave `permission` for `running`, and touch no other phase. For
	/// an adapter detaching mid-dialog: nothing it would have sent can
	/// arrive now, and the L2a tick never moves a non-running phase, so
	/// without this the harness would read "permission needed" until
	/// the user typed or the PTY died. `running` hands it back to L2a.
	releasePermission(id: string, source: TransitionSource): void {
		if (store.get(id)?.phase !== "permission") return;
		setPhase(id, "running", source);
	},

	/// A subagent's tool result landed (#298). Proves *that subagent's*
	/// gate is gone, and only that one — with several subagents running
	/// concurrently, an unrelated subagent finishing its own tool call
	/// must not clear a different subagent's still-open dialog. No-op
	/// unless the harness is actually in `permission`. When it is:
	/// `agentId` is compared against the stored `permissionAgentId` —
	/// a match clears; a mismatch (a *different* subagent's result) is
	/// left alone. A stored `null` still clears unconditionally, for
	/// two folded-together cases that read the same way from here: a
	/// main-session dialog (there is no subagent to disambiguate
	/// against), and an adapter event where `agentId` wasn't reported
	/// at all — the field is confirmed live as of 2026-09-20, so that
	/// second case is now the rare one (injection off, or a future CLI
	/// change), but falling back to the pre-#298 behaviour is still the
	/// safe direction; a dialog cleared slightly early is recoverable,
	/// one left stuck for minutes is the bug this exists to fix. Unlike
	/// `releasePermission` (adapter vanished entirely) this fires on a
	/// live, healthy adapter mid-conversation, so it must do the one
	/// narrow thing it's proof of and nothing more: never touch
	/// `waiting`, `idle`, `spawning` or `exited`. A subagent tool
	/// result is not "the harness is now doing work" (it might be the
	/// main session sitting at `waiting` while a background subagent
	/// finishes up) — it is only "whatever dialog was on screen has
	/// been answered." All further phase/notification policy belongs
	/// to #277.
	clearPermission(id: string, source: TransitionSource, agentId?: string | null): void {
		const cur = store.get(id);
		if (cur?.phase !== "permission") return;
		if (cur.permissionAgentId != null && cur.permissionAgentId !== agentId) return;
		setPhase(id, "running", source);
	},
};
