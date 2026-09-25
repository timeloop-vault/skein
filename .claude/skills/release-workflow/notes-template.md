<!--
Skeleton for a Skein release body. The title is NOT part of the body:
pass it with --title "vX.Y.Z — <summary>".
Delete every section that has no entries, and delete these comments.
Section order is fixed. See SKILL.md for depth and voice, and read
v0.3.0 side by side before creating the draft.
-->
<One to three sentences: what this release is, in the user's terms. If something breaks, say so in bold and point at the first section.>

## ⚠️ Breaking: <what changed> (#n)

<What changed and why. If names changed, a before/after table.>

**What to update:**

- <each thing a user or agent must change>

<What did NOT change.>

## ✨ New

### <What the user or agent can now do> (#n)

<Before, after, how to use it. UI labels in bold. Limits, defaults and the Settings switch that turns it off.>

## 🎨 Changes

- **<What is different now> (#n).** <Why, and how it behaves now.>

## 🐛 Fixes

- **<What no longer goes wrong> (#n).** <What used to happen.> <What happens now.>

## ⚡ Performance (#n)

<One lead sentence.>

- <each measurable change>

## 🧰 Under the hood

- <internal work, logging, refactors, repo skills and docs (#n)>

## ✅ Verified

<Prose: what was tested, how (live, by hand, install-tested), on which OS and build, and plainly what was not. Only checks someone actually reported. End with "The full pre-commit gate passes." only if every merged PR says so.>

## 📦 Packaging

Unchanged: macOS **Apple Silicon**, **Windows**, **Linux**. Update in the app from Settings → About → Check for updates.
