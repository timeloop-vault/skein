import type { SpawnSettings, ToolGroups } from "../types.ts";

export type ToolGroupKey = keyof ToolGroups;

export interface ToolGroupInfo {
	key: ToolGroupKey;
	label: string;
	/** Tool names, rendered as code. */
	tools: string[];
	/** One line on what the group does or risks. */
	note: string;
}

export const TOOL_GROUPS: readonly ToolGroupInfo[] = [
	{
		key: "review",
		label: "Review",
		tools: ["list_comments", "get_comment", "get_diff", "reply", "mark_addressed", "review_status"],
		note: "Read review comments and the diff, answer them, check the sign-off.",
	},
	{
		key: "mail",
		label: "Mail",
		tools: ["send_message", "read_messages", "message_history"],
		note: "Message other harnesses and rooms.",
	},
	{
		key: "rooms",
		label: "Rooms",
		tools: ["create_room", "close_room", "open_harness", "close_harness"],
		note: "Opens real rooms and agent processes.",
	},
	{
		key: "directory",
		label: "Directory",
		tools: ["find_rooms_for_path", "list_rooms", "get_room", "list_harnesses"],
		note: "Read-only, across all rooms.",
	},
	{
		key: "design",
		label: "Design pane",
		tools: [
			"list_design_harnesses",
			"get_design_state",
			"open_design_entry",
			"set_design_device",
			"show_element",
			"invoke_element",
			"show_changes",
			"get_design_screenshot",
			"show_design_pane",
		],
		note: "The design-pane verbs, for the agent's own room only.",
	},
];

/** A missing `toolGroups` object or key counts as on. */
export const groupEnabled = (
	settings: Pick<SpawnSettings, "toolGroups">,
	key: ToolGroupKey,
): boolean => settings.toolGroups?.[key] ?? true;

/** A copy of `settings` with one group switched; the input is untouched. */
export const withGroup = <T extends Pick<SpawnSettings, "toolGroups">>(
	settings: T,
	key: ToolGroupKey,
	on: boolean,
): T => ({ ...settings, toolGroups: { ...settings.toolGroups, [key]: on } });
