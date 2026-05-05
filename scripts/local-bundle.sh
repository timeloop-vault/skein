#!/usr/bin/env bash
# Local bundled-.app smoke test (macOS).
#
# Builds Skein into a real .app via `tauri build` and Finder-launches
# it via `open`. Reproduces production launch behavior — stripped PATH
# from launchd, OS-conventional log dir, etc. — without burning a CI
# release cycle.
#
# `npm run tauri dev` doesn't catch these classes of bug because it
# inherits the dev shell's environment. Use this script when the bug
# is "looks fine in dev, broken in the installed .app."
#
# Updater artifact signing is skipped by default (createUpdaterArtifacts
# is overridden to false). The local .app still has the updater plugin
# embedded — it'll check GitHub Releases just like a release build —
# but we don't sign a local .tar.gz, which means no need for the
# private key on disk.
#
# Opt into signed local builds by setting TAURI_SIGNING_PRIVATE_KEY_PASSWORD
# in the environment; the script reads the key from ~/.tauri/skein.key
# automatically.
#
# Usage:
#   scripts/local-bundle.sh            # build + launch
#   scripts/local-bundle.sh --debug    # debug profile (faster compile,
#                                      # full symbols)

set -e

cd "$(git rev-parse --show-toplevel)/app"

case "${1:-}" in
    --debug) MODE="debug"; FLAGS=(--debug --bundles app) ;;
    "")      MODE="release"; FLAGS=(--bundles app) ;;
    *)       echo "Unknown arg: $1" >&2; exit 1 ;;
esac

# Updater signing: opt-in. If the user has set the password env var and
# the key file exists, sign the local .tar.gz the same way CI does.
# Otherwise override the config to skip updater artifact creation
# entirely so the build doesn't fail with "private key missing".
if [[ -n "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" && -f "$HOME/.tauri/skein.key" ]]; then
    export TAURI_SIGNING_PRIVATE_KEY
    TAURI_SIGNING_PRIVATE_KEY="$(cat "$HOME/.tauri/skein.key")"
    echo "==> Updater signing: enabled (key from ~/.tauri/skein.key)"
else
    FLAGS+=(--config '{"bundle":{"createUpdaterArtifacts":false}}')
    echo "==> Updater signing: skipped (set TAURI_SIGNING_PRIVATE_KEY_PASSWORD to enable)"
fi

echo "==> Building Skein.app ($MODE)..."
npx tauri build "${FLAGS[@]}"

APP="src-tauri/target/$MODE/bundle/macos/Skein.app"
if [[ ! -d "$APP" ]]; then
    echo "ERROR: bundle not found at $APP" >&2
    exit 1
fi

# Kill any running instance so `open` actually re-launches the new
# binary instead of just focusing the old window.
pkill -f "Skein.app/Contents/MacOS/skein-app" 2>/dev/null || true
sleep 0.5

# Wipe today's log so we see clean output from this run.
LOG_DIR="$HOME/Library/Logs/com.timeloop-vault.skein"
TODAY="$(date +%Y-%m-%d)"
if [[ -f "$LOG_DIR/skein.log.$TODAY" ]]; then
    rm "$LOG_DIR/skein.log.$TODAY"
fi

echo "==> Launching $APP"
open "$APP"

echo
echo "Logs (live):"
echo "  tail -f \"$LOG_DIR/skein.log.$TODAY\""
