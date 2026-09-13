// Which agent a harness is on, as far as Skein can honestly say. #248
// (sub-issue C of #219).
//
// Two sources, and the difference between them is the whole module:
//
// - `Harness.agent`, the record. What the process was *launched* with.
//   For a kind that binds its agent at launch (Claude) that is also what
//   it is running as, for the life of the conversation.
// - What opencode last *reported*. opencode lets the user switch agent
//   mid-session, so the record is only ever the starting point there.
//   Every user message opencode records carries the agent it was sent
//   to, and the SSE adapter forwards it (`UserMessageAgent`). That is
//   strictly better than the record — but it is still "as of the last
//   message", not "now": a Tab press that has not been followed by a
//   message is invisible to it. The label says which of the two it is.
//
// The observation is deliberately not written back to the record. The
// record is what `resumeCmd` re-passes as `--agent`, and a session the
// user switched to `plan` for one question should not come back as
// `plan` after a restart.

import { useSyncExternalStore } from "react";
import { HARNESS_KINDS } from "./data.tsx";
import type { Harness } from "./types.ts";

/** The agent opencode stamped on the latest user message it recorded. */
export interface ObservedAgent {
	/** The session the message belongs to — see `agentLabel` for why it
	 *  has to match the harness's own. */
	sessionId: string;
	agent: string;
}

const observed = new Map<string, ObservedAgent>();
const listeners = new Map<string, Set<() => void>>();

export const observedAgents = {
	record(harnessId: string, sessionId: string, agent: string): void {
		const prev = observed.get(harnessId);
		if (prev?.sessionId === sessionId && prev.agent === agent) return;
		observed.set(harnessId, { sessionId, agent });
		for (const cb of listeners.get(harnessId) ?? []) cb();
	},

	get(harnessId: string): ObservedAgent | undefined {
		return observed.get(harnessId);
	},

	/** Drop what was observed, when the process it was observed on goes
	 *  away. A respawn re-passes the record's `--agent`, so from that
	 *  moment "started as" is the true label again. */
	forget(harnessId: string): void {
		if (!observed.delete(harnessId)) return;
		for (const cb of listeners.get(harnessId) ?? []) cb();
	},

	subscribe(harnessId: string, cb: () => void): () => void {
		let set = listeners.get(harnessId);
		if (!set) {
			set = new Set();
			listeners.set(harnessId, set);
		}
		set.add(cb);
		return () => {
			const s = listeners.get(harnessId);
			if (!s) return;
			s.delete(cb);
			if (s.size === 0) listeners.delete(harnessId);
		};
	},
};

export const useObservedAgent = (harnessId: string): ObservedAgent | undefined =>
	useSyncExternalStore(
		(cb) => observedAgents.subscribe(harnessId, cb),
		() => observedAgents.get(harnessId),
	);

/** How a harness's agent reads on screen: `key value`, e.g.
 *  "agent coder", "started as coder", "last message to plan". Split so
 *  the hover popover can style the two halves the way it styles
 *  "harness Claude Code". */
export interface AgentLabel {
	key: string;
	value: string;
	/** The one sentence saying how much the label knows. */
	title: string;
}

/** Shown for a harness that names no agent. Not "(default)": with a
 *  per-kind default in Settings, "default" would read as *Skein's*
 *  default, and "no agent named, the tool decides" is a different state
 *  that has to stay tellable apart from any name. */
export const TOOL_DEFAULT_AGENT = "tool default";

export const agentLabel = (
	h: Pick<Harness, "kind" | "agent" | "sessionId">,
	seen: ObservedAgent | undefined,
): AgentLabel | null => {
	const { capabilities: caps, name: tool } = HARNESS_KINDS[h.kind];
	if (!caps.agents) return null;
	const named = h.agent?.trim() ? h.agent : undefined;

	if (!caps.agentSwitchable) {
		return {
			key: "agent",
			value: named ?? TOOL_DEFAULT_AGENT,
			title: named
				? `${tool} binds its agent at launch, so this harness runs as ${named} for the whole conversation.`
				: `No agent chosen, so ${tool}'s own agent setting decides. It cannot change after launch.`,
		};
	}

	// Only an observation from the harness's *own* session counts.
	// opencode's `/event` stream is instance-wide, and a subagent's child
	// session reports its own agent there — `explore` running on behalf
	// of `build` must not relabel the harness. An opencode harness whose
	// session id has not been captured yet has nothing to match against,
	// so it keeps the honest fallback until it does.
	if (seen && h.sessionId && seen.sessionId === h.sessionId) {
		return {
			key: "last message to",
			value: seen.agent,
			title: `Your last message in this session went to ${seen.agent}. ${tool} can switch agent at any time (Tab), so it may have changed since.`,
		};
	}

	return {
		key: "started as",
		value: named ?? TOOL_DEFAULT_AGENT,
		title: `${tool} can switch agent mid-session (Tab), so this is only the agent it was launched with. Send a message to see the current one.`,
	};
};
