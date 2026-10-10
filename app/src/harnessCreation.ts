// Pure record-building for harness/room creation, split out of
// useHarnessCreation.ts (#459): what a new Harness / Room looks like and
// how a captured sessionId lands on one. No React, no invoke — the hook
// owns the async/port/state wiring around these.

import { HARNESS_KINDS } from "./data.tsx";
import type { Harness, HarnessKind, RemoteSpec, Room } from "./types.ts";

export const newId = (prefix: string): string => prefix + Math.random().toString(36).slice(2, 7);

// #330: what `createRoom` hands back — everything the `create_room`
// agent verb reports to the caller. `null`, not an absent key, for
// every field that has no value right now: this crosses into a
// `serde_json::Value` on its way back to Rust, where an absent object
// key and an explicit JSON `null` are NOT the same "I don't know" the
// rest of this codebase treats them as.
export interface CreateRoomResult {
	roomId: string;
	name: string;
	cwd: string;
	repo: string | null;
	branch: string | null;
	harnessId: string;
	kind: HarnessKind;
	agent: string | null;
	sessionId: string | null;
}

// #411: what `createHarnessInRoom` hands back for the `open_harness`
// agent verb to report — everything the caller can't otherwise
// reconstruct from the request it sent (the resolved `agent` and the
// name Skein picked). `undefined` when the target room didn't exist by
// the time this ran, mirroring `createHarnessInRoom`'s existing no-op
// early return.
export interface CreateHarnessResult {
	harnessId: string;
	kind: HarnessKind;
	agent: string | null;
	name: string;
}

// The agent only survives onto the record for kinds that take it.
// Every other path reads it back from there, so a name that got
// this far on a shell harness would ride into the argv.
export const resolveAgentName = (kind: HarnessKind, agent?: string): string | undefined =>
	HARNESS_KINDS[kind].capabilities.agents && agent?.trim() ? agent : undefined;

export const harnessDisplayName = (kind: HarnessKind, existingCount: number): string =>
	`${kind === "files" || kind === "design" ? kind : HARNESS_KINDS[kind].label}-${existingCount + 1}`;

export interface NewHarnessFields {
	id: string;
	kind: HarnessKind;
	name: string;
	cwd: string;
	cmd: string[] | undefined;
	sessionId: string | undefined;
	agentName: string | undefined;
	remote?: RemoteSpec | undefined;
	createdBy?: Harness["createdBy"] | undefined;
}

// `files` is a surface, not a process: no cmd, and it starts (and
// stays) idle.
export function buildHarness(f: NewHarnessFields): Harness {
	const pty = HARNESS_KINDS[f.kind].capabilities.pty;
	return {
		id: f.id,
		kind: f.kind,
		name: f.name,
		status: pty ? "running" : "idle",
		model: pty ? (f.kind === "copilot" ? "gpt-5" : "sonnet-4.5") : "",
		tokens: "0",
		...(pty ? { live: true } : {}),
		...(f.cmd ? { cmd: f.cmd } : {}),
		cwd: f.cwd,
		...(f.sessionId ? { sessionId: f.sessionId } : {}),
		...(f.agentName ? { agent: f.agentName } : {}),
		...(f.remote ? { remote: f.remote } : {}),
		...(f.createdBy ? { createdBy: f.createdBy } : {}),
	};
}

export interface NewRoomFields {
	roomId: string;
	harness: Harness;
	cwd: string;
	task: Room["task"];
	branch?: string | undefined;
	repoRoot?: string | undefined;
	folderName: string;
	activate: boolean;
	createdBy?: Room["createdBy"] | undefined;
}

export function buildRoom(f: NewRoomFields): Room {
	const pty = HARNESS_KINDS[f.harness.kind].capabilities.pty;
	return {
		id: f.roomId,
		name: f.folderName,
		task: f.task,
		// A files-only room has nothing running — and since a files
		// harness never registers activity, the aggregate can't
		// correct a wrong persisted "running" later.
		status: pty ? "running" : "idle",
		badge: 0,
		cwd: f.cwd,
		...(f.branch ? { branch: f.branch, repo: f.folderName } : {}),
		...(f.repoRoot ? { repoRoot: f.repoRoot } : {}),
		harnesses: [f.harness],
		activeHarnessId: f.harness.id,
		...(f.activate ? {} : { attention: true }),
		...(f.createdBy ? { createdBy: f.createdBy } : {}),
	};
}

// All session ids any harness has already captured.
export const claimedSessionIdsOf = (rooms: Room[]): Set<string> =>
	new Set(
		rooms
			.flatMap((s) => s.harnesses.map((h) => h.sessionId))
			.filter((id): id is string => typeof id === "string"),
	);

// Idempotent: first-writer wins. Epic #50 L2c-2 races the SSE adapter's
// `session.created` against the chapter-5 sqlite poll; whichever fires
// first sets sessionId, the other becomes a no-op. Without this guard,
// the sqlite poll could find a *different* session (e.g. user ran
// opencode in the same cwd from a shell alongside) and overwrite the
// right id.
export const withCapturedSessionId = (
	rooms: Room[],
	targetRoomId: string,
	harnessId: string,
	captured: string,
): Room[] =>
	rooms.map((r) => {
		if (r.id !== targetRoomId) return r;
		return {
			...r,
			harnesses: r.harnesses.map((h) => {
				if (h.id !== harnessId) return h;
				if (h.sessionId) return h;
				return { ...h, sessionId: captured };
			}),
		};
	});

// Authoritative overwrite (see replaceHarnessSessionId in the hook).
export const withReplacedSessionId = (
	rooms: Room[],
	targetRoomId: string,
	harnessId: string,
	sessionId: string,
): Room[] =>
	rooms.map((r) => {
		if (r.id !== targetRoomId) return r;
		return {
			...r,
			harnesses: r.harnesses.map((h) => (h.id === harnessId ? { ...h, sessionId } : h)),
		};
	});

// Map one harness; the original array and room objects come back untouched
// when `fn` returns the same harness (no churn for autosave/renders).
const mapHarness = (
	rooms: Room[],
	targetRoomId: string,
	harnessId: string,
	fn: (h: Harness) => Harness,
): Room[] => {
	let changed = false;
	const next = rooms.map((r) => {
		if (r.id !== targetRoomId) return r;
		let roomChanged = false;
		const harnesses = r.harnesses.map((h) => {
			if (h.id !== harnessId) return h;
			const n = fn(h);
			if (n !== h) roomChanged = true;
			return n;
		});
		if (!roomChanged) return r;
		changed = true;
		return { ...r, harnesses };
	});
	return changed ? next : rooms;
};

// #520: the post-exit-shell claim. Writes `sessionId` AND the claim in one
// update, so the invariant "claim.sessionId === sessionId" holds on the record.
export const withShellClaim = (
	rooms: Room[],
	targetRoomId: string,
	harnessId: string,
	sessionId: string,
	port?: number,
): Room[] =>
	mapHarness(rooms, targetRoomId, harnessId, (h) =>
		h.sessionId === sessionId && h.shellClaim?.sessionId === sessionId && h.shellClaim.port === port
			? h
			: { ...h, sessionId, shellClaim: port === undefined ? { sessionId } : { sessionId, port } },
	);

// #520: the claim was released; the key is removed, not set to undefined.
export const withoutShellClaim = (rooms: Room[], targetRoomId: string, harnessId: string): Room[] =>
	mapHarness(rooms, targetRoomId, harnessId, (h) => {
		if (!("shellClaim" in h)) return h;
		const { shellClaim: _released, ...rest } = h;
		return rest;
	});

// A cmd replacement (Enter-for-shell, restart) is a fresh process: bump
// spawnGen, optionally set the session id, and drop any shell claim (#520).
export const withRespawnedCmd = (
	rooms: Room[],
	targetRoomId: string,
	harnessId: string,
	cmd: string[],
	sessionId?: string,
): Room[] =>
	mapHarness(rooms, targetRoomId, harnessId, (h) => {
		const { shellClaim: _dropped, ...rest } = h;
		return {
			...rest,
			cmd,
			...(sessionId !== undefined ? { sessionId } : {}),
			spawnGen: (h.spawnGen ?? 0) + 1,
		};
	});
