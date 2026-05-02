import type { HarnessKind } from "./types.ts";

export interface HarnessKindMeta {
	id: HarnessKind;
	label: string;
	name: string;
	chip: string;
	desc: string;
}

export const HARNESS_KINDS: Record<HarnessKind, HarnessKindMeta> = {
	claude: {
		id: "claude",
		label: "CC",
		name: "Claude Code",
		chip: "h-claude",
		desc: "Anthropic. Direct API.",
	},
	opencode: {
		id: "opencode",
		label: "oc",
		name: "opencode",
		chip: "h-opencode",
		desc: "Local server, OSS.",
	},
	copilot: {
		id: "copilot",
		label: "gh",
		name: "Copilot CLI",
		chip: "h-copilot",
		desc: "GitHub entitlement.",
	},
	// `byoh` is the kind id we kept from the design's "bring your own
	// harness" idea; today it spawns a plain shell (the user's pwsh/
	// bash/whatever from `default_shell`). When we eventually build the
	// in-app agent loop the design originally envisioned, that becomes
	// a separate kind — for now `byoh` *is* the shell entry point.
	byoh: {
		id: "byoh",
		label: "sh",
		name: "Shell",
		chip: "h-byoh",
		desc: "Plain shell — run anything.",
	},
};

export const HARNESS_ORDER: HarnessKind[] = ["claude", "opencode", "copilot", "byoh"];
