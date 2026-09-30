# Design fixtures

Static pages for exercising the `design` harness (#433) by hand.

## invoke-probe

Verifies that a Tauri `invoke` from the design preview iframe is refused.

1. Run a dev build (`npm run tauri:dev`) and open a room on this repo.
2. Add a `design` harness and pick `docs/design-fixtures/invoke-probe/index.html`.
3. Expect the page to show PASS.

Background: iframes lost IPC with the CVE-2024-35222 fix. On Windows the IPC
object is present but every invoke is refused ("Origin header is not a valid
URL"); on macOS the object is absent (`docs/design-surface-recon.md` §2.2).
