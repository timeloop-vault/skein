// editor.ts — CodeMirror 6 assembly for the Files editor (#185).
//
// One module owns the extension stack so every buffer looks and
// behaves the same: basicSetup (line numbers, undo/redo history,
// find/replace panel, multi-cursor, bracket matching), the Skein
// token theme + the design's syntax palette (--syn-* vars in
// styles.css, dark + light), a per-extension language, the Mod+S
// save hook, and read-only wiring for truncated large files —
// editing a 256 KB-truncated read and saving it would destroy the
// file's tail, so truncated buffers are view-only by construction.

import { indentWithTab } from "@codemirror/commands";
import { css } from "@codemirror/lang-css";
import { html } from "@codemirror/lang-html";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { python } from "@codemirror/lang-python";
import { rust } from "@codemirror/lang-rust";
import { yaml } from "@codemirror/lang-yaml";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { basicSetup } from "codemirror";

const langFor = (path: string): Extension | null => {
	const lower = path.toLowerCase();
	const ext = lower.slice(lower.lastIndexOf(".") + 1);
	switch (ext) {
		case "ts":
		case "tsx":
			return javascript({ typescript: true, jsx: ext === "tsx" });
		case "js":
		case "jsx":
		case "mjs":
		case "cjs":
			return javascript({ jsx: ext === "jsx" });
		case "rs":
			return rust();
		case "json":
			return json();
		case "md":
		case "mdx":
		case "markdown":
			return markdown();
		case "css":
			return css();
		case "html":
		case "htm":
			return html();
		case "py":
			return python();
		case "yml":
		case "yaml":
			return yaml();
		default:
			return null;
	}
};

// The design prototype's syntax palette, themed through CSS vars so
// dark/light both work (values in styles.css .sk-dark / .sk-light).
const skeinHighlight = HighlightStyle.define([
	{ tag: [tags.keyword, tags.operatorKeyword, tags.modifier], color: "var(--syn-keyword)" },
	{
		tag: [
			tags.function(tags.variableName),
			tags.function(tags.propertyName),
			tags.definition(tags.variableName),
			tags.typeName,
			tags.className,
		],
		color: "var(--syn-fn)",
	},
	{ tag: [tags.string, tags.special(tags.string), tags.regexp], color: "var(--syn-string)" },
	{ tag: [tags.number, tags.bool, tags.atom, tags.null], color: "var(--syn-number)" },
	{ tag: [tags.comment, tags.meta], color: "var(--syn-comment)" },
	{ tag: tags.heading, color: "var(--fg-0)", fontWeight: "600" },
]);

const skeinTheme = EditorView.theme({
	"&": {
		height: "100%",
		backgroundColor: "var(--bg-0)",
		color: "var(--fg-1)",
		fontSize: "12px",
	},
	".cm-scroller": { fontFamily: "var(--sk-mono)", lineHeight: "1.55" },
	".cm-content": { caretColor: "var(--fg-0)" },
	".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--fg-0)" },
	"&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground":
		{
			backgroundColor: "color-mix(in srgb, var(--accent) 22%, transparent)",
		},
	".cm-gutters": {
		backgroundColor: "var(--bg-0)",
		color: "var(--fg-3)",
		border: "none",
	},
	".cm-activeLine": { backgroundColor: "rgba(127, 127, 127, 0.06)" },
	".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--fg-1)" },
	".cm-panels": {
		backgroundColor: "var(--bg-1)",
		color: "var(--fg-1)",
		borderTop: "1px solid var(--line)",
		fontFamily: "var(--sk-mono)",
		fontSize: "11px",
	},
	".cm-panels input, .cm-panels button, .cm-panels label": { fontFamily: "var(--sk-mono)" },
	".cm-searchMatch": {
		backgroundColor: "color-mix(in srgb, var(--warn) 30%, transparent)",
	},
	".cm-searchMatch-selected": { backgroundColor: "var(--accent)", color: "var(--bg-0)" },
	".cm-selectionMatch": {
		backgroundColor: "color-mix(in srgb, var(--warn) 14%, transparent)",
	},
});

export interface BufferHooks {
	onDocChanged: () => void;
	onSave: () => void;
}

export const createBufferState = (
	doc: string,
	path: string,
	readOnly: boolean,
	hooks: BufferHooks,
): EditorState =>
	EditorState.create({
		doc,
		extensions: [
			// Preserve the file's line endings: CM normalizes to "\n"
			// internally; the facet makes state.sliceDoc() rejoin with
			// the original separator so a one-line edit can't rewrite
			// every line ending in the file (#185 review).
			doc.includes("\r\n") ? EditorState.lineSeparator.of("\r\n") : [],
			// Listed before basicSetup so Mod-s beats its keymaps.
			keymap.of([
				{
					key: "Mod-s",
					preventDefault: true,
					run: () => {
						hooks.onSave();
						return true;
					},
				},
				indentWithTab,
			]),
			basicSetup,
			langFor(path) ?? [],
			skeinTheme,
			syntaxHighlighting(skeinHighlight),
			EditorView.updateListener.of((u) => {
				if (u.docChanged) hooks.onDocChanged();
			}),
			...(readOnly ? [EditorState.readOnly.of(true), EditorView.editable.of(false)] : []),
		],
	});
