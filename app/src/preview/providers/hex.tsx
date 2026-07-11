// Hex provider — fallback for any file that no specific provider
// handled and that the text provider rejected as binary (issue #11
// / #14). Better than the previous "cannot preview: binary" dead
// end: at least you can see whether it's a PNG, a sqlite db, a
// compiled binary, etc.
//
// Priority is below `text`'s 0 so it only fires when text falls
// through. The fall-through itself happens in `FileTree` — when
// `read_file_text` returns the conventional `"binary"` error, the
// host moves on to the next matching provider.
//
// Rendering is a classic 16-bytes-per-row hex dump with an offset
// column and an ASCII gutter. We cap displayed bytes at 16 KB
// regardless of how much Rust returned: the DOM cost of a single
// `<pre>` for 16 KB is fine, but 5 MB hex-dumped is not.

import { registerPreviewProvider } from "../registry.ts";

const ROW_BYTES = 16;
const HEX_DISPLAY_BYTES = 16 * 1024;

const decodeBase64 = (b64: string): Uint8Array => {
	const bin = atob(b64);
	const arr = new Uint8Array(bin.length);
	for (let i = 0; i < bin.length; i++) {
		arr[i] = bin.charCodeAt(i);
	}
	return arr;
};

const formatHex = (bytes: Uint8Array): string => {
	const lines: string[] = [];
	for (let off = 0; off < bytes.length; off += ROW_BYTES) {
		const slice = bytes.subarray(off, off + ROW_BYTES);
		const offsetCol = off.toString(16).padStart(8, "0");
		const hexParts: string[] = [];
		for (let i = 0; i < ROW_BYTES; i++) {
			// Extra space in the middle for the canonical hexdump
			// look — easier on the eyes when scanning.
			if (i === 8) hexParts.push("");
			if (i < slice.length) {
				const byte = slice[i] ?? 0;
				hexParts.push(byte.toString(16).padStart(2, "0"));
			} else {
				hexParts.push("  ");
			}
		}
		const hexCol = hexParts.join(" ");
		let ascii = "";
		for (let i = 0; i < slice.length; i++) {
			const byte = slice[i] ?? 0;
			ascii += byte >= 0x20 && byte < 0x7f ? String.fromCharCode(byte) : ".";
		}
		lines.push(`${offsetCol}  ${hexCol}  |${ascii}|`);
	}
	return lines.join("\n");
};

const HexPreview = ({
	bytesBase64,
	truncatedByHost,
}: {
	bytesBase64: string;
	truncatedByHost: boolean;
}) => {
	const decoded = decodeBase64(bytesBase64);
	const visible = decoded.subarray(0, HEX_DISPLAY_BYTES);
	const truncatedByProvider = decoded.length > HEX_DISPLAY_BYTES;
	const display = formatHex(visible);
	return (
		<>
			{(truncatedByProvider || truncatedByHost) && (
				<div style={{ color: "var(--warn)", marginBottom: 8, fontSize: 11 }}>
					hex view: first {HEX_DISPLAY_BYTES / 1024} KB of {decoded.length.toLocaleString()} bytes
					{truncatedByHost && " (host capped at 5 MB)"}
				</div>
			)}
			<pre
				style={{
					fontFamily: "var(--sk-mono)",
					fontSize: 11,
					color: "var(--fg-1)",
					margin: 0,
					whiteSpace: "pre",
				}}
			>
				{display}
			</pre>
		</>
	);
};

registerPreviewProvider({
	id: "hex",
	// `*` so we're a candidate for every path; priority below the
	// text fallback (0) means we're only reached when text returns
	// the `"binary"` error.
	patterns: ["*"],
	priority: -10,
	needs: "bytes",
	render: ({ bytesBase64, truncated }) => {
		if (!bytesBase64) return null;
		return <HexPreview bytesBase64={bytesBase64} truncatedByHost={truncated} />;
	},
});
