// Error, compaction and generic fallback rows, plus the error-toast copy (#80 D2b, split #460).

import { num, obj, type Payload, str } from "./payload.ts";
import { ResultPreview } from "./ResultPreview.tsx";
import { formatDuration, Row } from "./Row.tsx";
import { harnessTool, normalizeResultString, type ToolRowProps } from "./toolShared.ts";

export const CompactRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const auto = payload.auto === true;
	return (
		<Row kind="compact" harness={harness} timestampMs={timestampMs}>
			<span className="tool">compacted context</span>
			{auto ? <span className="dim"> · auto</span> : null}
		</Row>
	);
};

export const ApiErrorRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const status = errorStatus(payload);
	const attempt = num(payload.retry_attempt);
	// Preview = the human message + the (static) retry interval. The
	// interval is shown as a fixed "retrying in Xs", not a live ticking
	// countdown: these rows are almost always historical by the time the
	// feed is read, so a running timer would lie.
	const message = apiErrorMessage(payload);
	const retryMs = num(payload.retry_in_ms);
	const retryLine = retryMs != null ? `retrying in ${formatDuration(Math.round(retryMs))}` : "";
	const detail = [message, retryLine].filter(Boolean).join("\n");
	const preview = detail ? (
		<ResultPreview label="error" body={detail} variant="api-error" />
	) : undefined;
	return (
		<Row
			kind="error"
			harness={harness}
			timestampMs={timestampMs}
			right={attempt != null ? <span className="dim">attempt {attempt}</span> : undefined}
			extra={preview}
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

export const ToolErrorRow = ({
	tool,
	payload,
	harness,
	timestampMs,
}: ToolRowProps & { tool: string }) => {
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
export const GenericToolRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
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

/// Cross-room error-toast copy (D2f, handover §6 step 2): a headline
/// like `Overloaded (529), retrying · attempt 4 of 10 · retry in 4.4s`,
/// composed from the same payload paths the inline ApiErrorRow reads.
export function apiErrorToastText(payload: Payload): string {
	const message = apiErrorMessage(payload);
	const status = errorStatus(payload);
	const retryMs = num(payload.retry_in_ms);
	const attempt = num(payload.retry_attempt);
	const max = num(payload.max_retries);
	const line = `${message || "api error"}${status ? ` (${status})` : ""}${
		retryMs != null ? ", retrying" : ""
	}`;
	return [
		line,
		attempt != null ? `attempt ${attempt}${max != null ? ` of ${max}` : ""}` : "",
		retryMs != null ? `retry in ${formatDuration(Math.round(retryMs))}` : "",
	]
		.filter(Boolean)
		.join(" · ");
}

/// Human-readable api_error message. The raw SDK error object nests one
/// level deeper than the obvious path (live data: error.error.error.message),
/// so walk a few shapes and fall back to the type tag.
function apiErrorMessage(payload: Payload): string {
	const e = obj(payload.error);
	if (!e) return "";
	return (
		str(obj(obj(e.error)?.error)?.message) ??
		str(obj(e.error)?.message) ??
		str(e.message) ??
		str(e.type) ??
		""
	);
}
