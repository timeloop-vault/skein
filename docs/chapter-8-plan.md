# Chapter 8 — Distribution + auto-update

After chapter 7, Skein's runtime is solid on every OS we'd ship. But
"runs in `npm run tauri dev`" isn't an installable app. Chapter 8
turns it into one: matrix-built artifacts (`.dmg` / `.msi` /
`.AppImage`) on GitHub Releases, downloadable per platform, with
the Tauri updater plugin so future versions install in-place from
within the running app.

This is the "B" tier discussed at chapter 7's close: **shareable
unsigned builds.** No Apple Developer Program, no Authenticode
certificate. Users dismiss Gatekeeper / SmartScreen warnings on
first install (documented in the README) but the app runs fine.
Tauri's own update-signing key (independent of OS code-signing)
verifies that auto-updates haven't been tampered with — that gives
us trustable updates without the paid-cert chain. Real OS code
signing is a future chapter when there's an audience justifying
the certificate cost.

The release workflow is lifted nearly wholesale from
[`timeloop-vault/poe-inspect`](https://github.com/timeloop-vault/poe-inspect)'s
`release.yml`, which solved this exact problem (4-target matrix,
tag-derived versioning with pre-release suffix handling, Tauri
signing for the updater, dual-trigger via release event +
`workflow_dispatch`). Chapter 8 mostly adapts that to Skein's
paths and wires the updater UI into the existing SettingsModal.

Chapter 7 phase 5 deferred Windows / Linux runtime verification to
"when chapter 8 produces an artifact." Phase 6 below cashes that
in.

## Phase 1 — Branding, bundle config, version bump

**Goal:** Skein has its own identity in the bundle and the
artifacts we'll start publishing aren't labelled "tauri-app."

- **App icon.** Default Tauri icon → Skein icon. `tauri icon
  <source.png>` regenerates the full set
  (32×32 / 128×128 / 128×128@2x / .icns / .ico) from one source.
  Decide the source PNG before this phase.
- **`tauri.conf.json`:**
  - `productName: "Skein"` (already set).
  - `version: "0.1.0"` (was `0.0.0-dev` — it's been a real
    prototype for a while).
  - `bundle.createUpdaterArtifacts: true` (signed `.sig` files
    that the updater verifies).
  - `bundle.targets: "all"` is already implicit; keep.
  - `plugins.updater: { pubkey: "...", endpoints: [...] }` —
    pubkey from phase 2's keypair generation, endpoint pointing
    at `https://github.com/timeloop-vault/skein/releases/latest/download/latest.json`.
- **Cargo.toml + package.json**: version bumped to 0.1.0 in both
  to match.
- **Window title in About menu** (chapter 4 phase 3) — already
  reads from `env!("CARGO_PKG_VERSION")`, no change needed.

## Phase 2 — Tauri updater plugin

**Goal:** Skein can check for, download, and install updates from
within the running app, with cryptographic verification that the
update came from us.

- Add `tauri-plugin-updater = "2"` to `app/src-tauri/Cargo.toml`.
- Add `"@tauri-apps/plugin-updater": "^2"` to `app/package.json`.
- Register the plugin in `lib.rs`:
  `.plugin(tauri_plugin_updater::Builder::new().build())`.
- **Generate the Tauri signing keypair** (one-time, locally,
  never committed):
  ```
  npx tauri signer generate -w ~/.tauri/skein.key
  ```
  Stores the private key at `~/.tauri/skein.key` and prints the
  public key. Public key goes into `tauri.conf.json`'s
  `plugins.updater.pubkey`. Private key + its passphrase get
  added as repo secrets `TAURI_SIGNING_PRIVATE_KEY` and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (phase 4 reads these).
- **UI surface:** new "About" or "Updates" section in
  `SettingsModal` (chapter 4). Shows current version, "Check for
  updates" button, current state (idle / checking / available /
  downloading / installed / error). Wire via `check()` from
  `@tauri-apps/plugin-updater`, then `update.downloadAndInstall(progressCb)`.
  Pattern exists in poe-inspect's `AboutSettings.tsx`; lift it.
- **Capability grant** in `app/src-tauri/capabilities/default.json`:
  add `updater:default` so the frontend can call `check` and
  `download_and_install`.

## Phase 3 — CI workflow

**Goal:** every push and PR validates clean builds across all OSes.
Replaces the trust-the-pre-commit-hook model with platform-matrix
verification.

- New `.github/workflows/ci.yml`, lifted from poe-inspect's
  `ci.yml` and adapted:
  - `cargo test --workspace` (Linux runner — fast, covers
    skein-git's 17 tests).
  - `cargo clippy --all-targets -- -D warnings` (workspace +
    src-tauri).
  - `npx biome check .` + `npx tsc --noEmit` (frontend).
  - `cargo tauri build --debug --target <triple>` per platform —
    confirms the Tauri build doesn't break.
- Triggers: `pull_request` to `main`, `push` to `main`, manual
  `workflow_dispatch`.
- Caching: `Swatinem/rust-cache@v2` per matrix label (saves ~5
  minutes per Rust build).
- Linux deps: same apt list poe-inspect uses (libwebkit2gtk-4.1,
  gtk, x11/wayland, libxdo).

This isn't a release — no artifacts published. Just a green tick
per push.

## Phase 4 — Release workflow

**Goal:** publishing a GitHub Release attaches signed artifacts for
all four targets automatically.

- New `.github/workflows/release.yml`, lifted from poe-inspect's
  with path adjustments. Targets:
  - `ubuntu-22.04` → `x86_64-unknown-linux-gnu` → AppImage + .deb
  - `windows-latest` → `x86_64-pc-windows-msvc` → .msi
  - `macos-latest` → `aarch64-apple-darwin` → .dmg + .app
  - `macos-14` → `x86_64-apple-darwin` → .dmg + .app
- Triggers: `release` event (`types: [published]`) → publish
  artifacts to the release; `workflow_dispatch` → build only,
  upload as workflow artifacts (for testing without cutting a
  real release).
- Tag-derived version patching: poe-inspect's pattern strips
  pre-release suffixes (`-beta.1`) for MSI/Cargo (strict semver)
  but keeps the full version for `tauri.conf.json` and
  `package.json` (updater + About display).
- Repo secrets needed (added once via the GitHub UI):
  - `TAURI_SIGNING_PRIVATE_KEY` — contents of `~/.tauri/skein.key`.
  - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — its passphrase.
- `tauri-action` with `includeUpdaterJson: true` produces the
  `latest.json` manifest the updater plugin polls.
- **First test:** trigger via `workflow_dispatch` before any
  release tag exists. Confirms the matrix builds cleanly on all
  four targets and produces the expected artifacts. Iterate on
  failures here, not on a real release.

## Phase 5 — Install docs + first real release

**Goal:** v0.1.0 is published and a friend could plausibly
install it.

- README.md gets a real "Install" section per OS, replacing the
  current "run via cargo run" instructions. Per platform:
  - **macOS:** download `.dmg`, drag to Applications. First
    launch: right-click → Open → confirm (Gatekeeper warns
    "unidentified developer" — expected without code signing).
  - **Windows:** download `.msi`, run. SmartScreen: "More info
    → Run anyway" (expected without Authenticode).
  - **Linux:** download `.AppImage`, `chmod +x`, run.
- Tag `v0.1.0` on `main`, create a GitHub Release. Workflow
  fires, attaches artifacts, generates `latest.json`. Skein in
  the wild as v0.1.0.
- Install the macOS DMG locally — verify the bundled `.app`
  works the same as `npm run tauri dev` (chapter 7 test
  checklist against the bundled artifact).

## Phase 6 — Cross-platform runtime verification

**Status: deferred.** Needs a Windows machine and a Linux machine
neither of which I have access to. Will revisit when a tester
materialises or when bug reports start coming in from real users.
The MSI / AppImage artifacts *build* cleanly on the release
matrix (CI passes on all four targets), but they haven't been
*run*. Chapters 6/7 may have macOS-only assumptions that won't
surface until then.

**Goal:** chapter 7's phase 5 hand-off finally happens. Confirms
chapter 6 + 7 work on the OSes we don't dev on.

Test plan from `chapter-7-plan.md`'s phase 5 against each
platform's actual installed artifact:

1. Multi-harness in one room (no UUID collision regression).
2. Enter-for-shell after `/exit` (clean shell prompt, no blank
   viewport).
3. `claude --resume <uuid>` from the shell re-attaches.
4. Skein restart re-attaches all harnesses with no picker.

For each OS:
- **macOS** (already done in phase 5, just record results).
- **Windows** — install MSI on a Windows machine (borrow / VM /
  cloud). Walk the checklist. Document any failures. Particular
  watch-points from chapter 7 phase 5: Claude's goodbye line
  visible (data-flush timeout works on ConPTY), no hangs from
  rapid Enter-for-shell (chapter 7's refactor sidestepped the
  ConPTY kill/spawn race; restoring the throttle is a one-liner
  if a hang shows up).
- **Linux** — install AppImage on a Linux machine. Walk the
  checklist. Watch for GTK / Wayland WebView2-equivalent
  rendering quirks.

Update `docs/chapter-7-plan.md`'s phase 5 with results, or
spin off a `chapter-8-validation.md` notes file if there's
enough to say.

## Out of scope for chapter 8

See [`backlog.md`](./backlog.md). Notably:

- **macOS code signing + notarization.** Apple Developer Program
  ($99/year) + the actual signing/notarization workflow. Defer
  until there's a meaningful distribution audience that justifies
  the certificate cost — or until first-launch UX friction starts
  filtering testers in a way that matters.
- **Windows Authenticode signing.** Same — paid certificate
  ($200-500/year, EV preferred for SmartScreen reputation),
  defer until distribution scale justifies it.
- **Mac App Store / Microsoft Store / Flathub.** Each requires
  its own packaging + review process. Direct download via
  GitHub Releases is fine for v0.1.x.
- **Auto-update background polling.** Phase 2 ships manual
  "Check for updates" button only. Background polling on app
  launch is a polish item.
- **Delta updates.** Tauri's updater downloads the full new
  bundle. Delta patching exists as a separate plugin; defer.
- **Code-signed updater key rotation.** If the private key ever
  leaks, key rotation is a real procedure. Don't worry about it
  for v0; treat the key as a committed credential and rotate if
  it leaks.
