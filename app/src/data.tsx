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
	/// The CLI takes `--agent <name>` and binds it at launch, so Skein
	/// can offer a choice before the process starts (#247). Gate every
	/// agent code path on this, never on `kind === "claude"`: `copilot`
	/// is a managed program with no agent concept, which is exactly the
	/// distinction a kind comparison loses.
	agents: boolean;
	/// The agent can change after launch without Skein seeing the argv
	/// change (#248): opencode's Tab / `switch_agent` / `@` mentions.
	/// Decides whether the harness's agent label may say "is X" (Claude,
	/// which cannot change) or has to say "started as X". Meaningless
	/// without `agents`.
	agentSwitchable: boolean;
	/// Does `sendPrompt` (#238) get a second, delayed "\r" if the first
	/// one doesn't appear to have landed (#380)? True only for `claude`
	/// — Claude Code 2.1.283 has a startup-timing bug (upstream
	/// anthropics/claude-code#91205) where the Enter ending a first
	/// bracketed paste into a freshly spawned harness can be dropped;
	/// every other kind's behaviour is unchanged by this flag.
	submitRetry: boolean;
}

export interface HarnessKindMeta {
	id: HarnessKind;
	label: string;
	name: string;
	chip: string;
	desc: string;
	/** The program Skein spawns for this kind, or `null` where Skein
	 *  doesn't choose it (`byoh` takes the user's shell; `files` has no
	 *  process). Mirrored in Rust by `HarnessKind::program` — the two
	 *  are kept honest by an agreement test in `harness_kind.rs`. */
	program: string | null;
	/** How a user invokes a repo skill (#359) in this CLI's own TUI, as a
	 *  one-line template containing the literal placeholder `{name}` —
	 *  e.g. Claude Code's `"/{name}"`, or Copilot CLI's sentence form
	 *  `"Use the /{name} skill."` (per its docs: it invokes a skill via a
	 *  `/name` token embedded in a prompt, not a bare slash command).
	 *  `repoSkills.ts`'s `skillInvocationLine` fills in `{name}` with the
	 *  skill's `command` (its directory name — see `RepoSkill`). `null`
	 *  when the kind has no user-typed invocation at all: opencode's
	 *  skill tool is model-invoked only, `byoh` is a plain shell with no
	 *  skill concept, and `files` has no pty to type into. Not a
	 *  capability — it's a per-kind string, not a yes/no gate other code
	 *  branches on. */
	skillInvocation: string | null;
	capabilities: HarnessCapabilities;
}

export const HARNESS_KINDS: Record<HarnessKind, HarnessKindMeta> = {
	claude: {
		id: "claude",
		label: "CC",
		name: "Claude Code",
		chip: "h-claude",
		desc: "Anthropic. Direct API.",
		program: "claude",
		skillInvocation: "/{name}",
		capabilities: {
			pty: true,
			resume: true,
			notify: true,
			agents: true,
			agentSwitchable: false,
			submitRetry: true,
		},
	},
	opencode: {
		id: "opencode",
		label: "oc",
		name: "opencode",
		chip: "h-opencode",
		desc: "Local server, OSS.",
		program: "opencode",
		skillInvocation: null,
		capabilities: {
			pty: true,
			resume: true,
			notify: true,
			agents: true,
			agentSwitchable: true,
			submitRetry: false,
		},
	},
	copilot: {
		id: "copilot",
		label: "gh",
		name: "Copilot CLI",
		chip: "h-copilot",
		desc: "GitHub entitlement.",
		program: "gh",
		skillInvocation: "Use the /{name} skill.",
		capabilities: {
			pty: true,
			resume: false,
			notify: true,
			agents: false,
			agentSwitchable: false,
			submitRetry: false,
		},
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
		program: null,
		skillInvocation: null,
		capabilities: {
			pty: true,
			resume: false,
			notify: false,
			agents: false,
			agentSwitchable: false,
			submitRetry: false,
		},
	},
	// #49 phase A: the file surface as a harness. Deliberately not a
	// coloured process chip — the ◇ renders in --accent via .h-files.
	files: {
		id: "files",
		label: "◇",
		name: "Files",
		chip: "h-files",
		desc: "Browse + edit the worktree.",
		program: null,
		skillInvocation: null,
		capabilities: {
			pty: false,
			resume: false,
			notify: false,
			agents: false,
			agentSwitchable: false,
			submitRetry: false,
		},
	},
};

export const HARNESS_ORDER: HarnessKind[] = ["claude", "opencode", "copilot", "byoh", "files"];
