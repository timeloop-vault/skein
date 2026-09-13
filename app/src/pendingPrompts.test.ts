import { describe, expect, it } from "vitest";
import { pendingPrompts } from "./pendingPrompts.ts";

// #86 — opencode's permission/question prompt bookkeeping, kept pure
// so it's testable without the activity store or an SSE stream.

const nextId = (() => {
	let n = 0;
	return () => `h_${++n}`;
})();

describe("pendingPrompts", () => {
	it("a permission always yields permission", () => {
		const h = nextId();
		expect(pendingPrompts.permissionAsked(h, "req_1")).toBe("permission");
	});

	it("draining the only permission with nothing else pending yields running", () => {
		const h = nextId();
		pendingPrompts.permissionAsked(h, "req_1");
		expect(pendingPrompts.permissionReplied(h, "req_1")).toBe("running");
	});

	it("draining one of several pending permissions stays on permission", () => {
		const h = nextId();
		pendingPrompts.permissionAsked(h, "req_1");
		pendingPrompts.permissionAsked(h, "req_2");
		expect(pendingPrompts.permissionReplied(h, "req_1")).toBe("permission");
		expect(pendingPrompts.permissionReplied(h, "req_2")).toBe("running");
	});

	it("a question with nothing else pending yields waiting", () => {
		const h = nextId();
		expect(pendingPrompts.questionAsked(h, "q_1")).toBe("waiting");
	});

	it("a question is outranked by a pending permission", () => {
		const h = nextId();
		pendingPrompts.permissionAsked(h, "req_1");
		expect(pendingPrompts.questionAsked(h, "q_1")).toBe("permission");
		// Resolving the question alone doesn't clear the still-pending
		// permission.
		expect(pendingPrompts.questionResolved(h, "q_1")).toBe("permission");
		// Only draining the permission itself reaches running.
		expect(pendingPrompts.permissionReplied(h, "req_1")).toBe("running");
	});

	it("resolving a question with no permission pending yields running", () => {
		const h = nextId();
		pendingPrompts.questionAsked(h, "q_1");
		expect(pendingPrompts.questionResolved(h, "q_1")).toBe("running");
	});

	it("a permission that outlasts a question still reads permission after the question resolves", () => {
		const h = nextId();
		pendingPrompts.questionAsked(h, "q_1");
		pendingPrompts.permissionAsked(h, "req_1");
		expect(pendingPrompts.questionResolved(h, "q_1")).toBe("permission");
	});

	it("forget drops every pending id for that harness", () => {
		const h = nextId();
		pendingPrompts.permissionAsked(h, "req_1");
		pendingPrompts.questionAsked(h, "q_1");
		pendingPrompts.forget(h);
		// Replying/resolving ids that no longer exist is a no-op, and
		// with nothing tracked the harness reads as drained (running).
		expect(pendingPrompts.permissionReplied(h, "req_1")).toBe("running");
		expect(pendingPrompts.questionResolved(h, "q_1")).toBe("running");
	});

	it("tracks harnesses independently", () => {
		const a = nextId();
		const b = nextId();
		pendingPrompts.permissionAsked(a, "req_1");
		expect(pendingPrompts.questionAsked(b, "q_1")).toBe("waiting");
		expect(pendingPrompts.permissionReplied(a, "req_1")).toBe("running");
	});
});
