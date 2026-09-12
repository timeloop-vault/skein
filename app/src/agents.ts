// Which agents a harness kind will accept at `--agent`, as the pickers
// see them. Issue #247 (sub-issue B of #219); the discovery itself is
// #246's, in `crates/skein-harness` behind `list_harness_agents`.
//
// DTO mirror of `app/src-tauri/src/agents.rs` plus the two rules the
// frontend applies to that data: what a stored name means when the
// list no longer contains it (`validateAgent`), and what a row is
// allowed to claim about the review loop.

import { invoke } from "@tauri-apps/api/core";
import { HARNESS_KINDS } from "./data.tsx";
import type { HarnessKind } from "./types.ts";

/** Where a definition came from. Cosmetic, except that `builtin`
 *  explains a row with no subtitle: it never had one. */
export type AgentSource = "builtin" | "user" | "project" | "plugin" | "config";

/** One row of the picker. Mirrors `AgentDto`. */
export interface AgentInfo {
	/** Exactly what goes after `--agent`, namespace included. */
	name: string;
	description: string | null;
	/** `null` = unrestricted, which is not the same as an empty list. */
	tools: string[] | null;
	model: string | null;
	source: AgentSource;
	/** The plugin half of a namespaced name. */
	plugin: string | null;
	/** False = this agent cannot see Skein's review MCP tools at all.
	 *  Not a warning the agent will ever raise itself: the connection is
	 *  healthy, `tools/list` arrives, and the allowlist filters them out
	 *  before the model sees them (#215). The picker has to say so. */
	allowsReviewTools: boolean;
}

/** Mirrors `AgentListDto`. */
export interface AgentListing {
	agents: AgentInfo[];
	/** Why the list may be incomplete — the CLI was missing, failed, or
	 *  printed something unparseable. Non-null means "cannot prove a
	 *  name is absent", which is what `validateAgent` keys off. */
	degraded: string | null;
	/** True when the kind has no agent concept at all. */
	unsupported: boolean;
}

/** An empty listing, for the "kind has no agents" and pre-fetch cases.
 *  Deliberately `degraded`, not clean: a listing nobody has fetched
 *  must never be read as proof that a name is gone. */
export const NO_AGENTS: AgentListing = {
	agents: [],
	degraded: "not fetched",
	unsupported: true,
};

/** Whether this kind takes `--agent` at all (#184's capability
 *  pattern — never compare kinds). */
export const kindHasAgents = (kind: HarnessKind): boolean =>
	HARNESS_KINDS[kind].capabilities.agents;

/** Ask the backend what `kind` will accept for a harness spawned in
 *  `cwd`. Never rejects for "found nothing": the listing carries
 *  `degraded` instead, so a picker shows a list plus a reason rather
 *  than an error (#176). A genuine invoke failure — the command is
 *  gone, the webview lost its bridge — still comes back as a degraded
 *  listing, because the caller's only other option is to pretend the
 *  agent does not exist. */
export const listHarnessAgents = async (kind: HarnessKind, cwd: string): Promise<AgentListing> => {
	if (!kindHasAgents(kind)) return NO_AGENTS;
	try {
		return await invoke<AgentListing>("list_harness_agents", { kind, cwd });
	} catch (err: unknown) {
		const msg = err instanceof Error ? err.message : String(err);
		return { agents: [], degraded: `could not ask ${kind}: ${msg}`, unsupported: false };
	}
};

/** What a named agent is, measured against a listing.
 *
 *  - `ok` — no agent named (the tool's own default), or the list
 *    vouches for it.
 *  - `unknown` — the CLI answered and this name was not in the answer.
 *    The only verdict that blocks a spawn.
 *  - `unverified` — the list is degraded, so absence proves nothing.
 *    Spawning is the right call: refusing here would strand every
 *    harness in the app the moment the CLI is briefly unavailable, and
 *    the spawn itself fails loudly if the name really is gone. */
export type AgentVerdict =
	| { kind: "ok" }
	| { kind: "unknown"; agent: string }
	| { kind: "unverified"; agent: string; why: string };

export const validateAgent = (agent: string | undefined, listing: AgentListing): AgentVerdict => {
	if (!agent?.trim()) return { kind: "ok" };
	if (listing.degraded !== null) {
		return { kind: "unverified", agent, why: listing.degraded };
	}
	if (listing.agents.some((a) => a.name === agent)) return { kind: "ok" };
	return { kind: "unknown", agent };
};

/** The message a harness shows instead of spawning with a name the CLI
 *  no longer knows.
 *
 *  Worth a sentence of *why* rather than just the name: the fresh spawn
 *  that Claude would refuse is the loud case, but `--resume` and both
 *  opencode paths accept a dead name and quietly fall back to something
 *  else (#219), which is the failure this check exists to prevent. */
export const unknownAgentMessage = (agent: string, kind: HarnessKind): string => {
	const tool = HARNESS_KINDS[kind].name;
	const quiet = kind === "claude" ? "a resumed" : "an opencode";
	return `agent "${agent}" is not one ${tool} offers any more. Not spawning: ${quiet} harness would have run as a different agent without saying so. Pick another agent for this harness.`;
};
