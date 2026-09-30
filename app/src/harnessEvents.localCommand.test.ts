import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

// #463 — a local slash command (`/context`, `/model`, ...) is not a turn.
// Its rows, including the `isMeta` user row carrying the output, must
// start nothing, so a waiting harness stays waiting and its mail keeps
// flowing. Driven through the real `attachClaudeEvents` translator on a
// mocked Channel. This is a downstream contract test: it drives the
// frontend translator with the event stream the Rust tail now emits (none
// for a local command) and does not exercise `LocalCommandTracker`; the
// fix itself is pinned by the `local_command_*` tests in
// `app/src-tauri/src/harness_events_claude.rs` and the tracker's own
// tests. Own file: the idle tick is a module-global interval,
// so fake timers must be installed before the first `spawned()`.

const h = vi.hoisted(() => {
	const channels: Array<{ onmessage: (e: unknown) => void }> = [];
	return { channels };
});

vi.mock("@tauri-apps/api/core", () => ({
	Channel: class {
		onmessage: (e: unknown) => void = () => {};
		constructor() {
			h.channels.push(this);
		}
	},
	invoke: () => new Promise<void>(() => {}),
}));
vi.mock("./frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { HARNESS_KINDS } from "./data.tsx";
import { atSafeStoppingPoint, harnessActivity } from "./harnessActivity.ts";
import { attachClaudeEvents } from "./harnessEvents.ts";
import { canSendPrompt } from "./harnessInput.ts";
import { decideMailNudge } from "./mailNudge.ts";

let n = 0;
const send = (event: unknown) => h.channels[h.channels.length - 1]?.onmessage(event);

/// A Claude harness at the end of a turn (`waiting`) with its tail
/// attached and #215 injection on, so the send gate can pass.
const waiting = (): string => {
	const id = `lc_${++n}`;
	harnessActivity.spawned(id);
	attachClaudeEvents(id, "room", "sid", "/cwd");
	harnessActivity.setInjected(id, true);
	send({ kind: "assistant_turn" });
	send({ kind: "awaiting_prompt" });
	harnessActivity.adapterDelivered(id);
	return id;
};

const decide = (id: string, lastNudged: number, unread = 1) =>
	decideMailNudge({
		atStoppingPoint: atSafeStoppingPoint(
			harnessActivity.get(id) as NonNullable<ReturnType<typeof harnessActivity.get>>,
		),
		unread,
		lastNudged,
		gate: canSendPrompt({
			capabilities: HARNESS_KINDS.claude.capabilities,
			activity: harnessActivity.get(id),
			registered: true,
			bracketedPasteOn: false,
			body: "mail",
		}),
	});

describe("a local slash command is not a turn (#463)", () => {
	beforeAll(() => {
		vi.useFakeTimers();
	});
	afterAll(() => {
		vi.useRealTimers();
	});
	beforeEach(() => {
		h.channels.length = 0;
	});

	it("a local command leaves a waiting harness waiting and its mail deliverable", () => {
		const id = waiting();
		expect(harnessActivity.get(id)?.phase).toBe("waiting");

		// The local command's rows produce NO ClaudeEvent: the Rust tail pins
		// that (tests in harness_events_claude.rs, added by #463), so nothing
		// is sent here.

		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(decide(id, 0)).toEqual({ nudge: true, lastNudged: 1 });
		harnessActivity.forget(id);
	});

	it("held mail goes out once the harness is back at a stopping point", () => {
		const id = waiting();

		// The #463 failure shape, for contrast: the tail used to emit a
		// `user_prompt` for the command's `isMeta` output row, which moved the
		// harness to `running` with no end of turn to follow, so mail was held.
		send({ kind: "user_prompt" });
		expect(harnessActivity.get(id)?.phase).toBe("running");
		expect(decide(id, 0).nudge).toBe(false);

		send({ kind: "assistant_turn" });
		send({ kind: "awaiting_prompt" });
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(decide(id, 0)).toEqual({ nudge: true, lastNudged: 1 });
		harnessActivity.forget(id);
	});
});
