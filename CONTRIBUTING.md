# Contributing to Skein

This is a prototype — the goal is to learn whether the design ideas behind
Skein hold up when wired into a real desktop app.

## Setup

```sh
git clone https://github.com/timeloop-vault/skein.git
cd skein

# Activate pre-commit hook
git config core.hooksPath .githooks

cd app && npm install && cd ..
```

## Running

```sh
cd app && npm run tauri dev
```

## Pre-commit

The hook in `.githooks/pre-commit` runs:

- `cargo fmt --check` (Tauri crate)
- `cargo clippy -- -D warnings` (clippy pedantic, Tauri crate)
- `tsc --noEmit` (strict TS in `app/`)
- `biome check .` (`app/`)

Zero warnings, zero errors.
