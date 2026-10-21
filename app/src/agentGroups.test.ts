import { describe, expect, it } from "vitest";
import { groupAgents } from "./agentGroups.ts";
import type { AgentInfo } from "./agents.ts";

const agent = (name: string, over: Partial<AgentInfo> = {}): AgentInfo => ({
	name,
	description: null,
	tools: null,
	model: null,
	source: "user",
	plugin: null,
	allowsReviewTools: true,
	...over,
});

const summary = (agents: AgentInfo[]) =>
	groupAgents(agents).map((g) => [g.key, g.heading, g.agents.map((a) => a.name)]);

describe("groupAgents", () => {
	it("groups a Claude-shaped list, plugins interleaved, by first appearance", () => {
		const list = [
			agent("a-plugin", { source: "plugin", plugin: "alpha" }),
			agent("explore"),
			agent("proj", { source: "project" }),
			agent("b-plugin", { source: "plugin", plugin: "beta" }),
			agent("a-plugin-2", { source: "plugin", plugin: "alpha" }),
			agent("Explore", { source: "builtin" }),
			agent("Plan", { source: "builtin" }),
		];
		expect(summary(list)).toEqual([
			["own", "Your own", ["explore"]],
			["project", "This project", ["proj"]],
			["plugin:alpha", "Plugin: alpha", ["a-plugin", "a-plugin-2"]],
			["plugin:beta", "Plugin: beta", ["b-plugin"]],
			["builtin", "Built-in", ["Explore", "Plan"]],
		]);
	});

	it("puts user and config agents together under Your own (opencode)", () => {
		const list = [
			agent("build", { source: "builtin" }),
			agent("mine", { source: "config" }),
			agent("plan", { source: "builtin" }),
			agent("other"),
			agent("general", { source: "builtin" }),
		];
		expect(summary(list)).toEqual([
			["own", "Your own", ["mine", "other"]],
			["builtin", "Built-in", ["build", "plan", "general"]],
		]);
	});

	it("omits empty sections", () => {
		const keys = groupAgents([agent("x", { source: "project" })]).map((g) => g.key);
		expect(keys).toEqual(["project"]);
	});

	it("keeps input order within a section", () => {
		const list = [agent("c"), agent("a"), agent("b")];
		expect(groupAgents(list)[0]?.agents.map((a) => a.name)).toEqual(["c", "a", "b"]);
	});

	it("returns [] for empty input", () => {
		expect(groupAgents([])).toEqual([]);
	});

	it("places a plugin agent with no plugin name in Plugins, by first appearance", () => {
		const list = [
			agent("n", { source: "plugin", plugin: null }),
			agent("p", { source: "plugin", plugin: "zed" }),
		];
		expect(summary(list)).toEqual([
			["plugin:", "Plugins", ["n"]],
			["plugin:zed", "Plugin: zed", ["p"]],
		]);
	});

	it("still groups a degraded listing with builtins appended after disk agents", () => {
		const list = [
			agent("mine"),
			agent("proj", { source: "project" }),
			agent("Explore", { source: "builtin" }),
		];
		expect(summary(list)).toEqual([
			["own", "Your own", ["mine"]],
			["project", "This project", ["proj"]],
			["builtin", "Built-in", ["Explore"]],
		]);
	});
});
