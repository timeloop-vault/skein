import type { HarnessKind } from "./types.ts";

/// What a harness kind can do. `files` (#49 phase A) is the first
/// non-process harness — code paths branch on these flags, never on
/// kind string comparisons, so the next capability split doesn't mean
/// another grep for `=== "byoh"`.
export interface HarnessCapabilities {
	/// Spawns a PTY child and renders a terminal body. False = the
	/// body is a Skein-native surface (Files) — no spawn, no resume
	/// rewrite, no ports, no exit handling.
	pty: boolean;
	/// The underlying tool can resume its conversation across Skein
	/// restarts (drives the boot/reopen resumeCmd rewrite).
	resume: boolean;
	/// Transitions may raise badges/toasts/OS notifications. Shells
	/// aren't agents — an idle prompt isn't "your turn" (#127); a
	/// Files surface has no turns at all.
	notify: boolean;
}

export interface HarnessKindMeta {
	id: HarnessKind;
	label: string;
	name: string;
	chip: string;
	desc: string;
	capabilities: HarnessCapabilities;
}

export const HARNESS_KINDS: Record<HarnessKind, HarnessKindMeta> = {
	claude: {
		id: "claude",
		label: "CC",
		name: "Claude Code",
		chip: "h-claude",
		desc: "Anthropic. Direct API.",
		capabilities: { pty: true, resume: true, notify: true },
	},
	opencode: {
		id: "opencode",
		label: "oc",
		name: "opencode",
		chip: "h-opencode",
		desc: "Local server, OSS.",
		capabilities: { pty: true, resume: true, notify: true },
	},
	copilot: {
		id: "copilot",
		label: "gh",
		name: "Copilot CLI",
		chip: "h-copilot",
		desc: "GitHub entitlement.",
		capabilities: { pty: true, resume: false, notify: true },
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
		capabilities: { pty: true, resume: false, notify: false },
	},
	// #49 phase A: the file surface as a harness. Deliberately not a
	// coloured process chip — the ◇ renders in --accent via .h-files.
	files: {
		id: "files",
		label: "◇",
		name: "Files",
		chip: "h-files",
		desc: "Browse & edit the room's files.",
		capabilities: { pty: false, resume: false, notify: false },
	},
};

export const HARNESS_ORDER: HarnessKind[] = ["claude", "opencode", "copilot", "byoh", "files"];
