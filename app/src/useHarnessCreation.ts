// Harness/room creation, extracted out of App.tsx (#19) — pure move,
// no behaviour change. Owns the two creation entry points (pickHarness
// off the + harness menu, createRoom off New room), the shared
// claimedSessionIds bookkeeping both lean on, the opencode
// sqlite-poll capture fallback, and the Mod+E Files-harness toggle
// (with its own "where did I come from" / in-flight-creation refs).
// App.tsx calls this at the point `createHarnessInRoom` used to start
// and destructures the return value the same way it does
// `useRoomsStore`.
//
// #459: the record building (harnessCreation.ts) and the opencode
// capture poll (opencodeCapture.ts) live in their own modules; this
// file is the React wiring around them. The post-spawn write-backs
// (setHarnessSessionId and friends) live in useHarnessWriteBacks.ts.

import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import { useRef } from "react";
import { ArgvPrepareError, prepareOrThrow, reportArgvFailure } from "./argvFailure.ts";
import { type CreationOutcome, runUserCreation } from "./creationOutcome.ts";
import { HARNESS_KINDS } from "./data.tsx";
import { harnessFreshCmd } from "./harnessArgvApi.ts";
import {
	buildHarness,
	buildRoom,
	type CreateHarnessResult,
	type CreateRoomResult,
	claimedSessionIdsOf,
	harnessDisplayName,
	newId,
	resolveAgentName,
} from "./harnessCreation.ts";
import type { CreateRoomArgs } from "./NewRoomDialog.tsx";
import { captureOpencodeSessionId } from "./opencodeCapture.ts";
import { buildRemoteSpec, type RemoteInput } from "./remoteCmd.ts";
import { pendingSessionIds } from "./reportedSessions.ts";
import { defaultRoomName } from "./roomName.ts";
import type { Harness, HarnessKind, Room } from "./types.ts";
import { useHarnessWriteBacks } from "./useHarnessWriteBacks.ts";

export type { CreateHarnessResult, CreateRoomResult };

export function useHarnessCreation(
	roomsRef: MutableRefObject<Room[]>,
	setRooms: Dispatch<SetStateAction<Room[]>>,
	setOpencodePorts: Dispatch<SetStateAction<Map<string, number>>>,
	setActiveRoomId: Dispatch<SetStateAction<string>>,
	activeRoomIdRef: MutableRefObject<string>,
	defaultCwd: string,
	showPicker: string | null,
	setShowPicker: Dispatch<SetStateAction<string | null>>,
	setShowNewRoom: Dispatch<SetStateAction<boolean>>,
	switchHarnessInRoom: (roomId: string, harnessId: string) => void,
) {
	const writeBacks = useHarnessWriteBacks(setRooms);
	const { setHarnessSessionId } = writeBacks;
	// #49 phase A: per room, the last PTY harness the user was on
	// before Mod+E jumped to a Files harness — so Mod+E toggles back
	// to where they came from, not just "the first terminal".
	const lastPtyHarnessRef = useRef(new Map<string, string>());
	// Rooms with a files-harness creation in flight (Mod+E latch).
	const creatingFilesRef = useRef(new Set<string>());

	// A rejected daemon argv call on a user-driven creation: a toast, and
	// nothing was created (the throw comes before any state change). Agent
	// callers use the throwing versions and answer their request with it.
	const reportCreationFailure = (action: string, err: unknown, roomId?: string) => {
		if (!(err instanceof ArgvPrepareError)) throw err;
		const owner = roomsRef.current.find((r) => r.id === (roomId ?? activeRoomIdRef.current));
		reportArgvFailure(action, err, owner);
	};

	const rememberPort = (harnessId: string, port: number) =>
		setOpencodePorts((prev) => new Map(prev).set(harnessId, port));

	// All session ids any harness has already captured. captureOpencode
	// excludes these so a fresh capture can't claim someone else's id
	// when two opencode harnesses race in the same cwd.
	const claimedSessionIds = (): Set<string> =>
		// #755: a just-reported id is in no record until its rooms-changed lands.
		new Set([...claimedSessionIdsOf(roomsRef.current), ...pendingSessionIds()]);

	// #49 phase A: harness creation, callable from both the + harness
	// menu (pickHarness) and the Mod+E files jump. Kind-specific setup
	// branches on capabilities — `files` is a surface, not a process:
	// no session id, no port, no cmd, and it starts (and stays) idle.
	//
	// #411: `opts.activate` (default true, same default/shape as
	// `createRoom`'s below) is what lets `open_harness` add a harness in
	// the background — the agent verb's contract is that it never steals
	// focus or switches the room's own active harness, unlike every
	// human-driven caller of this function. `opts.createdBy` is stamped
	// onto the new harness record at construction, same as `createRoom`
	// stamps `Room.createdBy` — never written after the fact.
	const createHarnessInRoom = async (
		targetRoomId: string,
		kind: HarnessKind,
		agent?: string,
		opts?: {
			activate?: boolean;
			createdBy?: Harness["createdBy"];
			remote?: RemoteInput;
			hermesProfile?: string;
		},
	): Promise<CreateHarnessResult | undefined> => {
		const activate = opts?.activate ?? true;
		const targetRoom = roomsRef.current.find((r) => r.id === targetRoomId);
		if (!targetRoom) return undefined;
		const caps = HARNESS_KINDS[kind].capabilities;
		const agentName = resolveAgentName(kind, agent);
		const id = newId("h");
		// #568: the session name is minted from this same id, so the
		// record and its argv agree. A remote harness without a host is
		// refused rather than spawned with an empty argv.
		if (kind === "remote" && !opts?.remote) return undefined;
		const remote = opts?.remote ? buildRemoteSpec(targetRoomId, id, opts.remote) : undefined;
		const cwd = targetRoom.cwd ?? defaultCwd;
		// Computed here (off the pre-await snapshot above) rather than
		// inside the `setRooms` updater below, so the same value can be
		// returned to the caller — `open_harness` reports it back.
		const name = harnessDisplayName(kind, targetRoom.harnesses.length);
		// #730: the daemon builds the argv. It mints Claude's conversation id
		// (so the harness resumes to *this* conversation on restart),
		// allocates opencode's embedded-server port and resolves the Hermes
		// profile (new Hermes harnesses run in Skein's own unless told
		// otherwise). All of it lands *before* the cmd goes into state -
		// LiveTerminal mounts the PTY synchronously off the new record.
		let cmd: string[] | undefined;
		let sessionId: string | undefined;
		let hermesProfile: string | undefined;
		if (caps.pty) {
			const fresh = await prepareOrThrow("harness_fresh_cmd", () =>
				harnessFreshCmd({
					kind,
					newHarness: true,
					...(agentName ? { agent: agentName } : {}),
					...(remote ? { remote } : {}),
					...(opts?.hermesProfile ? { hermesProfile: opts.hermesProfile } : {}),
				}),
			);
			if (kind === "remote" && fresh.cmd.length === 0) return undefined;
			cmd = fresh.cmd;
			sessionId = fresh.sessionId ?? undefined;
			hermesProfile = fresh.hermesProfile ?? undefined;
			// A failed port allocation comes back null with a bare argv: the
			// L2c-2 adapter just doesn't attach (the pre-#730 fallback).
			if (fresh.opencodePort !== null) rememberPort(id, fresh.opencodePort);
		}
		setRooms((prev) =>
			prev.map((r) => {
				if (r.id !== targetRoomId) return r;
				const newH = buildHarness({
					id,
					kind,
					name,
					cwd,
					cmd,
					sessionId,
					agentName,
					hermesProfile,
					remote,
					createdBy: opts?.createdBy,
				});
				return {
					...r,
					harnesses: [...r.harnesses, newH],
					...(activate ? { activeHarnessId: id } : {}),
				};
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
		return { harnessId: id, kind, agent: agentName ?? null, name };
	};

	const pickHarness = (
		kind: HarnessKind,
		agent?: string,
		remote?: RemoteInput,
		hermesProfile?: string,
	) => {
		const targetRoomId = showPicker;
		setShowPicker(null);
		if (!targetRoomId) return;
		void createHarnessInRoom(targetRoomId, kind, agent, {
			...(remote ? { remote } : {}),
			...(hermesProfile ? { hermesProfile } : {}),
		}).catch((err: unknown) => reportCreationFailure("add the harness", err, targetRoomId));
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
		{
			cwd,
			task,
			harness,
			agent,
			hermesProfile: profileArg,
			branch,
			repoRoot,
			createdBy,
		}: CreateRoomArgs,
		opts?: { activate?: boolean },
	): Promise<CreateRoomResult> => {
		const activate = opts?.activate ?? true;
		const sid = newId("s");
		const hid = newId("h");
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
		const agentName = resolveAgentName(harness, agent);
		// #730: argv, Claude's session id, opencode's port and the Hermes
		// profile all come from the daemon, before the record exists.
		const fresh = startCaps.pty
			? await prepareOrThrow("harness_fresh_cmd", () =>
					harnessFreshCmd({
						kind: harness,
						newHarness: true,
						...(agentName ? { agent: agentName } : {}),
						...(profileArg ? { hermesProfile: profileArg } : {}),
					}),
				)
			: undefined;
		if (fresh && fresh.opencodePort !== null) rememberPort(hid, fresh.opencodePort);
		const sessionId = fresh?.sessionId ?? undefined;
		const hermesProfile = fresh?.hermesProfile ?? undefined;
		const newRoom: Room = buildRoom({
			roomId: sid,
			harness: buildHarness({
				id: hid,
				kind: harness,
				name: "main",
				cwd,
				cmd: fresh?.cmd,
				sessionId,
				agentName,
				hermesProfile,
			}),
			cwd,
			task,
			branch,
			repoRoot,
			folderName,
			activate,
			createdBy,
		});
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

	// The New Room dialog awaits the outcome to clear its busy flag.
	const createRoomForUser = (
		args: CreateRoomArgs,
		opts?: { activate?: boolean },
	): Promise<CreationOutcome> =>
		runUserCreation(
			() => createRoom(args, opts),
			(err) => reportCreationFailure("create the room", err),
		);

	return {
		claimedSessionIds,
		writeBacks,
		createHarnessInRoom,
		pickHarness,
		toggleFilesHarness,
		createRoom,
		createRoomForUser,
	};
}
