// Live Context — Design canvas: sections of artboards exploring the
// right-pane card stack. Reuses live-context-rows and live-context-cards.

const { useState, useRef } = React;

// ─── Sample data drawn from the brief's recon examples ──────────

// "Where we are" — Claude's auto-generated away_summary.
const AWAY_SUMMARY_TEXT =
  "Implementing #80 Live Context — backend persists harness_actions; this PR adds the SQLite migration + Claude adapter.";

// Diff body — simulates an Edit on Cargo.toml from the brief.
const DIFF_CARGO = (
  <>
    <Line ctx n1={55} n2={55} src={<span>[<span className="tk-fn">target</span>.<span className="tk-fn">'cfg(target_os="macos")'</span>.<span className="tk-fn">dependencies</span>]</span>} />
    <Line ctx n1={56} n2={56} src={<span>core-foundation = <span className="tk-str">"0.10"</span></span>} />
    <Line ctx n1={57} n2={57} src={<span></span>} />
    <Line add n2={58} src={<span>[<span className="tk-fn">dev-dependencies</span>]</span>} />
    <Line add n2={59} src={<span>tempfile = <span className="tk-str">"3"</span></span>} />
    <Line add n2={60} src={<span>insta = {'{'} version = <span className="tk-str">"1.41"</span>, features = [<span className="tk-str">"json"</span>] {'}'}</span>} />
    <Line ctx n1={58} n2={61} src={<span></span>} />
    <Line ctx n1={59} n2={62} src={<span>[<span className="tk-fn">workspace</span>]</span>} />
  </>
);

function Line({ ctx, add, del, n1, n2, src }) {
  const kind = add ? 'add' : del ? 'del' : 'ctx';
  return (
    <div className={`lc-line ${kind}`}>
      <div className="gutter">
        <span className="ln">{n1 ?? ''}</span>
        <span className="ln">{n2 ?? ''}</span>
      </div>
      <span className="marker">{add ? '+' : del ? '−' : ''}</span>
      <span className="src">{src}</span>
    </div>
  );
}

// Plan groups for the alive scenario — two harnesses on same room.
const PLAN_GROUPS_ALIVE = [
  {
    harness: 'claude',
    items: [
      { status: 'done', text: 'Build harness_events_claude.rs adapter + tests' },
      { status: 'done', text: 'Migration 0014 — harness_actions table' },
      { status: 'now',  text: 'Wire backfill on attach' },
      { status: 'next', text: 'Tail-forward subscription channel' },
      { status: 'next', text: 'Drop deprecated activity_feed code' },
    ],
  },
  {
    harness: 'opencode',
    items: [
      { status: 'done', text: 'Read existing schema migrations',  priority: 'high' },
      { status: 'now',  text: 'Spec the opencode adapter shape', priority: 'high', inferred: true },
      { status: 'next', text: 'Add per-step snapshot hash plumbing', priority: 'med' },
    ],
  },
];

// Activity items — a mid-flight session, mixed harness, recent permission.
const ALIVE_ACTIVITY = (
  <>
    <TurnSep label="turn · 14:01:54" endTime="14:02:31 · 37s" />
    <EditRow harness="claude"   time="14:02:18" file="Cargo.toml"                        adds={3} />
    <EditRow harness="claude"   time="14:02:24" file="crates/skein-core/src/harness_events_claude.rs" adds={142} dels={4} />
    <ReadRow harness="claude"   time="14:02:26" file="docs/live-context-recon.md"        lines={612} />
    <BashRow harness="claude"   time="14:02:31" title="View issue #50 epic details" command="gh issue view 50" ms={812} />
    <TurnCost tokens="4,218" usd="0.18" ms="37.4s" />

    <TurnSep label="turn · 14:03:04" endTime="14:03:58 · 54s" />
    <ReadRow harness="opencode" time="14:03:04" file="crates/skein-core/src/harness_events_claude.rs" lines={146} />
    <TodoWriteRow harness="opencode" time="14:03:11" count={8} />
    <SearchRow harness="opencode" time="14:03:18" pattern="harness_action" matches={23} />
    <EditRow harness="opencode" time="14:03:29" file="crates/skein-core/src/lib.rs" adds={1} />
    <BashRow harness="opencode" time="14:03:42" title="cargo check -p skein-core" ms={3920} expanded
      output={"Checking skein-core v0.0.1 (/Users/.../skein-core)\nFinished `dev` profile in 3.91s"} />
    <TurnCost tokens="2,910" usd="0.11" ms="54.0s" />

    <TurnSep label="turn · 14:04:12" />
    <AskRow harness="claude" time="14:04:18" question="Release version?" chosen="v0.1.7" />
    <PermissionRow harness="claude" time="14:04:34" command="cargo test -p skein-core harness_events_claude" />
    <ActivityTail />
  </>
);

// ─── Composition: full right pane mocks (the way it'll look in Skein) ──

const RightPane_Alive = () => (
  <div className="lc lc-pane">
    <RoomSubtitle text={AWAY_SUMMARY_TEXT} age="2m ago · Claude" />
    <div className="lc-stack">
      <DiffCard flex={1}
        tabs={[
          { file: 'Cargo.toml', adds: 3, harness: 'claude', active: true },
          { file: 'lib.rs',     adds: 1, harness: 'opencode', flicker: true },
          { file: 'harness_events_claude.rs', adds: 142, dels: 4, harness: 'claude' },
        ]}
        body={DIFF_CARGO} />
      <div className="lc-divider" />
      <PlanCard flex={1} groups={PLAN_GROUPS_ALIVE} />
      <div className="lc-divider" />
      <ActivityCard flex={1} totals={{ events: 619, cost: '1.42', tokens: '18.4k' }}>
        {ALIVE_ACTIVITY}
      </ActivityCard>
    </div>
  </div>
);

// State: empty room
const RightPane_Empty = () => (
  <div className="lc lc-pane">
    <RoomSubtitle empty text="No agent has worked here yet" age="created just now" />
    <div className="lc-stack">
      <DiffCard flex={1} body={(
        <div className="lc-empty"><div className="lc-empty-inner">
          <div className="big">◌</div>
          when an agent edits a file in this room, it appears here
        </div></div>
      )} />
      <div className="lc-divider" />
      <PlanCard flex={1} groups={[]} />
      <div className="lc-divider" />
      <ActivityCard flex={1} totals={{ events: 0 }}>
        <div className="lc-empty"><div className="lc-empty-inner">
          <div className="big">·</div>
          activity will tail here as agents work
        </div></div>
        <ActivityTail idle />
      </ActivityCard>
    </div>
  </div>
);

// State: long quiet — the most recent activity was 2 hours ago
const RightPane_Quiet = () => (
  <div className="lc lc-pane">
    <RoomSubtitle text="Paused — Claude finished test coverage for the worktree adapter." age="2h 14m ago" />
    <div className="lc-stack">
      <DiffCard flex={1}
        tabs={[{ file: 'worktree.rs', adds: 38, dels: 6, harness: 'claude', active: true }]}
        body={DIFF_CARGO} />
      <div className="lc-divider" />
      <PlanCard flex={1} groups={[{
        harness: 'claude',
        items: [
          { status: 'done', text: 'cover the rename edge case' },
          { status: 'done', text: 'add property test for stale worktrees' },
          { status: 'next', text: 'pair with opencode on session-attach' },
        ],
      }]} />
      <div className="lc-divider" />
      <ActivityCard flex={1} totals={{ events: 47, idleFor: '2h 14m' }}>
        <TurnSep label="last turn · 11:48:02" endTime="11:48:54 · 52s" />
        <EditRow  harness="claude" time="11:48:09" file="worktree.rs" adds={38} dels={6} />
        <BashRow  harness="claude" time="11:48:21" title="cargo test worktree" ms={2104} />
        <TurnCost tokens="3,180" usd="0.13" ms="52.0s" />
        <ActivityTail idle />
      </ActivityCard>
    </div>
  </div>
);

// State: burst storm — single collapsed row showing 12 edits in 28s
const RightPane_Burst = () => (
  <div className="lc lc-pane">
    <RoomSubtitle text="Sweeping the codebase to rename HarnessEvent → HarnessAction." age="happening now" />
    <div className="lc-stack">
      <DiffCard flex={1}
        tabs={[
          { file: 'lib.rs',          adds: 14, dels: 14, harness: 'claude', active: true },
          { file: 'adapter.rs',      adds: 8,  dels: 8,  harness: 'claude' },
          { file: 'migrations.rs',   adds: 12, dels: 12, harness: 'claude' },
        ]}
        body={DIFF_CARGO} />
      <div className="lc-divider" />
      <PlanCard flex={1} groups={[{
        harness: 'claude',
        items: [
          { status: 'now',  text: 'rename HarnessEvent → HarnessAction across the workspace' },
          { status: 'next', text: 'regenerate the SQLx offline cache' },
        ],
      }]} />
      <div className="lc-divider" />
      <ActivityCard flex={1} totals={{ events: 41, cost: '0.32', tokens: '6.1k' }}>
        <TurnSep label="turn · 14:21:02" />
        <ReadRow harness="claude" time="14:21:04" file="rg.json" lines={612} />
        <BurstRow harness="claude" time="14:21:08" count={12} tool="edit" scope="crates/skein-core/src/**" adds={142} dels={37} live />
        <BurstRow harness="claude" time="14:21:36" count={8}  tool="edit" scope="crates/skein-app/src/**"  adds={68}  dels={22} live />
        <SearchRow harness="claude" time="14:21:54" pattern="HarnessEvent" matches={0} />
        <BashRow harness="claude" time="14:21:58" title="cargo build" ms={null} />
        <ActivityTail />
      </ActivityCard>
    </div>
  </div>
);

// State: errored harness — graduated treatment (inline → toast → status bar)
const RightPane_Errored = () => (
  <div className="lc lc-pane" style={{ position: 'relative' }}>
    <RoomSubtitle text="Anthropic capacity blip — Claude is retrying. opencode kept going." age="happening now" />
    <div className="lc-stack">
      <DiffCard flex={1}
        tabs={[
          { file: 'harness_events_claude.rs', adds: 142, dels: 4, harness: 'claude', active: true },
          { file: 'lib.rs', adds: 1, harness: 'opencode', flicker: true },
        ]}
        body={DIFF_CARGO} />
      <div className="lc-divider" />
      <PlanCard flex={1} groups={PLAN_GROUPS_ALIVE} />
      <div className="lc-divider" />
      <ActivityCard flex={1} totals={{ events: 623, cost: '1.44', tokens: '18.5k' }}>
        <EditRow harness="claude" time="14:05:02" file="harness_events_claude.rs" adds={4} />
        <ApiErrorRow harness="claude" time="14:05:18" status="HTTP 529" attempt={1} retryIn={8}
          message="overloaded — anthropic capacity" />
        <EditRow harness="opencode" time="14:05:24" file="lib.rs" adds={1} />
        <ApiErrorRow harness="claude" time="14:05:31" status="HTTP 529" attempt={2} retryIn={16}
          message="overloaded — anthropic capacity" />
        <ActivityTail />
      </ActivityCard>
    </div>

    {/* Graduated treatment: error appeared inline; while user was scrolled
        elsewhere it surfaced as this toast; if dismissed it stays in the
        room's status bar segment. */}
    <div className="lc-toast">
      <span style={{ color: 'var(--err)', fontFamily: 'var(--sk-mono)', fontWeight: 700 }}>✕</span>
      <div className="body">
        Claude · HTTP 529, retrying.
        <span className="dim">attempt 2 · retry in 14s</span>
      </div>
    </div>
  </div>
);

// State: permission as row — what happens when BYOH (or any harness) pauses for bash
const RightPane_Permission = () => (
  <div className="lc lc-pane">
    <RoomSubtitle text="BYOH harness paused for a bash command — waiting for you to approve." age="just now" />
    <div className="lc-stack">
      <DiffCard flex={1}
        tabs={[{ file: 'src/fs/watcher.rs', adds: 5, dels: 1, harness: 'byoh', active: true }]}
        body={DIFF_CARGO} />
      <div className="lc-divider" />
      <PlanCard flex={1} groups={[{
        harness: 'byoh',
        items: [
          { status: 'done', text: 'add debounce wrapper around notify channel' },
          { status: 'now',  text: 'run cargo test fs::watcher' },
          { status: 'next', text: 'update CHANGELOG with debounce default' },
        ],
      }]} />
      <div className="lc-divider" />
      <ActivityCard flex={1} totals={{ events: 8 }}>
        <ReadRow  harness="byoh" time="13:51:02" file="src/fs/watcher.rs" lines={38} />
        <SearchRow harness="byoh" time="13:51:14" pattern="recommended_watcher" matches={1} />
        <EditRow  harness="byoh" time="13:51:18" file="src/fs/watcher.rs" adds={5} dels={1} />
        <PermissionRow harness="byoh" time="13:52:01" command="cargo test --package skein-core fs::watcher" />
        <ActivityTail idle />
      </ActivityCard>
    </div>
  </div>
);

// ─── Card drill-ins ─────────────────────────────────────────────

// Plan card: 3-harness grouping (one Claude, one opencode, one BYOH all on same room)
const PlanCard_Triple = () => (
  <div className="lc lc-pane">
    <div className="lc-stack">
      <PlanCard flex={1} groups={[
        {
          harness: 'claude',
          items: [
            { status: 'done', text: 'design v1 of the harness_actions schema' },
            { status: 'now',  text: 'write the Claude adapter against the new schema' },
            { status: 'next', text: 'backfill 5MB of JSONL on attach' },
          ],
        },
        {
          harness: 'opencode',
          items: [
            { status: 'done', text: 'review schema against opencode parts',  priority: 'high' },
            { status: 'now',  text: 'add the opencode adapter alongside',     priority: 'high', inferred: true },
            { status: 'next', text: 'spec the cross-harness flag stream',     priority: 'med' },
          ],
        },
        {
          harness: 'byoh',
          items: [
            { status: 'next', text: 'wire the BYOH agent loop into the same actions table' },
          ],
        },
      ]} />
    </div>
  </div>
);

// Activity card: every row type stacked so designers can see the taxonomy
const ActivityCard_Taxonomy = () => (
  <div className="lc lc-pane">
    <div className="lc-stack">
      <ActivityCard flex={1} totals={{ events: '·', cost: '·', tokens: '·' }}>
        <TurnSep label="taxonomy reference — every row kind, one per line" />
        <EditRow      harness="claude"   time="14:02:18" file="Cargo.toml" adds={3} />
        <EditRow      harness="opencode" time="14:02:24" file="lib.rs" adds={1} kind="write" />
        <ReadRow      harness="claude"   time="14:02:26" file="docs/recon.md" lines={612} />
        <SearchRow    harness="claude"   time="14:02:28" pattern="harness_action" matches={23} />
        <SearchRow    harness="opencode" time="14:02:30" pattern="*.rs" matches={142} kind="glob" />
        <BashRow      harness="claude"   time="14:02:31" title="View issue #50 epic details" command="gh issue view 50" ms={812} />
        <TaskRow      harness="claude"   time="14:02:34" op="create" text="Build harness_events_claude.rs adapter + tests" />
        <TaskRow      harness="claude"   time="14:02:36" op="update" text="Build harness_events_claude.rs adapter + tests" from="pending" to="in_progress" />
        <TodoWriteRow harness="opencode" time="14:02:38" count={8} />
        <AskRow       harness="claude"   time="14:02:42" question="Release version?" chosen="v0.1.7" />
        <AgentRow     harness="claude"   time="14:02:44" title="general-purpose: Research xterm.js bug" ms="49.7s" tokens="1,234 tok" toolCount={8} />
        <PrRow        harness="claude"   time="14:02:50" number={61} repo="skein" title="harness_actions table + Claude adapter" />
        <QueueRow     harness="claude"   time="14:02:54" text="when you're done, please run the formatter" />
        <UserFileRow  harness="claude"   time="14:02:58" file="README.md" />
        <SlashRow     harness="claude"   time="14:03:02" name="clear" output="context cleared" />
        <CompactRow   harness="opencode" time="14:03:04" before="40k" after="12k" />
        <CostRow      harness="claude"   time="14:03:06" tokens="2,140" usd="0.09" ms="22s" />
        <ApiErrorRow  harness="claude"   time="14:03:08" status="HTTP 529" attempt={1} retryIn={8} message="overloaded" />
        <ToolErrorRow harness="opencode" time="14:03:11" tool="bash" message="Tool execution aborted" />
        <PermissionRow harness="byoh"    time="14:03:14" command="cargo test fs::watcher" />
        <BurstRow     harness="claude"   time="14:03:18" count={12} tool="edit" scope="crates/skein-core/src/**" adds={142} dels={37} />
        <UserPromptRow time="14:03:22" text="please run the formatter when you're done" />
        <PermissionModeRow harness="claude" time="14:03:24" from="ask" to="always_for_session" />
        <AiTitleRow harness="opencode" time="14:03:26" title="adapter parity work" />
        <BridgeStatusRow harness="claude" time="14:03:28" status="reconnecting" detail="attempt 2 of 5" />
        <BridgeStatusRow harness="opencode" time="14:03:30" status="connected" detail="ws/online" />
      </ActivityCard>
    </div>
  </div>
);

// Sub-agent inspector — the activity row in the BG, sheet slides in.
const SubAgentInspector = () => (
  <div className="lc lc-pane" style={{ position: 'relative' }}>
    <RoomSubtitle text="Inspecting Claude's sub-agent: Research xterm.js bug." age="49s elapsed" />
    <div className="lc-stack">
      <DiffCard flex={1} tabs={[{ file: 'xterm-research.md', harness: 'claude', active: true }]} body={DIFF_CARGO} />
      <div className="lc-divider" />
      <ActivityCard flex={1.2} totals={{ events: 412 }}>
        <TurnSep label="turn · 14:01:54" />
        <EditRow harness="claude" time="14:02:18" file="Cargo.toml" adds={3} />
        <AgentRow harness="claude" time="14:02:44"
          title="general-purpose: Research xterm.js bug"
          ms="49.7s" tokens="1,234 tok" toolCount={8} />
        <ReadRow harness="claude" time="14:03:32" file="docs/research-notes.md" lines={84} />
      </ActivityCard>
    </div>

    {/* The inspector overlay */}
    <div className="lc-inspector">
      <div className="lc-inspector-panel">
        <div className="lc-inspector-head" style={{ position: 'relative' }}>
          <div className="title">
            <Chip kind="claude" size={12} />
            <span>general-purpose: Research xterm.js bug</span>
            <span className="kind">sub-agent</span>
          </div>
          <div className="meta">
            <span>49.7s</span>
            <span>1,234 tok</span>
            <span>8 tool calls</span>
            <span>completed</span>
          </div>
          <button className="lc-inspector-close">×</button>
        </div>
        <div className="lc-inspector-body">
          <div className="lc-inspector-section-head">prompt</div>
          <div className="lc-inspector-prompt">
            Research the xterm.js issue where output is dropped when the buffer
            fills mid-write. Find authoritative source code references, summarise
            the proposed fix, and propose two patch strategies.
          </div>

          <div className="lc-inspector-section-head">tool calls (8)</div>
          <ReadRow   harness="claude" time="0.2s"  file="node_modules/xterm/src/InputHandler.ts" lines={1842} />
          <SearchRow harness="claude" time="2.4s"  pattern="enqueueData|writeBuffer" matches={6} />
          <ReadRow   harness="claude" time="3.1s"  file="node_modules/xterm/src/parser/EscapeSequenceParser.ts" lines={612} />
          <BashRow   harness="claude" time="6.8s"  title="check xterm changelog for buffer-overflow fixes" command="gh issue list" ms={912} />
          <ReadRow   harness="claude" time="14.2s" file="https://github.com/xtermjs/xterm.js/issues/4892" lines={31} />
          <ReadRow   harness="claude" time="18.0s" file="https://github.com/xtermjs/xterm.js/pull/4901" lines={84} />
          <ReadRow   harness="claude" time="24.4s" file="docs/research-notes.md" lines={0} />
          <EditRow   harness="claude" time="28.1s" file="docs/research-notes.md" adds={84} />

          <div className="lc-inspector-section-head">final report</div>
          <div className="lc-inspector-finalreport">
            The drop happens in <code style={{fontFamily:'var(--sk-mono)'}}>InputHandler.parse()</code> when a partial escape sequence
            spans a buffer boundary. PR #4901 fixes this by buffering parser state across calls.
            Two patch strategies:
            <br /><br />
            <strong style={{color:'var(--fg-0)'}}>A · Vendor PR #4901's commit.</strong> Minimal local diff; needs a rebase if upstream changes.
            <br />
            <strong style={{color:'var(--fg-0)'}}>B · Wrap our adapter.</strong> Coalesce writes &lt; 4KB at our boundary so xterm never sees a partial. More work but no upstream dependency.
            <br /><br />
            Recommend B for the v1 release, vendor A in a follow-up if the upstream PR drags.
          </div>
        </div>
      </div>
    </div>
  </div>
);

// ─── The canvas ────────────────────────────────────────────────

const RIGHT_W = 540;
const RIGHT_H = 820;
const STATE_W = 420;
const STATE_H = 640;

function App() {
  return (
    <div className="canvas-host">
      <DesignCanvas>
        <DCSection
          id="composition"
          title="Composition · the live, alive state"
          subtitle="3 cards, equal-weight, drag dividers. The room subtitle hosts Claude's away-summary. Diff auto-follows the focused harness; another harness editing a different file appears as an ambient pill rather than stealing focus."
        >
          <DCArtboard id="alive" label="A · Live · mixed harness · alive" width={RIGHT_W} height={RIGHT_H}>
            <RightPane_Alive />
          </DCArtboard>
        </DCSection>

        <DCSection
          id="states"
          title="State variations"
          subtitle="The shell stays the same — what differs is what fills it. Each state is the entire right pane at a smaller scale."
        >
          <DCArtboard id="empty" label="Empty · brand-new room" width={STATE_W} height={STATE_H}>
            <RightPane_Empty />
          </DCArtboard>
          <DCArtboard id="quiet" label="Long quiet · last activity 2h ago" width={STATE_W} height={STATE_H}>
            <RightPane_Quiet />
          </DCArtboard>
          <DCArtboard id="burst" label="Burst storm · 20+ rows in 30s" width={STATE_W} height={STATE_H}>
            <RightPane_Burst />
          </DCArtboard>
          <DCArtboard id="errored" label="Errored · graduated inline → toast" width={STATE_W} height={STATE_H}>
            <RightPane_Errored />
          </DCArtboard>
          <DCArtboard id="permission" label="Permission as a row · BYOH" width={STATE_W} height={STATE_H}>
            <RightPane_Permission />
          </DCArtboard>
        </DCSection>

        <DCSection
          id="cards"
          title="Card drill-ins"
          subtitle="Each card at full height. The Plan triple shows what 3 harnesses on one room looks like; the Activity taxonomy is a reference for every row kind."
        >
          <DCArtboard id="plan-triple" label="Plan · 3 harnesses, sub-lists" width={STATE_W} height={STATE_H}>
            <PlanCard_Triple />
          </DCArtboard>
          <DCArtboard id="activity-taxonomy" label="Activity · every row kind" width={RIGHT_W} height={RIGHT_H + 60}>
            <ActivityCard_Taxonomy />
          </DCArtboard>
        </DCSection>

        <DCSection
          id="subagent"
          title="Sub-agent inspector"
          subtitle="Clicking a sub-agent row opens an 80%-wide sheet over the right pane. Sub-agents run minutes and emit their own tool calls — keep them out of the main feed so the activity stays readable, but make them one click away."
        >
          <DCArtboard id="inspector" label="Inspector · Research xterm.js bug" width={RIGHT_W + 80} height={RIGHT_H}>
            <SubAgentInspector />
          </DCArtboard>
        </DCSection>

        <DCPostIt x={40} y={40} width={300}>
          <strong>Live Context · design exploration</strong>
          <br /><br />
          Decisions locked: 3 equal cards, drag-resize; live tailing; Diff follows focused harness with ambient flicker; Plan grouped per harness; away-summary as room subtitle; smart-collapsed tool results; graduated error treatment; sub-agent inspector.
          <br /><br />
          Open any artboard fullscreen via the expand button (top-right of each).
        </DCPostIt>
        <DCPostIt x={40} y={300} width={300}>
          <strong>My two "decide for me" answers</strong>
          <br /><br />
          <strong>Activity density · hybrid.</strong> Each event is a row. Same-tool same-file from same harness collapse into "Edit ×12 · path"; each assistant turn gets a faint divider; bursts render as a shimmering collapsed row.
          <br /><br />
          <strong>Cost · quiet.</strong> Session total in the Activity card header. Per-turn cost is a hair-line row under each turn divider. Easy to dismiss with a Tweak.
        </DCPostIt>
      </DesignCanvas>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById('root')).render(<App />);
