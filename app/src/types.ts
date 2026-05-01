import type { ReactNode } from "react";

// Domain types for the Skein prototype.
//
// Sessions are workspaces (repo + branch + task). A session owns N harnesses;
// every harness in a session shares the same worktree. The right pane
// (files / diff / plan / activity) belongs to the session, not the harness.

export type HarnessKind = "claude" | "opencode" | "copilot" | "byoh";

export type Status = "running" | "waiting" | "idle" | "error";

export interface Harness {
	id: string;
	kind: HarnessKind;
	name: string;
	status: Status;
	model: string;
	tokens: string;
	// Phase 1: harnesses spawned via "+ harness" run inside a real PTY.
	// Seeded demo harnesses (s1-s5) leave these undefined, so they keep
	// rendering the frozen TUI mocks from the design.
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
	// Phase 2: every harness in the session spawns into this directory.
	// Optional because the seeded demo sessions (s1-s5) point at fictional
	// repos and don't get spawned into; new sessions always have one.
	cwd?: string;
}

export interface TreeNode {
	name: string;
	kind: "dir" | "file";
	depth: number;
	open?: boolean;
	touched?: string;
	active?: boolean;
}

export type DiffLineKind = "ctx" | "add" | "del";

export interface DiffLine {
	kind: DiffLineKind;
	n1: number | "";
	n2: number | "";
	src: ReactNode;
}

export interface ActiveFile {
	path: string;
	adds: number;
	dels: number;
}

export type PlanState = "done" | "now" | "next";

export interface PlanItem {
	state: PlanState;
	text: string;
	by: string;
}

export interface ActivityEvent {
	time: string;
	by: string;
	kind: HarnessKind;
	msg: ReactNode;
}

export interface SessionData {
	tree: TreeNode[];
	activeFile: ActiveFile;
	diff: DiffLine[];
	plan: PlanItem[];
	activity: ActivityEvent[];
}

export type Theme = "dark" | "light";
export type Density = "compact" | "regular" | "comfy";
export type RightTab = "stack" | "files" | "diff" | "plan";
