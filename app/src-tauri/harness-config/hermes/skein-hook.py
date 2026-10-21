# Skein's Hermes hook (used on Windows, where Hermes runs a `.py` hook
# with its own Python). Skein installs this and points a Hermes
# harness's hooks at it with the event name as argv[1] (#625); the
# macOS/Linux twin is skein-hook.sh. It forwards Hermes' hook JSON
# (stdin) to Skein's local agent API so the harness's phase is exact
# instead of screen-read. Inert outside Skein: with SKEIN_REVIEW_URL,
# SKEIN_REVIEW_TOKEN or SKEIN_HARNESS_ID unset it does nothing. Prints
# nothing (Hermes parses stdout), stdlib only, always exits 0.
import json
import os
import sys
import urllib.request

ROUTES = {
    "on_session_start": "session-start",
    "pre_approval_request": "permission",
    "post_approval_response": "permission",
    "pre_llm_call": "hermes-turn",
    "pre_tool_call": "hermes-turn",
    "on_session_end": "hermes-turn",
    "subagent_start": "hermes-turn",
    "subagent_stop": "hermes-turn",
}


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def _body(raw):
    try:
        data = json.loads(raw)
    except Exception:
        return raw
    if not isinstance(data, dict):
        return raw
    extra = data.get("extra")
    if isinstance(extra, dict):
        extra.pop("conversation_history", None)
        msg = extra.get("user_message")
        if isinstance(msg, str) and len(msg) > 2000:
            extra["user_message"] = msg[:2000]
    return json.dumps(data).encode("utf-8")


def main():
    route = ROUTES.get(sys.argv[1]) if len(sys.argv) > 1 else None
    url = os.environ.get("SKEIN_REVIEW_URL")
    token = os.environ.get("SKEIN_REVIEW_TOKEN")
    harness = os.environ.get("SKEIN_HARNESS_ID")
    if not (route and url and token and harness):
        return
    if url.endswith("/mcp"):
        url = url[: -len("/mcp")]
    raw = sys.stdin.buffer.read()
    req = urllib.request.Request(
        "%s/api/harness/%s" % (url, route),
        data=_body(raw),
        method="POST",
        headers={
            "Content-Type": "application/json",
            "Authorization": "Bearer " + token,
            "X-Skein-Harness": harness,
        },
    )
    opener = urllib.request.build_opener(
        urllib.request.ProxyHandler({}), _NoRedirect
    )
    opener.open(req, timeout=2).close()


if __name__ == "__main__":
    try:
        main()
    except BaseException:
        pass
    sys.exit(0)
