// harnessInput — the seam for putting text into a harness's terminal
// and submitting it, plus the safety gate over doing so (#238).
//
// #238's Nudge button is the first caller; #41 (drop a file onto a
// harness) reuses the same registry rather than inventing its own way
// to reach a live PTY, but pairs it with its own gate — a drop has no
// submit, so it is safe at points a nudge is not.
//
// Split (#458) into three modules; this file only re-exports them so
// every importer keeps using `./harnessInput.ts`:
//
//   - `harnessInputRegistry.ts` — the per-harness target registry, user
//     input counters and the #383/#413 composer-draft state;
//   - `harnessInputGate.ts` — the pure `canSendPrompt` / `canInsertText`
//     gates;
//   - `harnessInputSend.ts` — `sendPrompt` / `insertText`, the
//     paste/submit mechanics.

export type { CanInsertTextInput, CanSendPromptInput, GateResult } from "./harnessInputGate.ts";
export { canInsertText, canSendPrompt, formatDroppedPaths } from "./harnessInputGate.ts";
export type { HarnessInputTarget } from "./harnessInputRegistry.ts";
export { harnessInput, SCREEN_SETTLE_MS } from "./harnessInputRegistry.ts";
export { insertText, sendPrompt } from "./harnessInputSend.ts";
