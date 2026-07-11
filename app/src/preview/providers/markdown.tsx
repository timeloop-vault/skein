// Markdown provider — renders `.md` / `.mdx` / `.markdown` as a
// styled HTML view (issue #11 / #14).
//
// We parse with `marked` and sanitize the resulting HTML with
// DOMPurify before injecting it. `marked` 12+ removed its in-house
// sanitizer specifically because they recommend DOMPurify — so any
// raw HTML (including `<script>`, event handlers, `<iframe>`, ...)
// inside the markdown is stripped before it can run.
//
// `.mdx` is treated as plain markdown for v1 — full MDX needs a
// React-component runtime, which is more than a preview pane
// warrants. Any embedded JSX-style tags fall through DOMPurify's
// allow-list and get rendered (or stripped) as plain HTML.
//
// Links are routed through tauri-plugin-opener on click so they
// open in the user's external browser instead of trying to
// navigate the embedded webview (which Tauri's WebKit refuses).
// Relative links are no-ops — there's no preview-internal
// navigation yet; #15's interaction model is the right place to
// design that.

import { openUrl } from "@tauri-apps/plugin-opener";
import DOMPurify from "dompurify";
import { marked } from "marked";
import type { MouseEvent } from "react";
import { registerPreviewProvider } from "../registry.ts";

const isAbsoluteUrl = (href: string): boolean => /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(href);

const handleClick = (e: MouseEvent<HTMLDivElement>) => {
	const target = (e.target as HTMLElement).closest("a");
	if (!target) return;
	const href = target.getAttribute("href");
	if (!href) {
		e.preventDefault();
		return;
	}
	e.preventDefault();
	if (isAbsoluteUrl(href)) {
		void openUrl(href).catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.warn("[skein] markdown openUrl failed:", href, msg);
		});
	}
	// Relative links are intentionally inert for v1.
};

const renderMarkdown = (source: string): string => {
	// marked.parse can return a Promise when async extensions are
	// loaded; we use only sync core, so the cast is safe.
	const raw = marked.parse(source, { async: false, gfm: true, breaks: false }) as string;
	return DOMPurify.sanitize(raw, {
		// FORBID_TAGS is belt-and-braces: DOMPurify's default profile
		// already blocks <script>, event handlers, and `javascript:`
		// hrefs, but explicit denial keeps intent obvious for the
		// next reader.
		FORBID_TAGS: ["script", "style", "iframe", "object", "embed"],
		FORBID_ATTR: ["onerror", "onload", "onclick"],
	});
};

const MarkdownPreview = ({ text, truncated }: { text: string; truncated: boolean }) => {
	const html = renderMarkdown(text);
	return (
		<>
			{truncated && (
				<div style={{ color: "var(--warn)", marginBottom: 8, fontSize: 11 }}>
					truncated to first 256 KB
				</div>
			)}
			<div
				className="sk-md"
				onClick={handleClick}
				// biome-ignore lint/security/noDangerouslySetInnerHtml: sanitized by DOMPurify above
				dangerouslySetInnerHTML={{ __html: html }}
			/>
		</>
	);
};

registerPreviewProvider({
	id: "markdown",
	patterns: ["*.md", "*.mdx", "*.markdown"],
	priority: 50,
	needs: "text",
	render: ({ text, truncated }) => {
		if (text === undefined) return null;
		return <MarkdownPreview text={text} truncated={truncated} />;
	},
});
