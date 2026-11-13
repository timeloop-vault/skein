// Harness-native event adapters — frontend side. Epic #50 L2c.
//
// Rust-side adapters (`harness_events_claude.rs`, `harness_events_opencode.rs`)
// emit semantic events over a Tauri Channel. Since #732 the phase machine
// lives in the daemon, which hears those events itself; the webview keeps
// only their non-phase effects (the Claude CLI version, opencode's session
// capture and observed agent).

// This file is the public entry: the implementation lives in
// `harnessEventsClaude.ts`, `harnessEventsOpencode.ts` and the shared
// `harnessEventsShared.ts`, re-exported here so importers are unchanged.

export type {
	BackgroundEndStatus,
	BackgroundTaskKind,
	ClaudeEvent,
	ReattachOutcome,
} from "./harnessEventsClaude.ts";
export {
	attachClaudeEvents,
	hasClaudeTranscriptTail,
	reattachClaudeTelemetry,
} from "./harnessEventsClaude.ts";
export type { OpencodeEvent } from "./harnessEventsOpencode.ts";
export { attachOpencodeEvents } from "./harnessEventsOpencode.ts";
export { guardChannelHandler } from "./harnessEventsShared.ts";
