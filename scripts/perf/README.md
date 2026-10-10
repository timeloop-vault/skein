# Live Context load script

`fake-busy-rooms.js` fakes several busy rooms by emitting `harness-action`
events into the webview (issue #593, epic #596). It writes nothing to disk or
sqlite; row ids start at 9e12 and a window reload discards everything.

## Recipe

1. Run the dev profile: `cd app && npm run tauri:dev`. Do this once on a main
   checkout and once on the branch under test, on the same machine with the
   same rooms.
2. Open N rooms (for example 6), each with at least one harness.
3. Devtools -> Performance. Paste the contents of `fake-busy-rooms.js` into the
   console, then run `await skeinLoad.rooms()` to list room and harness ids.
4. Start recording, then run
   `skeinLoad.start({ roomIds: [...], rowsPerSecPerRoom: 20, seed: 1 })`.
5. For about 30 s type in one room's terminal and select text in it.
6. Run `skeinLoad.stop()` and stop recording. `skeinLoad.stats()` shows what
   was emitted.
7. Compare main-thread scripting time and long tasks between the two builds.

Same `seed` gives the same row stream. Kinds emitted: `tool_call`
(Read/Grep/Bash), `patch` (Edit), `turn_cost`, `turn_duration`, `cost_state`,
`plan_change` (Claude TaskCreate/TaskUpdate deltas) and `away_summary`.
