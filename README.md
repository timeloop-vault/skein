# skein

An IDE where the agent harness and the code live side by side. Each Room
pins a folder + task; inside a Room you can run multiple Harnesses (Claude
Code, opencode, GitHub Copilot CLI, or a built-in shell) sharing the same
worktree.

This repository is a **prototype** — the goal is to see whether the design
ideas hold up when wired into something runnable. It is not a production
app, and builds are unsigned across all OSes.

## Install

Pre-built artifacts are attached to each release on
[GitHub Releases](https://github.com/timeloop-vault/skein/releases/latest).
First-launch warnings are expected on every OS — Skein doesn't have paid
code-signing certificates yet (defer to a future release).

### macOS

1. Download `Skein_*_aarch64.dmg` (Apple Silicon) or `Skein_*_x64.dmg` (Intel).
2. Open the DMG, drag Skein into Applications.
3. **First launch only:** right-click Skein → **Open** → **Open**. macOS will
   say "unidentified developer"; right-click-Open is Apple's standard bypass.
   Subsequent launches don't ask.

### Windows

1. Download `Skein_*_x64-setup.msi`.
2. Double-click. SmartScreen will say "Windows protected your PC"; click
   **More info → Run anyway**.

### Linux

1. Download `Skein_*_amd64.AppImage` (or `.deb` if you prefer Debian/Ubuntu).
2. `chmod +x Skein_*.AppImage`
3. Run it. (`.deb` installs via `sudo apt install ./Skein_*.deb`.)

Skein checks for updates from inside the app — Settings (cog icon or
⌘,) → About → **Check for updates**. Updates verify against an
embedded public key, independent of the OS code-signing chain.

## Stack

- **Tauri v2** desktop shell (Rust)
- **React 18 + TypeScript (strict)** for the UI
- **Vite** for the dev server / bundle
- **Biome** for lint + format on the frontend
- **clippy pedantic** on the Rust side

## Getting started (development)

Prerequisites: Rust (stable, edition 2024), Node.js 20+, and the platform
build deps for Tauri (see <https://tauri.app/start/prerequisites/>).

```sh
git clone https://github.com/timeloop-vault/skein.git
cd skein

# Activate the pre-commit hook
git config core.hooksPath .githooks

# Install frontend deps
cd app && npm install && cd ..

# Run the app in dev
cd app && npm run tauri dev
```

To verify bundled-.app behavior (stripped PATH, OS-conventional log
dir, signed-asset codepaths) without going through the CI release
cycle, build + Finder-launch a real bundle locally:

```sh
scripts/local-bundle.sh
# Or for a faster, debug-symbols build:
scripts/local-bundle.sh --debug
```

`npm run tauri dev` inherits your shell environment, so launch-time
bugs that only show up in installed apps don't reproduce there. Use
the bundle script when you're chasing one.

## Pre-commit

A pre-commit hook runs `cargo fmt`, `cargo clippy -- -D warnings`,
`tsc --noEmit`, and `biome check`. To run them manually:

```sh
cargo fmt --manifest-path app/src-tauri/Cargo.toml
cargo clippy --manifest-path app/src-tauri/Cargo.toml --tests -- -D warnings
cd app && npx tsc --noEmit && npx biome check .
```

Zero-warning policy.

## Layout

```
skein/
├── app/                  # Tauri app
│   ├── src/              # React + TS UI
│   ├── src-tauri/        # Rust shell
│   └── ...
├── .githooks/            # Pre-commit hook (point core.hooksPath here)
├── Cargo.toml            # Workspace (Rust)
└── README.md
```
