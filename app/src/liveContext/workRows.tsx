// Shell, plan, question and sub-agent rows (#80 D2b, split #460).

import { num, obj, type Payload, str } from "./payload.ts";
import { byteLen, formatBytes, ResultPreview } from "./ResultPreview.tsx";
import { formatDuration, Row } from "./Row.tsx";
import { normalizeResultString, PREVIEW_MIN, type ToolRowProps } from "./toolShared.ts";

export const BashRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const command = str(obj(payload.input)?.command) ?? "";
	const title = str(payload.title); // opencode supplies a human title; Claude does not
	const ms = num(payload.duration_ms);
	const isError = payload.is_error === true;
	const out = bashOutput(payload);
	const outBytes = byteLen(out);
	const preview =
		outBytes > PREVIEW_MIN ? (
			<ResultPreview label="output" body={out} sizeLabel={formatBytes(outBytes)} />
		) : undefined;
	return (
		<Row
			kind="bash"
			harness={harness}
			timestampMs={timestampMs}
			right={ms != null ? <span className="dim">{ms}ms</span> : undefined}
			extra={preview}
		>
			<span className="tool">bash</span> <span className="target">{title ?? command}</span>
			{isError && <span className="err-text"> · failed</span>}
			{/* When a friendly title is shown, surface the raw command dim
			    behind it. (No title → the command is already the target.) */}
			{title && command ? <span className="dim"> · {command}</span> : null}
		</Row>
	);
};

export const TaskRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
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

export const TodoWriteRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const pi = obj(payload.plan_item);
	const count = num(pi?.count) ?? 0;
	// opencode persists the full item list at plan_item.items (Claude
	// never reaches this row — it emits create/update, not write).
	const itemsRaw = pi?.items;
	const items = Array.isArray(itemsRaw) ? itemsRaw : [];
	const body = todoItemsText(items);
	const preview = body ? (
		<ResultPreview label="plan" body={body} sizeLabel={`${count} todos`} />
	) : undefined;
	return (
		<Row
			kind="todowrite"
			harness={harness}
			timestampMs={timestampMs}
			right={<span className="dim">replaced plan</span>}
			extra={preview}
		>
			<span className="tool">todowrite</span> <span className="target">{count} todos</span>
		</Row>
	);
};

export const AskRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const question = firstQuestion(obj(payload.input));
	const chosen = chosenAnswer(payload.result);
	// Claude carries a structured {answers} object (the chosen value is
	// already inline above); opencode carries a result string, which is
	// the only thing long enough to warrant a preview.
	const detail = normalizeResultString(payload.result);
	const detailBytes = byteLen(detail);
	const preview =
		detailBytes > PREVIEW_MIN ? (
			<ResultPreview label="result" body={detail} sizeLabel={formatBytes(detailBytes)} />
		) : undefined;
	return (
		<Row
			kind="ask"
			harness={harness}
			timestampMs={timestampMs}
			right={<span className="dim">user chose</span>}
			extra={preview}
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

export const AgentRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const input = obj(payload.input);
	const title =
		str(payload.title) ?? str(input?.description) ?? str(input?.subagent_type) ?? "sub-agent";
	const ms = num(payload.duration_ms);
	// The sub-agent inspector (handover §7) is deferred: its centrepiece —
	// the ordered list of tool calls the sub-agent made — isn't in
	// harness_actions (Claude drops isSidechain rows; opencode's run is a
	// separate session), and capturing it is backend work tracked under
	// #91. Until then the row stays non-clickable rather than open a
	// near-empty drawer.
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

/// Bash preview body: Claude's result is an object → stdout, with
/// stderr appended when present; opencode's is the plain output string.
function bashOutput(payload: Payload): string {
	const r = obj(payload.result);
	if (r) {
		const out = str(r.stdout) ?? "";
		const err = str(r.stderr) ?? "";
		return err.trim() ? `${out}${out ? "\n" : ""}${err}` : out;
	}
	return normalizeResultString(payload.result);
}

/// Render an opencode todo list (plan_item.items: {content,status,…})
/// as a status-marked text block. Empty when no items have text.
function todoItemsText(items: unknown[]): string {
	return items
		.map((it) => {
			const o = obj(it);
			const text = str(o?.content) ?? str(o?.text) ?? str(o?.subject) ?? "";
			return `${todoMark(str(o?.status))} ${text}`.trimEnd();
		})
		.filter((line) => line.length > 1)
		.join("\n");
}

/// Glyph for a todo status. Unknown/pending → "○".
function todoMark(status: string | undefined): string {
	switch (status) {
		case "completed":
		case "done":
			return "✓";
		case "in_progress":
		case "active":
			return "▸";
		default:
			return "○";
	}
}
