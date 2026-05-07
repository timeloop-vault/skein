// Fallback text provider — handles whatever no specific provider
// claimed. UTF-8 lossy decode happens in Rust (`read_file_text`),
// then we just render as monospace pre.

import { registerPreviewProvider } from "../registry.ts";

const TextPreview = ({ text, truncated }: { text: string; truncated: boolean }) => (
	<>
		{truncated && (
			<div style={{ color: "var(--warn)", marginBottom: 8 }}>truncated to first 256 KB</div>
		)}
		<div>{text}</div>
	</>
);

registerPreviewProvider({
	id: "text",
	// `*` = catch-all fallback. Lower priority than every specific
	// provider so it only fires when nothing else matched.
	patterns: ["*"],
	priority: 0,
	needs: "text",
	render: ({ text, truncated }) => {
		if (text === undefined) return null;
		return <TextPreview text={text} truncated={truncated} />;
	},
});
