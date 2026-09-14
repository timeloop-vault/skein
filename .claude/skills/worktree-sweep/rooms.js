// Which Skein rooms point at a worktree folder, and whether they are
// still active. Read-only; run it against a COPY of skein.db so the
// running app's WAL is never touched.
//
//   bun run rooms.js <path-to-skein.db-copy> <worktree-folder-substring>
//
// Rooms live in the `sessions` table (legacy name) as one JSON blob
// per row; `cwd` is the room's folder and `archived` is an epoch ms
// when the room is closed, absent while it is open.
import { Database } from "bun:sqlite";

const [dbPath, needle] = process.argv.slice(2);
if (!dbPath || !needle) {
	console.error("usage: bun run rooms.js <skein.db copy> <worktree folder substring>");
	process.exit(2);
}

const db = new Database(dbPath, { readonly: true });
const rows = db.query("select data from sessions").all();
const norm = (s) =>
	String(s ?? "")
		.replaceAll("\\", "/")
		.toLowerCase();
const hits = rows
	.map((r) => {
		try {
			return JSON.parse(r.data);
		} catch {
			return null;
		}
	})
	.filter((r) => r && norm(r.cwd).includes(norm(needle)))
	.sort((a, b) => norm(a.cwd).localeCompare(norm(b.cwd)));

for (const r of hits) {
	const state = r.archived ? "ARCHIVED" : "ACTIVE  ";
	console.log(`${state} ${r.cwd}  room="${r.name}" branch=${r.branch ?? "-"}`);
}
if (hits.length === 0) console.log(`no rooms under a path containing "${needle}"`);
