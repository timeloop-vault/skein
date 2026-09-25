---
name: release-workflow
description: Draft a Skein GitHub release whose title, opening, section order, depth and voice match the recent releases (v0.3.0, v0.2.15, v0.2.14), and create it as a DRAFT only. Use when the user says "draft a release", "cut a release", "prepare vX.Y.Z", "write the release notes" or "release notes". Covers picking the version from what landed since the last tag, writing the notes from the merged PRs and issues rather than commit subjects, gh release create --draft --notes-file with a read-back, and the traps: placeholder version strings, the updater's latest.json skipping prereleases, and workflow_dispatch building nothing publishable. Publishing stays the user's step.
---

# Release Workflow

Draft a Skein release that reads like the ones before it, and leave it
as a draft for the user to publish.

## When to Use This Skill

- User says "draft a release", "cut a release", "prepare v0.3.2"
- User asks for release notes for what landed since the last release
- An existing draft needs rewriting (only when the user asks)

## What publishing does

A Skein release is a GitHub Release with a `vX.Y.Z` tag. **Publishing**
it (`release: published`) is what starts `.github/workflows/release.yml`.
That run builds macOS (Apple Silicon), Windows and Linux, attaches the
installers and the updater's `latest.json`, and patches the version into
the build. A **draft builds nothing**, and neither does creating a tag.
Installed apps update from
`releases/latest/download/latest.json` (`app/src-tauri/tauri.conf.json`),
so a published release reaches every user who clicks
**Check for updates**.

**This skill stops at the draft.** Publishing is the user's step.
Never publish, never `gh release edit --draft=false`, never push a tag.

## Where the format comes from

Nothing but the past release bodies records the format. The pattern
below comes from **v0.3.0, v0.2.15 and v0.2.14** (the current shape),
with **v0.2.13 and v0.2.12** as the shape for a fix-only release.
Where older releases differ, the recent ones win:

- **v0.2.8 to v0.2.11** gave each headline feature its own H2 with its
  own emoji (`## 🧭 Your PATH reaches your harnesses (#72)`). Since
  v0.2.14 features sit as `###` under one `## ✨ New`. Use `## ✨ New`.
- **v0.2.14 and earlier** closed Packaging with "; in-app update via
  Settings → About → Check for updates." v0.3.0 writes it as a
  sentence. Use the v0.3.0 line (below).
- **v0.2.7 and earlier** use short lowercase titles ("hotfix: …",
  "reopen + popover fixes"). Don't copy them.

Before you write, read the last two releases in full. They are the
reference, and this file only summarises them:

```bash
gh release list --limit 5
gh release view v0.3.0 --json name,body -q '.name, .body'
gh release view v0.2.15 --json name,body -q '.name, .body'
```

If a newer published release exists and differs from what follows, the
newer release wins. Say so to the user, and suggest updating this skill.

## The shape

The skeleton is in [`notes-template.md`](notes-template.md) beside this
file. Section by section:

### Title: `vX.Y.Z — <summary>`

- An em dash with a space on each side, never a hyphen or colon.
- The summary names the one or two headline changes as plain phrases
  about what the user gets. It is sentence case, with no full stop,
  and two items are joined with ", and":
  - `v0.2.15 — Rooms grouped by repository, and harnesses that see their subagents`
  - `v0.2.14 — Clickable Windows notifications, and a review that sees every edit`
  - `v0.2.12 — Claude harnesses stop reading running when they are not`
- If the release breaks something, the title says so in parentheses:
  `v0.3.0 — Harnesses that message each other and open rooms (breaking: MCP tool names)`.
- No issue numbers, no "various fixes", no version-bump words.

### Opening paragraph (no heading)

One to three sentences, straight after the title, that say what the
release is in the user's terms. It usually restates the title a bit
more fully:

- v0.2.15: "Rooms grouped by repository, harnesses that see their
  subagents, and sessions that follow `/clear`, `/resume` and `/new`."
- v0.2.13: "One Windows fix that was hard to miss."
- v0.3.0 adds the warning: "…and **that change breaks existing
  setups**: read the first section before you update."

Don't write "This release adds…" or "This release includes…". Don't
list everything here, either. The sections do that.

### Sections, in this order

Leave out any section that has no entries. Never write an empty
section or "None".

| Section | When | Shape |
|---|---|---|
| `## ⚠️ Breaking: <what> (#n)` | only when something a user or agent relies on stops working (renamed MCP tools, a changed default, removed behaviour) | first section. What changed, a before/after table if names changed, a bold **What to update:** list, and what did *not* change |
| `## ✨ New` | new things a user or agent can do | one `### <headline> (#n)` per feature, 1–3 short paragraphs each |
| `## 🎨 Changes` | behaviour or look changed on purpose, not a fix | bullets: `- **<What is different now> (#n).** <why / how>` |
| `## 🐛 Fixes` | something used to go wrong and no longer does | bullets: `- **<What no longer goes wrong> (#n).** <what used to happen>. <what happens now>.` |
| `## ⚡ Performance (#n)` | a responsiveness pass worth its own section (v0.2.14) | one lead sentence, then bullets |
| `## 🧰 Under the hood` | internal work, refactors, logging, tests, repo skills and docs | plain bullets, one or two sentences each |
| `## ✅ Verified` | always | prose (see below) |
| `## 📦 Packaging` | always | the fixed line (see below) |

In a **fix-only release** (v0.2.13, v0.2.12) each fix gets its own
`### <what no longer goes wrong> (#n)` under `## 🐛 Fixes`, with a
paragraph of what the user saw and a paragraph of why. That is the only
time fixes get `###` headings.

### Entries: depth and voice

- **Cite the issue number, not the PR number.** `(#330, #328)` lists
  the issues. Use a PR number only when there is no issue. Never pair an
  issue with its own PR (`(#357, #372)`), which is the v0.3.1-draft
  mistake below.
- **Headlines say what the user can now do or no longer suffers**, in
  plain words: "Rename a room (#241)", "Closing a room keeps you nearby
  (#334)", "A fresh Claude harness leaves "spawning" (#273)". Not the
  commit subject, and not the implementation ("Add nudge registry").
- **Body: the before, the after, and how to use it.** Name UI elements
  exactly as they appear, in bold: **Rename room**, **Branch template**,
  Settings → About → **Check for updates**. Give the limits, defaults
  and kill switches as numbers and labels ("64 KB per message, 30 sends
  per room per minute… To turn it off, uncheck **Let agents message
  other harnesses** in Settings").
- **Mechanism only as far as it explains the behaviour.** v0.2.13
  explains why only release builds flashed a console window, because
  that is what a user noticed. It doesn't walk through the code.
- **Agent-facing changes are user-facing here.** Agents are Skein's
  users too: new verbs go under New with their names in backticks
  (`send_message {to, body}`), and a renamed verb is Breaking.
- **Skills and docs under `.claude/skills/` or `docs/` go to Under the
  hood**, unless they change what an agent in a Skein room does (see
  v0.2.13's `worktree-sweep` bullet).
- **Short sentences, one idea each.** Present tense, "you" for the
  reader. No marketing words ("seamless", "powerful", "enhanced",
  "improved handling", "robustness"). If a sentence would fit any
  product's release notes, cut it.
- **Numbers when you have them.** "Measured on real sessions, this was
  happening in 94% of those with subagents." Only numbers that a PR or
  issue actually states.

### `## ✅ Verified`

A short prose paragraph, not bullets. It says what was tested, how
(live, by hand, install-tested), on which OS and build, and **plainly
what was not**:

- v0.3.0: "The close/quit dialog was tested on Windows but **not yet on
  macOS**. The full pre-commit gate passes."
- v0.2.13: "macOS still unexercised since v0.2.9."
- v0.2.8: "The Windows PATH union is reasoned from `portable-pty`'s
  source, not yet exercised on real hardware…"

Take it from the PRs' test plans and the issues. **Never claim a test
nobody reported.** When a user-visible change has no stated check, say
it was covered only by the unit tests and the gate, and ask the user
what they checked by hand before you finalise. End with "The full
pre-commit gate passes." only if every merged PR says so.

### `## 📦 Packaging`

The current line (v0.3.0), verbatim unless the platforms changed:

```markdown
Unchanged: macOS **Apple Silicon**, **Windows**, **Linux**. Update in the app from Settings → About → Check for updates.
```

## The counter-example: the v0.3.1 draft of 2026-09-25

An ad hoc draft of v0.3.1 was written without this skill, and the user
rejected it because it did not read like v0.3.0. Every point below is a
rule above. Check your draft against the list:

- **Wrong sections.** It had `## What's new`, `## Fixes and refinements`
  and `## Testing`, without emoji and with invented theme subsections
  ("Robustness and observability"). The sections come from the table
  above.
- **Filler that no PR backs.** "Improved error messages for malformed
  agent inputs", "Better handling of stale agent session metadata".
  Every entry must trace to a merged PR or issue, and vague entries are
  usually invented.
- **A false Verified.** "All changes have been tested against real
  worktrees and agent workflows on both macOS and Windows" was not true.
  Verified states only what someone actually ran.
- **Implementation voice.** "Enhanced tracing… to improve diagnosability"
  and "Added a dedicated **Nudge** button" describe the diff. Say what
  changed for the user, or put it under the hood.
- **An internal change filed as a feature.** Tracing for the tail
  adapter is Under the hood, not New.
- **"This release adds…"** as the opening.
- **Issue and PR numbers paired** in every entry.
- **No Packaging line.**

## Agent Instructions

### 1. Check state

```bash
git fetch origin --tags
gh release list --limit 5                      # last published tag, any existing draft
LAST=$(gh release view --json tagName -q .tagName)   # latest published (non-prerelease)
git log --oneline "$LAST"..origin/main
```

If a draft for the next version already exists, read it. **Don't edit it
unless the user asked you to.** Write your notes to a file and show them.

### 2. Pick the version

- Anything **breaking** (see the Breaking row above) → next **minor**
  (v0.2.15 → v0.3.0 was the MCP tool rename).
- Otherwise → next **patch**, features included. The whole v0.2.x line
  shipped features in patch releases.
- The user's word overrides both.

### 3. Gather from the PRs and issues, not the commit subjects

Commit subjects tell you which PRs to read, not what to write. For each
commit in the range:

```bash
gh pr view <pr> --json title,body,files
gh issue view <issue>
```

Collect what the user sees, the before and after, UI labels, limits and
switches, what was tested and how, and anything breaking. Then sort
every PR into exactly one section of the table. A PR can be left out
only if it is invisible and trivial. Say which ones you left out when
you report.

### 4. Write the notes to a file outside the worktree

Start from `notes-template.md`, delete the sections you don't use, and
write the title separately (it goes in `--title`, not in the body).
Keep the file out of the repository: no release notes are committed.

Before you create anything, check the draft against **every** bullet
in "The counter-example" above, and against v0.3.0 read side by side.
The repo is **public**: no local paths, usernames, machine names or
room names.

### 5. Create or update the draft

```bash
SHA=$(git rev-parse origin/main)

# New draft
gh release create vX.Y.Z --draft --target "$SHA" \
  --title "vX.Y.Z — <summary>" --notes-file <notes-file>

# Existing draft, only when the user asked
gh release edit vX.Y.Z --target "$SHA" \
  --title "vX.Y.Z — <summary>" --notes-file <notes-file>
```

- **Always `--notes-file`.** Never `--notes "$(cat …)"` in PowerShell,
  and never `--body @-` or `--notes @-`: `gh` does not read `@-` as
  stdin and stores the literal string (same trap as `pr-workflow`).
- **Pin `--target` to a SHA**, not `main`. A draft has no tag yet, and
  GitHub creates it at publish time from the target. With `main` as the
  target, the tag lands on whatever main is by then, including commits
  the notes don't mention.
- No `--prerelease` unless the user asks. It keeps the release away
  from every installed app (see Traps).

### 6. Read it back

`gh` printing a URL does not mean the body landed:

```bash
gh release view vX.Y.Z --json name,isDraft,targetCommitish,body \
  -q '.name, .isDraft, .targetCommitish'
gh release view vX.Y.Z --json body -q .body | diff - <notes-file>
```

`isDraft` must be `true`, the target must be the SHA, and the diff must
show nothing beyond trailing whitespace. If it shows more, fix it with
`gh release edit … --notes-file` and read it back again.

### 7. Report and stop

Tell the user the title, the draft URL, the target SHA, and which PRs
went where (and any left out). List the gaps in Verified that only they
can fill. Then stop. **Publishing is the user's step**, and when they
publish, it tags the target and starts the build.

## Traps

- **`0.1.0` version strings are placeholders.** `tauri.conf.json`,
  `package.json` and `Cargo.toml` all say `0.1.0`. The pipeline patches
  the tag's version in at build time. Never "fix" them, and never bump
  them as part of a release.
- **Prereleases are never offered to installed apps.** The updater
  reads `releases/latest/download/latest.json`, and GitHub's "latest"
  skips prereleases.
- **`workflow_dispatch` on `release.yml` only builds artifacts.** It
  uploads them to the workflow run and creates no release, no tag and
  no `latest.json`. It is not a way to ship.
- **A draft builds nothing, and neither does the tag on its own.** Only
  `release: published` starts the pipeline.
- **Public repo.** No local paths, usernames, machine names, room names
  or internal hostnames in the notes, title or anything you post.
- **`gh release view` with no tag** returns the latest *published*
  release. It skips drafts and prereleases, which is why step 1 uses it
  to find `$LAST`.
