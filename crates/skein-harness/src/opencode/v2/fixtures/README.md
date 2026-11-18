# opencode v2 SSE fixtures

Captured from opencode 2.2.0 (`GET /api/event`, free model). The first file
on 2026-10-28, the rest on 2026-10-29. Absolute filesystem paths are scrubbed
to `C:/work/proj` (as a JSON-escaped Windows path); everything else is
byte-identical to the capture, heartbeats included. The project id hashes are
those of a throwaway scratch repo.

Upstream types the stream as an opaque string, so these fixtures are the
contract for `../../v2.rs`.

- `turn-tool-question-permission.sse`: a tool turn, a question form that is
  answered, and a permission request.
- `permission-reply.sse`: permission.asked, one reply, permission.replied,
  turn succeeded. Trimmed to end at that session's `execution.succeeded`
  (the recorder ran on and caught other sessions' frames).
- `question-reply.sse`: form.created, form.replied, turn succeeded.
- `question-cancel.sse`: form.created, form.cancelled, then the tool and step
  fail and the execution is interrupted.
- `subagent.sse`: a root session delegating to a child whose
  `session.created` carries `data.parentID`; the child has its own
  execution.started/succeeded and its own permission round trips; the root
  ends with execution.succeeded.
- `session-get-root.json` / `session-get-child.json`: `GET /api/session/<id>`
  bodies for the two sessions above (`data.parentID` on the child only).
- `interrupt.sse`: user interrupt mid-turn; step.failed, then
  execution.interrupted (reason "user").
- `agent-switch.sse`: `session.agent.selected` (build to plan) with no turn.
- `reconnect-before.sse` / `reconnect-after.sse`: one turn, the stream killed
  mid-turn; the new stream opens with `server.connected` and then carries only
  live frames, with no replay of what was missed.

## Stored messages and action frames (opencode 2.3.2, 2026-11-18)

Captured from opencode 2.3.2 (free model) for the action rows (#678). Paths
are scrubbed to `C:/work/proj` and `C:/work/scratch`; nothing else was
altered. `store-<X>.sse` is the raw `GET /api/event` of scenario X and
`store/messages_<X>.jsonl` is every `session_message` row of the same session
in `seq` order, one JSON object per line with all columns (`data` already
parsed). Scenarios: A plain text; B read + shell tools; C reasoning, read,
edit, write; D reasoning + text only; E a failing tool; F a subagent
delegation (`messages_F_child.jsonl` is the child session).
`store/schema.sql` holds the `CREATE` statements of the captured database.
Contract for `../store.rs` and `../action.rs`.
