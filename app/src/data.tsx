import type { HarnessKind, Session, SessionData } from "./types.ts";

export interface HarnessKindMeta {
	id: HarnessKind;
	label: string;
	name: string;
	chip: string;
	desc: string;
}

export const HARNESS_KINDS: Record<HarnessKind, HarnessKindMeta> = {
	claude: {
		id: "claude",
		label: "CC",
		name: "Claude Code",
		chip: "h-claude",
		desc: "Anthropic. Direct API.",
	},
	opencode: {
		id: "opencode",
		label: "oc",
		name: "opencode",
		chip: "h-opencode",
		desc: "Local server, OSS.",
	},
	copilot: {
		id: "copilot",
		label: "gh",
		name: "Copilot CLI",
		chip: "h-copilot",
		desc: "GitHub entitlement.",
	},
	byoh: {
		id: "byoh",
		label: "sk",
		name: "Skein BYOH",
		chip: "h-byoh",
		desc: "Built-in agent loop.",
	},
};

export const HARNESS_ORDER: HarnessKind[] = ["claude", "opencode", "copilot", "byoh"];

export const INITIAL_SESSIONS: Session[] = [
	{
		id: "s1",
		name: "kit · skein-tauri-shell",
		branch: "feat/window-chrome",
		repo: "skein",
		task: "Build the Tauri window chrome",
		status: "running",
		badge: 0,
		harnesses: [
			{
				id: "h1a",
				kind: "claude",
				name: "shell + tabs",
				status: "running",
				model: "sonnet-4.5",
				tokens: "14.2k",
			},
			{
				id: "h1b",
				kind: "opencode",
				name: "design check",
				status: "idle",
				model: "sonnet-4.5",
				tokens: "2.1k",
			},
		],
		activeHarnessId: "h1a",
	},
	{
		id: "s2",
		name: "kit · agent-loop-v0",
		branch: "main",
		repo: "skein",
		task: "Stand up the BYOH loop",
		status: "waiting",
		badge: 1,
		harnesses: [
			{
				id: "h2a",
				kind: "byoh",
				name: "main",
				status: "waiting",
				model: "sonnet-4.5",
				tokens: "8.1k",
			},
		],
		activeHarnessId: "h2a",
	},
	{
		id: "s3",
		name: "work · example-pim-search",
		branch: "fix/index-rebuild",
		repo: "pim",
		task: "Rebuild ES index without downtime",
		status: "running",
		badge: 0,
		harnesses: [
			{
				id: "h3a",
				kind: "copilot",
				name: "main",
				status: "running",
				model: "gpt-5",
				tokens: "22.7k",
			},
			{
				id: "h3b",
				kind: "copilot",
				name: "review pass",
				status: "idle",
				model: "gpt-5",
				tokens: "4.4k",
			},
		],
		activeHarnessId: "h3a",
	},
	{
		id: "s4",
		name: "kit · sqlite-migrations",
		branch: "feat/sessions-table",
		repo: "skein",
		task: "Sessions + messages tables",
		status: "idle",
		badge: 0,
		harnesses: [
			{
				id: "h4a",
				kind: "claude",
				name: "main",
				status: "idle",
				model: "sonnet-4.5",
				tokens: "3.4k",
			},
		],
		activeHarnessId: "h4a",
	},
	{
		id: "s5",
		name: "work · example-pim-search",
		branch: "spike/embeddings",
		repo: "pim",
		task: "Try pgvector for product search",
		status: "error",
		badge: 1,
		harnesses: [
			{
				id: "h5a",
				kind: "copilot",
				name: "main",
				status: "error",
				model: "gpt-5",
				tokens: "17.9k",
			},
		],
		activeHarnessId: "h5a",
	},
];

export const SESSION_DATA: Record<string, SessionData> = {
	s1: {
		tree: [
			{ name: "src", kind: "dir", depth: 0, open: true },
			{ name: "components", kind: "dir", depth: 1, open: true },
			{ name: "Titlebar.tsx", kind: "file", depth: 2, touched: "+38", active: true },
			{ name: "TabStrip.tsx", kind: "file", depth: 2 },
			{ name: "src-tauri", kind: "dir", depth: 0, open: true },
			{ name: "tauri.conf.json", kind: "file", depth: 1, touched: "+3 −1" },
		],
		activeFile: { path: "src/components/Titlebar.tsx", adds: 38, dels: 0 },
		diff: [
			{
				kind: "add",
				n1: "",
				n2: 1,
				src: (
					<span>
						<span className="tk-key">import</span> {"{"} useEffect {"}"}{" "}
						<span className="tk-key">from</span> <span className="tk-str">"react"</span>
						{";"}
					</span>
				),
			},
			{
				kind: "add",
				n1: "",
				n2: 2,
				src: (
					<span>
						<span className="tk-key">import</span> {"{"} getCurrentWindow {"}"}{" "}
						<span className="tk-key">from</span>{" "}
						<span className="tk-str">"@tauri-apps/api/window"</span>
						{";"}
					</span>
				),
			},
			{ kind: "add", n1: "", n2: 3, src: <span /> },
			{
				kind: "add",
				n1: "",
				n2: 4,
				src: (
					<span>
						<span className="tk-key">export function</span> <span className="tk-fn">Titlebar</span>
						() {"{"}
					</span>
				),
			},
			{
				kind: "add",
				n1: "",
				n2: 5,
				src: (
					<span>
						{"  "}
						<span className="tk-key">return</span> (
					</span>
				),
			},
			{
				kind: "add",
				n1: "",
				n2: 6,
				src: (
					<span>
						{"    <"}
						<span className="tk-fn">div</span> className=
						<span className="tk-str">"sk-titlebar"</span> data-tauri-drag-region{">"}
					</span>
				),
			},
			{
				kind: "add",
				n1: "",
				n2: 7,
				src: (
					<span>
						{"      <"}
						<span className="tk-fn">TrafficLights</span> /{">"}
					</span>
				),
			},
			{
				kind: "add",
				n1: "",
				n2: 8,
				src: (
					<span>
						{"      <"}
						<span className="tk-fn">span</span> className=
						<span className="tk-str">"sk-app-name"</span>
						{">"}skein{"</"}
						<span className="tk-fn">span</span>
						{">"}
					</span>
				),
			},
			{
				kind: "add",
				n1: "",
				n2: 9,
				src: (
					<span>
						{"    </"}
						<span className="tk-fn">div</span>
						{">"}
					</span>
				),
			},
			{ kind: "add", n1: "", n2: 10, src: <span>{"  );"}</span> },
			{ kind: "add", n1: "", n2: 11, src: <span>{"}"}</span> },
		],
		plan: [
			{ state: "done", text: "wire titleBarStyle Overlay on macOS", by: "h1a" },
			{ state: "done", text: "add drag region to titlebar", by: "h1a" },
			{ state: "now", text: "fix space-3 token usage (h1b flagged)", by: "h1a" },
			{ state: "next", text: "tabstrip overflow scroll", by: "h1a" },
		],
		activity: [
			{
				time: "14:02:18",
				by: "h1a",
				kind: "claude",
				msg: (
					<span>
						str_replace <span className="arg">tauri.conf.json</span> <span className="ok">✓</span>
					</span>
				),
			},
			{
				time: "14:02:31",
				by: "h1a",
				kind: "claude",
				msg: (
					<span>
						write_file <span className="arg">Titlebar.tsx</span> <span className="ok">✓</span>
					</span>
				),
			},
			{
				time: "14:03:04",
				by: "h1b",
				kind: "opencode",
				msg: (
					<span>
						read_file <span className="arg">Titlebar.tsx</span> <span className="ok">✓</span>
					</span>
				),
			},
			{
				time: "14:03:11",
				by: "h1b",
				kind: "opencode",
				msg: (
					<span>
						flagged: <span className="arg">space-3 token mismatch</span>
					</span>
				),
			},
		],
	},
	s2: {
		tree: [
			{ name: "src", kind: "dir", depth: 0, open: true },
			{ name: "agent", kind: "dir", depth: 1, open: true },
			{ name: "loop.rs", kind: "file", depth: 2 },
			{ name: "tools.rs", kind: "file", depth: 2, touched: "+12" },
			{ name: "fs", kind: "dir", depth: 1, open: true },
			{ name: "watcher.rs", kind: "file", depth: 2, touched: "+5 −1", active: true },
			{ name: "worktree.rs", kind: "file", depth: 2 },
			{ name: "Cargo.toml", kind: "file", depth: 0, touched: "+2" },
		],
		activeFile: { path: "src/fs/watcher.rs", adds: 5, dels: 1 },
		diff: [
			{
				kind: "ctx",
				n1: 24,
				n2: 24,
				src: (
					<span>
						<span className="tk-com">{"// poll the worktree for fs changes; debounce 80ms"}</span>
					</span>
				),
			},
			{
				kind: "ctx",
				n1: 25,
				n2: 25,
				src: (
					<span>
						<span className="tk-key">pub async fn</span>{" "}
						<span className="tk-fn">watch_worktree</span>(path:{" "}
						<span className="tk-key">&Path</span>) {"-> "}
						<span className="tk-key">Result</span>
						{"<"}
						<span className="tk-key">Watcher</span>
						{">"} {"{"}
					</span>
				),
			},
			{
				kind: "ctx",
				n1: 26,
				n2: 26,
				src: (
					<span>
						{"    "}
						<span className="tk-key">let</span> (tx, rx) = mpsc::
						<span className="tk-fn">channel</span>(<span className="tk-num">64</span>);
					</span>
				),
			},
			{
				kind: "del",
				n1: 27,
				n2: "",
				src: (
					<span>
						{"    "}
						<span className="tk-key">let mut</span> watcher = notify::
						<span className="tk-fn">recommended_watcher</span>(tx)?;
					</span>
				),
			},
			{
				kind: "add",
				n1: "",
				n2: 27,
				src: (
					<span>
						{"    "}
						<span className="tk-key">let mut</span> watcher = notify::
						<span className="tk-fn">recommended_watcher</span>(
						<span className="tk-fn">debounce</span>(tx, <span className="tk-num">80</span>))?;
					</span>
				),
			},
			{
				kind: "ctx",
				n1: 28,
				n2: 28,
				src: (
					<span>
						{"    "}watcher.<span className="tk-fn">watch</span>(path, RecursiveMode::
						<span className="tk-fn">Recursive</span>)?;
					</span>
				),
			},
			{
				kind: "add",
				n1: "",
				n2: 30,
				src: (
					<span>
						{"    "}
						<span className="tk-com">
							{"// emit only on .rs / .toml / .md to keep diff pane quiet"}
						</span>
					</span>
				),
			},
			{
				kind: "add",
				n1: "",
				n2: 31,
				src: (
					<span>
						{"    "}
						<span className="tk-key">let</span> filtered = rx.
						<span className="tk-fn">filter_map</span>(
						<span className="tk-fn">interesting_path</span>);
					</span>
				),
			},
			{
				kind: "ctx",
				n1: 30,
				n2: 32,
				src: (
					<span>
						{"    "}
						<span className="tk-key">Ok</span>(<span className="tk-key">Watcher</span> {"{"}{" "}
						watcher, rx: filtered {"}"})
					</span>
				),
			},
			{ kind: "ctx", n1: 31, n2: 33, src: <span>{"}"}</span> },
		],
		plan: [
			{ state: "done", text: "read src/fs/watcher.rs", by: "h2a" },
			{ state: "done", text: "add debounce wrapper around notify channel", by: "h2a" },
			{ state: "done", text: "filter to .rs / .toml / .md", by: "h2a" },
			{ state: "now", text: "run cargo test for fs::watcher", by: "h2a" },
			{ state: "next", text: "update CHANGELOG with debounce default", by: "h2a" },
		],
		activity: [
			{
				time: "13:51:02",
				by: "h2a",
				kind: "byoh",
				msg: (
					<span>
						read_file <span className="arg">watcher.rs</span> <span className="ok">✓</span>
					</span>
				),
			},
			{
				time: "13:51:18",
				by: "h2a",
				kind: "byoh",
				msg: (
					<span>
						str_replace <span className="arg">watcher.rs</span> <span className="ok">+5 −1</span>
					</span>
				),
			},
			{
				time: "13:52:01",
				by: "h2a",
				kind: "byoh",
				msg: (
					<span>
						requested permission for <span className="arg">cargo test</span>
					</span>
				),
			},
		],
	},
	s3: {
		tree: [
			{ name: "src", kind: "dir", depth: 0, open: true },
			{ name: "indexer", kind: "dir", depth: 1, open: true },
			{ name: "main.rs", kind: "file", depth: 2, active: true },
			{ name: "batch.rs", kind: "file", depth: 2 },
			{ name: "api", kind: "dir", depth: 1 },
		],
		activeFile: { path: "src/indexer/main.rs", adds: 0, dels: 0 },
		diff: [
			{
				kind: "ctx",
				n1: 102,
				n2: 102,
				src: (
					<span>
						<span className="tk-key">const</span> <span className="tk-fn">BATCH_SIZE</span>:{" "}
						<span className="tk-key">usize</span> = <span className="tk-num">50_000</span>
						{";"}
					</span>
				),
			},
			{ kind: "ctx", n1: 103, n2: 103, src: <span /> },
			{
				kind: "ctx",
				n1: 104,
				n2: 104,
				src: (
					<span>
						<span className="tk-key">async fn</span> <span className="tk-fn">reindex</span>() {"{"}
					</span>
				),
			},
			{
				kind: "ctx",
				n1: 105,
				n2: 105,
				src: (
					<span>
						{"    "}
						<span className="tk-key">while let Some</span>(batch) ={" "}
						<span className="tk-fn">next_batch</span>().<span className="tk-fn">await</span> {"{"}
					</span>
				),
			},
			{
				kind: "ctx",
				n1: 106,
				n2: 106,
				src: (
					<span>
						{"        "}es.<span className="tk-fn">bulk_index</span>(batch).
						<span className="tk-fn">await</span>?;
					</span>
				),
			},
			{ kind: "ctx", n1: 107, n2: 107, src: <span>{"    }"}</span> },
			{ kind: "ctx", n1: 108, n2: 108, src: <span>{"}"}</span> },
		],
		plan: [
			{ state: "now", text: "measure memory under current batch size", by: "h3a" },
			{ state: "next", text: "reduce BATCH_SIZE to 5_000", by: "h3a" },
			{ state: "next", text: "add tokio::time::yield between batches", by: "h3a" },
		],
		activity: [
			{
				time: "13:48:11",
				by: "h3a",
				kind: "copilot",
				msg: (
					<span>
						read_file <span className="arg">main.rs</span> <span className="ok">✓</span>
					</span>
				),
			},
			{
				time: "13:48:42",
				by: "h3a",
				kind: "copilot",
				msg: (
					<span>
						grep <span className="arg">"BATCH_SIZE"</span> <span className="ok">4 matches</span>
					</span>
				),
			},
		],
	},
	s4: {
		tree: [
			{ name: "src", kind: "dir", depth: 0, open: true },
			{ name: "db", kind: "dir", depth: 1, open: true },
			{ name: "schema.sql", kind: "file", depth: 2, active: true },
			{ name: "migrations", kind: "dir", depth: 2 },
		],
		activeFile: { path: "src/db/schema.sql", adds: 0, dels: 0 },
		diff: [
			{
				kind: "ctx",
				n1: 1,
				n2: 1,
				src: (
					<span>
						<span className="tk-com">-- sessions, messages, tool_calls</span>
					</span>
				),
			},
			{
				kind: "ctx",
				n1: 2,
				n2: 2,
				src: (
					<span>
						<span className="tk-key">CREATE TABLE</span> sessions (
					</span>
				),
			},
			{
				kind: "ctx",
				n1: 3,
				n2: 3,
				src: (
					<span>
						{"    "}id <span className="tk-key">TEXT PRIMARY KEY</span>,
					</span>
				),
			},
			{
				kind: "ctx",
				n1: 4,
				n2: 4,
				src: (
					<span>
						{"    "}name <span className="tk-key">TEXT NOT NULL</span>
					</span>
				),
			},
			{ kind: "ctx", n1: 5, n2: 5, src: <span>);</span> },
		],
		plan: [{ state: "next", text: "finalize sessions schema", by: "h4a" }],
		activity: [],
	},
	s5: {
		tree: [
			{ name: "src", kind: "dir", depth: 0, open: true },
			{ name: "embeddings", kind: "dir", depth: 1, open: true },
			{ name: "pgvector.rs", kind: "file", depth: 2, touched: "+24 −2", active: true },
			{ name: "migrate.sql", kind: "file", depth: 2, touched: "+8" },
			{ name: "api", kind: "dir", depth: 1 },
		],
		activeFile: { path: "src/embeddings/pgvector.rs", adds: 24, dels: 2 },
		diff: [
			{
				kind: "ctx",
				n1: 1,
				n2: 1,
				src: (
					<span>
						<span className="tk-key">use</span> sqlx::PgPool;
					</span>
				),
			},
			{
				kind: "add",
				n1: "",
				n2: 2,
				src: (
					<span>
						<span className="tk-key">use</span> pgvector::Vector;
					</span>
				),
			},
			{ kind: "ctx", n1: 2, n2: 3, src: <span /> },
			{
				kind: "ctx",
				n1: 3,
				n2: 4,
				src: (
					<span>
						<span className="tk-key">pub async fn</span>{" "}
						<span className="tk-fn">upsert_embedding</span>(pool:{" "}
						<span className="tk-key">&PgPool</span>, id: <span className="tk-key">&str</span>, v:{" "}
						<span className="tk-key">&[f32]</span>) {"-> "}
						<span className="tk-key">Result</span>
						{"<"}
						<span className="tk-key">()</span>
						{">"} {"{"}
					</span>
				),
			},
			{
				kind: "add",
				n1: "",
				n2: 5,
				src: (
					<span>
						{"    "}
						<span className="tk-key">let</span> v = <span className="tk-key">Vector</span>::
						<span className="tk-fn">from</span>(v.<span className="tk-fn">to_vec</span>());
					</span>
				),
			},
			{
				kind: "ctx",
				n1: 4,
				n2: 6,
				src: (
					<span>
						{"    "}sqlx::<span className="tk-fn">query</span>(
						<span className="tk-str">"INSERT INTO embeddings (id, v) VALUES ($1, $2)"</span>)
					</span>
				),
			},
			{ kind: "ctx", n1: 5, n2: 7, src: <span>{"}"}</span> },
		],
		plan: [
			{ state: "done", text: "add pgvector dep + migrations", by: "h5a" },
			{ state: "done", text: "upsert_embedding helper", by: "h5a" },
			{ state: "now", text: "wire similarity_search endpoint", by: "h5a" },
			{ state: "next", text: "benchmark vs sqlite-vss", by: "h5a" },
		],
		activity: [
			{
				time: "12:14:01",
				by: "h5a",
				kind: "copilot",
				msg: (
					<span>
						read_file <span className="arg">pgvector.rs</span> <span className="ok">✓</span>
					</span>
				),
			},
			{
				time: "12:14:33",
				by: "h5a",
				kind: "copilot",
				msg: (
					<span>
						str_replace <span className="arg">pgvector.rs</span> <span className="ok">+24 −2</span>
					</span>
				),
			},
			{
				time: "12:15:08",
				by: "h5a",
				kind: "copilot",
				msg: (
					<span>
						stream <span className="err">interrupted · 401 unauthorized</span>
					</span>
				),
			},
		],
	},
};

export interface RepoPreset {
	id: string;
	label: string;
	path: string;
	lastBranch: string;
}

export const REPO_PRESETS: RepoPreset[] = [
	{ id: "skein", label: "skein", path: "~/code/skein", lastBranch: "main" },
	{ id: "pim", label: "example-pim-search", path: "~/work/pim", lastBranch: "main" },
	{ id: "design", label: "skein-design", path: "~/code/skein-design", lastBranch: "main" },
];
