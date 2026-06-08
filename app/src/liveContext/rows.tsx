// Activity-row dispatcher + the simple single-shape rows.
//
// Two-level dispatch (see docs/live-context-d2-buildmap.md): switch on
// the backend `kind` here; the tool_call/patch/plan_change family
// (where all the Claude↔opencode divergence lives) is handed to
// ToolFamilyRow (toolRows.tsx) for its second-level dispatch. The rows
// in this file are the kinds with one clean shape and no divergence.

import type { HarnessKind } from "../types.ts";
import { ResultPreview } from "./ResultPreview.tsx";
import { Row, basename } from "./Row.tsx";
import { type Payload, num, parsePayload, str } from "./payload.ts";
import type { HarnessAction } from "./store.ts";
import { ToolFamilyRow } from "./toolRows.tsx";

interface SimpleRowProps {
	payload: Payload;
	harness: HarnessKind | undefined;
	timestampMs: number;
}

// ── the dispatcher ─────────────────────────────────────────────────

/// Render one action as its Activity row, or `null` for kinds that are
/// consumed elsewhere (turn_duration → separators, turn_cost → cost
/// hair-lines; both land in D2d).
export const ActivityRow = ({
	row,
	harnessKindOf,
}: {
	row: HarnessAction;
	harnessKindOf: (harnessId: string) => HarnessKind;
}) => {
	const payload = parsePayload(row.payload);
	const harness = harnessKindOf(row.harnessId);
	const ts = row.timestampMs;

	switch (row.kind) {
		case "user_prompt":
			return <UserPromptRow payload={payload} harness={harness} timestampMs={ts} />;
		case "ai_title":
			return <AiTitleRow payload={payload} harness={harness} timestampMs={ts} />;
		case "permission_mode":
			return <PermissionModeRow payload={payload} harness={harness} timestampMs={ts} />;
		case "bridge_status":
			return <BridgeStatusRow payload={payload} harness={harness} timestampMs={ts} />;
		case "pr_link":
			return <PrRow payload={payload} harness={harness} timestampMs={ts} />;
		case "queue_op":
			return <QueueRow payload={payload} harness={harness} timestampMs={ts} />;
		case "edited_text_file":
			return <UserFileRow payload={payload} harness={harness} timestampMs={ts} />;
		case "slash_command":
			return <SlashRow payload={payload} harness={harness} timestampMs={ts} />;
		case "tool_call":
		case "patch":
		case "plan_change":
		case "api_error":
		case "compaction":
			return <ToolFamilyRow kind={row.kind} payload={payload} harness={harness} timestampMs={ts} />;
		// turn_duration → turn separator, turn_cost → cost hair-line:
		// both rendered by the flattened-item layer in D2d, not as rows.
		default:
			return null;
	}
};

// ── simple rows ────────────────────────────────────────────────────

const UserPromptRow = ({ payload, harness, timestampMs }: SimpleRowProps) => {
	// Claude: the typed text. opencode: always null (text lives in part
	// rows) → placeholder so the row isn't blank.
	const text = str(payload.prompt);
	return (
		<Row kind="user" harness={harness} timestampMs={timestampMs}>
			<span className="tool">user</span>{" "}
			<span className="target" style={{ color: "var(--fg-0)", fontWeight: 500 }}>
				{text || "(prompt)"}
			</span>
		</Row>
	);
};

const AiTitleRow = ({ payload, harness, timestampMs }: SimpleRowProps) => {
	const title = str(payload.ai_title);
	if (!title) return null; // Claude ai_title can be null — nothing to show
	return (
		<Row
			kind="title"
			harness={harness}
			timestampMs={timestampMs}
			right={<span className="dim">harness titled</span>}
		>
			<span className="tool">title</span>{" "}
			<span className="target" style={{ fontStyle: "italic", color: "var(--fg-1)" }}>
				"{title}"
			</span>
		</Row>
	);
};

const PermissionModeRow = ({ payload, harness, timestampMs }: SimpleRowProps) => {
	// Payload carries only the new mode (no prior value), so we render
	// "→ <mode>" rather than a synthesized from→to transition.
	const mode = str(payload.permission_mode);
	return (
		<Row
			kind="perm-mode"
			harness={harness}
			timestampMs={timestampMs}
			right={mode ? <span className="dim">→ {mode}</span> : undefined}
		>
			<span className="tool">permission mode</span>
		</Row>
	);
};

const BridgeStatusRow = ({ payload, harness, timestampMs }: SimpleRowProps) => {
	// The payload has no connected/disconnected status field, so this is
	// a flat "bridge session" notice (no up/down semantics) per the v1
	// decision.
	const sid = str(payload.bridge_session_id);
	return (
		<Row kind="bridge" harness={harness} timestampMs={timestampMs}>
			<span className="tool">bridge</span>
			{sid ? (
				<>
					{" "}
					<span className="target">{sid}</span>
				</>
			) : null}
		</Row>
	);
};

const PrRow = ({ payload, harness, timestampMs }: SimpleRowProps) => {
	const n = num(payload.pr_number);
	const repo = str(payload.pr_repository);
	return (
		<Row
			kind="pr"
			harness={harness}
			timestampMs={timestampMs}
			right={repo ? <span className="dim">{repo}</span> : undefined}
		>
			<span className="tool">opened</span> <span className="target">PR #{n ?? "?"}</span>
		</Row>
	);
};

const QueueRow = ({ payload, harness, timestampMs }: SimpleRowProps) => {
	const content = str(payload.content) ?? "";
	return (
		<Row
			kind="queue"
			harness={harness}
			timestampMs={timestampMs}
			right={<span className="dim">queued</span>}
		>
			<span className="tool">queue</span> <span className="target">"{content}"</span>
		</Row>
	);
};

const UserFileRow = ({ payload, harness, timestampMs }: SimpleRowProps) => {
	const filename = str(payload.filename);
	const snippet = str(payload.snippet);
	return (
		<Row
			kind="userfile"
			harness={harness}
			timestampMs={timestampMs}
			right={<span className="dim">user edited</span>}
			extra={snippet ? <ResultPreview label="snippet" body={snippet} /> : undefined}
		>
			<span className="tool">noticed</span> <span className="target">{basename(filename)}</span>{" "}
			<span className="dim">edited outside</span>
		</Row>
	);
};

const SlashRow = ({ payload, harness, timestampMs }: SimpleRowProps) => {
	const name = parseSlashName(str(payload.content) ?? "");
	return (
		<Row kind="slash" harness={harness} timestampMs={timestampMs}>
			<span className="tool">slash</span> <span className="target">/{name}</span>
		</Row>
	);
};

/// Pull the command name out of a slash_command payload's `content`.
/// Preferred form is a `<command-name>/clear</command-name>` wrapper.
/// But the backend extracts slash_command from the `local_command`
/// system row, whose content is usually the command's *output* (wrapped
/// in `<local-command-stdout>`), not the name — so we only fall back to
/// a leading token when the content actually looks like a "/cmd"
/// invocation, otherwise a neutral "command" placeholder. (Grabbing the
/// first word of arbitrary stdout would render a garbage name.)
/// Capturing the real name upstream is a backend follow-up (issue #91).
function parseSlashName(content: string): string {
	const tag = content.match(/<command-name>\s*\/?([^<\s]+)/i);
	if (tag?.[1]) return tag[1];
	const trimmed = content.trim();
	if (trimmed.startsWith("/")) {
		const lead = trimmed.match(/^\/([^\s]+)/);
		if (lead?.[1]) return lead[1];
	}
	return "command";
}
