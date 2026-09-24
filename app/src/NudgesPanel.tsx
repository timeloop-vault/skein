// Settings → Nudges (#355).
//
// #238 shipped three fixed nudge prompts; #355's registry
// (`nudgeRegistry.ts`) turned them into data with an optional
// per-nudge override. This is the missing piece: a place to actually
// read, edit and reset one, mirroring `SpawnEnvPanel.tsx`'s shape —
// draft state per field, committed explicitly rather than on every
// keystroke — but simpler, since there is nothing here that needs a
// round trip to Rust: `nudgeStore.ts` already persists synchronously.
//
// Save-on-blur, not a debounce: a nudge body can run to a page (#358
// pastes a whole procedure), so committing per-keystroke would push a
// localStorage write and a store-wide re-render (every `ReviewPane`
// listening via `useNudgeOverrides`) on every character typed. Blur is
// the natural "done editing this one" signal a textarea already gives
// for free.

import { useEffect, useRef, useState } from "react";
import {
	type NudgeDef,
	type NudgeOverrides,
	type NudgeScope,
	isOverridden,
	nudgeBody,
	nudgesInScope,
} from "./nudgeRegistry.ts";
import { resetNudgeBody, setNudgeBody, useNudgeOverrides } from "./nudgeStore.ts";

const SCOPE_LABEL: Record<NudgeScope, string> = {
	review: "Review",
	actions: "Actions",
};

/// One nudge's editor: label, description, an "edited" marker, a
/// disabled-unless-overridden reset, and the body itself.
const NudgeRow = ({ def, overrides }: { def: NudgeDef; overrides: NudgeOverrides }) => {
	const resolved = nudgeBody(def, overrides);
	const edited = isOverridden(def, overrides);
	const [draft, setDraft] = useState(resolved);

	// Latest draft/resolved/id, read only by the unmount flush below — refs
	// so that effect can stay mount-once (its cleanup reads `.current`,
	// which doesn't make it a reactive dependency) instead of re-running,
	// and re-arming its flush, on every keystroke. `draftRef` is written
	// synchronously wherever `draft` itself is set (never from a render
	// body), so the two refs can never be caught mid-update relative to
	// each other by an unmount landing between a render and its effects.
	const draftRef = useRef(draft);
	const resolvedRef = useRef(resolved);
	const idRef = useRef(def.id);
	idRef.current = def.id;

	// Follow the store's resolved body when it changes from outside this
	// row's own typing — another window, or this row's own Reset button.
	// `resolved` only changes on a committed override, never mid-keystroke
	// (the draft below is local until blur), so this can't clobber an edit
	// in progress — true as long as this row is the only writer of its own
	// id, which holds today (no other UI edits a nudge body). Both refs are
	// set together, here, so Reset immediately followed by unmount can
	// never see `draftRef` still holding the pre-reset text while
	// `resolvedRef` has already moved on.
	useEffect(() => {
		setDraft(resolved);
		draftRef.current = resolved;
		resolvedRef.current = resolved;
	}, [resolved]);

	// Escape closes Settings (and unmounts every row) without ever firing
	// the textarea's blur — a draft typed right before Escape would
	// otherwise be silently lost. Flush it on unmount instead.
	useEffect(() => {
		return () => {
			if (draftRef.current !== resolvedRef.current) {
				setNudgeBody(idRef.current, draftRef.current);
			}
		};
	}, []);

	const handleChange = (value: string) => {
		setDraft(value);
		draftRef.current = value;
	};

	const commit = () => {
		if (draft !== resolved) setNudgeBody(def.id, draft);
	};

	return (
		<div className="sk-nudge">
			<div className="sk-nudge-head">
				<span className="sk-nudge-label">{def.label}</span>
				{edited && <span className="sk-nudge-edited">edited</span>}
				<button
					type="button"
					className="sk-btn ghost sk-nudge-reset"
					disabled={!edited}
					onClick={() => resetNudgeBody(def.id)}
				>
					Reset to default
				</button>
			</div>
			<div className="sk-help">{def.description}</div>
			<textarea
				className="sk-textarea sk-nudge-body"
				spellCheck={false}
				rows={4}
				value={draft}
				onChange={(e) => handleChange(e.target.value)}
				onBlur={commit}
			/>
		</div>
	);
};

export const NudgesPanel = () => {
	const overrides = useNudgeOverrides();
	return (
		<div className="sk-nudges">
			<div className="sk-help">
				The body is pasted into the harness as one message — only when the harness is waiting at the
				end of its turn, never mid-response.
			</div>
			{(Object.keys(SCOPE_LABEL) as NudgeScope[]).map((scope) => {
				const defs = nudgesInScope(scope);
				if (defs.length === 0) return null;
				return (
					<div className="sk-nudge-group" key={scope}>
						<div className="sk-nudge-group-label">{SCOPE_LABEL[scope]}</div>
						{defs.map((def) => (
							<NudgeRow key={def.id} def={def} overrides={overrides} />
						))}
					</div>
				);
			})}
		</div>
	);
};
