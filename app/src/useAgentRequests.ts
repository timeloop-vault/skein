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
import { useEffect, useRef } from "react";
import type { CreateRoomArgs } from "./NewRoomDialogTypes.ts";
import {
	parseCreateArgs,
	parseResolveArgs,
	resolveAgent,
	resolveKind,
	specFromCreateArgs,
} from "./agentRequests.ts";
import { listHarnessAgents } from "./agents.ts";
import type { ToastEntry } from "./notifications.tsx";
import { type DefaultAgents, type NewRoomMemory, branchTemplateFor, defaultsFor } from "./prefs.ts";
import type { CreateRoomResult } from "./useHarnessCreation.ts";
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
	pushToast: (entry: ToastEntry) => void,
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
			// #330: deliberately does NOT call `onRemember`/`rememberFolder`
			// — that per-folder memory feeds the New Room DIALOG's own
			// prefill, and an agent's one-off choice for this call is not
			// the human's "usual" pick for the folder. Applying it here
			// would let an agent silently steer what the user sees next
			// time they open New Room by hand.
			const result = await createRoomRef.current(
				{ ...outcome.args, createdBy: args.createdBy },
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

		const unlistenPromise = listen<AgentRequestPayload>("skein://agent-request", (event) => {
			const { id, kind, args } = event.payload;
			void (async () => {
				try {
					if (kind === "create_room.resolve") {
						await handleResolve(id, args);
					} else if (kind === "create_room") {
						await handleCreate(id, args);
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
