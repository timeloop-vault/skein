// The tool_call / patch / plan_change family — issue #80 D2b.
//
// This is where every Claude↔opencode payload divergence lives (see
// docs/live-context-d2-buildmap.md): patch deltas, result shape,
// tool-name casing, the plan_change two-mode split. `ToolFamilyRow`
// does the second dispatch level (after rows.tsx switches on kind):
// it runs the is_error short-circuit, then sub-classifies by the
// normalized tool name / plan_item.op.

import type { HarnessKind } from "../types.ts";
import { ApiErrorRow, CompactRow, GenericToolRow, ToolErrorRow } from "./errorRows.tsx";
import { EditRow, ReadRow, SearchRow } from "./fileRows.tsx";
import { obj, type Payload, str } from "./payload.ts";
import { harnessTool, type ToolRowProps } from "./toolShared.ts";
import { AgentRow, AskRow, BashRow, TaskRow, TodoWriteRow } from "./workRows.tsx";

export { apiErrorToastText } from "./errorRows.tsx";

// ── dispatch ───────────────────────────────────────────────────────

/// Second-level dispatch for the tool family. Errors short-circuit to
/// ToolErrorRow (except bash, whose non-zero exits are informational
/// and shown inline). Then route by kind / normalized tool.
export const ToolFamilyRow = ({
	kind,
	payload,
	harness,
	timestampMs,
}: {
	kind: string;
	payload: Payload;
	harness: HarnessKind | null | undefined;
	timestampMs: number;
}) => {
	const p: ToolRowProps = { payload, harness, timestampMs };
	const isError = payload.is_error === true;
	const tool = harnessTool(payload);

	if (kind === "api_error") return <ApiErrorRow {...p} />;
	if (kind === "compaction") return <CompactRow {...p} />;

	if (kind === "plan_change") {
		const op = str(obj(payload.plan_item)?.op);
		if (op === "write") return <TodoWriteRow {...p} />;
		if (isError) return <ToolErrorRow tool={tool || "task"} {...p} />;
		return <TaskRow {...p} />;
	}

	if (kind === "patch") {
		if (isError) return <ToolErrorRow tool={tool || "edit"} {...p} />;
		return <EditRow {...p} />;
	}

	// kind === "tool_call"
	switch (tool) {
		case "bash":
			return <BashRow {...p} />; // handles non-zero exit inline
		case "read":
			return isError ? <ToolErrorRow tool="read" {...p} /> : <ReadRow {...p} />;
		case "grep":
		case "glob":
			return isError ? <ToolErrorRow tool={tool} {...p} /> : <SearchRow {...p} />;
		case "askuserquestion":
		case "question":
			return isError ? <ToolErrorRow tool={tool} {...p} /> : <AskRow {...p} />;
		case "task":
		case "agent":
			return isError ? <ToolErrorRow tool={tool} {...p} /> : <AgentRow {...p} />;
		default:
			return isError ? <ToolErrorRow tool={tool || "tool"} {...p} /> : <GenericToolRow {...p} />;
	}
};
