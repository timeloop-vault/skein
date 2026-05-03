// Domain types.
//
// Sessions are workspaces (repo + branch + task). A session owns N harnesses;
// every harness in a session shares the same worktree.

export type HarnessKind = "claude" | "opencode" | "copilot" | "byoh";

export type Status = "running" | "waiting" | "idle" | "error";

export interface Harness {
	id: string;
	kind: HarnessKind;
	name: string;
	status: Status;
	model: string;
	tokens: string;
	live?: boolean;
	cmd?: string[];
	cwd?: string;
	// Conversation id assigned by the underlying tool. For Claude this
	// is pre-allocated by Skein at spawn time via `--session-id <uuid>`
	// (chapter 5 phase 2a). For opencode it's captured after spawn from
	// the session table (phase 2b). Undefined for fresh harnesses, for
	// kinds without a resume concept (copilot, shell), and for legacy
	// harnesses created before the field existed.
	sessionId?: string;
}

export interface Session {
	id: string;
	name: string;
	branch: string;
	repo: string;
	task: string;
	status: Status;
	badge: number;
	harnesses: Harness[];
	activeHarnessId: string;
	cwd?: string;
}

export type Theme = "dark" | "light";
export type Density = "compact" | "regular" | "comfy";
