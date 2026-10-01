// The #247 pre-spawn agent check, split out of useTerminalSpawn.ts (#459).

import type { Terminal } from "@xterm/xterm";
import { listHarnessAgents, unknownAgentMessage, validateAgent } from "./agents.ts";
import { harnessActivity } from "./harnessActivity.ts";
import type { HarnessKind } from "./types.ts";

export interface AgentCheck {
	agent: string | undefined;
	cmdToSpawn: string[];
	harnessKind: HarnessKind;
	harnessId: string;
	cwd: string;
	term: Terminal;
	isCancelled: () => boolean;
	/** Called when the spawn is refused, before the activity store is
	 *  told the harness exited — flips the effect's own `phase`. */
	onRefused: () => void;
}

/** Refuse the spawn when the harness names an agent the CLI no
 *  longer has (#247).
 *
 *  Here rather than at pick time *as well as* at pick time: a room
 *  restored from sqlite names an agent chosen weeks ago, and only
 *  one of the four spawn paths fails loudly on its own. Claude
 *  refuses a fresh spawn, but `claude --resume` and both opencode
 *  paths accept a dead name and quietly run as something else —
 *  #176's category, and invisible in a TUI where the warning
 *  scrolls past. Returning false costs one CLI probe (~0.35 s) and
 *  only for a harness that names an agent at all.
 *
 *  Verdicts other than `unknown` spawn: a degraded list cannot
 *  prove a name is gone, and refusing on a CLI that would not run
 *  would strand every harness in the app.
 *
 *  Gated on the argv actually carrying the flag, because the
 *  record outlives the process it described: a harness swapped to
 *  a shell by "Enter for shell" keeps `agent` set, and a dead
 *  agent name must not stop the user getting a shell. That is a
 *  question about *this* argv, not an attempt to recover a
 *  decision from it — the name still comes from the record. */
export async function agentResolves(c: AgentCheck): Promise<boolean> {
	const { agent, cmdToSpawn, harnessKind, harnessId, cwd, term } = c;
	if (!agent || !cmdToSpawn.includes("--agent")) return true;
	const verdict = validateAgent(agent, await listHarnessAgents(harnessKind, cwd));
	if (c.isCancelled()) return false;
	if (verdict.kind === "unverified") {
		console.warn(`[skein] could not verify agent "${agent}": ${verdict.why}`);
	}
	if (verdict.kind !== "unknown") return true;
	term.write(`\r\n\x1b[31m[skein] ${unknownAgentMessage(agent, harnessKind)}\x1b[0m\r\n`);
	// Same footer every other dead-harness path writes, and for the
	// same reason: without it the pane is a wall of red with no
	// visible way forward.
	term.write("\x1b[2m[skein] Press \x1b[0;1mEnter\x1b[0;2m for shell.\x1b[0m\r\n");
	c.onRefused();
	harnessActivity.exited(harnessId, null);
	return false;
}
