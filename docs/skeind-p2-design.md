# skeind P2a: the daemon owns the harness lifecycle (#583)

Status: design decided 2026-11-12. Slices 1 to 4 implemented (#729,
#730, #731, #732); slices 5 and 6 not yet.
Epic: #575. Design: docs/skeind-design.md. Recon:
docs/skeind-recon.md (section 2 rows 6, 14-18, 28-30, 37, 45, 46, and
section 8).

#583 is a prerequisite of #584 (socket, reattach, snapshots) and #585
(transport). Today a daemon with no client cannot work: argv, ports,
event logging, the phase machine and mail delivery all live in the
TypeScript frontend, rooms are saved wholesale by the client, and every
agent token is revoked at boot. #583 moves what unattended operation
needs into skein-daemon.

The daemon is still embedded in the app after #583. Nothing survives a
UI restart yet. #583 only makes the daemon able to run a harness with
no client attached. Surviving the restart is #584.

## Today

| Concern | Owner today | Where |
| --- | --- | --- |
| Argv per kind, resume argv | Daemon since slice 2 (#730); before it, the frontend | `harness_argv/`; the app calls `harness_fresh_cmd`, `harness_resume_cmd`, `rooms_resume_cmds`. `harnessCmd.ts` keeps only `isOpencodeV2` and the Hermes profile constant |
| Spawn defaults (shell, cwd, agent) | Daemon since slice 2 (#730); before it, the frontend held the agent | `default_agents` in the daemon's `settings.json`; `default_shell_impl`, `default_cwd_impl`. `prefs.ts` `defaultAgents` survives only as the legacy import source |
| opencode port, sidecar | Daemon since slice 2 (#730); before it, the frontend | The port is allocated inside the argv commands; the spawn (`ensure_running` since slice 3, `pty_spawn` before) probes the version and starts the sidecar. `opencodeV2Spawn.ts` is gone |
| Spawn and kill of the PTY | Daemon since slice 3 (#731); before it, the frontend | `harness_runner/`: spawns from the stored record on first request, stops when the record goes. `useTerminalSpawn.ts` asks, attaches and detaches |
| PTY output | Replaceable sink per PTY | `pty/sink_slot.rs`. No ring buffer in the daemon (#584); with no sink attached output is dropped |
| Room and harness records | Daemon since slice 1 (#729); before it, the frontend saved wholesale | `RoomStore` (`room_store/`); clients send mutations through `rooms_apply`. `db_save_rooms` and `roomsSaveScheduler.ts` are gone |
| Boot resume | Daemon since slice 3 (#731); before it, the frontend | `boot_resume.rs`: launch migration, shell claims, a fresh opencode port; retried until it succeeds (#753), saved under origin `daemon-boot`. `useRoomsHydrate.ts` keeps the folder and identity gates |
| Adapters (Claude JSONL, opencode SSE, Hermes) | Daemon attaches them after a spawn (slice 3); the client still attaches its own Claude and opencode event channels, teed into the daemon's phase sink, for non-phase consumers (slice 4) | `harness_runner/adapters.rs`, `terminalSpawnAdapters.ts` |
| Transition log (`harness_events`), action rows (`harness_actions`) | Daemon: action rows since slice 3, the transition log since slice 4 | `harness_phase/stream.rs`, `useHarnessActions.ts` |
| Phase machine | Daemon since slice 4 (#732); before it, the frontend. The webview mirrors it | `harness_phase/`; `phaseMirror.ts`, `harnessActivity.ts`, `subagents.ts`, `backgroundTasks.ts` |
| Mail and nudge delivery | Frontend | `useMailDelivery.ts`, `harnessInput*.ts`, xterm `term.paste()` |
| Agent API verbs | Daemon, some ask the webview | `request_frontend`, 3 to 60 s timeouts |
| Agent tokens | Daemon | `agent_tokens` table; all revoked at `Daemon::build` |

Four facts matter most.

- The phase machine is about 3,800 lines of TypeScript. Its tests are
  pure node vitest, no DOM. Nearly all its inputs are already produced
  by daemon code: L2c Claude and opencode events, the permission hook,
  PTY bytes, PTY exit and timers. The subagent and background-task
  state machines already live in `crates/skein-harness`.
- Three inputs exist only in a client: the decisive keystroke that
  clears `permission` (xterm `onKey`, plus the empty-composer read of
  the xterm buffer), focus-induced output muting, and the Hermes
  screen-phase reader (`screenPhase.ts`).
- Rust had no phase. `list_rooms`, `get_room` and `list_harnesses` asked
  the webview (request kind `harness_phases`, 3 s). No answer meant
  every phase read "unknown". Since slice 4 they read `PhaseService`.
- The agent API URL, with its ephemeral port, is baked into each
  harness env beside the token (`SKEIN_REVIEW_URL`,
  `SKEIN_REVIEW_TOKEN`, `SKEIN_ROOM_ID`, `SKEIN_HARNESS_ID`). A daemon
  restart orphans live harnesses by port as well as by token. The
  epic already says harnesses do not survive a daemon restart.

## Target

After #583 the daemon owns the records and the lifecycle. A client
renders state and sends intent.

Daemon: room and harness records, argv and ports, spawn defaults,
PtyManager and the stop decision, the adapters, the event and action
logs, the phase machine, mail and nudge delivery, the agent API, and
tokens.

Client: xterm, badges, toasts and OS notifications, the design pane,
Files buffers, and the three client-only signals.

```mermaid
flowchart LR
    subgraph D["Daemon"]
        REC["Room and harness records"]
        ARGV["Argv, ports, defaults"]
        PTY["PtyManager"]
        ADP["Adapters and logs"]
        PH["Phase machine"]
        MAIL["Mail delivery"]
        API["Agent API and tokens"]
    end
    subgraph C["Client"]
        TERM["xterm"]
        NOTE["Badges, toasts, OS notifications"]
        DES["Design pane"]
        FILES["Files buffers"]
        SIG["Client-only signals"]
    end
    C -- "mutations, intent" --> REC
    REC -- "change events" --> C
    PH -- "phase stream" --> NOTE
    SIG -- "input facts" --> PH
    PTY -- "output" --> TERM
    ARGV --> PTY
    PTY --> ADP
    ADP --> PH
    PH --> MAIL
    MAIL --> PTY
    API --> REC
    API -. "design verbs only" .-> DES
```

Figure 1: who owns what after #583. Renders section "Target"; the
text is authoritative (non-normative).

## Decisions

All four were made on 2026-11-12.

### D1. Token lifetime

**Decision.** A token lives as long as the daemon process. A room's
token is also revoked when the room is archived or closed. The revoke
at daemon start stays: PTYs die with the daemon, and a UI restart no
longer reaches it. The per-room revoke uses the existing
`revoke_agent_tokens(Some(room))`, which has no caller today. A
reopened room mints a fresh token at its next spawn.

**Reason.** It is the smallest change that keeps a surviving
harness's tools working and still bounds a leaked token.

**Rejected.**
- Daemon-life only: archived rooms' tokens would linger.
- Per-harness tokens: a schema change, and it breaks "the token is the
  room scope".
- Persisting tokens and port across a daemon restart: contradicts the
  epic's non-goal that durability comes from resume.

**Consequences.** For #584: the daemon restart path must re-spawn with
resume, which mints fresh env anyway. Archive and close must call the
per-room revoke (slice 6).

### D2. Phase machine

**Decision.** Port the whole machine to Rust in skein-daemon. The
daemon owns the phase store, deferral, the subagent and background
registries, watchdogs and ceilings, the supervisor invariants, the
transition log, and mail and nudge delivery. Clients subscribe to a
phase stream and render badges, toasts and OS notifications.

The three client-only signals become client-to-daemon inputs:
- Decisive input. The daemon sees stdin writes itself; the client adds
  the fact "the composer was empty".
- Focus mute.
- The Hermes screen phase. It stays client-reported until #584's
  headless emulator exists. With no client attached, a screen-phase
  harness reads `unknown`. This is a known limit.

The `harness_phases` round trip is deleted.

Mitigation: port the vitest tables to Rust tests first, as the spec.
Then run the Rust machine in shadow beside the TypeScript one, logging
every divergence. Only then flip the client to the daemon's phase and
delete the TypeScript machine. (As built: see "Slice 4 as built", which
also lists the one visible change at spawn.)

**Reason.** One source of truth. Nearly all inputs are already on the
daemon side. #587 needs it there.

**Rejected.**
- Split authority: two machines must agree.
- A headless TypeScript host: a JS runtime on every daemon host.
- Deferring: fails #583's acceptance and blocks #587.

**Consequences.** The largest port in the phase, and the one that
needs the shadow run. It lands late (slice 4; as built, see "Slice 4 as
built").

### D3. Slice order

**Decision.** State first. Six child issues, in the order listed under
"Slices".

**Reason.** Every later move needs a record the daemon writes. Each
slice ships alone with no visible change in attached mode. The riskiest
port lands late, on a stable base.

**Rejected.**
- Phase machine first: its inputs would be re-plumbed twice.
- One harness kind at a time: two lifecycles side by side, and
  persistence cannot be split per kind.

**Consequences.** The first two slices touch the most frontend code
and the least new behaviour.

### D4. Verbs with no client

**Decision.** The daemon executes `create_room`, `open_harness`,
`close_harness` and `close_room` itself, over the slice 1 mutations and
the slice 3 spawn, and reads phases from its own machine. Attached
clients learn through events. `(kind, agent)` resolution uses the
daemon-side defaults from slice 2.

Only the design-pane verbs and the native screenshot need a client.
With none attached they refuse with a named code, `no_client`, distinct
from today's generic unavailable.

`close_harness` and unsaved Files buffers: unsaved text exists only in
a client. The daemon asks each attached client and refuses if any
reports dirty buffers. With no client attached there is nothing
unsaved, and it proceeds.

`activate: false` semantics stay: a verb never takes over a screen.

**Reason.** An unattended director must be able to open and close
rooms.

**Rejected.**
- Queueing until a client attaches: stalls the director.
- Refusing everything: defeats detached mode.

**Consequences.** Today's `request_frontend` kinds for these four verbs
(`create_room.resolve`, `create_room`, `open_harness.resolve`,
`open_harness`, `close_harness`, `close_room`) are retired. The design
kinds stay.

## Lifecycle across a UI restart

The daemon is embedded today, so steps 1 to 3 and 6 to 7 describe what
#583 makes possible; steps 4 and 5 need #584 to be real.

```mermaid
sequenceDiagram
    participant U as Client
    participant D as Daemon
    participant H as Harness
    U->>D: 1 detach
    H->>D: 2 events and output
    D->>D: 3 track phase, deliver mail
    Note over U,D: client gone, harness runs
    U->>D: 4 attach (needs 584)
    D-->>U: 5 snapshot and current phase (needs 584)
    U->>D: 6 client-only inputs
    D-->>U: 7 change and phase events
```

Figure 2: a harness across a UI restart. Renders section "Lifecycle
across a UI restart"; the text is authoritative (non-normative).

1. The client detaches. Unmounting a terminal no longer kills the PTY;
   only an explicit `stop_harness` does (slice 3).
2. The harness keeps running. The daemon's adapters keep tailing its
   transcript or event stream and write `harness_actions` rows
   (slice 3).
3. The daemon's phase machine keeps tracking the phase, including
   deferral and watchdogs, and writes the `harness_events` transition
   log (slice 4). Mail and nudges go out through the daemon's
   PTY writer. Agent verbs keep working under D4. The token stays
   valid (D1).
4. A client attaches. This needs the socket and the event stream with
   a sequence number from #584.
5. The client gets current state: records, phase, and a terminal
   snapshot. The snapshot and ring buffer are #584.
6. The client sends its own facts: decisive input with the
   empty-composer flag, focus mute, and the Hermes screen phase.
7. The client applies daemon change events and phase events from then
   on. It renders; it decides nothing about the lifecycle.

What #583 delivers in attached mode: the same screen, with the daemon
as the author of every record, argv, port, log row and phase. What
#584 adds: a process boundary, reattach, and terminal snapshots.

## Slices

Six child issues, in this order.

1. **Per-room mutations.** The daemon owns Room and Harness records.
   Replace `db_save_rooms` / `save_all` with per-room and per-harness
   mutation commands. The client becomes a subscriber that applies
   daemon change events. Keep the #167 safety properties (quarantine,
   snapshots) and the #594 structural-versus-debounced distinction as
   a daemon concern. `save_all` remains only for migration.
2. **Argv, ports and spawn defaults.** Port `cmdForKind`, `resumeCmd`,
   `withResumeCmds`, `unarchiveRoomTransform`, `remoteArgv` and
   `validateAgent` to Rust over the Harness record. Move
   `defaultAgents` and default shell and cwd into daemon settings
   (`settings.json`), with a one-time import from the client's
   localStorage. Allocate the opencode port and own the sidecar at
   spawn. The daemon-host probes (login-shell probe, program
   resolution, agent enumeration, CLI version probe, the #565 login
   env with loopback kept in `NO_PROXY`) already live in skein-daemon.
   They are preserved, not moved: the Rust argv builder uses the
   daemon's own program resolution and agent enumeration.
3. **Daemon-initiated spawn and stop.** `ensure_running(harness)` and
   explicit `stop_harness`. The daemon attaches the L2c adapters and
   writes the `harness_actions` rows (action extraction). Boot resume
   moves to daemon start. Define the sink contract: detach does not
   kill.
4. **Phase machine in Rust.** Shadow, then flip (D2). The transition
   log (`harness_events`, written today by `useTransitionLog.ts`)
   moves with it, since it records phase transitions.
5. **Mail and nudge delivery.** The #238 / #381 gate and the
   bracketed-paste write go through the daemon's PTY writer.
6. **Client-less agent verbs and the token policy.** D4 and D1.

| Slice | Depends on |
| --- | --- |
| 1 | #582 (done) |
| 2 | 1 |
| 3 | 1, 2 |
| 4 | 3 |
| 5 | 4 |
| 6 | 1, 3, 4 |

```mermaid
flowchart LR
    S1["Slice 1: mutations"] --> S2["Slice 2: argv, ports"]
    S2 --> S3["Slice 3: spawn, stop"]
    S1 --> S3
    S3 --> S4["Slice 4: phase machine"]
    S4 --> S5["Slice 5: mail delivery"]
    S1 --> S6["Slice 6: verbs, tokens"]
    S3 --> S6
    S4 --> S6
```

Figure 3: slice dependencies. Renders section "Slices"; the text is
authoritative (non-normative).

## Slice 1 as built (#729)

The daemon now holds the room and harness records in memory, in
`RoomStore` (`crates/skein-daemon/src/room_store/`). That copy is the
authority. A client diffs what it has against what the daemon is known
to hold, sends the difference as mutations, and applies the change
events the daemon broadcasts as `skein://rooms-changed`. The daemon
writes sqlite one row at a time.

The 37 `setRooms` call sites in the frontend were left alone on
purpose. Nearly every shipped regression has lived in the frontend, and
rewriting 37 call sites would be the riskiest way to move the records.
The seam is the save path instead: it used to send the whole array to
`db_save_rooms`, and now it diffs (`diffRooms` in `roomMutations.ts`)
and sends mutations. The React state is unchanged.

Wire types are in `crates/skein-proto/src/room_mutations.rs`.

| Mutation | Effect |
| --- | --- |
| `createRoom` | Adds a whole room, harnesses included, at the end. Skipped as `exists` if the id is taken |
| `updateRoom` | Replaces the room-level fields. The payload's `harnesses` is ignored; the stored ones are kept |
| `removeRoom` | Removes a room. Skipped as `absent` if it is not there |
| `reorderRooms` | Listed ids take the listed order; unlisted rooms follow in their old order |
| `putHarness` | Replaces a harness by id, or appends it if new. Since #755 an existing harness keeps its stored `sessionId` and `opencodeVersion` |
| `removeHarness` | Removes a harness. Skipped as `absent` if it is not there |
| `reorderHarnesses` | Same rule as `reorderRooms`, within one room |

| Change | Meaning |
| --- | --- |
| `roomPut` | The full room after the batch (create, update or any harness change). Idempotent |
| `roomRemoved` | The room is gone |
| `roomsOrder` | The full order of all rooms after the batch |

A room-scoped mutation (`updateRoom`, `putHarness`, `removeHarness`,
`reorderHarnesses`) for a room the store does not hold is skipped as
`unknown_room`. The outcome lists skipped mutations by index. A batch
where everything was skipped emits no event.

```mermaid
sequenceDiagram
    participant A as Client A
    participant D as Daemon
    participant B as Client B
    A->>D: 1 rooms_apply (batch)
    D->>D: 2 apply in order, one lock
    D-->>B: 3 rooms-changed (origin A)
    D-->>A: 4 rooms-changed (origin A)
    A->>A: 5 ignore own echo
    B->>B: 6 rebase onto local edits
    D->>D: 7 persist dirty rows
    D-->>A: 8 outcome (skipped list)
    A->>A: 9 baseline becomes sent snapshot
```

Figure 4: a room mutation with two clients. Renders section "Slice 1
as built" and crates/skein-daemon/src/room_store/; the text is
authoritative (non-normative).

1. Client A diffs its baseline against its current rooms and sends the
   mutations in one `rooms_apply` call with its `clientId`.
2. The store applies them in order under its lock. It refuses the whole
   call if no rooms load has succeeded yet.
3. The store emits one `rooms-changed` event carrying `origin` and the
   changes. Client B receives it.
4. Client A receives the same event.
5. Client A sees `origin` equals its own `clientId` and ignores it.
6. Client B merges the changes into its React state and its baseline
   (see "The conflict rule").
7. The store persists the dirty rows. A structural batch is written
   before the call returns. Anything else is written by the debounce.
8. The call returns the outcome, listing skipped mutations.
9. Client A moves its baseline to the snapshot it sent, with any remote
   changes that arrived during the call applied on top. On failure the
   baseline stays put and the send is retried after 2 s.

Steps 3 to 7 are not strictly ordered in time. The code emits the event
before it persists; the figure follows the code.

### Where the #167 safety properties live

- **Quarantine.** `load_all` is unchanged: an unparseable row goes to
  `sessions_quarantine`, never dropped.
- **Snapshots.** The `.bak` and `.bak.1` rotation is unchanged.
- **No write before a successful load.** The store enforces it for
  every mutation, not only an empty save. `RoomStore::apply` returns an
  error until `load_rooms` has filled the store. A reload goes through
  `RoomStore::load_with`: under the store lock it persists anything
  dirty, then reads sqlite and installs the result, so a reload cannot
  overwrite a mutation that landed in between. If that persist fails, the
  reload returns before it reads.
- **Writes cannot wipe.** `Database::write_room_rows` upserts the
  touched rows, deletes the removed ids and writes the order into
  `created_at`, all in one transaction. A row nobody named is not
  touched, so a client cannot erase another client's room.

### Structural versus debounced

The client no longer has a timer or a scheduler. It sends every change
at once, and the daemon decides when to write.

Structural batches are written before `rooms_apply` returns:
`createRoom`, `removeRoom`, `reorderRooms`, `removeHarness`,
`reorderHarnesses`, a `putHarness` that adds a harness, and an
`updateRoom` or `putHarness` that changes a key field. The room key is
id, name, archived, retired, cwd, repoRoot, branch, createdBy and
closedBy. The harness key is id, kind, name, agent, cwd, remote,
sessionId and createdBy. This is the #594 list, now in `structural.rs`.

Everything else is coalesced: a 500 ms debounce, with a 2 s maximum wait
from the first unwritten change. `rooms_flush` writes at once. The
client calls it on window close, Cmd+Q, updater install and page hide,
after draining its own send.

### The conflict rule

A remote `roomPut` replaces the local room in the baseline
(`roomChanges.ts`): a field the remote room lacks is cleared, and an
equal room keeps its object identity. The React state is then merged
three-way, in `roomsRebase.ts`: per key over the union of keys, for the
room-level fields and, per harness id, for the harness fields. A local
value that differs from the baseline is an edit not yet synced. It wins,
and the next diff sends it. So a remote change to one field and an
unsynced local edit to another field of the same room or harness both
survive.

`spawnGen` is the one Harness field the daemon does not store
(`LOCAL_ONLY_HARNESS_KEYS` in `roomMutations.ts`). It is never sent on
its own, and a remote change never overwrites it.

Since #755 two more harness fields are not three-way merged: `sessionId`
and `opencodeVersion` are daemon-owned (`DAEMON_OWNED_HARNESS_KEYS`).
`diffRooms` ignores them and a rebase takes the remote value, so for these
two the remote always wins. See "Daemon-owned fields (#755)".

- A room removed locally and not yet synced is not brought back by a
  remote `roomPut`.
- A room created locally and not yet synced is kept whole.
- A remote `roomsOrder` is ignored while a local reorder is pending.
- On the daemon, the skip rules never resurrect a room: a mutation for
  a removed room is skipped as `unknown_room`.

### Known limits

- No sequence number and no reattach yet. A client that misses an event
  has no way to catch up. That is #584.
- The agent API verbs (`create_room`, `open_harness`, `close_harness`,
  `close_room`) still go through the frontend until slice 6.
- `Database::save_all` survives for migration and tests only.
- The event goes out before a structural write. If that write fails,
  `rooms_apply` returns an error, the rows stay dirty and the writer
  re-arms itself (`persist_or_rearm`). A client retry of the same
  mutations is answered with an error while the write still fails, and
  with Ok only once the row is on disk.
- The client has no debounce. Each change is one small `rooms_apply`
  call. Calls never overlap, and changes made during a call collapse
  into the next one.
- A batch the daemon always rejects is retried every 2 s, with no
  backoff, as the old scheduler did.
- The client subscribes to `skein://rooms-changed` at mount, not after
  load. Events from other clients that arrive during the load are
  buffered and replayed when the loaded rooms are installed
  (`beginHydrate` / `finishHydrate` in `roomsSync.ts`), so none is
  merged against an empty list. An event emitted before the listener
  has registered is not seen; the load that follows reflects it.

## Slice 2 as built (#730)

The daemon now builds every harness argv and allocates the opencode
port. The app asks for the argv, records the result and spawns what it
was given. The builders are a pure Rust port of the old frontend ones,
in `crates/skein-daemon/src/harness_argv/`:

- `mod.rs`: `cmd_for_kind`, `resolve_hermes_profile`.
- `resume.rs`: `resume_harness`, `with_resume_cmds`,
  `unarchive_room_transform`.
- `remote.rs`: `remote_argv`, `validate_remote_host`.
- `agent_check.rs`: `validate_agent`, `with_verdict`.
- `opencode_v2.rs`: `opencode_v2_argv`, `opencode_spawn_plan`,
  `is_opencode_v2`.
- `plan.rs`: `fresh_cmd`, `resume_one`, `resume_rooms`. These wrap the
  builders with port allocation and session id minting.

| Command | The app calls it | The daemon |
| --- | --- | --- |
| `harness_fresh_cmd` | A new harness, and repoint | Builds the first argv for the kind, allocates the opencode port, mints a Claude session id, takes the shell for a byoh harness from its own default |
| `harness_resume_cmd` | Restart of one harness | Rebuilds the argv from the record in resume form, with a fresh opencode port |
| `rooms_resume_cmds` | Boot, and unarchive | Does the same for every harness of the given rooms; unarchive also clears `archived`, `retired` and `closedBy` |

`pty_spawn` changed too. It takes `agent` and `sessionId` from the
record. For an opencode harness on a v2 CLI it probes the version,
starts the sidecar, swaps in `opencode --server <url> --session <id>`
and binds the sidecar to the PTY. The result carries
`sidecarGeneration`, `opencodeVersion` and `sessionId`. The old
`opencodeV2Spawn.ts` is gone, and so are the `pick_free_port` and
`opencode_sidecar_start` and `opencode_sidecar_stop` commands.

The agent verdict moved as well. `list_harness_agents` takes an optional
`check` name and returns `verdict` (`ok`, `unknown` or `unverified`)
from the same enumeration. The TS `validateAgent` is gone. The spawn
gate, `create_room` resolution and the harness picker read the daemon's
verdict.

```mermaid
sequenceDiagram
    participant C as Client
    participant D as Daemon
    participant O as opencode
    C->>D: 1 harness_fresh_cmd
    D->>D: 2 allocate port, build argv
    D-->>C: 3 cmd and port
    C->>D: 4 rooms_apply putHarness
    C->>D: 5 pty_spawn with agent
    D->>D: 6 probe version, v1 keeps argv
    D->>D: 7 spawn PTY
    D-->>C: 8 spawn result
    C->>D: 9 opencode_events_attach
    D->>O: 10 connect to event stream on the port
```

Figure 5: a fresh opencode harness on a v1 CLI, from asking for argv to
a running PTY with SSE attached. The v2 differences are in step 6 to 10
below. Renders section "Slice 2 as built" and
crates/skein-daemon/src/harness_argv/; the text is authoritative
(non-normative).

1. The client sends `harness_fresh_cmd` the kind, an optional session
   id, the agent, the remote spec and the Hermes profile (no room),
   plus `newHarness`. It is true for a brand-new harness, where the
   daemon mints a Claude session id when none is given and defaults the
   Hermes profile; it is false for a repoint, where both pass through
   as given.
2. The daemon binds a free port on `127.0.0.1` and builds the argv with
   `cmd_for_kind`.
3. It returns the argv and the port. The client keeps the port to attach
   the SSE adapter later (v1 only).
4. The client records the harness through `rooms_apply`, as in slice 1.
5. The client calls `pty_spawn` with the argv, the `agent` and the
   `sessionId` of the record.
6. The daemon probes the opencode version. On an older (v1) CLI the argv
   from step 3 is used as is.
7. It spawns the PTY.
8. The result carries `sidecarGeneration`, `opencodeVersion` and
   `sessionId`. The client writes the session id back to the record
   only when one is set and differs from the stored one (the v2 path).
9. The client asks the daemon to attach the SSE adapter
   (`opencode_events_attach`).
10. The daemon connects to opencode's event stream on the port from
    step 3.

Steps 4 and 5 may happen in either order in the code; the figure shows
the usual one.

On a v2 CLI, step 6 also starts a sidecar for the agent and session
inside `pty_spawn`, swaps in `opencode --server <url> --session <id>`
and binds the sidecar to the PTY. The result's `sidecarGeneration` is
set, and steps 9 and 10 attach by sidecar generation with no port
(`ptySpawnOutcome.ts`); the port from step 2 goes unused. The live check
exercised the v1 path.

### The merge rule

The results of `harness_resume_cmd` and `rooms_resume_cmds` are merged
into the client's rooms by id (`harnessArgvApply.ts`). Only `cmd`,
`sessionId` and `shellClaim` are taken. On unarchive `archived`,
`retired` and `closedBy` are dropped as well. The reason is that the
Rust `Room` and `Harness` structs have no catch-all, so they do not
carry frontend-only fields. Replacing the room wholesale with the
daemon's copy would lose those fields. Merging by id also keeps a room
the user edited while the call was in flight.

### Defaults and the one-time import

`defaultAgents` lives in the daemon's `settings.json`
(`SpawnSettings.default_agents`; logic in
`spawn_env/default_agents.rs`). `None` means the daemon has never owned
a value.

- `default_agents_import` stores the client's legacy localStorage map
  only if the daemon has never owned one. The check and the write happen
  under the settings lock. Later calls change nothing and return what is
  stored. Blank names and keys that are not a harness kind are dropped.
- `default_agent_set` edits one kind. A blank name removes it.
- A whole-settings save never changes `defaultAgents`; only
  `default_agents_import` and `default_agent_set` write it.
- While `settings.json` is unreadable (the daemon runs on defaults in
  its degraded state), the import and the setter refuse and write
  nothing, so a corrupt file is never replaced by the automatic boot
  import.
- The client calls the import on every boot and never deletes the legacy
  key (`defaultAgentsStore.ts`, `useAppSettings.ts`).

The default shell and the default cwd were already computed by the
daemon for its own host (`default_shell_impl`, `default_cwd_impl` in
`spawn_env/lookups.rs`; `SpawnSettings.shell` is the user override). The
byoh argv now takes the shell from there, inside the daemon, instead of
receiving it from the client. Nothing for shell or cwd was ever in
localStorage, so nothing is imported for them.

### What stays with the client until slice 3

Slice 3 (#731) moved these (see "Slice 3 as built"):

- Deciding when to spawn and when to stop.
- The stale-session probes that run before the client asks for resume
  argv.
- Passing the argv it was given back into `pty_spawn`.

`buildRemoteSpec` is not in that list: the client still builds the
remote spec when it creates a `remote` harness.

A failed daemon argv call at boot shows the same retry card as a failed
rooms load. In creation, reopen and repoint it shows a message and
changes nothing.

### Known limits

- New Room creates the worktree before it asks for the argv. If
  `harness_fresh_cmd` then fails, no room is created but the worktree
  and its branch stay behind, and have to be removed by hand before the
  same branch name can be used again. Skein performs no git mutations
  beyond adding the worktree, so it does not clean this up.
- The import marker is the presence of the key. A daemon whose
  `settings.json` was written by hand with an empty map counts as
  having imported.
- Other clients are not told when a default agent changes. They see the
  new value at their next boot or settings read.
- The argv round trip ended in slice 3: `pty_spawn` is gone and the
  daemon spawns from the stored record. The client's copy of the argv is
  now only an expectation (`expectCmd`).

## Slice 3 as built (#731)

The daemon now starts and stops harnesses itself. A view no longer
spawns or kills anything. It asks the daemon to make sure a harness is
running, attaches its terminal, and detaches when it goes away. The code
is `crates/skein-daemon/src/harness_runner/`:

- `mod.rs`: `HarnessRunner` with `ensure_running`, `stop_harness`,
  `detach_view`, `detach_view_if`, `release_events` and `reconcile`.
- `gates.rs`: the refusals and `SpawnPlan::from_record`.
- `adapters.rs`: attaches the Claude, opencode and Hermes adapters.
- `record.rs`: writes the spawn result back to the record.

Three more pieces: `boot_resume.rs` (boot resume), `pty/sink_slot.rs`
(the replaceable output sink) and, in the app,
`commands/harness_runner.rs` (the three Tauri commands).

### Decisions made on 2026-11-12

- **Exit policy.** A harness that exits stays exited. The daemon starts
  nothing in its place. The post-exit shell starts only when a client
  asks: Enter sets the record's `launch` to the shell (#752; it rewrote
  `cmd` before, see "Spawn-time argv (#752)"), and the view then
  asks again with `restart`. Rejected: the daemon starts the shell, and
  the daemon restarts the harness.
- **Re-attach with no ring buffer.** The terminal starts blank, and the
  view forces a repaint by resizing the PTY one row down and back
  (`app/src/terminalRepaint.ts`). A plain shell stays blank until its
  next output. Rejected: an interim raw tail buffer in the daemon, and
  staying blank until new output arrives.
- **Spawn on first request.** The daemon prepares at start and spawns a
  harness when a view or a verb first asks for it. A daemon with no
  client resumes nothing by itself. Rejected: spawn everything at daemon
  start. Startup output would be lost until #584 and the size would be a
  guessed 80x24.

### The commands

| Command | Effect |
| --- | --- |
| `harness_ensure_running` | Spawns the harness from its record, or reuses its process, and attaches this view's channel as the output sink. Returns the spawn result, `started`, `exited`, `exitCode` and an `attachId` |
| `harness_detach` | Clears the output sink. With an `attachId` only if that attachment is still current. Never kills |
| `harness_stop` | Kills the process and its sidecar, detaches the adapters and forgets the harness. Idempotent. No client calls it yet |

`pty_spawn`, `pty_kill` and the never-called `db_record_harness_action`
are removed.

`ensure_running` takes `{roomId, harnessId, rows, cols, envProfile,
restart, expectCmd, expectLaunch}`. Argv, folder, kind, agent and session
id come from the stored record, never from the caller; since #752 the argv
is derived at spawn (see "Spawn-time argv (#752)"). It refuses with one of these
codes:

| Code | When |
| --- | --- |
| `unknown_harness` | No such harness in that room |
| `room_archived` | The room is archived |
| `no_pty` | The kind has no terminal (`files`, `design`) or is unknown |
| `no_cmd` | The record has no argv |
| `folder_missing` | The harness's folder, else the room's, is not a directory |
| `repo_mismatch` | The #418 identity check (`check_identity`) says the folder holds another repository. An unknown identity passes |
| `stale_record` | `expectCmd` is set and differs from the record's `cmd`, or `expectLaunch` differs from its `launch`. Checked before anything is touched |
| `spawn_failed` | The spawn itself failed |

An existing process is reused (`started: false`) unless `restart` is
true. A reused process that already exited delivers its `exit` event to
the new sink through the attach.

```mermaid
sequenceDiagram
    participant V as View
    participant D as Daemon
    participant H as Harness
    D->>D: 1 boot resume prepares records
    V->>D: 2 ensure_running
    D->>H: 3 spawn from the record
    D->>D: 4 attach adapters
    D-->>V: 5 started, output flows
    V->>D: 6 detach
    H-->>D: 7 output, dropped
    D->>D: 8 adapters write rows
    V->>D: 9 ensure_running again
    D-->>V: 10 started false, sink attached
    V->>D: 11 resize one row down, then back
    H-->>V: 12 redraw
    D->>D: 13 record removed or room archived
    D->>H: 14 stop
```

Figure 6: the harness lifecycle across a spawn, a detach, a re-attach and
a stop. Renders section "Slice 3 as built" and
crates/skein-daemon/src/harness_runner/; the text is authoritative
(non-normative).

1. At the first successful rooms load of the daemon process, boot
   preparation runs the launch migration per room, the shell-claim step and
   a fresh opencode port, and is retried until it succeeds (#753). It
   spawns nothing. (As built in #731 it also probed sessions and rewrote
   every `cmd` to resume form; #753 reduced it, see "Boot preparation
   after #753". Since #752 the spawn derives its argv itself.)
2. A view asks for the harness with `harness_ensure_running`. Before it
   does, the client drains its pending room mutations so the record is
   at the daemon.
3. The daemon checks the gates and spawns the process from the record.
4. It attaches the adapters itself (Claude JSONL tail, opencode SSE,
   Hermes actions tail).
5. The call returns with `started: true`. Output flows to the view's
   sink.
6. The view goes away and calls `harness_detach`. The process keeps
   running.
7. With no sink, output is dropped. The daemon answers a ConPTY cursor
   query itself.
8. The adapters keep writing `harness_actions` rows.
9. A new view calls `harness_ensure_running` with `restart` false.
10. The daemon finds the process and attaches the sink. The result has
    `started: false`.
11. The view resizes the PTY one row down, then back after 80 ms.
12. The tool redraws and the new view fills.
13. A record change removes the harness or archives its room. The room
    store tells the runner after each applied batch.
14. `reconcile` stops it without taking the spawn gate: it kills the
    process and its sidecar and detaches the adapters. An explicit
    `harness_stop` does the same, after waiting for the gate.

### Boot resume

(#753 reduced this step; see "Boot preparation after #753". The rest of
this section is the record of #731 as built.)

`BootResume::load_rooms` (`boot_resume.rs`) wraps the rooms load. At the
first successful load with at least one room in this daemon process it:

- probes each stored session id (Claude, opencode, Hermes) and clears the
  stale ones. A probe error keeps the id. An opencode v2 id is trusted
  and not probed;
- rewrites each `cmd` to resume form with a fresh opencode port, using
  the slice 2 builders (#752: first it runs `migrate_launch`, and the
  rewrite keeps a shell `launch`; the spawn does not depend on the
  rewritten `cmd`);
- saves the changed harnesses through `RoomStore::apply` under origin
  `daemon-boot`, then flushes. A failed flush is logged, and the
  prepared rooms are still what the client receives.

The preparation writes one info log line with the counts (rooms, `cmd`s
changed, stale sessions dropped).

A later load in the same process (a retry or a webview reload) returns
the stored rows, already prepared, and does not prepare again. A load that
starts while the preparation is running waits for it and then returns the
prepared rows (#744): the preparation holds a mutex, taken before the store
and db locks. A failed preparation is logged, the unprepared rooms are
returned and it is not retried. The frontend no longer probes at boot
and no longer calls `rooms_resume_cmds` at boot. It takes the opencode
ports from the stored `cmd` (`opencodePort.ts`). `rooms_resume_cmds`
remains for unarchive.

### The output sink

`PtyManager::spawn_with_sink` takes an optional sink. `attach`,
`detach`, `is_alive` and `exit_code` work on a running PTY. The reader
and waiter threads write through a `SinkSlot` (`pty/sink_slot.rs`)
and never hold a sink themselves.

- Detach never kills.
- With no sink, output is dropped. The ring buffer and snapshots are
  #584.
- An exit with no sink is recorded. The next attach receives it at once.
- Each attach gets an id. `detach_if(attach_id)` clears the sink only if
  that attachment is still current, so a late detach from an old view
  cannot blank a newer one.

ConPTY withholds all child output until the host answers its `ESC[6n`
cursor query (#484). While a view is attached its xterm answers, and
`ptyInputBuffer.ts` is unchanged. While none is attached the slot
answers `ESC[1;1R` itself, and never while a live sink is attached, or
the child would see a second report as input. A sink whose channel is
dead (a view that vanished without detaching) stays attached, but it
cannot answer, so the slot answers and ConPTY output is not withheld. A
query split across two chunks is handled. `pty/sink_pty_tests.rs` spawns a real child with no
sink on Windows and shows that its output arrives once a view attaches.
The same test timed out after 20 s with no output when the answer was
disabled; that check was run by hand and is not an automated assertion.

### Adapters and rows

After a spawn the daemon attaches the adapters itself. They write the
`harness_actions` rows, and since slice 4 their events feed the daemon's
phase machine. opencode
attaches by the `--port` in the stored `cmd`, or by sidecar generation on
v2. The client still attaches its own event channels for the consumers
that are not phase (see "Slice 4 as built"). Its attach replaces the
daemon's, with the daemon's phase sink teed in. When the client detaches (`claude_events_detach` and the
opencode one), the app calls `release_events`: the runner puts its own
adapter back if it still has the harness and the process is alive, and
detaches otherwise (also when the process has exited). The
transition log `harness_events` is written by the daemon since slice 4.

### Stops

Stops follow the records. `RoomStore::observe` calls
`HarnessRunner::reconcile` after every applied batch, with the store lock
released. A harness whose record is gone, or whose room is archived, is
stopped. `harness_stop` is the explicit way. An unmount stops nothing.

`ensure_running`, the public `stop_harness` and `release_events` take one
gate per harness, so calls for the same harness run one at a time. A stop
that arrives while the harness is still starting waits, then stops what
was started. `reconcile` takes no gate, because a spawn's own record
write-back re-enters the observer on the thread that holds it, and the
observer must not wait for a spawn. It reads the current records
(`store.rooms()`), not the batch it was handed, so observers that run out
of order reach the same answer. After a spawn, `ensure_running` looks the
record up again and stops the process if the harness was removed or
archived meanwhile, which covers what `reconcile` could not see yet.

### Client side

- `harnessEnsure.ts` drains the pending room mutations, calls
  `harness_ensure_running` with `expectCmd`, and retries a bounded number
  of times (4 attempts) on `stale_record` and `unknown_harness`.
- `harnessMounts.ts` sets `restart` only when the mount key changed in
  this webview: Restart harness, Enter for shell and repoint (since #752 the
  mount key is derived from the spawn argv, so a launch change moves it). The key is
  read with `restartFor` and recorded with `commitMount` only when the
  ensure call is actually dispatched (`harnessEnsure.ts`), so a React
  StrictMode double mount cannot swallow a restart.
- `useTerminalSpawn.ts` detaches on unmount. Since slice 4 it sends no
  phase input for a re-attached harness: the mirror already holds the
  daemon's record.
- `terminalRepaint.ts`: a cancelled repaint sends the restoring resize at
  once, so the PTY is never left one row short.
- `useRoomsHydrate.ts` mounts the rows as the daemon returns them.

### Known limits

- The agent verdict check before a spawn is still client-side
  (`terminalSpawnAgent.ts`, calling `list_harness_agents`).
- Output written while no view is attached is lost until #584.
- An unbound Hermes session capture stays client-side. The daemon tails
  Hermes actions only when the session id is already bound and the status
  hooks work. The `hermes_work` poller is the daemon's since slice 4.
- No agent API verb reaches `ensure_running` yet (slice 6).
- `release_events` re-attaches an opencode v1 adapter with the port of
  the spawn-time argv. After an opencode restarted from the post-exit
  shell (#517) was re-pointed by the client, a client letting go leaves
  the daemon's adapter on the old port until the next attach.
- A pending permission state is not carried across a re-attach. It moves
  with the phase machine (#732).

### What stays with the client

- `buildRemoteSpec`, when a `remote` harness is created.
- The agent verdict check before a spawn.
- The phase machine and `harness_events` (#732).
- The Hermes work poller and unbound Hermes session capture.

### Live verification

On 2026-11-12 an isolated dev instance on Windows was driven over CDP.
These passed:

- spawn of claude (with and without `--agent`), opencode, shell and
  files harnesses;
- tab and room switches with unchanged child PIDs;
- a webview reload re-attaching to the same PIDs (a plain shell also
  showed its earlier screen, because ConPTY redraws on the resize);
- closing a tab;
- Restart harness with `claude --resume`;
- the post-exit shell: nothing started for more than 10 s, Enter started
  a shell;
- archive and reopen;
- app restart with resume of the same session ids.

Not covered: macOS and Linux, Hermes, opencode v2, remote harnesses.

Observed, not a result of this slice: the Activity card read "0 events"
after a restart while `harness_actions` held rows (#740).

## Spawn-time argv (#752)

Since #752 the argv is derived when the harness spawns, not read from the
stored `cmd`. `spawn_argv` (`harness_argv/spawn.rs`, pure) takes the
record's `launch`, `kind`, `session_id` and `agent`, plus a session-exists
probe that `ensure_running` runs at spawn. The stored `cmd` text no longer
decides resume versus fresh or shell versus tool, so a stale `cmd` cannot
resume a conversation that is gone.

```mermaid
flowchart TD
    A["ensure_running"] --> B["gates: lookup, expectCmd and expectLaunch, existing process"]
    B --> C["SpawnPlan::from_record, probe injected"]
    C --> D{"launch is shell?"}
    D -->|yes| E["argv = the shell argv"]
    D -->|no| F{"kind"}
    F -->|Claude| G{"transcript exists?"}
    G -->|yes| H["claude --resume sid"]
    G -->|no or no id| I["claude --session-id sid, minted if none"]
    F -->|opencode, Hermes| J{"session exists?"}
    J -->|yes| K["resume this session"]
    J -->|no| L["fresh, never --continue"]
    F -->|"shell, remote, other"| M["remote argv or the stored cmd"]
    E --> N["spawn PTY"]
    H --> N
    I --> N
    K --> N
    L --> N
    M --> N
    N --> O["write back session id if minted, dropped or changed"]
```

Figure 7: how `ensure_running` turns a record into an argv. Renders this
section and crates/skein-daemon/src/harness_argv/spawn.rs; the text is
authoritative (non-normative).

1. The gates run first. `expectCmd` and the new `expectLaunch` are
   compared with the record and a mismatch is `stale_record`.
2. `SpawnPlan::from_record` calls `spawn_argv` with the probe. The probe
   runs at most once and only for a non-blank session id of a Claude,
   opencode or Hermes harness (opencode v2 excepted: its stored id is
   trusted, #679).
3. The argv is spawned. Claude with no id mints one. opencode and Hermes
   with no live session start fresh.
4. The minted or dropped session id is written back to the record. Since
   #755 that is one field-level edit under the store lock, and only if
   the stored id still equals what the spawn read (see "Daemon-owned
   fields (#755)").

Who writes `launch` (`{"mode":"tool"}` or `{"mode":"shell","argv":[..]}`;
absent means the kind's own tool):

| Event | Effect on `launch` |
| --- | --- |
| Enter on an exited pane (`withShellLaunch`, `harnessCreation.ts`) | Sets shell; `cmd` is left alone |
| Restart harness (`back_to_tool`, `plan.rs`) | Clears it: back to the tool |
| Boot preparation (#753), unarchive | Keep a shell launch |
| A live shell claim (#517; boot preparation, #753) | Clears it, the claimed session resumes |
| `migrate_launch` (`launch.rs`, first in boot preparation) | Infers shell once for old records whose `cmd[0]` is not the kind's program |

`cmd` is still stored. The client writes it on create, restart and
unarchive, and since #753 boot preparation writes it only to give an
opencode harness a fresh `--port` (or to reduce it to the program when a
shell claim goes back to its tool). It carries the opencode `--port`
(`stored_opencode_port` in `gates.rs`), is the argv for kinds that do not
resume, and feeds the `expectCmd` staleness check.

Two facts from the live check on 2026-11-13, both fixed by #753 on the same
day (see "Boot preparation after #753"):

- Boot resume dropped a Claude session id that had no transcript and
  minted a new one, so a never-prompted Claude harness got a new id on
  each app start. Boot preparation no longer touches session ids.
- Boot resume wrote `--continue` into an opencode harness's STORED `cmd`
  when no session id was known, though the spawned argv never carried it.
  Boot preparation writes no `--continue` now; restart and unarchive
  (`resume_harness`) still can write it into the stored `cmd`, and the
  spawn never passes it.

User decisions, 2026-11-13: Enter-for-shell is a launch mode, not a
rewritten `cmd`; boot and unarchive keep a post-exit shell (#745 part 1);
Restart harness returns a post-exit shell to its tool.

Seams for the follow-ups: #754 replaces `stored_opencode_port` with a
daemon-owned port; #755 and #754 together remove the stored `cmd` and the
`expectCmd` check (#753 reduced boot preparation but did not remove it).
#755 shipped the field-level write path and made `sessionId` daemon-owned,
but `cmd` is still stored and client-written; see "Daemon-owned fields
(#755)".

## Boot preparation after #753

#753 reduced the once-per-process preparation that "Boot resume" above
recorded as built in #731. `BootResume::load_rooms` still wraps every
rooms load and holds a mutex across the load and the preparation, so a
concurrent load waits and then reads prepared rows (#744). The lock order
is unchanged. The preparation is now only this:

1. `migrate_launch` runs once per room, over all its harnesses, running or
   not, and fills `Harness.launch` for records older than #752.
2. The shell claim (#517, #520), Claude and opencode only. A shell-mode
   harness whose `shellClaim.sessionId` is non-blank, equals `sessionId`
   and names a session that still exists goes back to its tool: `launch`
   and the claim are cleared and `cmd` is reduced to the program. This is
   the only probe boot still runs, and a probe error counts as existing. A
   claim that is not live is removed and the shell is kept. A Claude
   harness in tool mode just loses a leftover claim.
3. Each opencode harness in tool mode, in a room that is not archived, gets
   a fresh free port. Its stored `cmd` becomes exactly
   `opencode --port <p> --hostname 127.0.0.1`. The spawn
   (`harness_argv/spawn.rs`) adds `--session` and `--agent` from the
   record.

It no longer probes and drops stale session ids (the spawn does that and
writes the id back), mints Claude session ids, rewrites Claude, Hermes or
remote `cmd`s to resume form, or writes `--continue`. A never-prompted
Claude harness therefore keeps its session id across restarts.

The preparation counts as done only after it succeeded on a load that
returned at least one room, so an empty load does not mark it done. A
failure (a port could not be picked, or the store refused the write) is
logged, that load returns the unprepared rooms, and the next load tries
again. A retry skips every harness that is running, checked through the
runner, so a live process's record is never rewritten. The changes are
saved through `RoomStore::apply` under origin `daemon-boot` as
whole-harness `PutHarness` mutations, then flushed (since #755 they are
field-level edits instead, see "Daemon-owned fields (#755)"). The frontend still
reads the opencode ports from the stored `cmd` (`opencodePort.ts`), and
unarchive still runs `rooms_resume_cmds`.

Decision, 2026-11-13: full removal was deferred because the port and the
claim have no spawn-time home yet. #755 replaced the whole-harness writes
with field-level ones (shipped). #754 moves the port out of the stored `cmd` to the
runner at spawn, after which only `migrate_launch` and the claim step
remain.

Known limits:

- The running check asks whether the runner holds an entry for the
  harness, so a harness whose process exited but whose entry is still
  there is also skipped by a retry.
- The running check and the write are not atomic. A harness that spawns
  between a retry's snapshot and its `RoomStore::apply` can have its
  record overwritten by the snapshot: a new port in `cmd` while the
  process is on the old one, and a session id the spawn just wrote back
  lost. A retry only happens after an empty first load or a failed
  preparation, so this is rare, and it is the whole-harness-write class
  that #755 removes. Since #755 this limit is closed: the check and the
  write both happen under the store lock, and a stale plan is skipped.
  What is left is the first limit above, and a leftover entry still reads
  as busy.
- A pane that mounted before a retry holds the old `cmd` as `expectCmd`,
  so its first ensure after the retry can be refused as `stale_record`
  until the client applies the `daemon-boot` change.

```mermaid
flowchart TD
    A["load_rooms: take the barrier"] --> B["load rooms"]
    B --> C{"already done?"}
    C -->|yes| R1["return the rows"]
    C -->|no| D{"any rooms?"}
    D -->|no| R2["return the rows, not done"]
    D -->|yes| E["per room: migrate_launch over all its harnesses"]
    E --> F{"per harness: running?"}
    F -->|yes| G["skip this harness"]
    F -->|no| H["claim step, Claude and opencode"]
    H --> I["port step, opencode in tool mode"]
    G --> J{"pick failed?"}
    I --> J
    J -->|yes| K["log, return unprepared rows, not done"]
    J -->|no| L["apply under daemon-boot, per harness under the store lock (#755), then flush"]
    L --> M{"store refused?"}
    M -->|yes| K
    M -->|no| N["mark done"]
    N --> O["return the prepared rows"]
```

Figure 8: one rooms load through boot preparation. Renders this section
and crates/skein-daemon/src/boot_resume.rs; the text is authoritative
(non-normative).

## Daemon-owned fields (#755)

Before #755 the daemon wrote a harness record by sending a whole-harness
`PutHarness` through `apply`, and so did the client. Two writers that each
send the whole record overwrite each other's fields: a boot write could
lose a session id the spawn had just recorded, and a stale client put
could revert one. #755 gives the store a field-level edit path and makes
two fields the daemon's alone.

`RoomStore::edit_harnesses` and `RoomStore::edit_harness`
(`room_store/edit.rs`) run a closure per harness under the store lock. The
closure changes only the fields it means to. Everything after it is what
`apply` does for a changed `PutHarness`: dirty marking, a structural
persist when a key field changed, one `rooms-changed` event carrying the
origin, and the observers. A closure that changes nothing emits nothing.
No daemon code builds a whole-harness `PutHarness` any more.

The daemon-owned fields are `sessionId` and `opencodeVersion`. In `apply`
(`keep_daemon_owned`), a client `PutHarness` for an existing harness keeps
the stored values of both. A new harness, and a new room, are taken as
sent. On the client, `DAEMON_OWNED_HARNESS_KEYS` (`roomMutations.ts`)
makes `diffRooms` ignore them and makes a rebase (`roomsRebase.ts`) take
the daemon's value.

```mermaid
flowchart TD
    A["client: rooms_apply, PutHarness"] --> E["apply: keep sessionId and opencodeVersion"]
    B["client: harness_session_report"] --> F["report_session: capture, replace, clear"]
    C["runner: write-back after a spawn"] --> G["edit_harness"]
    D["boot preparation: plan applied per harness"] --> H["edit_harnesses"]
    F --> G
    E --> S["the store lock"]
    G --> S
    H --> S
    S --> P["dirty rows, structural persist"]
    S --> V["one rooms-changed event, origin says who"]
    V --> X["every client: rebase, daemon value wins for the two fields"]
```

Figure 9: the write paths into a harness record after #755. Renders this
section and crates/skein-daemon/src/room_store/{edit,apply}.rs; the text
is authoritative (non-normative).

1. The client sends a `PutHarness` through `rooms_apply`. For an existing
   harness the two owned fields are replaced by the stored values before
   the put is written.
2. The client reports a session id it observed through
   `harness_session_report`. It becomes one `edit_harness` call.
3. The runner records the spawn outcome with one `edit_harness` call.
4. Boot preparation applies its plan with one `edit_harnesses` call.
5. All four meet at the store lock, one writer at a time.
6. Dirty rows are written, at once for a structural change (`sessionId`
   is in the harness key, so a changed session id is one) and debounced
   otherwise.
7. One event goes out. Every client, the sender too for the report, sees
   the daemon's value arrive and the daemon's value wins.

### Who writes what

| Field | Written by | How |
| --- | --- | --- |
| `sessionId` | The runner, after a spawn | `edit_harness`, origin `daemon-runner`; only if the stored id still equals what the spawn read, else the newer id is kept (logged) |
| `sessionId` | The client, for what it observed | `harness_session_report`, origin `daemon-session`; modes below |
| `opencodeVersion` | The runner, at each opencode spawn | Same edit; the probed version, or none to clear a stale one |
| `launch` | The client; boot preparation | Client: `PutHarness` (Enter on an exited pane, Restart harness). Boot: `migrate_launch` and a live shell claim, by edit |
| `shellClaim` | The client; boot preparation | Client: `PutHarness` (#520). Boot: settles or clears it, by edit |
| `cmd` | The client; boot preparation | Client: `PutHarness` on create, restart, unarchive. Boot: the fresh opencode `--port`, by edit |

The `harness_session_report` command (`commands/harness_runner.rs`, over
`HarnessRunner::report_session`, logic in `harness_runner/record.rs`)
takes a room id, a harness id and a report with a `mode`:

- `capture`: first writer wins. It sets the id only when the record has
  none, or a blank one.
- `replace`: authoritative. The conversation moved to this id (Claude's
  hook, opencode's follow of its TUI, a post-exit-shell claim).
- `clear`: the stored id no longer applies (a repointed folder).

A blank id is ignored. It returns `true` when the record changed and
`false` for an unknown harness or no change. Nothing is applied locally:
the client's copy changes when `skein://rooms-changed` arrives, one round
trip later (`harnessSessionReport.ts`, `useHarnessWriteBacks.ts`). A
failed report is logged and not retried. The ensure-result write-backs of
the session id and the opencode version, and the restart and unarchive
writes of the session id, are gone from the client: the spawn derives and
records them.

### Boot preparation as edits

The plan is still computed outside the lock, from the snapshot the load
returned (`boot_resume/plan.rs`, `BootPlan`). It is applied under the
store lock, per harness, by `edit_harnesses` under origin `daemon-boot`.
For each harness, on the record as stored now, in this order:

- The launch migration is applied whenever the stored `launch`, `cmd` and
  `shellClaim` (what `migrate_launch` inferred it from) all still equal the
  snapshot's, even for a stale or busy harness (so a legacy shell-mode
  record whose spawn wrote a session id in between still gets its
  `launch`). A client restart that rewrote `cmd` or removed the claim is
  not migrated over.
- Stale: `shellClaim`, `cmd` or `sessionId` differs from the snapshot, or
  `launch` had already changed. Nothing more is written, the plan is
  skipped (the harness counts as stale even if it got its migration).
- Busy: the runner has an entry, or the harness's spawn gate is held
  (`HarnessRunner::is_busy`). Only the `launch` migration is written.
- Otherwise `launch`, `shellClaim` and `cmd` are written.

It never writes another field, `sessionId` least of all. The preparation
logs how many harnesses went each way. The earlier non-atomic running
check is gone, because the check now runs under the lock.

### Lock order

The boot barrier comes first, then the store lock. Inside an edit the
runner's `entries` and gate map are taken briefly, through `is_busy`,
which only try-locks a spawn gate and never waits for one. The runner
calls the store while holding a per-harness gate, but never while holding
`entries` or the gate map. So the order has no cycle: an edit under the
store lock never waits for a gate, and a gate holder never waits on
`entries` while calling the store. An edit closure must not call back into
the store and must not block.

### Known limits

- `cmd`, `launch` and `shellClaim` are still client-written through
  `PutHarness`. A stale client put can overwrite a daemon write to those
  three: the boot's fresh port in `cmd`, a cleared claim. The boot's stale
  check protects the other direction only. #754 takes the port out of
  `cmd` and #756 adds intent commands with a revision.
- The window between a session report and its `rooms-changed` event is
  narrowed by `app/src/reportedSessions.ts`: the client's capture/follow
  decisions and the claimed-session set use a reported id until the
  record catches up, the daemon refuses it, or 2 s after the daemon
  accepted it. What is still possible: a report that fails is logged and
  not retried, so that observation is lost until the next event.
- The shell claim and the session id it names are two writes, a put and
  a report. They are briefly inconsistent, and a failed report leaves a
  claim whose session id does not match, which boot preparation just
  drops.
- A busy check that sees an exited process's leftover runner entry still
  skips the harness (unchanged from #753).
- Restarting a Claude harness that has no transcript yet writes a stored
  `cmd` whose `--session-id` differs from the record's `sessionId` (seen
  in the live check on 2026-11-13): the resume builder mints an id for
  the `cmd` text, and the client no longer copies it into `sessionId`.
  The spawn derives its argv from `sessionId`, never from the stored
  `cmd`, so the process uses the record's id. It ends when the stored
  `cmd` goes (#754 and after).

### Seams for the follow-ups

- #754 (C) moves the opencode port out of `cmd` to the runner; boot's port
  step then has no write left (see "Boot preparation after #753").
- #756 (E) adds intent commands, each with a revision. The edit path is
  what they call: one closure per harness under the store lock, with the
  event, persistence and observers already shared.
- #758 (G) builds on the same edit path.
- `harness_session_report` is the first intent-shaped command: the client
  says what it saw, the daemon decides what the record becomes.

## Slice 4 as built (#732)

The daemon's `PhaseService` is the authority for harness phase. The
webview runs no phase machine. `app/src/harnessActivity.ts` is a mirror
of the daemon's stream (`app/src/phaseMirror.ts`) with the read surface
the old store had: `get`, `phaseSnapshot`, `subscribe`,
`subscribeTransitions`, `subscribeDelegationDeferred`, the hooks and the
labels. `app/src/subagents.ts` and `backgroundTasks.ts` serve the
daemon's live lists. Badges, toasts, OS notifications and status labels
stay in the frontend, driven by the mirrored transitions.

The code is `crates/skein-daemon/src/harness_phase/`. Tests sit in
`tests/` subfolders beside the files they cover.

| File | The question it answers |
| --- | --- |
| `machine.rs` | What is the phase of each harness, and what does a tick change? (`PhaseMachine`) |
| `lifecycle.rs` | What do spawn, exit, attach, detach and a session switch do? |
| `io.rs` | What do PTY output, a keystroke and a submitted prompt do? |
| `permission.rs` | When does a permission dialog open, and what clears it? |
| `deferral.rs`, `work.rs` | Is an end of turn deferred while subagents or background tasks still run, and until when? |
| `screen.rs` | What does the rendered Hermes screen say (`waiting`, `running`, `unknown`)? |
| `supervisor.rs`, `supervisor_state.rs`, `invariants.rs` | Is the machine's state consistent with the facts, and what repairs a stale one? |
| `subagents.rs`, `background_tasks.rs` | Which delegated work is outstanding per harness? |
| `patterns.rs` | Does the screen tail end in a waiting prompt? |
| `constants.rs`, `types.rs` | The thresholds and the vocabulary (phase, source, transition) |
| `translate/` | What does one Claude, opencode or Hermes event mean for the machine? |
| `service.rs` | Who owns the clock, the lock and the tick thread? (`PhaseService`) |
| `stream.rs` | What does the service publish, and who writes the log rows? (`PhaseUpdate`, the `skein-phase-out` thread) |
| `transcript_sampler.rs` | How often is a Claude transcript statted for the supervisor? (`TranscriptSampler`) |
| `sinks.rs` | How do adapter events reach the service beside a client's own sink? (phase sinks, `Tee`) |
| `client_input.rs` | What does the webview still have to tell the daemon? (`ClientPhaseInput`) |

Pieces outside it: `pty/observer.rs` (`PtyObserver`),
`harness_runner/phase_tap.rs` (the runner's observer),
`harness_runner/events.rs` (the phase sinks the app tees, and
`release_events`), `agent_api/phase_tap.rs` (the hook routes),
`harness_work_hermes/attacher.rs` (the Hermes work poller) and
`daemon.rs` (the facts closure for the tick). In the app,
`commands/harness_phase.rs` has the two commands, `harness_phase_input`
and `harness_phase_snapshot`.

### The port

The machine is pure. Time is injected as `now_ms`, and it has no
globals, threads or I/O. The TypeScript listeners became outboxes:
`drain_transitions`, `drain_changed` and `drain_delegation_armed`.

The vitest tables were ported as the spec before the machine was
written to pass them. There are about 350 `#[test]` functions under
`harness_phase/`. The cases that test code staying in the webview are
marked `// NOT PORTED` with the reason: status labels and room
aggregation (display), the `canSendPrompt` gate (send), and the mocked
`invoke` and `Channel` plumbing assertions.

The workspace has no regex dependency. The eight waiting-prompt regexes
of the old `harnessPatterns.ts` are a hand-written matcher
(`patterns.rs`) that reproduces the JavaScript semantics they use: ASCII
case folding, `$` at line terminators, JavaScript `\s` and ASCII `\b`.

`PhaseService` is the driver. It holds the machine and the translators
behind one `std::sync::Mutex`. The clock is read and the facts are
gathered before the lock is taken. A poisoned lock is taken over. One std
thread, `skein-phase-tick`, calls `tick_once` every 1 s: the machine's
tick, then the supervisor. It holds only a `Weak` and ends when the
service is dropped, and a panic in one pass is logged and the next one
runs.

### The output side

After every call and every tick the service builds a `PhaseUpdate` for
each changed harness (`stream.rs`):

| Field | Meaning |
| --- | --- |
| `seq` | One process-wide counter |
| `harnessId`, `roomId` | Which harness |
| `view` | A `PhaseView`, the part of the record that goes to clients; `null` means forgotten. The machine's own `HarnessActivity` is never serialized |
| `subagents`, `backgroundTasks` | The live lists |
| `transitions` | The phase changes in this update, oldest first |
| `delegationArmed` | The #277 deferral armed in this update (#381) |

The update goes onto an unbounded channel under the lock. One thread,
`skein-phase-out`, drains it in order. For each update it writes one
`harness_events` row per transition, then emits `skein://harness-phase`.
No database write or emit happens under the phase lock, and rows and
events keep lock order. Each message is handled under a panic guard: a
panic in the event sink or the database layer is logged (target
`skein::phase`) and the thread keeps going. A failed channel send is
logged once.

`PhaseView` has two groups of fields. Display and notifications:
`phase`, `phaseSince`, `spawnedAt`, `hasUserInput`, `permissionTool`,
`permissionAgentType`, `delegatedCount`, `lastTurnSignal`,
`lastSubmitAt`. The send gate: `authoritative`, `adapterHeard`,
`adapterSilent`, `launchSignalAt`, `lastOutputAt`, `questionOpen`,
`delegationDeferredAt`, `injected`. The second group exists only
because `canSendPrompt` and mail delivery still decide in the webview;
it leaves the wire with #733, which moves them into the daemon. A test
pins the exact key set, so adding a field is a deliberate act. Today
`PhaseView` lives in the daemon crate (`stream.rs`); moving the phase
wire types to `skein-proto` belongs with the transport work.

The throttle compares views, so a change to a field that is not on the
wire makes no update. An update that differs from the last one only in
`lastOutputAt` waits for the tick. Anything with a transition or an armed deferral goes out
at once.

`harness_phase_snapshot` gives a freshly loaded webview the current
state, one update per known harness at the current `seq`. The mirror
listens first and buffers. It applies the snapshot, then the buffered
updates with a higher `seq`, then applies live updates directly. A failed
snapshot, and the listen, are retried for as long as the app runs (1 s
apart for the first five attempts, then every 5 s) while live updates
apply. The mirror drops any update whose `seq` is not above the last one
applied for that harness. `onFirstSnapshot` fires once, when the first
snapshot is applied. Mail delivery (`useMailDelivery.ts`) and the Claude
version watch (`useClaudeVersionWatch.ts`) re-run their checks on it,
because a reload produces no transition for a harness that is already
`waiting`.

The mirror stamps `lastOutputAt` locally from its own PTY output
(`stampOutput`), so the send gate does not read the throttled value. It
does not stamp an `exited` record. The screen-reading feed
(`screenPhaseTick.ts`) re-sends an unchanged reading after a respawn.

The app writes no `harness_events` rows. `db_record_harness_event` and
`useTransitionLog.ts` are gone. The `user-restart` and
`version-auto-restart` rows are written by the daemon when it hears the
client input `restartRequested`. That row is not a machine transition: it
changes no state and is not on the stream.

### The inputs

| Group | Input | How it reaches the machine |
| --- | --- | --- |
| Daemon | Spawn, restart, stop | `HarnessRunner` calls `spawned` through the observer, and `forget` on a stop or a failed restart. A restart's `spawned` clears the old record itself. A retired observer drops the old process's late output and exit |
| Daemon | Every PTY output chunk and the exit | `PtyObserver` on the `SinkSlot`, whether or not a view is attached |
| Daemon | Claude and opencode adapter events | The adapters the runner attaches deliver into the phase sinks |
| Daemon | Hermes work events | `HermesWorkAttacher` attaches the poller itself: at spawn when the record has a session id, otherwise from the `on_session_start` hook. A late hook attaches no poller when the harness's record is absent or `exited` |
| Daemon | Claude `PermissionRequest` hook | `/api/harness/permission` calls `phase_permission`. It is dropped in post-exit-shell mode unless the stored shell claim accepts it. Both are read from the record. The webview has no listener for it |
| Daemon | Hermes status hooks | `phase_hermes_hook` into the Hermes translator |
| Client, teed | Claude and opencode event channels | The app attaches its own channel with `Tee::new(client sink, phase sink)`. The new attach replaces the daemon's adapter, so the phase sink goes with it |

The variants of `ClientPhaseInput` go through `harness_phase_input`
(`{harnessId, input}`), one tagged enum. Each is something only the
client can know. The call is fire-and-forget: `phaseInput.ts` swallows
any failure. The command is `async` so a keystroke does not run on the
main thread.

| Variant | Sent when | Why only a client knows |
| --- | --- | --- |
| `input` | A keystroke in the xterm, with the empty-composer flag | The daemon sees PTY bytes, not xterm's `onKey`, and not whether the composer was empty |
| `promptSubmitted` | A prompt goes through the #238 seam | The seam is a webview paste |
| `muteInducedOutput` | The client is about to cause a redraw (focus) | Only the client knows it caused the output |
| `screenReading` | The rendered Hermes screen reads `waiting`, `running`, `unknown`, or null to clear (#623) | Only the xterm buffer has the rendered screen |
| `hermesLaunchPoll` | One 500 ms Hermes launch poll, with whether the screen read `waiting` (#625) | Same: it reads the screen |
| `launchSignal` | The Claude CLI reported its own launch (`SessionStart`) | The hook decision depends on shell-claim and session-follow state that stays client-side |
| `sessionSwitched` | The tail was re-pointed: `clear`, `resume`, `fork` or `startup` (#116, #539) | Same |
| `recoverUnheardSilence` | A mail nudge's lost first paste (#404) | The paste was the webview's |
| `startFailed` | A start produced no process (a refused agent, a failed ensure call). Wire: `{"kind":"startFailed"}` | The client holds the agent verdict check. The record becomes `exited` |
| `restartRequested` | The user or the version auto-restart asks for a respawn. Wire: `{"kind":"restartRequested","source":"user-restart"\|"version-auto-restart"}` | The client decides to restart. The daemon writes the row |

Inputs enter through `HarnessRunner::client_phase_input`
(`harness_runner/client_input.rs`), which `harness_phase_input` calls.
`startFailed` and `restartRequested` carry no room id. The entry point
drops both unless the harness is in a stored room, and uses the stored
room id for the record or row. It also drops `startFailed` while the
harness has a live PTY or a spawn in progress. These two can create a
record or a row, so the daemon does not trust a client's ids. All other
inputs pass straight to `PhaseService`.

### Deviation from D2

D2 says the daemon sees stdin writes itself and the client only adds the
empty-composer fact. As built, the client reports each keystroke as an
input (the data and the flag), and the daemon does not tap stdin. The
machine counts only keystrokes (xterm `onKey`), while the PTY's stdin
also carries pastes, mail nudges, focus reports and terminal replies.
Tapping all writes would change the decisive-input rule.

Likewise the Claude `SessionStart` hook's launch signal and session
switch are forwarded by the client (`app/src/useHarnessHookEvents.ts`),
and the hook route does not feed them. The shell claim, the session
follow and the startup adoption stay client-side.

### The double attach

The double attach stays for Claude and opencode, on purpose. The webview
still attaches its own event channel (`claude_events_attach`,
`opencode_events_attach`), teed into the daemon's phase sink, because
consumers that are not phase remain in the webview. When the client
detaches, `release_events` hands the adapter back to the runner.

| Consumer | Channel it keeps | File |
| --- | --- | --- |
| Claude `cli_version` (version notice, auto-restart) | Claude event channel | `claudeVersionStore.ts`, `harnessEventsClaude.ts` |
| opencode session capture and follow, observed agent label | opencode event channel | `harnessEventsOpencode.ts`, `opencodeShellFollow.ts` |
| Hermes session capture, `hermes_actions_turn_end` | Hermes hook event (`HERMES_HOOK_EVENT`) | `harnessEventsHermes.ts` |

Hermes work is different. The daemon attaches the work poller itself
(`harness_work_hermes/attacher.rs`) and the webview no longer consumes
it.

### Mail delivery

Mail delivery (#329, #381) stays client-triggered until #733. It works
through the mirror: transitions to `waiting`,
`subscribeDelegationDeferred`, the `canSendPrompt` gate (it reads the
mirrored record) and `recoverUnheardSilence`. That one is a
fire-and-forget input now. The existing 2 s mail retry re-evaluates the
gate once the mirror holds the daemon's answer.

### Agent verbs

The `harness_phases` round trip is deleted. The agent verbs read
`PhaseService` (`agent_api/verbs/room_listing.rs`, `harness_snapshots`).
A harness the service has no record of reads `unknown`, with null counts.
`list_rooms`, `get_room` and `list_harnesses` no longer depend on a
webview.

### The supervisor

The supervisor runs in the daemon on the tick. Its effects log under
`skein::supervisor` at the invariant's own severity: a violation at info,
warn or error as the invariant defines, `cleared` at info, a recovery at
info, and its outcome at info when applied and warn when not. The line
shape is the TypeScript supervisor's (`supervisor harness=<id> code=<code>
violation evidence=...`). The facts for the tick come from
`daemon.rs` (`start_phase_ticker`):

- Whether an adapter is attached, per harness, from the three adapter
  registries.
- Claude transcript samples from `transcript_sampler.rs`: one stat per
  Claude harness every 15 s, the newest 8 kept. They reset when the
  session id changes, the harness is respawned, or the attach stops being
  Claude. They feed `tail_silent_file_growing`.

`mail_held` still gets no facts. Mail moves in #733.

### A behaviour change

The daemon attaches its adapter at spawn. A fresh Claude harness
therefore reads `spawning` until its launch signal, and then `waiting`.
The old webview machine in a dev build often saw PTY output first and
read `running`. When a folder-trust dialog delays the `SessionStart`
hook, a 15 s launch-silent fallback (`LAUNCH_SILENT_AFTER_MS`) moves the
harness on.

```mermaid
flowchart LR
    H[Harness process]
    T["Daemon taps"]
    S["PhaseService"]
    O["Output thread"]
    D["harness_events"]
    M["Webview mirror"]
    F["Frontend consumers"]
    I[harness_phase_input]
    A["Agent verbs"]
    H -->|"1 output, exit, transcript, hooks"| T
    T -->|"2 events and hooks"| S
    S -->|"3 updates"| O
    O -->|"4 one row per transition"| D
    O -->|"5 skein://harness-phase"| M
    M -->|"6 mirrored state, transitions"| F
    F -->|"7 keys, screen, signals"| I
    I -->|"8 client inputs"| S
    A -->|"9 phase and work reads"| S
```

Figure 10: the daemon decides the phase, publishes it, and the webview
mirrors it. Renders section "Slice 4 as built" and
crates/skein-daemon/src/harness_phase/; the text is authoritative
(non-normative).

1. The harness process writes output and eventually exits. The PTY
   observer hears both, whether or not a view is attached. The adapters
   tail the tool's own transcript or event stream, and the hook routes
   hear the CLIs' hooks.
2. The taps feed `PhaseService`: PTY output and exit, adapter events
   through the phase sinks (the webview's teed Claude and opencode
   channels reach the same sinks), the Claude permission hook (gated on
   shell mode and the stored shell claim) and the Hermes hooks and work
   poller. A tick every second runs the machine and the supervisor.
3. After each call and each tick the service hands one `PhaseUpdate` per
   changed harness to the output thread, under the lock and in order.
4. The thread writes one `harness_events` row per transition. The
   daemon also writes the restart row here.
5. Then it emits `skein://harness-phase`. A freshly loaded webview first
   reads `harness_phase_snapshot` and drops updates at or below its `seq`.
6. The mirror holds the latest view, subagents and tasks per harness
   and notifies the frontend consumers: badges, toasts, OS notifications,
   status labels and mail delivery.
7. The frontend observes what the daemon cannot: keystrokes, the rendered
   Hermes screen, a submitted prompt, the focus mute, the Claude launch
   and session decisions, a failed start and a restart request.
8. It forwards those through `harness_phase_input` and the runner's
   `client_phase_input`, which validates the two that name a record or a
   row, then the service applies them.
9. The agent verbs read phase and outstanding-work counts from the
   service directly.

### Known limits

- With no client attached, the Hermes screen phase reads `unknown`
  (#584). The machine treats "no client reporting" as an `unknown`
  reading.
- `mail_held` gets no facts until #733.
- The shell claim reaches the daemon through debounced room saves
  (`shellClaim` and `cmd` are not in the structural key). A permission
  ping within that window can be gated against the old claim.
- The mirror lags the daemon by one update. A read straight after a
  forwarded input sees the old record.
- An update that changes only `lastOutputAt` reaches the mirror within
  about 1 s.
- The screen reading is forwarded only while the webview samples it, and
  only when it changes (`screenPhaseTick.ts`).
- `startFailed` and the restart row depend on a client. With none, a
  refused start leaves no `exited` record and a restart writes no row.
- During the attach swap, two sources can briefly feed one translator:
  the daemon's adapter until the client's attach replaces it, and the
  client's channel after.
- After a relaunch that resumes a Claude conversation,
  `tail_silent_file_growing` can fire once and clear about 20 s later
  without anything being wrong. On resume Claude appends a small row that
  produces no adapter event (2026-11-13: two resumed Claude harnesses,
  each `grewBytes=267 silentFor=61999ms`, cleared after 17 s and 21 s).
  The invariant only logs. The flipped build confirmed the row is
  `bridge-session`.
- After `/exit` and Enter for shell, the harness still reads `waiting`: the
  Claude adapter is attached by kind, not by the spawned program. This
  pre-dates #732.
- `permission_orphaned` can warn on a dialog that is still open (a
  transcript row for the awaiting tool counts as later work). It only
  logs.
- A resumed harness reads `waiting` from the replayed end of turn before
  its TUI is ready. The launch-settle rule (`launchReady.ts`) guards the
  send gate.

### Live verification

Shadow runs, 2026-11-12. Before the flip, an isolated dev instance on
Windows was driven over CDP twice with the Rust machine running beside
the TypeScript one and a comparator logging the differences.

- Run 1: a Claude turn, a permission dialog, a short subagent,
  `/context`, an opencode busy/idle cycle, a shell, a reload, `/exit` and
  an app restart. 42 transitions paired, 14 did not. Three were the
  webview's rebuild rows after the reload. Eleven were attach order: the
  daemon attaches at spawn and goes `spawning` to `waiting`, where the
  TypeScript machine saw output first and went to `running`. One launch
  took the 15 s launch-silent fallback behind a folder-trust dialog.
- Run 2 (after fixes): a fresh Claude launch, a turn, a background
  subagent and a background task that outlived the turn, a reload, Restart
  harness, a shell and an opencode cycle. 21 paired, 4 did not: two
  attach order, one a reload losing the TypeScript side's deferral, one a
  comparator miss on a real restart.
- Run 3 (the part 1 build, by another room on 2026-11-13; three launches:
  fresh, normal relaunch, kill and relaunch): 9 unpaired transitions, all
  of the attach-order class above. The webview's machine went `spawning`
  to `running` on PTY output and then read `running` for an idle Claude,
  where the Rust machine went `spawning` to `waiting` on the launch
  signal, or after the 15 s launch-silent fallback behind a trust prompt.
  Two `tail_silent_file_growing` lines also appeared (see Known limits).

The end states agreed in every case. Not covered live: a subagent's own
permission dialog, a background task reaching its ceiling, a real
opencode reply, Hermes, opencode v2, remote harnesses, macOS and Linux.
The comparator and its target (`skein::phase_shadow`) are deleted.

#### After the flip

On 2026-11-13 an isolated dev instance on Windows (own profile, Claude Code
2.1.323) was driven over CDP on the flipped build, across three launches:
fresh, normal close and relaunch, and kill by PID and relaunch. One room on
a scratch worktree held a Claude harness, a shell harness and a second
Claude harness. 54 `harness_events` rows were written, all by the daemon.
No row was duplicated (same harness, from, to and source within 1 s). The
log held no ERROR line and no panic.

| Case | Result |
| --- | --- |
| Fresh Claude launch | One row `spawning -> waiting` (`l2c1-claude-session-start`). The UI read waiting and never `running`. |
| A short turn | `waiting -> running` (`l2c1-claude-user-prompt`) and `running -> waiting` (`l2c1-claude-end-turn`), once each. |
| `/context` | No row. The UI stayed waiting. |
| Main-session permission dialog (manual mode) | `running -> permission` (`l2c1-claude-permission-request`), UI "permission needed · Bash". A keystroke cleared it (`permission -> running`, `user-input-permission`), then the end of turn. |
| Webview reload with a dialog open | The UI showed permission again from the snapshot. The reload wrote no row, the terminal repainted, and the answer cleared it. |
| A subagent's own dialog | UI "permission needed · general-purpose · Bash". Cleared by the keystroke, not by the subagent's tool result, which this run did not exercise. |
| A background shell command outliving the turn | UI "background · 1 task" for about 70 s, no `waiting` meanwhile. Then `running -> waiting` on the session's own next end of turn. No `delegation-settled` row was needed. |
| Shell harness | `spawning -> running` (`pty-output`), `running -> idle` (`l2a-idle`), back to `running` on new output. |
| Agent API `list_harnesses` from the shell harness | Phase and outstanding counts matched the UI in four samples, one of them with a live background task. |
| Mail (`send_message` from the shell harness to the waiting Claude harness) | Delivered and a turn started in all three tries: plain, sent just before a reload, sent two seconds after a reload. |
| Restart harness | Exactly one `<previous> -> spawning` row with source `user-restart`, then the launch rows. |
| `/exit` | `waiting -> exited` (`pty-exit`). |
| Normal close and relaunch, and kill and relaunch | Every harness got its phases from the daemon. The resumed Claude harnesses read waiting. Nothing failed to start. |

Seen in this run:

- On a resume (Restart harness, app relaunch) the first row is
  `spawning -> waiting` with source `l2c1-claude-end-turn`, 20 to 65 ms
  after the spawn. It is the replay of the transcript's last end of turn at
  attach, before the TUI has painted. The send gate's launch-settle rule
  (`app/src/launchReady.ts`) keeps a paste from going in that early.
- After `/exit` and Enter for shell, the pane is a plain shell but the
  harness reads `waiting` (the same replayed row), and stays so through
  Restart harness and a relaunch. The Claude adapter is attached by harness
  kind, not by the spawned program
  (`crates/skein-daemon/src/harness_runner/adapters.rs`, `attach_claude`).
  The webview's attach made the same decision before the flip
  (`hasClaudeTranscriptTail`: kind and a session id), so this is not new
  with #732. Not fixed here.
- `permission_orphaned` (arm `turn_moved_on`) was logged at WARN about 11 s
  into two dialogs that were still open, and cleared when they were
  answered. The rule is unchanged from the TypeScript supervisor: a
  transcript row for the tool awaiting the dialog counts as a later work
  signal. It has no recovery, so it only logs.
- `tail_silent_file_growing` was logged once after each relaunch for the
  resumed Claude harness (`grewBytes=267` and `268`, about 60 s after the
  resume). Claude appends a `bridge-session` row at resume, which produces
  no adapter event.
- A webview reload made the shell harness go `idle -> running` once
  (`pty-output`): the re-attach repaint's resize made the shell redraw.
  Fixed after the run: the repaint now sends the induced-output mute before
  each resize (`app/src/terminalRepaint.ts`).

Not covered live: opencode, Hermes, remote harnesses, macOS and Linux; the
delegation and background-task ceilings; a subagent dialog cleared by the
subagent's own tool result; an unknown agent name (`startFailed`).

After this run the stream's payload was narrowed from the machine's record
to `PhaseView` and the repaint mute was added. A shorter second run on that
build is recorded below.

Second run, same day and setup, fresh profile, two launches: a fresh
Claude launch, a turn, a main-session permission dialog with a reload
while it was open, a shell harness reloaded twice while idle, the agent
API, mail, Restart harness, a background task outliving the turn, and a
normal close and relaunch. 22 rows, none duplicated, no ERROR line. All
nine cases behaved as in the first run. The two reloads wrote no row for
the idle shell. The `user-restart` row carried the room's own id. The
status text, the permission tool name and the "background · 1 task"
label arrived through `PhaseView`. `permission_orphaned` was logged once
more, on the dialog that stayed open across the reload.

Observed, not a result of this slice: after an app kill and relaunch a
Claude harness was spawned with `--session-id` for a transcript that
already existed and exited with "Session ID ... is already in use", with
no earlier claude process alive; after `/exit` and Enter for shell, the
harness came back as a shell on relaunch and Restart harness also
started the shell (both fixed by #752: see "Spawn-time argv"); New Room on a linked worktree's path resolved to the
main checkout with a trailing slash and tailed the wrong transcript
directory.

## What #583 assumes of #584 and #585

Not designed here. #583 assumes:

- An event stream with a sequence number, so a client can resume from
  the last event it saw. Slice 1's change events and slice 4's phase
  events ride on it.
- PTY attach with a snapshot and a ring buffer. #583 only defines
  "detach does not kill".
- Daemon-host paths: state, logs and settings live on the daemon's
  host (the paths seam).
- N clients. Slice 1's mutations must be safe for two writers; the
  client-only inputs of D2 must name the client they came from.

## Risks

- **Frontend regressions with thin coverage.** Nearly every shipped
  regression has lived in the frontend. Mitigation: state first, each
  slice ships alone, no visible change in attached mode.
- **Phase-machine port drift.** Two implementations can disagree.
  Mitigation: ported vitest tables as the spec, then a shadow run that
  logs every divergence before the flip.
- **ConPTY cursor-query with no terminal.** Resolved in slice 3. While
  no view is attached the daemon answers `ESC[6n` with `ESC[1;1R`
  itself (`pty/sink_slot.rs`); a test with a real child on Windows
  (`pty/sink_pty_tests.rs`) shows output arriving once a view attaches.
  See "Slice 3 as built".
- **Hermes screen phase.** Without a client it reads `unknown` until
  #584's headless emulator exists. A known limit.
- **localStorage defaults migration.** `defaultAgents` moves to
  daemon settings. A one-time import must not overwrite a value the
  daemon already has, and must not lose one the user set.
- **Two writers during the slice 1 transition.** Resolved in slice 1.
  `db_save_rooms` is removed, so the client cannot write rows. Every
  write goes through the daemon's `RoomStore`, which writes per row.
  `Database::save_all` is left for migration and tests only.
- **Post-exit shell unattended.** Resolved in slice 3 by policy (D5a
  below): a harness that exits stays exited and the daemon starts
  nothing. The post-exit shell starts only when a view asks, so
  `opencodeShellFollow.ts` stays a client concern.
- **File-size limits.** New Rust files aim for 600 lines or fewer
  (CLAUDE.md). The phase machine port must be split by concern across
  several files and a sibling `tests.rs`.

## Acceptance mapping

| #583 acceptance item | Satisfied by |
| --- | --- |
| Attached mode: the app no longer builds argv or allocates ports | Slice 2 |
| Attached mode: the app no longer writes `harness_events` rows (the daemon does; `harness_actions` rows move in slice 3) | Slice 4 (with slice 3 for actions) |
| A no-client test creates a room, spawns a harness through the daemon API, and the phase and `harness_events` rows update | Slices 3 and 4 together (slice 3: `harness_actions` rows, tested by `harness_runner/tests/acceptance.rs`; slice 4: phase and transition log; room creation: slice 1; verbs: slice 6) |
| Two simulated clients write different rooms without erasing each other | Slice 1 |
| `cargo test --workspace`, `cargo test --manifest-path app/src-tauri/Cargo.toml`, `cd app && npm test` pass | Every slice |
| No change in what the user sees in attached mode | Every slice; slice 4's shadow run guards the flip |

The scope items map as follows: room persistence to slice 1; argv and
port to slice 2; action logging to slice 3; the transition log and
phase logic to slice 4; mail delivery to slice 5; agent API verbs and
token lifetime to slice 6. The scope item "daemon-host probes stay on
the daemon" needs no move: they already live in skein-daemon, and
slice 2 preserves them (its argv builder uses them).

## Child issue drafts

These are drafts. They are NOT filed. Sibling slices are referred to
as "slice N".

### Draft 1: skeind P2a-1: per-room mutations, daemon owns records

Part of #583. Depends on: #582.

**Why.** `db_save_rooms` wipes and re-inserts every room, so a second
client erases the first's rooms. A daemon with N clients needs
per-room writes.

**Scope.**
- Per-room and per-harness mutation commands in the daemon.
- Change events the client applies; `useRoomsStore` becomes a
  subscriber.
- Keep quarantine, snapshots and the structural-versus-debounced
  distinction as daemon behaviour.
- `save_all` kept only for migration.

**Out of scope.** Argv and spawn (slice 2, slice 3). The socket (#584).

**Acceptance.**
- Two simulated clients write different rooms without erasing each
  other.
- The #167 safety properties still have tests.
- No change in what the user sees.
- `cargo test --workspace`, `cargo test --manifest-path
  app/src-tauri/Cargo.toml` and `cd app && npm test` pass.

### Draft 2: skeind P2a-2: argv, ports and spawn defaults in the daemon

Part of #583. Depends on: slice 1.

**Why.** Argv, the opencode port and the defaults feeding them are
built in the frontend with the client's notion of platform. The daemon
must build them for its own host.

**Scope.**
- Port `cmdForKind`, `resumeCmd`, `withResumeCmds`,
  `unarchiveRoomTransform`, `remoteArgv` and `validateAgent` to Rust.
- Move `defaultAgents`, default shell and default cwd into daemon
  settings, with a one-time import from localStorage.
- Allocate the opencode port and own the sidecar at spawn.
- Stays as is: the daemon-host probes (login-shell probe, program
  resolution, agent enumeration, CLI version probe, the #565 login
  env, loopback in `NO_PROXY`) already live in skein-daemon. The new
  argv builder uses them.

**Out of scope.** Spawn and stop ownership (slice 3).

**Acceptance.**
- The app no longer builds argv or calls `pick_free_port`.
- The Rust argv tests cover every kind, resume and `--agent`.
- The localStorage import runs once and does not overwrite.
- The three test commands pass.

### Draft 3: skeind P2a-3: daemon-initiated spawn and stop

Part of #583. Depends on: slice 1, slice 2.

**Why.** Spawn happens in a React mount effect and unmount kills the
PTY, so no harness can outlive a terminal view.

**Scope.**
- `ensure_running(harness)` and explicit `stop_harness`.
- Unmounting a terminal detaches; it does not kill.
- The daemon attaches the L2c adapters and writes the
  `harness_actions` rows. The transition log (`harness_events`) stays
  with the phase machine and moves in slice 4.
- Boot resume (probes, stale-session drop, repo-identity gate) moves
  to daemon start.
- Sink contract: detach does not kill. Buffering and snapshots stay
  with #584.
- A daemon-side answer to the ConPTY cursor query (unproven).
- Post-exit shell policy.

**Out of scope.** Ring buffer and snapshots (#584).

**Acceptance.**
- A test starts the daemon with no client, creates a room, spawns a
  harness through the daemon API, and `harness_actions` rows appear.
- Closing a terminal view leaves the PTY running.
- The app no longer calls `db_record_harness_action`.
- The three test commands pass.

### Draft 4: skeind P2a-4: phase machine in Rust

Part of #583. Depends on: slice 3.

**Why.** The phase machine is the largest piece of lifecycle logic
still in the webview, and #587 needs it in the daemon.

**Scope.**
- Port the vitest tables to Rust tests first.
- Port the machine, deferral, registries, watchdogs, ceilings and
  supervisor invariants.
- Client-to-daemon inputs: decisive input with the empty-composer
  flag, focus mute, Hermes screen phase.
- Move the transition log (`harness_events`, today
  `useTransitionLog.ts`) into the daemon, written on each transition.
- Phase stream to clients; delete the `harness_phases` round trip.
- Shadow run with divergence logging, then flip, then delete the
  TypeScript machine.

**Out of scope.** Mail delivery (slice 5). A headless emulator (#584).

**Acceptance.**
- Ported tables pass in Rust.
- The shadow run logs no unexplained divergence before the flip.
- A no-client test shows the phase updating and `harness_events`
  rows written by the daemon.
- The app no longer writes `harness_events` rows.
- The three test commands pass.

### Draft 5: skeind P2a-5: mail and nudge delivery in the daemon

Part of #583. Depends on: slice 4.

**Why.** Delivery pastes through xterm `term.paste()`, so mail to a
waiting harness stops when no client is open.

**Scope.**
- Move the #238 / #381 gate and held-mail logic to the daemon.
- Bracketed-paste write through the daemon's PTY writer.

**Out of scope.** Approval queue and push (#587).

**Acceptance.**
- A no-client test delivers mail to a waiting harness.
- Held mail goes out on the transition to waiting.
- The three test commands pass.

### Draft 6: skeind P2a-6: client-less agent verbs and token policy

Part of #583. Depends on: slice 1, slice 3, slice 4.

**Why.** `create_room`, `open_harness`, `close_harness` and
`close_room` ask the webview, so an unattended director gets
"unavailable". Tokens are revoked only at daemon start.

**Scope.**
- Execute the four verbs in the daemon over slice 1 and slice 3.
- Read phases from the daemon's machine.
- `no_client` refusal for design verbs and the native screenshot.
- `close_harness` asks each attached client about dirty Files
  buffers; no client means proceed.
- Per-room token revoke on archive and close (D1).

**Out of scope.** Changing which verbs are refused by name.

**Acceptance.**
- With no client, the four verbs work and design verbs return
  `no_client`.
- Archiving a room invalidates its token; reopening mints a new one.
- The three test commands pass.
