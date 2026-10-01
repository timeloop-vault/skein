import { describe, expect, it } from "vitest";
import { HARNESS_KINDS } from "./data.tsx";
import type { HarnessActivity } from "./harnessActivity.ts";
import type { CanInsertTextInput, CanSendPromptInput } from "./harnessInput.ts";
import { canInsertText, canSendPrompt, formatDroppedPaths } from "./harnessInput.ts";
import { LAUNCH_QUIET_MS } from "./launchReady.ts";

// Pure `canSendPrompt` tests build the input by hand — no store, no
// DOM, no xterm — exactly what the gate is supposed to allow.

const activity = (over: Partial<HarnessActivity> = {}): HarnessActivity => ({
	phase: "waiting",
	lastOutputAt: null,
	exitCode: null,
	spawnedAt: 0,
	hasUserInput: false,
	authoritative: true,
	tail: "",
	permissionTool: null,
	permissionAgentType: null,
	permissionAgentId: null,
	adapterHeard: true,
	promptSubmittedAt: null,
	adapterSilent: false,
	degradedBy: null,
	launchSignalAt: null,
	injected: true,
	delegationDeferredAt: null,
	delegationActivityAt: 0,
	delegationEmptiedAt: null,
	delegatedCount: 0,
	silenceRecovered: false,
	phaseSince: 0,
	lastTurnSignal: null,
	lastAdapterEvent: null,
	authorityLostAt: null,
	permissionAt: null,
	lastSubmitAt: null,
	...over,
});

const baseInput = (over: Partial<CanSendPromptInput> = {}): CanSendPromptInput => ({
	capabilities: HARNESS_KINDS.claude.capabilities,
	activity: activity(),
	registered: true,
	bracketedPasteOn: false,
	body: "nudge",
	...over,
});

describe("canSendPrompt", () => {
	it("allows a single-line prompt to a waiting, verified, injected, registered harness", () => {
		expect(canSendPrompt(baseInput())).toEqual({ ok: true });
	});

	it("refuses a kind with no terminal", () => {
		const r = canSendPrompt(baseInput({ capabilities: HARNESS_KINDS.files.capabilities }));
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("no terminal") });
	});

	it("refuses a harness that hasn't registered a live terminal", () => {
		const r = canSendPrompt(baseInput({ registered: false }));
		expect(r.ok).toBe(false);
	});

	it("refuses when there's no activity record at all", () => {
		const r = canSendPrompt(baseInput({ activity: null }));
		expect(r.ok).toBe(false);
	});

	it("gives permission its own reason rather than a generic 'not waiting'", () => {
		const r = canSendPrompt(baseInput({ activity: activity({ phase: "permission" }) }));
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("permission dialog") });
	});

	it("refuses any other non-waiting phase", () => {
		const r = canSendPrompt(baseInput({ activity: activity({ phase: "running" }) }));
		expect(r.ok).toBe(false);
		if (!r.ok) expect(r.reason).toContain("running");
	});

	// #381: a #277 delegation-deferred `running` is the OTHER safe
	// stopping point `atSafeStoppingPoint` allows — the main session
	// already ended its own turn and only stayed `running` because
	// background subagents were still working.
	it("allows a running harness with an armed delegation deferral", () => {
		const r = canSendPrompt(
			baseInput({ activity: activity({ phase: "running", delegationDeferredAt: Date.now() }) }),
		);
		expect(r).toEqual({ ok: true });
	});

	it("still refuses a running harness with no deferral armed", () => {
		const r = canSendPrompt(
			baseInput({ activity: activity({ phase: "running", delegationDeferredAt: null }) }),
		);
		expect(r.ok).toBe(false);
		if (!r.ok) expect(r.reason).toContain("running");
	});

	it("gives permission its own reason even with a deferral armed underneath it", () => {
		const r = canSendPrompt(
			baseInput({
				activity: activity({ phase: "permission", delegationDeferredAt: Date.now() }),
			}),
		);
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("permission dialog") });
	});

	it("refuses without a source that has proven it's watching the harness", () => {
		expect(canSendPrompt(baseInput({ activity: activity({ authoritative: false }) })).ok).toBe(
			false,
		);
		expect(canSendPrompt(baseInput({ activity: activity({ adapterHeard: false }) })).ok).toBe(
			false,
		);
		expect(canSendPrompt(baseInput({ activity: activity({ adapterSilent: true }) })).ok).toBe(
			false,
		);
	});

	it("refuses a harness that wasn't spawned with config injection", () => {
		const r = canSendPrompt(baseInput({ activity: activity({ injected: false }) }));
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("review tools") });
	});

	// #273: a launch signal is proof-of-life equivalent to `adapterHeard`
	// — the harness's CLI reaching its own prompt before the tail has
	// ever spoken (a fresh, just-past-trust-dialog spawn). #404: the PTY
	// also has to have been quiet since — set well in the past here so
	// this test is about the launch-signal equivalence, not settling.
	it("accepts a harness whose only proof is a launch signal", () => {
		const r = canSendPrompt(
			baseInput({
				activity: activity({ adapterHeard: false, launchSignalAt: Date.now() - 10_000 }),
			}),
		);
		expect(r).toEqual({ ok: true });
	});

	it("still refuses a launch-signalled harness that isn't injected", () => {
		const r = canSendPrompt(
			baseInput({
				activity: activity({ adapterHeard: false, launchSignalAt: Date.now(), injected: false }),
			}),
		);
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("review tools") });
	});

	it("still refuses a launch-signalled harness whose watchdog gave up", () => {
		const r = canSendPrompt(
			baseInput({
				activity: activity({
					adapterHeard: false,
					launchSignalAt: Date.now(),
					adapterSilent: true,
				}),
			}),
		);
		expect(r.ok).toBe(false);
	});

	// Regression for the #273/#259 interaction: a launch signal recorded
	// AFTER the #259 watchdog already gave up on the adapter must not
	// read as proof of life — the #259 diagnosis (a prompted tail that
	// stayed mute) says nothing about the launch hook's channel, so
	// `adapterSilent` staying true here (left by `degradeSilentAdapter`,
	// not the launch-silent path) must still refuse.
	it("refuses a harness left adapter-silent by the #259 watchdog even with a launch signal", () => {
		const r = canSendPrompt(
			baseInput({
				activity: activity({
					adapterHeard: false,
					launchSignalAt: Date.now(),
					adapterSilent: true,
					degradedBy: "adapter-silent",
				}),
			}),
		);
		expect(r.ok).toBe(false);
	});

	it("refuses a multi-line body when bracketed paste is off", () => {
		const r = canSendPrompt(baseInput({ body: "line one\nline two", bracketedPasteOn: false }));
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("paste mode") });
	});

	it("allows a multi-line body when bracketed paste is on", () => {
		const r = canSendPrompt(baseInput({ body: "line one\nline two", bracketedPasteOn: true }));
		expect(r).toEqual({ ok: true });
	});

	// #404: a freshly launch-signalled harness (no transcript heard yet)
	// must also have gone PTY-quiet before the first paste — the launch
	// ping alone doesn't prove the CLI can accept input yet.
	describe("#404 launch settling", () => {
		it("refuses the first paste while output is still recent after the launch ping", () => {
			const launchSignalAt = 1_000_000;
			const r = canSendPrompt(
				baseInput({
					activity: activity({
						adapterHeard: false,
						launchSignalAt,
						lastOutputAt: launchSignalAt + 200,
					}),
					nowMs: launchSignalAt + 200,
				}),
			);
			expect(r).toEqual({ ok: false, reason: expect.stringContaining("#404") });
		});

		it("allows the paste once nowMs is past the quiet period", () => {
			const launchSignalAt = 1_000_000;
			const r = canSendPrompt(
				baseInput({
					activity: activity({
						adapterHeard: false,
						launchSignalAt,
						lastOutputAt: launchSignalAt + 200,
					}),
					nowMs: launchSignalAt + 200 + LAUNCH_QUIET_MS,
				}),
			);
			expect(r).toEqual({ ok: true });
		});

		it("is unaffected by recent output once the adapter has heard from the transcript", () => {
			const launchSignalAt = 1_000_000;
			const r = canSendPrompt(
				baseInput({
					activity: activity({
						adapterHeard: true,
						launchSignalAt,
						lastOutputAt: launchSignalAt + 200,
					}),
					nowMs: launchSignalAt + 200,
				}),
			);
			expect(r).toEqual({ ok: true });
		});
	});
});

// #41: canInsertText is deliberately more permissive than canSendPrompt
// on phase — a drop has no submit — so its own test suite builds the
// input by hand the same way, minus body/bracketed-paste (irrelevant to
// a paste-only gate).

const insertInput = (over: Partial<CanInsertTextInput> = {}): CanInsertTextInput => ({
	capabilities: HARNESS_KINDS.claude.capabilities,
	activity: activity(),
	registered: true,
	...over,
});

describe("canInsertText", () => {
	it.each(["running", "idle", "waiting"] as const)("allows a drop mid-turn (phase %s)", (phase) => {
		expect(canInsertText(insertInput({ activity: activity({ phase }) }))).toEqual({ ok: true });
	});

	it("refuses a kind with no terminal", () => {
		const r = canInsertText(insertInput({ capabilities: HARNESS_KINDS.files.capabilities }));
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("no terminal") });
	});

	it("refuses a harness that hasn't registered a live terminal", () => {
		const r = canInsertText(insertInput({ registered: false }));
		expect(r.ok).toBe(false);
	});

	it("refuses when there's no activity record at all", () => {
		const r = canInsertText(insertInput({ activity: null }));
		expect(r.ok).toBe(false);
	});

	it("gives permission its own reason — a typed path could answer the dialog", () => {
		const r = canInsertText(insertInput({ activity: activity({ phase: "permission" }) }));
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("permission dialog") });
	});

	it.each(["spawning", "exited"] as const)("refuses phase %s", (phase) => {
		const r = canInsertText(insertInput({ activity: activity({ phase }) }));
		expect(r.ok).toBe(false);
		if (!r.ok) expect(r.reason).toContain(phase);
	});

	it("refuses without a source that has proven it's watching the harness", () => {
		expect(canInsertText(insertInput({ activity: activity({ authoritative: false }) })).ok).toBe(
			false,
		);
		expect(canInsertText(insertInput({ activity: activity({ adapterHeard: false }) })).ok).toBe(
			false,
		);
		expect(canInsertText(insertInput({ activity: activity({ adapterSilent: true }) })).ok).toBe(
			false,
		);
	});

	it("refuses a harness that wasn't spawned with config injection", () => {
		const r = canInsertText(insertInput({ activity: activity({ injected: false }) }));
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("review tools") });
	});

	// #273: same launch-signal equivalence as `canSendPrompt`.
	it("accepts a harness whose only proof is a launch signal", () => {
		const r = canInsertText(
			insertInput({ activity: activity({ adapterHeard: false, launchSignalAt: Date.now() }) }),
		);
		expect(r).toEqual({ ok: true });
	});

	it("still refuses a launch-signalled harness that isn't injected", () => {
		const r = canInsertText(
			insertInput({
				activity: activity({ adapterHeard: false, launchSignalAt: Date.now(), injected: false }),
			}),
		);
		expect(r).toEqual({ ok: false, reason: expect.stringContaining("review tools") });
	});

	it("still refuses a launch-signalled harness whose watchdog gave up", () => {
		const r = canInsertText(
			insertInput({
				activity: activity({
					adapterHeard: false,
					launchSignalAt: Date.now(),
					adapterSilent: true,
				}),
			}),
		);
		expect(r.ok).toBe(false);
	});

	// Same #273/#259 regression as `canSendPrompt` above.
	it("refuses a harness left adapter-silent by the #259 watchdog even with a launch signal", () => {
		const r = canInsertText(
			insertInput({
				activity: activity({
					adapterHeard: false,
					launchSignalAt: Date.now(),
					adapterSilent: true,
					degradedBy: "adapter-silent",
				}),
			}),
		);
		expect(r.ok).toBe(false);
	});
});

describe("formatDroppedPaths", () => {
	it("returns an empty string for no paths", () => {
		expect(formatDroppedPaths([])).toBe("");
	});

	it("joins plain paths with a single space and a trailing space", () => {
		expect(formatDroppedPaths(["/a/b.txt", "/c/d.txt"])).toBe("/a/b.txt /c/d.txt ");
	});

	it("quotes a path containing whitespace", () => {
		expect(formatDroppedPaths(["/a/my file.txt"])).toBe('"/a/my file.txt" ');
	});

	it("quotes only the paths that need it, in a mixed list", () => {
		expect(formatDroppedPaths(["/a/b.txt", "/c/my file.txt"])).toBe('/a/b.txt "/c/my file.txt" ');
	});
});
