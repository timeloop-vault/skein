#!/bin/sh
# Skein's Hermes hook (macOS/Linux). Skein installs this and points a
# Hermes harness's hooks at `sh <this file> <event>` (#625); the
# Windows twin is skein-hook.py. It forwards Hermes' hook JSON (stdin)
# to Skein's local agent API so the harness's phase is exact instead of
# screen-read. Inert outside Skein: with SKEIN_REVIEW_URL,
# SKEIN_REVIEW_TOKEN or SKEIN_HARNESS_ID unset it does nothing. Prints
# nothing (Hermes parses stdout) and always exits 0.

case "$1" in
on_session_start) route=session-start ;;
pre_approval_request | post_approval_response) route=permission ;;
pre_llm_call | pre_tool_call | on_session_end | subagent_start | subagent_stop)
  route=hermes-turn ;;
*) exit 0 ;;
esac

[ -n "$SKEIN_REVIEW_URL" ] && [ -n "$SKEIN_REVIEW_TOKEN" ] && [ -n "$SKEIN_HARNESS_ID" ] || exit 0

# The token goes to curl through a config on fd 3, never its argv (which
# any local user can read in the process list); stdin stays the body.
curl -fsS -m 2 --noproxy '*' -X POST -K /dev/fd/3   --data-binary @-   "${SKEIN_REVIEW_URL%/mcp}/api/harness/$route" 3<<EOF >/dev/null 2>&1 || true
header = "Content-Type: application/json"
header = "Authorization: Bearer $SKEIN_REVIEW_TOKEN"
header = "X-Skein-Harness: $SKEIN_HARNESS_ID"
EOF
exit 0
