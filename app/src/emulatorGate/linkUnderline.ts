// The one style quirk the gate excuses (#793): xterm.js reports underline on
// every OSC 8 hyperlink cell although no SGR 4 was sent. Probe: writing
// `ESC]8;;u BEL ab` gives isUnderline() true on `ab`; the engine keeps the SGR
// separate. The rule is deliberately one-sided and narrow: a cell is excused
// only when BOTH sides carry the same link there, xterm (A) is underlined, the
// engine (B) is not, and every other style field is equal. The reverse
// (engine underlined, xterm not), a bold or colour difference, or a link on
// one side only all stay differences. Excused cells are counted per cut so
// the rule can neither widen nor go dead unnoticed.

import type { Row, Run } from "./picture.ts";

/** The key without its underline flag, or undefined when it has none. */
export function withoutUnderline(style: string): string | undefined {
	const m = /^(fg=\S+ bg=\S+) (\S+)$/.exec(style);
	if (!m) return undefined;
	const flags = (m[2] ?? "").split(",").filter((f) => f !== "-");
	if (!flags.includes("underline")) return undefined;
	const rest = flags.filter((f) => f !== "underline").join(",");
	return `${m[1]} ${rest || "-"}`;
}

const at = (row: Row | undefined, x: number): Run | undefined =>
	row?.runs.find((r) => r.col <= x && x < r.end);

/** A's rows with the excused cells restyled as B has them, and how many cells that was. Texts of split runs are dropped: only the style view reads these rows. */
export function excuseLinkUnderline(a: Row[], b: Row[]): { rows: Row[]; excused: number } {
	let excused = 0;
	const rows = a.map((ra, i) => {
		const rb = b[i];
		const out: Run[] = [];
		for (const run of ra.runs) {
			const plain = run.link ? withoutUnderline(run.style) : undefined;
			if (plain === undefined) {
				out.push(run);
				continue;
			}
			for (let x = run.col; x < run.end; x++) {
				const other = at(rb, x);
				const ok = other !== undefined && other.link === run.link && other.style === plain;
				if (ok) excused++;
				const style = ok ? plain : run.style;
				const last = out[out.length - 1];
				if (last && last.style === style && last.end === x && last.link === run.link)
					last.end = x + 1;
				else out.push({ col: x, end: x + 1, style, text: "", link: run.link });
			}
		}
		return { ...ra, runs: out };
	});
	return { rows, excused };
}
