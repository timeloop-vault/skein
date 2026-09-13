import { beforeEach, describe, expect, it } from "vitest";
import {
	TRANSITION_SOURCE,
	activityToStatus,
	effectiveStatus,
	harnessActivity,
	higherPriorityStatus,
	isDecisiveInput,
	statusLabel,
} from "./harnessActivity.ts";

// #86 — permission promoted to its own activity phase. These tests
// exercise the pure store API (`harnessActivity`'s methods + `.get()`)
// rather than the React hooks (`useHarnessActivity`, `useRoomActivity`,
// `usePermissionHarnessIds`), which need a React render context this
// node-environment suite doesn't have — see vitest.config.ts.

const nextId = (() => {
	let n = 0;
	return () => `h_${++n}`;
})();

describe("permission phase", () => {
	let id: string;

	beforeEach(() => {
		id = nextId();
		harnessActivity.spawned(id);
	});

	it("enters permission with the tool name", () => {
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		const a = harnessActivity.get(id);
		expect(a?.phase).toBe("permission");
		expect(a?.permissionTool).toBe("Bash");
	});

	it("is not knocked out by PTY output while authoritative", () => {
		harnessActivity.attachAuthoritativeSource(id);
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		harnessActivity.recordOutput(id, "the dialog repaints\n");
		expect(harnessActivity.get(id)?.phase).toBe("permission");
	});

	it("is not knocked out by PTY output when non-authoritative either", () => {
		// Defensive: today only authoritative harnesses enter
		// `permission`, but recordOutput must not fight the phase
		// regardless of that flag.
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		harnessActivity.recordOutput(id, "(y/n)?");
		expect(harnessActivity.get(id)?.phase).toBe("permission");
	});

	it("is not cleared by a 'still working' adapter event", () => {
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		harnessActivity.setRunningFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeAssistant);
		expect(harnessActivity.get(id)?.phase).toBe("permission");
		harnessActivity.setRunningFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeToolUse);
		expect(harnessActivity.get(id)?.phase).toBe("permission");
	});

	it("is cleared by a tool result, which also drops the stale tool name", () => {
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		harnessActivity.setRunningFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeToolResult, {
			clearsPermission: true,
		});
		const a = harnessActivity.get(id);
		expect(a?.phase).toBe("running");
		expect(a?.permissionTool).toBeNull();
	});

	it("is released when the adapter detaches, and nothing else is", () => {
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		harnessActivity.releasePermission(id, TRANSITION_SOURCE.AdapterDetached);
		expect(harnessActivity.get(id)?.phase).toBe("running");

		harnessActivity.setWaitingFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		harnessActivity.releasePermission(id, TRANSITION_SOURCE.AdapterDetached);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
	});

	it("is cleared by a fresh user prompt", () => {
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		harnessActivity.setRunningFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeUserPrompt, {
			clearsPermission: true,
		});
		expect(harnessActivity.get(id)?.phase).toBe("running");
	});

	it("awaiting_prompt always leaves permission, landing on waiting", () => {
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		harnessActivity.setWaitingFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
	});

	it("is cleared by decisive user input", () => {
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		harnessActivity.recordInput(id, "\r");
		expect(harnessActivity.get(id)?.phase).toBe("running");
	});

	it("is not cleared by an arrow key or a focus escape", () => {
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		harnessActivity.recordInput(id, "\x1b[A"); // up arrow
		expect(harnessActivity.get(id)?.phase).toBe("permission");
		harnessActivity.recordInput(id, "\x1b[I"); // focus-in
		expect(harnessActivity.get(id)?.phase).toBe("permission");
		harnessActivity.recordInput(id, "\x1b[O"); // focus-out
		expect(harnessActivity.get(id)?.phase).toBe("permission");
	});

	it("exited wins over permission", () => {
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		harnessActivity.exited(id, 0);
		expect(harnessActivity.get(id)?.phase).toBe("exited");
		// No further adapter call can revive it.
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		expect(harnessActivity.get(id)?.phase).toBe("exited");
	});
});

describe("isDecisiveInput", () => {
	it("treats Enter/Return as decisive", () => {
		expect(isDecisiveInput("\r")).toBe(true);
		expect(isDecisiveInput("\n")).toBe(true);
	});

	it("treats a lone ESC as decisive but not an escape sequence", () => {
		expect(isDecisiveInput("\x1b")).toBe(true);
		expect(isDecisiveInput("\x1b[A")).toBe(false);
		expect(isDecisiveInput("\x1b[I")).toBe(false);
		expect(isDecisiveInput("\x1b[O")).toBe(false);
	});

	it("treats Ctrl+C as decisive", () => {
		expect(isDecisiveInput("\x03")).toBe(true);
	});

	it("treats a single digit or y/Y/n/N as decisive", () => {
		for (const c of ["0", "1", "9", "y", "Y", "n", "N"]) {
			expect(isDecisiveInput(c)).toBe(true);
		}
	});

	it("does not treat ordinary text as decisive", () => {
		expect(isDecisiveInput("a")).toBe(false);
		expect(isDecisiveInput("no")).toBe(false);
		expect(isDecisiveInput("")).toBe(false);
		expect(isDecisiveInput(" ")).toBe(false);
	});
});

describe("activityToStatus / statusLabel", () => {
	it("maps the permission phase to the permission status", () => {
		const id = nextId();
		harnessActivity.spawned(id);
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		expect(activityToStatus(harnessActivity.get(id))).toBe("permission");
	});

	it("reads as 'permission needed' plus the tool when known", () => {
		expect(statusLabel("permission", "Bash")).toBe("permission needed · Bash");
		expect(statusLabel("permission", null)).toBe("permission needed");
		expect(statusLabel("permission")).toBe("permission needed");
	});

	it("passes other statuses through unchanged", () => {
		expect(statusLabel("waiting")).toBe("waiting");
		expect(statusLabel("running")).toBe("running");
	});
});

describe("effectiveStatus", () => {
	it("does not downgrade permission when acknowledged", () => {
		const id = nextId();
		harnessActivity.spawned(id);
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		const a = harnessActivity.get(id);
		expect(effectiveStatus(a, 0)).toBe("permission");
		expect(effectiveStatus(a, 1)).toBe("permission");
	});

	it("still downgrades acknowledged waiting to idle", () => {
		const id = nextId();
		harnessActivity.spawned(id);
		harnessActivity.setWaitingFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		expect(effectiveStatus(harnessActivity.get(id), 0)).toBe("idle");
	});
});

describe("higherPriorityStatus (room aggregate ranking)", () => {
	it("ranks permission above waiting", () => {
		expect(higherPriorityStatus("waiting", "permission")).toBe("permission");
		expect(higherPriorityStatus("permission", "waiting")).toBe("permission");
	});

	it("ranks waiting above running, idle and exited", () => {
		expect(higherPriorityStatus("waiting", "running")).toBe("waiting");
		expect(higherPriorityStatus("idle", "waiting")).toBe("waiting");
		expect(higherPriorityStatus("exited", "waiting")).toBe("waiting");
	});
});
