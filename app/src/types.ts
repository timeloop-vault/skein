// Domain types.
//
// A Room is a top-level tab — repo + branch + task + cwd. A Room owns
// N Harnesses; every harness in a Room shares the same worktree. The
// agent tool's own conversation id (Claude / opencode) lives on the
// Harness as `sessionId`, which is the only meaning of "session" in
// Skein's vocabulary now.

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

export interface Room {
	id: string;
	name: string;
	task: string;
	status: Status;
	badge: number;
	harnesses: Harness[];
	activeHarnessId: string;
	cwd?: string;
	// Branch and repo display label are present only for git-backed
	// rooms (chapter 6 phase 3). When `branch` is set the room renders
	// a LiveStatus pane and the tab subtext shows `repo · branch`;
	// when absent the cwd is treated as a plain folder.
	branch?: string;
	repo?: string;
	// Close timestamp (epoch ms). Absent = active (rendered as a tab).
	// Present = archived (hidden from the tab strip but listed in the
	// reopen modal). Chapter 6 phase 2.
	archived?: number;
}

export type Theme = "dark" | "light";
export type Density = "compact" | "regular" | "comfy";
