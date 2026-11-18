#!/usr/bin/env node
// Writes scrollback-session.json: a synthetic, deterministic PTY recording
// that lives on the MAIN screen and scrolls (#793). No dependencies, no Date,
// no randomness; run it again and the file is byte-identical.
//
//   node crates/skein-daemon/src/pty/engine/fixtures/make-scrollback-session.mjs
//
// The cut indexes in scrollback-session.cuts.json refer to event positions;
// the script prints the index of each named mark so the cuts can be kept in
// step by hand after a change here.

import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ESC = "\x1b";
const events = [];
const marks = [];
let ms = 0;

function tick() {
	ms += 7;
	return ms;
}

/** One data event from a string (UTF-8) or raw bytes. */
function data(x) {
	const buf = typeof x === "string" ? Buffer.from(x, "utf8") : x;
	events.push({ ms: tick(), kind: "data", b64: buf.toString("base64") });
}

/** Splits `s` after `byteAt` bytes into two events, mid escape or mid character. */
function splitData(s, byteAt) {
	const buf = Buffer.from(s, "utf8");
	data(buf.subarray(0, byteAt));
	data(buf.subarray(byteAt));
}

function resize(cols, rows) {
	events.push({ ms: tick(), kind: "resize", cols, rows });
}

/** Names the state after the last event added so far. */
function mark(name) {
	marks.push({ name, afterEvent: events.length - 1 });
}

const sgr = (...p) => `${ESC}[${p.join(";")}m`;
const reset = sgr(0);
const CRLF = "\r\n";

// 1. About 200 lines of shell-like output with a rotation of styles.
const styles = [
	(t) => `${sgr(31)}${t}${reset}`, // 16-colour
	(t) => `${sgr(1, 32)}${t}${reset}`, // bold green
	(t) => `${sgr(38, 5, 208)}${t}${reset}`, // 256-colour
	(t) => `${sgr(38, 2, 90, 160, 250)}${t}${reset}`, // truecolor
	(t) => `${sgr(3)}${t}${reset}`, // italic
	(t) => `${sgr(4, 36)}${t}${reset}`, // underline cyan
	(t) => `${sgr(7)}${t}${reset}`, // inverse
	(t) => `${sgr(48, 5, 24)}${sgr(97)}${t}${reset}`, // background
];

for (let i = 0; i < 200; i++) {
	const num = String(i).padStart(3, "0");
	const body = styles[i % styles.length](`step ${num}: compiled module_${i * 7}`);
	const line = `${sgr(90)}$${reset} ${body} ${sgr(2)}ok${reset}${CRLF}`;
	if (i % 25 === 3) {
		// An escape sequence cut in the middle: after ESC, then after "38;2;".
		const cut = line.indexOf(`${ESC}[`) + 1;
		splitData(line, cut);
	} else if (i % 25 === 11 && i % styles.length === 3) {
		splitData(line, line.indexOf("38;2;") + 5);
	} else if (i % 40 === 17) {
		// A line delivered in two ordinary chunks.
		splitData(line, Math.floor(line.length / 2));
	} else {
		data(line);
	}
	if (i === 120) mark("mid-scroll");
}

// 2. Long lines that wrap: 2x and 3x the width of 100 columns.
data(`${"a".repeat(200)}${CRLF}`);
data(`${sgr(33)}${"b".repeat(250)}${reset}${CRLF}`);
data(`${sgr(1, 35)}${"c".repeat(100)}${sgr(0, 36)}${"d".repeat(100)}${sgr(4)}${"e".repeat(100)}${reset}${CRLF}`);
data(`${"0123456789".repeat(20)}${CRLF}`);
mark("after-wrapped");

// 3. Wide cells: CJK and emoji, with a wide character on the last column.
data(`${"日本語のテキスト、漢字かな交じり文。".repeat(2)}${CRLF}`);
data(`${sgr(32)}emoji: 🙂 🚀 🎉 done${reset}${CRLF}`);
data(`${"x".repeat(99)}日${CRLF}`); // wide char needs 2 cells, only 1 left: it wraps
data(`${"y".repeat(98)}日${CRLF}`); // exactly fills columns 99-100
{
	// A 3-byte character cut between two chunks.
	const s = `split: ${sgr(31)}漢字${reset} and 🚀 after${CRLF}`;
	splitData(s, Buffer.from("split: ", "utf8").length + Buffer.from(sgr(31), "utf8").length + 1);
}
mark("after-wide");

// 4. OSC 8 hyperlinks, terminated by ST (ESC \).
data(`see ${ESC}]8;;https://example.com/a${ESC}\\this link${ESC}]8;;${ESC}\\ and ${ESC}]8;id=7;https://example.com/b?x=1${ESC}\\${sgr(4)}another${reset}${ESC}]8;;${ESC}\\ for details${CRLF}`);
{
	const s = `${ESC}]8;;https://example.com/c${ESC}\\split link${ESC}]8;;${ESC}\\ tail${CRLF}`;
	splitData(s, s.indexOf("example") + 3);
}
mark("after-hyperlinks");

// 5. A synchronized-output frame (mode 2026) that redraws in place.
data(`status A: idle${CRLF}status B: idle${CRLF}status C: idle${CRLF}`);
data(`${ESC}[?2026h${ESC}[3A\r${ESC}[Kstatus A: ${sgr(32)}running${reset}${CRLF}${ESC}[K`);
mark("in-sync-frame");
data(`status B: ${sgr(33)}waiting${reset}${CRLF}${ESC}[Kstatus C: ${sgr(31)}failed${reset}${CRLF}${ESC}[?2026l`);
mark("after-sync-frame");

// 6. Resize narrower, more output, resize back and taller, more output.
resize(60, 30);
mark("resize-60x30");
for (let i = 0; i < 30; i++) {
	const text = i % 10 === 9 ? `${"n".repeat(70)} (wraps at 60)` : `narrow line ${i}`;
	data(`${styles[i % styles.length](text)}${CRLF}`);
	if (i === 3) mark("narrow-after-lines");
}
resize(100, 40);
mark("resize-100x40");
for (let i = 0; i < 30; i++) {
	data(`${styles[(i + 3) % styles.length](`tall line ${i}: ${"-".repeat(i)}`)}${CRLF}`);
}
mark("tall-after-lines");

// 7. The alternate screen: a small full-screen UI, then back to the main one.
data(`${ESC}[?1049h${ESC}[2J${ESC}[H`);
data(`${ESC}[1;1H${sgr(7)} editor ${reset}${ESC}[2;1H${"=".repeat(40)}`);
for (let r = 3; r <= 8; r++) {
	data(`${ESC}[${r};3Hrow ${r} of the alternate screen ${sgr(36)}|${reset}`);
}
data(`${ESC}[10;1H${sgr(1)}q to quit${reset}${ESC}[3;10H`);
mark("in-alt-screen");
data(`${ESC}[?1049l`);
mark("after-alt-screen");
for (let i = 0; i < 20; i++) {
	data(`${sgr(94)}back on main ${i}${reset}${CRLF}`);
}

// 8. A prompt-like line with the cursor left in the middle of it.
data(`${sgr(1, 32)}user@host${reset}:${sgr(1, 34)}~/project${reset}$ echo hello world`);
data(`${ESC}[5D`);
mark("end-of-stream");

const here = dirname(fileURLToPath(import.meta.url));
const file = {
	version: 1,
	cols: 100,
	rows: 30,
};
const body = events.map((e) => `  ${JSON.stringify(e)}`).join(",\n");
const head = JSON.stringify(file).slice(0, -1);
writeFileSync(join(here, "scrollback-session.json"), `${head},"events":[\n${body}\n]}\n`);

for (const m of marks) console.log(`${m.name}\t${m.afterEvent}`);
console.log(`${events.length} events`);
