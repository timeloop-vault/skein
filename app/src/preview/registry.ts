// Preview-provider registry (issue #14).
//
// Each provider claims a set of glob-ish file patterns, declares
// what shape of data it needs from disk (utf-8 text, raw bytes, or
// just the path), and renders the preview body. The host
// (`FileTree`) just picks the first matching provider for a path
// and delegates.
//
// v1 is in-tree only — every provider lives in
// `app/src/preview/providers/`. The registry shape is intended to
// stay stable so future external plugins can register here too.

import type { ReactNode } from "react";

export type PreviewNeeds = "text" | "bytes" | "path";

export interface PreviewCtx {
	path: string;
	/// Set when needs === "text" or when the provider opts to read
	/// text alongside bytes for tagged formats (svg, etc.).
	text?: string;
	/// Set when needs === "bytes". Base64-encoded; provider decodes
	/// or wraps in a data URL itself.
	bytesBase64?: string;
	/// True when the file was longer than the relevant cap and the
	/// host truncated. Provider chooses whether to render a banner or
	/// gracefully degrade.
	truncated: boolean;
}

export interface PreviewProvider {
	/// Stable id for diagnostics, settings overrides, telemetry.
	id: string;
	/// Glob suffix patterns. Currently only `*.ext` style (case-
	/// insensitive); we'll grow this as providers need more.
	patterns: string[];
	/// Higher first when multiple providers match. The fallback `text`
	/// provider sits at priority 0; specific formats use 10+.
	priority: number;
	needs: PreviewNeeds;
	/// `null` = explicit fall-through to the next match. Useful when
	/// a provider matches by extension but rejects on content (e.g.
	/// "this `.txt` is binary, hex provider please").
	render: (ctx: PreviewCtx) => ReactNode | null;
}

const REGISTRY: PreviewProvider[] = [];

export const registerPreviewProvider = (provider: PreviewProvider): void => {
	REGISTRY.push(provider);
	REGISTRY.sort((a, b) => b.priority - a.priority);
};

/// Returns every provider whose patterns match the path, ordered
/// by priority (highest first). The host iterates this list and
/// takes the first one whose render call yields a non-null body —
/// a provider can return `null` (or its underlying fetch can fail
/// with the conventional `"binary"` error) to signal "I matched by
/// pattern but I can't handle this content; fall through to the
/// next." That's how the text fallback yields to the hex viewer
/// for binary files.
export const findPreviewProviders = (path: string): PreviewProvider[] => {
	const lower = path.toLowerCase();
	return REGISTRY.filter((p) => p.patterns.some((pat) => matchPattern(lower, pat.toLowerCase())));
};

const matchPattern = (path: string, pattern: string): boolean => {
	// `*` = any extension/everything fallback
	if (pattern === "*") return true;
	// `*.ext` matches any file ending with `.ext`
	if (pattern.startsWith("*.")) return path.endsWith(pattern.slice(1));
	// Exact filename match
	const slash = path.lastIndexOf("/");
	const name = slash === -1 ? path : path.slice(slash + 1);
	return name === pattern;
};
