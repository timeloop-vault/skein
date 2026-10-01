// File and search rows: edit / read / grep+glob (#80 D2b, split #460).

import { ResultPreview, byteLen, formatBytes } from "./ResultPreview.tsx";
import { Row, basename } from "./Row.tsx";
import { num, obj, str } from "./payload.ts";
import {
	PREVIEW_MIN,
	type ToolRowProps,
	countNonEmptyLines,
	harnessTool,
	normalizeResultString,
} from "./toolShared.ts";

export const EditRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
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

export const ReadRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
	const input = obj(payload.input);
	const file = basename(str(input?.file_path) ?? str(input?.filePath));
	const lines = lineCount(payload.result);
	const content = normalizeResultString(payload.result);
	const contentBytes = byteLen(content);
	// Pill is bytes (the line count already rides in the row's right-meta,
	// so showing "N ln" in both places would be redundant).
	const preview =
		contentBytes > PREVIEW_MIN ? (
			<ResultPreview label="file" body={content} sizeLabel={formatBytes(contentBytes)} />
		) : undefined;
	return (
		<Row
			kind="read"
			harness={harness}
			timestampMs={timestampMs}
			right={lines != null ? <span className="dim">{lines} ln</span> : undefined}
			extra={preview}
		>
			<span className="tool">read</span> <span className="target">{file}</span>
		</Row>
	);
};

export const SearchRow = ({ payload, harness, timestampMs }: ToolRowProps) => {
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
