/*
 * fake-busy-rooms.js - synthetic Live Context load for before/after traces
 * (Skein issue #593, epic #596).
 *
 * WHAT IT DOES
 *   Emits realistic `harness-action` events (the Tauri event the backend
 *   broadcasts for live harness_actions rows) for N rooms at a steady rate,
 *   so a devtools Performance trace sees identical load on a main build and
 *   on the branch under test. Content is deterministic for a given seed.
 *
 * WRITES NOTHING TO DISK
 *   It only emits events to the webview's own listeners. No sqlite row, no
 *   file, no network. Row ids start at 9e12 so they can never collide with a
 *   real rowid. Reloading the window discards everything.
 *
 * USAGE (paste the whole file into the devtools console of a dev build)
 *   await skeinLoad.rooms()            // lists open rooms + harness ids
 *   skeinLoad.start({ roomIds: ["<id>", "<id>"], rowsPerSecPerRoom: 20, seed: 1 })
 *   skeinLoad.stats()                  // emitted counts so far
 *   skeinLoad.stop()
 *   Call skeinLoad.rooms() with no start args to pick ids; start() also
 *   accepts no roomIds, in which case every room from the last rooms() call
 *   is used.
 *
 * MEASURING RECIPE
 *   1. Dev profile: `cd app && npm run tauri:dev` (main checkout, then this
 *      branch; same machine, same rooms).
 *   2. Open N rooms (e.g. 6) with at least one harness each.
 *   3. Devtools -> Performance. Paste this script, run skeinLoad.rooms().
 *   4. Start recording, run skeinLoad.start({ roomIds, rowsPerSecPerRoom: 20, seed: 1 }).
 *   5. For ~30 s type in one room's terminal and select text in it.
 *   6. skeinLoad.stop(), stop recording.
 *   7. Compare main-thread scripting time and long tasks between builds.
 *
 * PAYLOAD SHAPES (each taken from the parser that reads it)
 *   HarnessAction envelope  app/src/liveContext/store.ts (HarnessAction)
 *   tool_call Read          app/src/liveContext/fileRows.tsx (input.file_path,
 *                           result string / result.file.numLines)
 *   tool_call Grep          fileRows.tsx (input.pattern, result)
 *   tool_call Bash          app/src/liveContext/workRows.tsx (input.command,
 *                           duration_ms, result.stdout)
 *   patch Edit              app/src/liveContext/feedItemsHelpers.ts
 *                           burstCandidate (tool, files[], patch_info.additions/deletions)
 *   turn_cost (Claude)      feedItemsHelpers.ts costFromClaude / roomFold.ts
 *                           foldCost (model, request_id, stop_reason, usage)
 *   turn_duration           app/src/liveContext/feedFlatten.ts (duration_ms,
 *                           message_count per docs/live-context-d2-buildmap.md)
 *   cost_state              roomFold.ts foldRows (total_cost_usd)
 *   plan_change (deltas)    app/src/liveContext/planItems.ts applyDelta
 *                           (plan_item.op create|update, id, subject, status,
 *                           status_change.to)
 *   away_summary            roomFold.ts foldAway (content)
 *   Emit mechanism          app/node_modules/@tauri-apps/api/event.js emit():
 *                           invoke('plugin:event|emit', { event, payload })
 *   Room listing            command db_load_rooms (app/src-tauri/src/commands/rooms.rs),
 *                           returns { rooms: Room[], skipped, backupRooms? }
 *                           (app/src/roomsStoreProbe.ts DbLoadOutcome)
 */
(() => {
	const EVENT = "harness-action";
	const ID_BASE = 9e12;

	const internals = () => {
		const t = window.__TAURI_INTERNALS__;
		if (!t || typeof t.invoke !== "function") {
			throw new Error("window.__TAURI_INTERNALS__.invoke missing - run inside the Skein webview");
		}
		return t;
	};
	const emit = (payload) =>
		internals().invoke("plugin:event|emit", { event: EVENT, payload });

	// mulberry32: small seeded PRNG so a given seed reproduces the same stream.
	const prng = (seed) => {
		let a = seed >>> 0;
		return () => {
			a = (a + 0x6d2b79f5) >>> 0;
			let t = a;
			t = Math.imul(t ^ (t >>> 15), t | 1);
			t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
			return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
		};
	};

	const DIRS = ["src/components", "src/lib", "src/hooks", "tests", "docs"];
	const NAMES = ["Panel", "Store", "Router", "Parser", "Cache", "Layout", "Client", "Queue"];
	const EXTS = ["ts", "tsx", "rs", "md"];
	const COMMANDS = ["npm test", "cargo check", "git status", "ls -la", "npx biome check ."];
	const PATTERNS = ["useEffect", "TODO", "fn main", "export default", "interface "];
	const TASKS = [
		"Add retry to the fetch layer",
		"Cover the parser with tests",
		"Split the large module",
		"Update the docs",
		"Fix flaky snapshot",
	];

	const state = {
		timer: null,
		counter: 0,
		emitted: 0,
		failed: 0,
		byKind: {},
		startedAt: 0,
		rooms: [],
	};

	const pick = (rnd, xs) => xs[Math.floor(rnd() * xs.length)];
	const filePath = (rnd) =>
		`/work/project/${pick(rnd, DIRS)}/${pick(rnd, NAMES)}${Math.floor(rnd() * 20)}.${pick(rnd, EXTS)}`;
	const lines = (rnd, n) =>
		Array.from({ length: n }, (_, i) => `line ${i} ${Math.floor(rnd() * 1e6)}`).join("\n");

	// Per-room generator state (task ids, turn counters) so deltas stay coherent.
	const makeRoomGen = (room, rnd) => {
		let tasks = 0;
		const open = [];
		let turn = 0;
		let cost = 0;
		const harnessId = () => pick(rnd, room.harnessIds);
		const row = (kind, payload, hid) => ({
			id: ID_BASE + ++state.counter,
			harnessId: hid ?? harnessId(),
			roomId: room.id,
			timestampMs: Date.now(),
			kind,
			payload: JSON.stringify(payload),
			source: null,
			harnessKind: "claude",
		});
		const read = () =>
			row("tool_call", {
				tool: "Read",
				input: { file_path: filePath(rnd) },
				result: lines(rnd, 5 + Math.floor(rnd() * 120)),
			});
		const grep = () =>
			row("tool_call", {
				tool: "Grep",
				input: { pattern: pick(rnd, PATTERNS) },
				result: lines(rnd, 3 + Math.floor(rnd() * 20)),
			});
		const bash = () =>
			row("tool_call", {
				tool: "Bash",
				input: { command: pick(rnd, COMMANDS) },
				duration_ms: 50 + Math.floor(rnd() * 4000),
				result: { stdout: lines(rnd, 2 + Math.floor(rnd() * 30)), stderr: "" },
			});
		const edit = () =>
			row("patch", {
				tool: "Edit",
				files: [filePath(rnd)],
				patch_info: {
					additions: 1 + Math.floor(rnd() * 30),
					deletions: Math.floor(rnd() * 15),
				},
			});
		const turnCost = () => {
			const hid = harnessId();
			return row(
				"turn_cost",
				{
					model: "claude-sonnet-4-5",
					request_id: `req_fake_${room.id}_${++turn}`,
					stop_reason: "end_turn",
					usage: {
						input_tokens: 200 + Math.floor(rnd() * 3000),
						output_tokens: 100 + Math.floor(rnd() * 2000),
						cache_creation_input_tokens: Math.floor(rnd() * 5000),
						cache_read_input_tokens: Math.floor(rnd() * 40000),
					},
				},
				hid,
			);
		};
		const turnDuration = () =>
			row("turn_duration", {
				duration_ms: 2000 + Math.floor(rnd() * 90000),
				message_count: 2 + Math.floor(rnd() * 30),
			});
		const costState = () => {
			cost += rnd() * 0.4;
			return row("cost_state", { total_cost_usd: Number(cost.toFixed(4)) });
		};
		const plan = () => {
			if (open.length === 0 || (tasks < 12 && rnd() < 0.4)) {
				const id = String(++tasks);
				open.push(id);
				return row("plan_change", {
					plan_item: { op: "create", id, subject: pick(rnd, TASKS), status: "pending" },
				});
			}
			const id = open[Math.floor(rnd() * open.length)];
			const to = pick(rnd, ["in_progress", "completed"]);
			if (to === "completed") open.splice(open.indexOf(id), 1);
			return row("plan_change", {
				plan_item: { op: "update", id, status_change: { from: "pending", to } },
			});
		};
		const away = () =>
			row("away_summary", {
				content: `Recap ${turn}: worked through ${Math.floor(rnd() * 9) + 1} files.`,
			});

		// Weighted mix: mostly tool traffic, occasional turn end / plan / recap.
		const table = [
			[read, 30],
			[grep, 12],
			[bash, 18],
			[edit, 20],
			[plan, 6],
			[turnCost, 5],
			[turnDuration, 5],
			[costState, 3],
			[away, 1],
		];
		const total = table.reduce((s, [, w]) => s + w, 0);
		return () => {
			let r = rnd() * total;
			for (const [fn, w] of table) {
				r -= w;
				if (r < 0) return fn();
			}
			return read();
		};
	};

	const skeinLoad = {
		/** List open rooms with their real harness ids; remembers them for start(). */
		async rooms() {
			const out = await internals().invoke("db_load_rooms");
			const list = (out && out.rooms) || [];
			state.rooms = list
				.filter((r) => !r.archived)
				.map((r) => ({
					id: r.id,
					name: r.name,
					harnessIds: (r.harnesses || []).map((h) => h.id),
				}))
				.filter((r) => r.harnessIds.length > 0);
			console.table(
				state.rooms.map((r) => ({ id: r.id, name: r.name, harnesses: r.harnessIds.length })),
			);
			return state.rooms;
		},

		start({ roomIds, rowsPerSecPerRoom = 20, seed = 1 } = {}) {
			skeinLoad.stop();
			const wanted = roomIds && roomIds.length ? roomIds : state.rooms.map((r) => r.id);
			const rooms = wanted.map((id) => {
				const known = state.rooms.find((r) => r.id === id);
				// Unknown ids still work; use the id as a stand-in harness id.
				return known ?? { id, name: id, harnessIds: [`${id}-h1`] };
			});
			if (rooms.length === 0) throw new Error("no rooms: pass roomIds or call rooms() first");
			const rnd = prng(seed);
			const gens = rooms.map((r) => makeRoomGen(r, rnd));
			state.emitted = 0;
			state.failed = 0;
			state.byKind = {};
			state.startedAt = performance.now();
			// 50 ms ticks; each tick emits the rows owed since the last one.
			const tickMs = 50;
			const owedPerTick = (rowsPerSecPerRoom * tickMs) / 1000;
			const owed = rooms.map(() => 0);
			state.timer = setInterval(() => {
				gens.forEach((gen, i) => {
					owed[i] += owedPerTick;
					while (owed[i] >= 1) {
						owed[i] -= 1;
						const r = gen();
						state.byKind[r.kind] = (state.byKind[r.kind] || 0) + 1;
						emit(r).then(
							() => {
								state.emitted++;
							},
							() => {
								state.failed++;
							},
						);
					}
				});
			}, tickMs);
			return { rooms: rooms.length, rowsPerSecPerRoom, seed };
		},

		stop() {
			if (state.timer !== null) clearInterval(state.timer);
			state.timer = null;
		},

		stats() {
			const secs = state.startedAt ? (performance.now() - state.startedAt) / 1000 : 0;
			return {
				running: state.timer !== null,
				emitted: state.emitted,
				failed: state.failed,
				seconds: Number(secs.toFixed(1)),
				byKind: { ...state.byKind },
			};
		},
	};

	window.skeinLoad = skeinLoad;
	console.log("skeinLoad ready: rooms(), start({ roomIds, rowsPerSecPerRoom, seed }), stop(), stats()");
})();
