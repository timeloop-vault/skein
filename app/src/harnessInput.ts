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
//   - `harnessInputGate.ts` — the `GateResult` type and the local
//     registration check (the daemon owns the rest of the gate, #733);
//   - `harnessInputSend.ts` — `sendPrompt` / `deliverMailNow` (ask the daemon)
//     and `insertText` (a paste for a file drop).

export type { GateResult } from "./harnessInputGate.ts";
export { formatDroppedPaths } from "./harnessInputGate.ts";
export { harnessInput } from "./harnessInputRegistry.ts";
export { deliverMailNow, insertText, sendPrompt } from "./harnessInputSend.ts";
