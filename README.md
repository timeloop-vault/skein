# skein

An IDE where the agent harness and the code live side by side. Each session
pins a repo + branch + task. Inside a session you can run multiple harnesses
(Claude Code, opencode, GitHub Copilot CLI, or a built-in BYOH agent) on the
same worktree.

This repository is a **prototype** — the goal is to see whether the design
ideas hold up when wired into something runnable. It is not a production app.

## Stack

- **Tauri v2** desktop shell (Rust)
- **React 18 + TypeScript (strict)** for the UI
- **Vite** for the dev server / bundle
- **Biome** for lint + format on the frontend
- **clippy pedantic** on the Rust side

## Getting started

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
