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
