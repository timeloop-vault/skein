import type { AgentInfo } from "./agents.ts";

export interface AgentGroup {
	/** Stable across renders: `own`, `project`, `plugin:<name>`, `builtin`. */
	key: string;
	heading: string;
	agents: AgentInfo[];
}

/** Group the CLI's flat agent list under headings by where each agent
 *  came from (#267).
 *
 *  The flat list hides which agents are yours: `explore` (a user agent)
 *  and `Explore` (the built-in) look alike, and a long plugin list buries
 *  everything else. Order: own (user + config), project, one section per
 *  plugin by first appearance (a plugin agent with no plugin name lands in
 *  "Plugins"), then built-in. Empty sections are omitted and the CLI's
 *  order is kept within each section. */
export const groupAgents = (agents: AgentInfo[]): AgentGroup[] => {
	const own: AgentInfo[] = [];
	const project: AgentInfo[] = [];
	const builtin: AgentInfo[] = [];
	const plugins = new Map<string, AgentInfo[]>();

	for (const a of agents) {
		switch (a.source) {
			case "user":
			case "config":
				own.push(a);
				break;
			case "project":
				project.push(a);
				break;
			case "builtin":
				builtin.push(a);
				break;
			case "plugin": {
				const name = a.plugin ?? "";
				const list = plugins.get(name);
				if (list) list.push(a);
				else plugins.set(name, [a]);
				break;
			}
		}
	}

	const groups: AgentGroup[] = [];
	if (own.length) groups.push({ key: "own", heading: "Your own", agents: own });
	if (project.length) groups.push({ key: "project", heading: "This project", agents: project });
	for (const [name, list] of plugins) {
		groups.push({
			key: `plugin:${name}`,
			heading: name === "" ? "Plugins" : `Plugin: ${name}`,
			agents: list,
		});
	}
	if (builtin.length) groups.push({ key: "builtin", heading: "Built-in", agents: builtin });
	return groups;
};
