#!/usr/bin/env bash
# Local bundled-.app smoke test (macOS).
#
# Builds Skein into a real .app via `tauri build` and Finder-launches
# it via `open`. Reproduces production launch behavior — stripped PATH
# from launchd, OS-conventional log dir, signed/unsigned-asset codepaths
# — without burning a CI release cycle.
#
# `npm run tauri dev` doesn't catch these classes of bug because it
# inherits the dev shell's environment. Use this script when the bug
# is "looks fine in dev, broken in the installed .app."
#
# Usage:
#   scripts/local-bundle.sh            # build + launch
#   scripts/local-bundle.sh --debug    # build the slower debug bundle
#                                      # (faster compile, full symbols)

set -e

cd "$(git rev-parse --show-toplevel)/app"

case "${1:-}" in
    --debug) MODE="debug"; FLAGS=(--debug --bundles app) ;;
    "")      MODE="release"; FLAGS=(--bundles app) ;;
    *)       echo "Unknown arg: $1" >&2; exit 1 ;;
esac

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
