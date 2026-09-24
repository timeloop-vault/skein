// Harness/room creation, extracted out of App.tsx (#19) — pure move,
// no behaviour change. Owns the two creation entry points (pickHarness
// off the + harness menu, createRoom off New room), the shared
// sessionId bookkeeping both lean on (claimedSessionIds,
// setHarnessSessionId, replaceHarnessSessionId), the opencode
// sqlite-poll capture fallback, and the Mod+E Files-harness toggle
// (with its own "where did I come from" / in-flight-creation refs).
// App.tsx calls this at the point `createHarnessInRoom` used to start
// and destructures the return value the same way it does
// `useRoomsStore`.

import { invoke } from "@tauri-apps/api/core";
import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import { useRef } from "react";
import type { CreateRoomArgs } from "./NewRoomDialog.tsx";
import { HARNESS_KINDS } from "./data.tsx";
import { cmdForKind } from "./harnessCmd.ts";
import { defaultRoomName } from "./roomName.ts";
import type { Harness, HarnessKind, Room } from "./types.ts";

const newId = (prefix: string): string => prefix + Math.random().toString(36).slice(2, 7);

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

// Mapping from harness kind → argv. Each binary must be on PATH for the
// spawn to succeed; if it isn't, the LiveTerminal renders the error
// inline and the user can pick another kind. The `byoh` kind is our
// "Shell" option — it drops into the user's default shell so they can
// run whatever they want.
// Phase 2b: opencode has no Claude-style --session-id pre-allocation.
// Snapshot opencode's existing sessions for this cwd, spawn the
// harness, then poll the same query looking for an id that wasn't in
// the snapshot AND isn't already claimed by some other Skein harness.
// First match wins; that's this harness's session.
//
// Why polling at all (not a file/db watcher): the capture window is
// short relative to a session lifetime, and opencode writes the row
// once. Watcher's lifetime cost > polling's burst.
//
// Why a long timeout (5 minutes): opencode appears to write the
// session row only on the first user input, not at spawn — so a
// short window misses it whenever the user takes a beat to start
// typing. 5 min covers nearly every realistic case; on timeout we
// quietly leave sessionId undefined and resume falls back to
// phase-5a's --continue.
//
// `claimedIds` returns the set of session ids any *other* harness
// has already captured. If two opencode harnesses spawn in the same
// cwd within seconds, the snapshot diff alone can't tell them apart;
// excluding already-claimed ids breaks the tie deterministically.
const captureOpencodeSessionId = async (
	cwd: string,
	claimedIds: () => Set<string>,
	onCapture: (sessionId: string) => void,
): Promise<void> => {
	let snapshot: string[];
	try {
		snapshot = await invoke<string[]>("opencode_list_sessions", { cwd });
	} catch (err) {
		console.warn("[skein] opencode capture: snapshot failed", err);
		return;
	}
	const before = new Set(snapshot);
	const startedAt = Date.now();
	const deadline = startedAt + 5 * 60 * 1000;
	console.info(`[skein] opencode capture started for ${cwd} (snapshot ${before.size} sessions)`);
	while (Date.now() < deadline) {
		// Backoff: tight (250 ms) for the first 5 s in case opencode
		// is fast, then 1 s for the next 25 s, then 5 s thereafter.
		const elapsed = Date.now() - startedAt;
		const waitMs = elapsed < 5_000 ? 250 : elapsed < 30_000 ? 1_000 : 5_000;
		await new Promise((resolve) => setTimeout(resolve, waitMs));
		try {
			const current = await invoke<string[]>("opencode_list_sessions", { cwd });
			const taken = claimedIds();
			const fresh = current.find((id) => !before.has(id) && !taken.has(id));
			if (fresh) {
				console.info(`[skein] opencode capture: ${fresh} (${cwd})`);
				onCapture(fresh);
				return;
			}
		} catch {
			// Transient — try again on the next tick.
		}
	}
	console.warn(`[skein] opencode capture timed out for ${cwd}`);
};

export function useHarnessCreation(
	roomsRef: MutableRefObject<Room[]>,
	setRooms: Dispatch<SetStateAction<Room[]>>,
	setOpencodePorts: Dispatch<SetStateAction<Map<string, number>>>,
	setActiveRoomId: Dispatch<SetStateAction<string>>,
	activeRoomIdRef: MutableRefObject<string>,
	defaultShell: string[],
	defaultCwd: string,
	showPicker: string | null,
	setShowPicker: Dispatch<SetStateAction<string | null>>,
	setShowNewRoom: Dispatch<SetStateAction<boolean>>,
	switchHarnessInRoom: (roomId: string, harnessId: string) => void,
) {
	// #49 phase A: per room, the last PTY harness the user was on
	// before Mod+E jumped to a Files harness — so Mod+E toggles back
	// to where they came from, not just "the first terminal".
	const lastPtyHarnessRef = useRef(new Map<string, string>());
	// Rooms with a files-harness creation in flight (Mod+E latch).
	const creatingFilesRef = useRef(new Set<string>());

	// All session ids any harness has already captured. captureOpencode
	// excludes these so a fresh capture can't claim someone else's id
	// when two opencode harnesses race in the same cwd.
	const claimedSessionIds = (): Set<string> =>
		new Set(
			roomsRef.current
				.flatMap((s) => s.harnesses.map((h) => h.sessionId))
				.filter((id): id is string => typeof id === "string"),
		);

	// Update one harness's sessionId after phase 2b's async capture
	// finds the new opencode row. Wrapped here so both creation paths
	// (pickHarness, createRoom) share the same setRooms shape.
	const setHarnessSessionId = (targetRoomId: string, harnessId: string, captured: string) => {
		setRooms((prev) =>
			prev.map((r) => {
				if (r.id !== targetRoomId) return r;
				return {
					...r,
					harnesses: r.harnesses.map((h) => {
						if (h.id !== harnessId) return h;
						// Idempotent: first-writer wins. Epic #50 L2c-2
						// races the SSE adapter's `session.created`
						// against the chapter-5 sqlite poll; whichever
						// fires first sets sessionId, the other becomes
						// a no-op. Without this guard, the sqlite poll
						// could find a *different* session (e.g. user
						// ran opencode in the same cwd from a shell
						// alongside) and overwrite the right id.
						if (h.sessionId) return h;
						return { ...h, sessionId: captured };
					}),
				};
			}),
		);
	};

	// #116 step two: overwrite a harness's sessionId after Claude's own
	// `/clear`/`/resume`/fork hook reports the conversation moved onto a
	// new id — and (step four) after opencode's own adapter decides the
	// harness followed its TUI onto a different root session (`/new` or
	// a `/sessions` pick). This is deliberately NOT `setHarnessSessionId`
	// above — that one is first-writer-wins to survive opencode's
	// create-vs-poll capture race, but both these reports are
	// authoritative: the old id is definitely gone (Claude) or definitely
	// superseded (opencode), so the new one must win even though
	// `sessionId` is already set. `cmd` is untouched here — it's part of
	// LiveTerminal's mountKey, so changing it would respawn the PTY;
	// `resumeCmd` (harnessCmd.ts) rebuilds the argv from `sessionId` on
	// the next boot/reopen, which is what makes the new conversation the
	// one resumed.
	const replaceHarnessSessionId = (targetRoomId: string, harnessId: string, sessionId: string) => {
		setRooms((prev) =>
			prev.map((r) => {
				if (r.id !== targetRoomId) return r;
				return {
					...r,
					harnesses: r.harnesses.map((h) => (h.id === harnessId ? { ...h, sessionId } : h)),
				};
			}),
		);
	};

	// #49 phase A: harness creation, callable from both the + harness
	// menu (pickHarness) and the Mod+E files jump. Kind-specific setup
	// branches on capabilities — `files` is a surface, not a process:
	// no session id, no port, no cmd, and it starts (and stays) idle.
	const createHarnessInRoom = async (targetRoomId: string, kind: HarnessKind, agent?: string) => {
		const targetRoom = roomsRef.current.find((r) => r.id === targetRoomId);
		if (!targetRoom) return;
		const caps = HARNESS_KINDS[kind].capabilities;
		// The agent only survives onto the record for kinds that take it.
		// Every other path reads it back from there, so a name that got
		// this far on a shell harness would ride into the argv.
		const agentName = caps.agents && agent?.trim() ? agent : undefined;
		const id = newId("h");
		const cwd = targetRoom.cwd ?? defaultCwd;
		// Phase 2a: pre-allocate Claude's conversation id so the harness
		// resumes to *this* conversation on Skein restart — no picker.
		const sessionId = kind === "claude" ? crypto.randomUUID() : undefined;
		// Epic #50 L2c-2: allocate the opencode embedded-server port
		// *before* we set the cmd into state — LiveTerminal mounts the
		// PTY synchronously off the new harness record, so the port has
		// to be baked into the argv at that moment.
		let opencodePort: number | undefined;
		if (kind === "opencode") {
			try {
				opencodePort = await invoke<number>("pick_free_port");
				setOpencodePorts((prev) => {
					const m = new Map(prev);
					m.set(id, opencodePort as number);
					return m;
				});
			} catch (err) {
				console.warn("[skein] pick_free_port failed; falling back to L2a:", err);
			}
		}
		const cmd = caps.pty
			? cmdForKind(kind, defaultShell, sessionId, opencodePort, agentName)
			: undefined;
		setRooms((prev) =>
			prev.map((r) => {
				if (r.id !== targetRoomId) return r;
				const newH: Harness = {
					id,
					kind,
					// The ◇ label makes a poor name stem — files harnesses
					// read better as "files-2" than "◇-2".
					name: `${kind === "files" ? "files" : HARNESS_KINDS[kind].label}-${r.harnesses.length + 1}`,
					status: caps.pty ? "running" : "idle",
					model: caps.pty ? (kind === "copilot" ? "gpt-5" : "sonnet-4.5") : "",
					tokens: "0",
					...(caps.pty ? { live: true } : {}),
					...(cmd ? { cmd } : {}),
					cwd,
					...(sessionId ? { sessionId } : {}),
					...(agentName ? { agent: agentName } : {}),
				};
				return { ...r, harnesses: [...r.harnesses, newH], activeHarnessId: id };
			}),
		);
		// Phase 2b: kick off async capture for opencode harnesses. The
		// snapshot has to happen *before* opencode writes its session
		// row, which it doesn't do until LiveTerminal mounts and spawns
		// the binary — fine to fire-and-forget here, the React render
		// cycle keeps us ahead of the spawn.
		//
		// Epic #50 L2c-2: this is the sqlite-poll *fallback*. The
		// primary path is the SSE adapter capturing `session.created`
		// (wired in LiveTerminal). If the adapter beats this poll,
		// `setHarnessSessionId` is idempotent — the second write sees
		// `sessionId` already populated and the diff lookup in
		// `captureOpencodeSessionId` excludes it via `claimedSessionIds`.
		if (kind === "opencode") {
			void captureOpencodeSessionId(cwd, claimedSessionIds, (captured) => {
				setHarnessSessionId(targetRoomId, id, captured);
			});
		}
	};

	const pickHarness = (kind: HarnessKind, agent?: string) => {
		const targetRoomId = showPicker;
		setShowPicker(null);
		if (!targetRoomId) return;
		void createHarnessInRoom(targetRoomId, kind, agent);
	};

	// #49 phase A: Mod+E = jump to the room's Files harness (creating
	// one if none exists — decided on the epic) ⇄ back to the PTY
	// harness the user came from. Reads through refs so the []-dep'd
	// keydown dispatcher can call it.
	const toggleFilesHarness = () => {
		const rid = activeRoomIdRef.current;
		if (!rid) return;
		const r = roomsRef.current.find((x) => x.id === rid);
		if (!r) return;
		// The picker pane hides every body — a jump would succeed
		// invisibly behind it. Dismiss it so the result is seen.
		setShowPicker(null);
		const activeH = r.harnesses.find((h) => h.id === r.activeHarnessId);
		if (activeH && !HARNESS_KINDS[activeH.kind].capabilities.pty) {
			// On a Files harness: bounce back to where the user came
			// from (falls back to the room's first PTY harness when the
			// remembered one is gone or the jump was never recorded).
			// Deliberately not gated on cwd — bouncing out never needs one.
			const backId = lastPtyHarnessRef.current.get(rid);
			const back =
				(backId ? r.harnesses.find((h) => h.id === backId) : undefined) ??
				r.harnesses.find((h) => HARNESS_KINDS[h.kind].capabilities.pty);
			if (back) switchHarnessInRoom(rid, back.id);
			return;
		}
		if (!r.cwd) return; // creating/jumping to Files needs a folder
		if (activeH) lastPtyHarnessRef.current.set(rid, activeH.id);
		const files = r.harnesses.find((h) => !HARNESS_KINDS[h.kind].capabilities.pty);
		if (files) {
			switchHarnessInRoom(rid, files.id);
			return;
		}
		// Synchronous latch: Mod+E key-repeats faster than a React
		// commit under load — without it a held key creates duplicates.
		if (creatingFilesRef.current.has(rid)) return;
		creatingFilesRef.current.add(rid);
		void createHarnessInRoom(rid, "files").finally(() => {
			creatingFilesRef.current.delete(rid);
		});
	};

	// #328: `opts.activate` (default true) is what lets a caller create a
	// room without switching to it — #330's incoming request handler is
	// the first one that needs this. `activate: false` leaves
	// `activeRoomId`/`showNewRoom` alone and marks the room `attention`
	// instead (see the field's doc in types.ts), which is what puts a
	// dot on its tab until the user visits it. Returns the new room's
	// identifying fields (#330: the `create_room` agent verb reports
	// these back to the caller), not just the id — every field a caller
	// can't otherwise reconstruct (kind/agent are on the record but
	// `sessionId` for an opencode room isn't settled until the async
	// capture below lands).
	const createRoom = async (
		{ cwd, task, harness, agent, branch, repoRoot, createdBy }: CreateRoomArgs,
		opts?: { activate?: boolean },
	): Promise<CreateRoomResult> => {
		const activate = opts?.activate ?? true;
		const sid = newId("s");
		const hid = newId("h");
		// Phase 2a: pre-allocate Claude's conversation id (see pickHarness).
		const sessionId = harness === "claude" ? crypto.randomUUID() : undefined;
		// Epic #50 L2c-2: pre-allocate opencode's embedded-server port
		// before we bake the cmd into the harness record.
		let opencodePort: number | undefined;
		if (harness === "opencode") {
			try {
				opencodePort = await invoke<number>("pick_free_port");
				setOpencodePorts((prev) => {
					const m = new Map(prev);
					m.set(hid, opencodePort as number);
					return m;
				});
			} catch (err) {
				console.warn("[skein] pick_free_port failed; falling back to L2a:", err);
			}
		}
		// Display name from the trailing path component — `D:\code\skein`
		// → `skein`. Cosmetic; the actual cwd is what spawns use. #241:
		// also the room's initial `name` (no "local · " prefix) — the user
		// can rename it afterwards; this only sets the default.
		const folderName = defaultRoomName(cwd);
		// Repo / branch are only set for git-backed rooms (chapter 6
		// phase 3). For non-git rooms the tab subtext shows just the
		// folder name and LiveStatus is replaced by a placeholder.
		const startCaps = HARNESS_KINDS[harness].capabilities;
		// Same gate as `createHarnessInRoom`: a kind that does not take
		// `--agent` never carries a name onto its record.
		const agentName = startCaps.agents && agent?.trim() ? agent : undefined;
		const newRoom: Room = {
			id: sid,
			name: folderName,
			task,
			// A files-only room has nothing running — and since a files
			// harness never registers activity, the aggregate can't
			// correct a wrong persisted "running" later.
			status: startCaps.pty ? "running" : "idle",
			badge: 0,
			cwd,
			...(branch ? { branch, repo: folderName } : {}),
			...(repoRoot ? { repoRoot } : {}),
			harnesses: [
				{
					id: hid,
					kind: harness,
					name: "main",
					status: startCaps.pty ? "running" : "idle",
					model: startCaps.pty ? (harness === "copilot" ? "gpt-5" : "sonnet-4.5") : "",
					tokens: "0",
					...(startCaps.pty ? { live: true } : {}),
					...(startCaps.pty
						? { cmd: cmdForKind(harness, defaultShell, sessionId, opencodePort, agentName) }
						: {}),
					cwd,
					...(sessionId ? { sessionId } : {}),
					...(agentName ? { agent: agentName } : {}),
				},
			],
			activeHarnessId: hid,
			...(activate ? {} : { attention: true }),
			...(createdBy ? { createdBy } : {}),
		};
		setRooms((prev) => [...prev, newRoom]);
		if (activate) {
			setActiveRoomId(sid);
			setShowNewRoom(false);
		}
		// Phase 2b sqlite-poll fallback (see pickHarness comment for
		// the relationship with L2c-2's SSE capture).
		if (harness === "opencode") {
			void captureOpencodeSessionId(cwd, claimedSessionIds, (captured) => {
				setHarnessSessionId(sid, hid, captured);
			});
		}
		return {
			roomId: sid,
			name: folderName,
			cwd,
			repo: branch ? folderName : null,
			branch: branch ?? null,
			harnessId: hid,
			kind: harness,
			agent: agentName ?? null,
			// Claude's is pre-allocated above and settled by construction;
			// opencode's isn't captured until the async poll/SSE race above
			// resolves, well after this function returns.
			sessionId: sessionId ?? null,
		};
	};

	return {
		claimedSessionIds,
		setHarnessSessionId,
		replaceHarnessSessionId,
		createHarnessInRoom,
		pickHarness,
		toggleFilesHarness,
		createRoom,
	};
}
