// Shared types and normalizers for the tool-family rows (#80 D2b, split #460).

import type { HarnessKind } from "../types.ts";
import { obj, type Payload, str } from "./payload.ts";

export interface ToolRowProps {
	payload: Payload;
	harness: HarnessKind | null | undefined;
	timestampMs: number;
}

/// Results smaller than this (in UTF-8 bytes, same basis as the size
/// pill) stay inline; larger ones get an expandable preview block.
/// Matches the handover's ~200-byte rule.
export const PREVIEW_MIN = 200;

/// Normalized (lowercase) tool name. Claude is CamelCase (`Edit`,
/// `Bash`, `Grep`, `AskUserQuestion`, `Task`); opencode is already
/// lowercase. "" when absent.
export function harnessTool(payload: Payload): string {
	return (str(payload.tool) ?? "").toLowerCase();
}

/// Normalize a tool result to a display string. Claude results are
/// `toolUseResult` objects (Bash `{stdout,…}`, Read `{file}`) or
/// strings; opencode results are plain `state.output` strings.
export function normalizeResultString(result: unknown): string {
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
export function countNonEmptyLines(s: string): number | undefined {
	if (!s) return undefined;
	const n = s.split("\n").filter((line) => line.trim().length > 0).length;
	return n || undefined;
}
