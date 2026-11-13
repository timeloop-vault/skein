// Tuning constants the webview still reads. The phase machine itself, and
// every other threshold it used, lives in the daemon (#732).

/// The #259 silent-adapter watchdog window in the daemon's phase machine
/// (`harness_phase`), mirrored here only so the submit-retry schedule can
/// be pinned under it (`submitRetry.test.ts`).
export const ADAPTER_SILENT_AFTER_MS = 10_000;
