// The tool_call / patch / plan_change family — issue #80 D2b.
//
// This is where every Claude↔opencode payload divergence lives (see
// docs/live-context-d2-buildmap.md): patch deltas, result shape,
// tool-name casing, the plan_change two-mode split. `ToolFamilyRow`
// does the second dispatch level (after rows.tsx switches on kind):
// it runs the is_error short-circuit, then sub-classifies by the
// normalized tool name / plan_item.op.

import type { HarnessKind } from "../types.ts";
import { Row, basename } from "./Row.tsx";
import { type Payload, num, obj, str } from "./payload.ts";

interface ToolRowProps {
	payload: Payload;
	harness: HarnessKind | undefined;
	timestampMs: number;
}

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
	harness: HarnessKind | undefined;
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

// ── components ─────────────────────────────────────────────────────

const EditRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const tool = harnessTool(payload);
	const files = Array.isArray(payload.files) ? payload.files : [];
	const pi = obj(payload.patch_info);

	// opencode emits a multi-file commit snapshot under kind=patch with
	// no `tool` and no `patch_info` ({files, hash}). Render it as a
	// files-changed marker rather than mislabeling it a single edit.
	if (!tool && !pi) {
		const n = files.length;
		return (
			<Row kind="edit" harness={harness} timestampMs={timestampMs}>
				<span className="tool">patch</span>{" "}
				<span className="target">
					{n} file{n === 1 ? "" : "s"}
				</span>
			</Row>
		);
	}

	const dk = tool === "write" || tool === "multiedit" ? "write" : "edit";
	const file = basename(str(files[0]));
	// additions/deletions share keys across harnesses (Claude derives
	// from structured_patch, opencode pre-computes); patch_info is
	// absent for a Write with no diff → deltas omitted.
	const adds = pi ? num(pi.additions) : undefined;
	const dels = pi ? num(pi.deletions) : undefined;
	return (
		<Row
			kind={dk}
			harness={harness}
			timestampMs={timestampMs}
			right={
				<>
					{adds != null && <span className="delta-add">+{adds}</span>}
					{adds != null && dels != null ? " " : ""}
					{dels != null && <span className="delta-del">−{dels}</span>}
				</>
			}
		>
			<span className="tool">{dk}</span> <span className="target">{file}</span>
		</Row>
	);
};

const ReadRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const input = obj(payload.input);
	const file = basename(str(input?.file_path) ?? str(input?.filePath));
	const lines = lineCount(payload.result);
	return (
		<Row
			kind="read"
			harness={harness}
			timestampMs={timestampMs}
			right={lines != null ? <span className="dim">{lines} ln</span> : undefined}
		>
			<span className="tool">read</span> <span className="target">{file}</span>
		</Row>
	);
};

const SearchRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const dk = harnessTool(payload) === "glob" ? "glob" : "grep";
	const pattern = str(obj(payload.input)?.pattern) ?? "";
	const matches = countNonEmptyLines(normalizeResultString(payload.result));
	return (
		<Row
			kind={dk}
			harness={harness}
			timestampMs={timestampMs}
			right={
				matches != null ? (
					<span className="dim">
						{matches} {matches === 1 ? "match" : "matches"}
					</span>
				) : undefined
			}
		>
			<span className="tool">{dk}</span> <span className="arg">{pattern}</span>
		</Row>
	);
};

const BashRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const command = str(obj(payload.input)?.command) ?? "";
	const title = str(payload.title); // opencode supplies a human title; Claude does not
	const ms = num(payload.duration_ms);
	const isError = payload.is_error === true;
	return (
		<Row
			kind="bash"
			harness={harness}
			timestampMs={timestampMs}
			right={ms != null ? <span className="dim">{ms}ms</span> : undefined}
		>
			<span className="tool">bash</span> <span className="target">{title ?? command}</span>
			{isError && <span className="err-text"> · failed</span>}
			{/* When a friendly title is shown, surface the raw command dim
			    behind it. (No title → the command is already the target.) */}
			{title && command ? <span className="dim"> · {command}</span> : null}
		</Row>
	);
};

const TaskRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const pi = obj(payload.plan_item) ?? {};
	const create = str(pi.op) === "create";
	const subject = str(pi.subject);
	const id = pi.id != null ? String(pi.id) : "";
	const text = subject ?? (id ? `#${id}` : "");
	const sc = obj(pi.status_change);
	const transition = sc ? `${str(sc.from) ?? "?"} → ${str(sc.to) ?? "?"}` : "";
	return (
		<Row
			kind="task"
			harness={harness}
			timestampMs={timestampMs}
			right={!create && transition ? <span className="dim">{transition}</span> : undefined}
		>
			<span className="tool">{create ? "+ task" : "update"}</span>{" "}
			<span className="target">{text}</span>
			{create && <span className="pill">pending</span>}
		</Row>
	);
};

const TodoWriteRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const count = num(obj(payload.plan_item)?.count) ?? 0;
	return (
		<Row
			kind="todowrite"
			harness={harness}
			timestampMs={timestampMs}
			right={<span className="dim">replaced plan</span>}
		>
			<span className="tool">todowrite</span> <span className="target">{count} todos</span>
		</Row>
	);
};

const AskRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const question = firstQuestion(obj(payload.input));
	const chosen = chosenAnswer(payload.result);
	return (
		<Row
			kind="ask"
			harness={harness}
			timestampMs={timestampMs}
			right={<span className="dim">user chose</span>}
		>
			<span className="tool">asked</span> <span className="target">{question}</span>
			{chosen ? (
				<>
					{" "}
					<span className="arg">→ {chosen}</span>
				</>
			) : null}
		</Row>
	);
};

const AgentRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const input = obj(payload.input);
	const title =
		str(payload.title) ?? str(input?.description) ?? str(input?.subagent_type) ?? "sub-agent";
	const ms = num(payload.duration_ms);
	// D4 wires the inspector (onOpen seam); D2b renders the row
	// non-clickable.
	return (
		<Row
			kind="agent"
			harness={harness}
			timestampMs={timestampMs}
			right={ms != null ? <span className="dim">{formatDuration(ms)}</span> : undefined}
		>
			<span className="tool">sub-agent</span> <span className="target">{title}</span>
		</Row>
	);
};

const CompactRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const auto = payload.auto === true;
	return (
		<Row kind="compact" harness={harness} timestampMs={timestampMs}>
			<span className="tool">compacted context</span>
			{auto ? <span className="dim"> · auto</span> : null}
		</Row>
	);
};

const ApiErrorRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const status = errorStatus(payload);
	const attempt = num(payload.retry_attempt);
	// The message + retry countdown preview block lands in D2c.
	return (
		<Row
			kind="error"
			harness={harness}
			timestampMs={timestampMs}
			right={attempt != null ? <span className="dim">attempt {attempt}</span> : undefined}
		>
			<span className="tool err-text">api error</span>
			{status ? (
				<>
					{" "}
					<span className="target">{status}</span>
				</>
			) : null}
		</Row>
	);
};

const ToolErrorRow = ({ tool, payload, harness, timestampMs }: ToolRowProps & { tool: string }) => {
	const message = errorMessage(payload);
	return (
		<Row kind="error" harness={harness} timestampMs={timestampMs}>
			<span className="tool err-text">{tool}</span>
			{message ? (
				<>
					{" "}
					<span className="target">{message}</span>
				</>
			) : null}
		</Row>
	);
};

/// Unknown tools (Skill / ToolSearch / Monitor / …) — show the tool
/// name + a best-effort target. No glyph entry → the neutral "·".
const GenericToolRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const tool = harnessTool(payload) || "tool";
	const target = genericTarget(tool, payload);
	return (
		<Row kind="tool" harness={harness} timestampMs={timestampMs}>
			<span className="tool">{tool}</span>
			{target ? (
				<>
					{" "}
					<span className="target">{target}</span>
				</>
			) : null}
		</Row>
	);
};

// ── helpers ────────────────────────────────────────────────────────

/// Normalized (lowercase) tool name. Claude is CamelCase (`Edit`,
/// `Bash`, `Grep`, `AskUserQuestion`, `Task`); opencode is already
/// lowercase. "" when absent.
function harnessTool(payload: Payload): string {
	return (str(payload.tool) ?? "").toLowerCase();
}

/// Normalize a tool result to a display string. Claude results are
/// `toolUseResult` objects (Bash `{stdout,…}`, Read `{file}`) or
/// strings; opencode results are plain `state.output` strings.
function normalizeResultString(result: unknown): string {
	if (typeof result === "string") return result;
	const r = obj(result);
	if (!r) return "";
	if (typeof r.stdout === "string") return r.stdout;
	if (typeof r.output === "string") return r.output;
	if (typeof r.content === "string") return r.content;
	if (typeof r.file === "string") return r.file;
	const file = obj(r.file);
	if (file && typeof file.content === "string") return file.content;
	return "";
}

/// Non-empty line count of a string, or undefined when empty.
function countNonEmptyLines(s: string): number | undefined {
	if (!s) return undefined;
	const n = s.split("\n").filter((line) => line.trim().length > 0).length;
	return n || undefined;
}

/// Read line count: Claude may carry it as `result.file.numLines`;
/// otherwise count newlines of the normalized result string.
function lineCount(result: unknown): number | undefined {
	const file = obj(obj(result)?.file);
	if (file) {
		const n = num(file.numLines);
		if (n != null) return n;
	}
	return countNonEmptyLines(normalizeResultString(result));
}

/// First question text from an AskUserQuestion/question input
/// (`{questions:[{question,…}]}`), with a flat `question` fallback.
function firstQuestion(input: Payload | undefined): string {
	if (!input) return "";
	const qs = input.questions;
	if (Array.isArray(qs) && qs.length > 0) {
		const q = str(obj(qs[0])?.question);
		if (q) return q;
	}
	return str(input.question) ?? "";
}

/// The chosen answer. Claude: `result.answers` is `{question: choice}`
/// → first value. opencode: result is a string → no structured answer
/// (omitted).
function chosenAnswer(result: unknown): string | undefined {
	const answers = obj(obj(result)?.answers);
	if (answers) {
		const first = Object.values(answers)[0];
		if (typeof first === "string") return first;
	}
	return undefined;
}

/// api_error status — `error` may be an object (`{status}`), a string,
/// or absent.
function errorStatus(payload: Payload): string {
	const e = payload.error;
	const eo = obj(e);
	if (eo) {
		const s = eo.status;
		if (typeof s === "number" || typeof s === "string") return String(s);
	}
	if (typeof e === "string") return e;
	return "";
}

/// Tool-error message. opencode carries an explicit `error` string;
/// Claude conveys it via the result body, so fall back to a truncated
/// normalized result.
function errorMessage(payload: Payload): string {
	const e = str(payload.error);
	if (e) return e;
	const body = normalizeResultString(payload.result).trim();
	if (!body) return "";
	const oneLine = body.replace(/\s+/g, " ");
	return oneLine.length > 120 ? `${oneLine.slice(0, 120)}…` : oneLine;
}

/// Best-effort target for an unknown generic tool — the low-divergence
/// input fields only.
function genericTarget(tool: string, payload: Payload): string {
	const inp = obj(payload.input) ?? {};
	switch (tool) {
		case "skill":
			return str(inp.skill) ?? str(inp.name) ?? "";
		case "toolsearch":
			return str(inp.query) ?? "";
		default:
			return "";
	}
}

/// ms → "850ms" / "4.2s" / "3m".
function formatDuration(ms: number): string {
	if (ms < 1000) return `${ms}ms`;
	if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
	return `${Math.round(ms / 60_000)}m`;
}
