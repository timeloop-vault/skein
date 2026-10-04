// #330: the frontend half of the `create_room` agent verb — the
// listener for #328's `skein://agent-request` round trip. Mounted once
// from App.tsx, alongside `useMailDelivery` (#329), which is the other
// hook driven entirely off Tauri events rather than `rooms` props.
//
// Every request MUST be answered exactly once, `ok` or `error` — the
// Rust side is a real HTTP handler blocked on the answer (`state.rs`'s
// `await_response`), and an unanswered request just times out rather
// than hanging forever, but a silently-swallowed one would leave the
// calling agent's tool call erroring for the wrong reason. The `kind`
// dispatch below and its `catch` both funnel into `complete`, so a
// thrown error (a bad git call inside `createRoomArgs`, say) still
// answers rather than leaking.
//
// The actual parsing/resolution/shaping logic lives in the pure
// `agentRequests.ts` — this file is only the Tauri plumbing: listen,
// dispatch by `kind`, call the one or two async things a pure module
// can't (list_harness_agents, git, createRoom), answer.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { MutableRefObject } from "react";
import { useEffect, useRef } from "react";
import {
	type CloseRoomAttribution,
	decideOpenHarness,
	derivePromptFirstLine,
	parseCloseHarnessArgs,
	parseCloseRoomArgs,
	parseCreateArgs,
	parseOpenHarnessArgs,
	parseOpenHarnessResolveArgs,
	parseResolveArgs,
	type RequestResult,
	resolveAgent,
	resolveKind,
	specFromCreateArgs,
} from "./agentRequests.ts";
import { listHarnessAgents } from "./agents.ts";
import { workSnapshot } from "./deferral.ts";
import { handleDesignRequest, isDesignKind } from "./designAgentRequests.ts";
import { harnessActivity } from "./harnessActivity.ts";
import type { CreateRoomArgs } from "./NewRoomDialogTypes.ts";
import { branchTemplateFor, type DefaultAgents, defaultsFor, type NewRoomMemory } from "./prefs.ts";
import { fetchScope } from "./review/api.ts";
import type { NewToast } from "./toastStack.ts";
import type { Harness, HarnessKind, Room } from "./types.ts";
import type { CreateHarnessResult, CreateRoomResult } from "./useHarnessCreation.ts";
import { createRoomArgs } from "./worktreeRoom.ts";

interface AgentRequestPayload {
	id: string;
	kind: string;
	args: unknown;
}

export function useAgentRequests(
	createRoom: (args: CreateRoomArgs, opts?: { activate?: boolean }) => Promise<CreateRoomResult>,
	newRoomMemory: NewRoomMemory,
	defaultAgents: DefaultAgents,
	appBranchTemplate: string,
	pushToast: (entry: NewToast) => void,
	// #411: `useRoomsStore`'s agent-close, never the user-facing
	// `closeRoom` — this one must never prompt.
	closeRoomForAgent: (
		roomId: string,
		closedBy: CloseRoomAttribution,
	) => RequestResult<{ roomId: string; archived: number }>,
	// #411: full room list (including archived) for `open_harness` and
	// its resolve round trip — both need to look a target room up by id,
	// the same shape `decideOpenHarness`/`decideCloseRoom` take.
	roomsRef: MutableRefObject<Room[]>,
	// #411: `useHarnessCreation`'s creation entry point, called with
	// `{activate: false}` so opening a harness in another room never
	// steals focus or switches that room's own active harness.
	createHarnessInRoom: (
		targetRoomId: string,
		kind: HarnessKind,
		agent?: string,
		opts?: { activate?: boolean; createdBy?: Harness["createdBy"] },
	) => Promise<CreateHarnessResult | undefined>,
	// #411: `useHarnessActions`'s agent-close for one harness, never the
	// user-facing `closeHarness` — this one must never confirm.
	closeHarnessForAgent: (
		roomId: string,
		harnessId: string,
	) => RequestResult<{ harnessId: string; phase: string }>,
	// #512: `useHarnessActions.setHarnessDesignEntry` — the toolbar picker's
	// own persist path, used by `design.open_entry`.
	setDesignEntry: (roomId: string, harnessId: string, entry: string) => void,
): void {
	// Refs so the listener (mounted once, below) always reads the latest
	// values without re-subscribing on every settings/memory change —
	// same shape `useHarnessNotifications.ts`'s `notifyBadgeRef` and
	// friends use.
	const createRoomRef = useRef(createRoom);
	createRoomRef.current = createRoom;
	const memoryRef = useRef(newRoomMemory);
	memoryRef.current = newRoomMemory;
	const defaultAgentsRef = useRef(defaultAgents);
	defaultAgentsRef.current = defaultAgents;
	const branchTemplateRef = useRef(appBranchTemplate);
	branchTemplateRef.current = appBranchTemplate;
	const pushToastRef = useRef(pushToast);
	pushToastRef.current = pushToast;
	const closeRoomForAgentRef = useRef(closeRoomForAgent);
	closeRoomForAgentRef.current = closeRoomForAgent;
	const createHarnessInRoomRef = useRef(createHarnessInRoom);
	createHarnessInRoomRef.current = createHarnessInRoom;
	const closeHarnessForAgentRef = useRef(closeHarnessForAgent);
	closeHarnessForAgentRef.current = closeHarnessForAgent;
	const setDesignEntryRef = useRef(setDesignEntry);
	setDesignEntryRef.current = setDesignEntry;

	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef comes from useRoomsStore (#19) — a ref, stable across renders, but biome can't prove that through a parameter.
	useEffect(() => {
		const complete = (id: string, ok?: unknown, error?: string): Promise<void> =>
			invoke<void>("agent_request_complete", { id, ok: ok ?? null, error: error ?? null }).catch(
				(err: unknown) => {
					console.warn(`[skein] agent_request_complete failed for ${id}:`, err);
				},
			);

		const handleResolve = async (id: string, raw: unknown): Promise<void> => {
			const parsed = parseResolveArgs(raw);
			if (!parsed.ok) return complete(id, undefined, parsed.error);
			const { path, kind: requestedKind, agent: requestedAgent } = parsed.value;
			const folderDefaults = defaultsFor(memoryRef.current, path);
			const kindResult = resolveKind(requestedKind, folderDefaults);
			if (!kindResult.ok) return complete(id, undefined, kindResult.error);
			const kind = kindResult.value;
			const listing = await listHarnessAgents(kind, path);
			const agentResult = resolveAgent(
				requestedAgent,
				kind,
				folderDefaults,
				defaultAgentsRef.current,
				listing,
			);
			if (!agentResult.ok) return complete(id, undefined, agentResult.error);
			return complete(id, { kind, agent: agentResult.value });
		};

		const handleCreate = async (id: string, raw: unknown): Promise<void> => {
			const parsed = parseCreateArgs(raw);
			if (!parsed.ok) return complete(id, undefined, parsed.error);
			const args = parsed.value;
			const branchTemplate = branchTemplateFor(
				memoryRef.current,
				args.path,
				branchTemplateRef.current,
			);
			const spec = specFromCreateArgs(args, branchTemplate);
			const outcome = await createRoomArgs(spec);
			if (!outcome.ok) return complete(id, undefined, outcome.error);
			// #356: the base commit the new worktree was cut from is just
			// its own fresh HEAD — nothing has been committed onto it yet.
			// Reuse the review pane's own `review_scope` read rather than
			// adding a new Rust command for one sha: `head_sha` there is
			// `repo.resolve_commit("HEAD")` on `cwd` alone (see
			// `review_surface/git.rs`'s `resolve_range`), so the room id
			// the command takes plays no part in it — the request `id` is
			// passed through as an otherwise-unused placeholder. Only
			// meaningful for `branchMode: "worktree"`: `"current"` reuses
			// an existing checkout rather than cutting anything, so there
			// is no "base" to report. Best-effort — a failed read leaves
			// `baseSha` unset rather than failing the whole room creation.
			let baseSha: string | undefined;
			if (args.branchMode === "worktree") {
				try {
					const scope = await fetchScope(id, outcome.args.cwd, "branch");
					baseSha = scope.headSha;
				} catch (err: unknown) {
					console.warn(`[skein] create_room: could not read the new worktree's HEAD sha:`, err);
				}
			}
			const promptFirstLine = derivePromptFirstLine(args.promptFirstLine, args.prompt);
			const createdBy = {
				...args.createdBy,
				...(promptFirstLine ? { promptFirstLine } : {}),
				...(baseSha ? { baseSha } : {}),
			};
			// #330: deliberately does NOT call `onRemember`/`rememberFolder`
			// — that per-folder memory feeds the New Room DIALOG's own
			// prefill, and an agent's one-off choice for this call is not
			// the human's "usual" pick for the folder. Applying it here
			// would let an agent silently steer what the user sees next
			// time they open New Room by hand.
			const result = await createRoomRef.current(
				{ ...outcome.args, createdBy },
				{ activate: false },
			);
			pushToastRef.current({
				id: `t_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
				roomId: result.roomId,
				harnessId: result.harnessId,
				kind: result.kind,
				roomName: result.name,
				harnessName: "main",
				state: "created",
				requesterRoomName: args.requesterRoomName,
			});
			return complete(id, {
				...result,
				// #367: only present when the worktree was branched from a
				// LOCAL base branch that is behind its upstream.
				...(outcome.baseBehindUpstream !== undefined
					? { baseBehindUpstream: outcome.baseBehindUpstream }
					: {}),
			});
		};

		// #356: `list_harnesses`'s phase column — the backend has no
		// authority of its own on this (the doc comment on
		// `harnessActivityCore.ts`'s `phaseSnapshot` says why), so it asks
		// the frontend store via the same #328 round trip `create_room`
		// uses. `args` is unused: every harness this process knows about
		// answers at once, and the caller picks out the ids it cares
		// about.
		const handlePhases = async (id: string): Promise<void> => {
			const phases = harnessActivity.phaseSnapshot();
			// #448: outstanding subagent / background-task counts, same ids.
			return complete(id, { phases, work: workSnapshot(Object.keys(phases)) });
		};

		// #411: `close_room` — parsing/refusal-ladder is all in the pure
		// `parseCloseRoomArgs`/`decideCloseRoom`; this is only the plumbing
		// (Rust has already checked creator/sign-off/rate-limit by the
		// time this request lands).
		const handleClose = async (id: string, raw: unknown): Promise<void> => {
			const parsed = parseCloseRoomArgs(raw);
			if (!parsed.ok) return complete(id, undefined, parsed.error);
			const result = closeRoomForAgentRef.current(parsed.value.roomId, parsed.value.closedBy);
			if (!result.ok) return complete(id, undefined, result.error);
			return complete(id, result.value);
		};

		// #411: `open_harness.resolve` — the same `resolveKind`/`resolveAgent`
		// defaulting `handleResolve` runs above, just against an existing
		// room's own `cwd` (via `decideOpenHarness`) rather than a fresh
		// folder path. The "can this kind read mail" check for `prompt`
		// (see `agentRequests.ts`'s doc on this section) is applied Rust-
		// side once it has this reply back, exactly like `create_room`
		// already does with `create_room.resolve`'s — so `prompt` is read
		// out of `raw` for validation only and never inspected here.
		const handleOpenResolve = async (id: string, raw: unknown): Promise<void> => {
			const parsed = parseOpenHarnessResolveArgs(raw);
			if (!parsed.ok) return complete(id, undefined, parsed.error);
			const { roomId, kind: requestedKind, agent: requestedAgent } = parsed.value;
			const decision = decideOpenHarness(roomsRef.current, roomId);
			if (!decision.ok) return complete(id, undefined, decision.error);
			const cwd = decision.value.room.cwd ?? "";
			const folderDefaults = defaultsFor(memoryRef.current, cwd);
			const kindResult = resolveKind(requestedKind, folderDefaults);
			if (!kindResult.ok) return complete(id, undefined, kindResult.error);
			const kind = kindResult.value;
			const listing = await listHarnessAgents(kind, cwd);
			const agentResult = resolveAgent(
				requestedAgent,
				kind,
				folderDefaults,
				defaultAgentsRef.current,
				listing,
			);
			if (!agentResult.ok) return complete(id, undefined, agentResult.error);
			return complete(id, { kind, agent: agentResult.value });
		};

		// #411: `open_harness` — adds a harness the way "+ harness" does,
		// in the background (`{activate: false}`): never switches rooms,
		// never switches the target room's own active harness. The first
		// prompt (if any) is delivered by Rust the same way `create_room`
		// delivers its own, so there is nothing to send here.
		const handleOpen = async (id: string, raw: unknown): Promise<void> => {
			const parsed = parseOpenHarnessArgs(raw);
			if (!parsed.ok) return complete(id, undefined, parsed.error);
			const args = parsed.value;
			const decision = decideOpenHarness(roomsRef.current, args.roomId);
			if (!decision.ok) return complete(id, undefined, decision.error);
			const result = await createHarnessInRoomRef.current(
				args.roomId,
				args.kind,
				args.agent ?? undefined,
				{
					activate: false,
					createdBy: args.createdBy,
				},
			);
			if (!result) {
				return complete(id, undefined, `not_found: room "${args.roomId}" not found`);
			}
			return complete(id, result);
		};

		// #411: `close_harness` — parsing/refusal-ladder is all in the pure
		// `parseCloseHarnessArgs`/`decideCloseHarness`; this is only the
		// plumbing (Rust has already checked scope/rate-limit by the time
		// this request lands).
		const handleCloseHarness = async (id: string, raw: unknown): Promise<void> => {
			const parsed = parseCloseHarnessArgs(raw);
			if (!parsed.ok) return complete(id, undefined, parsed.error);
			const result = closeHarnessForAgentRef.current(parsed.value.roomId, parsed.value.harnessId);
			if (!result.ok) return complete(id, undefined, result.error);
			return complete(id, result.value);
		};

		const unlistenPromise = listen<AgentRequestPayload>("skein://agent-request", (event) => {
			const { id, kind, args } = event.payload;
			void (async () => {
				try {
					if (kind === "create_room.resolve") {
						await handleResolve(id, args);
					} else if (kind === "create_room") {
						await handleCreate(id, args);
					} else if (kind === "harness_phases") {
						await handlePhases(id);
					} else if (kind === "close_room") {
						await handleClose(id, args);
					} else if (kind === "open_harness.resolve") {
						await handleOpenResolve(id, args);
					} else if (kind === "open_harness") {
						await handleOpen(id, args);
					} else if (kind === "close_harness") {
						await handleCloseHarness(id, args);
					} else if (isDesignKind(kind)) {
						await handleDesignRequest(
							kind,
							id,
							args,
							roomsRef.current,
							setDesignEntryRef.current,
							complete,
						);
					} else {
						await complete(id, undefined, `unknown agent request kind "${kind}"`);
					}
				} catch (err: unknown) {
					const msg = err instanceof Error ? err.message : String(err);
					await complete(id, undefined, msg);
				}
			})();
		});
		return () => {
			void unlistenPromise.then((un) => un());
		};
	}, []);
}
