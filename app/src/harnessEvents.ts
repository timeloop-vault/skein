// Harness-native event adapters — frontend translator. Epic #50 L2c.
//
// Rust-side adapters (today: `harness_events_claude.rs`) emit semantic
// events over a Tauri Channel. This module subscribes, marks the
// harness as having an authoritative source so the L2a idle tick
// stands down, and translates each event into a phase call on
// `harnessActivity`.
//
// Why keep the policy here and not in Rust: the state machine and
// every consumer of it (status bar, badges, OS notifications) lives
// in the frontend. Translating event → phase in Rust would split the
// policy across the boundary; keeping it here means the Rust adapter
// is a pure "what did Claude write to its log" producer, and the
// "what does it mean for the dot" logic stays next to everything else
// that reads from the store.

// This file is the public entry: the implementation lives in
// `harnessEventsClaude.ts`, `harnessEventsOpencode.ts` and the shared
// `harnessEventsShared.ts`, re-exported here so importers are unchanged.

export {
	attachClaudeEvents,
	hasClaudeTranscriptTail,
	reattachClaudeTelemetry,
} from "./harnessEventsClaude.ts";
export type {
	BackgroundEndStatus,
	BackgroundTaskKind,
	ClaudeEvent,
	ReattachOutcome,
} from "./harnessEventsClaude.ts";
export { attachOpencodeEvents } from "./harnessEventsOpencode.ts";
export type { OpencodeEvent } from "./harnessEventsOpencode.ts";
export { guardChannelHandler, liveAttach } from "./harnessEventsShared.ts";
export type { LiveAttach } from "./harnessEventsShared.ts";
