// The land dialog (#214, epic #52 D9).
//
// The end of a room's life: the review cycle is done, and the branch
// leaves — either merged into its base locally, or pushed with a pull
// request opened for people who were not in the room.
//
// Three rules shape everything here:
//
//   1. **Say what will move before it moves.** The backend's preflight
//      is the only reason this dialog can name the base, the commit
//      count, the worktree the merge would run in, and the uncommitted
//      work that will *not* travel. It renders all of it, including
//      every blocker verbatim — a disabled button with no reason beside
//      it is the failure mode this exists to avoid.
//   2. **Neither action fires without confirmation.** Push is
//      outward-facing and not undoable in practice; merge rewrites a
//      branch other rooms may be sitting on. Both go through a native
//      confirm naming the specific thing about to happen.
//   3. **The review stays local.** The PR body is prefilled from the
//      branch's own commits and is the user's to edit. No Skein comment
//      reaches it — that is D9, and there is no code path here that
//      could.

import { confirm } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useRef, useState } from "react";
import { useFocusRestore } from "../useFocusRestore.ts";
import {
	type LandPrResult,
	type LandPreflight,
	type MergeOutcome,
	dirtyWarning,
	landMerge,
	landOpenPr,
	landPreflight,
	mergeResultLine,
	mergeSummary,
} from "./land.ts";

type Done = { kind: "merge"; outcome: MergeOutcome } | { kind: "pr"; result: LandPrResult };

export const LandModal = ({
	roomId,
	cwd,
	roomName,
	onClose,
	onLanded,
}: {
	roomId: string;
	cwd: string;
	roomName: string;
	onClose: () => void;
	/** Fires after anything actually moved, so the pane re-reads git. */
	onLanded: () => void;
}) => {
	useFocusRestore();
	const [pre, setPre] = useState<LandPreflight | undefined>(undefined);
	const [error, setError] = useState<string | undefined>(undefined);
	const [busy, setBusy] = useState(false);
	const [done, setDone] = useState<Done | undefined>(undefined);
	const [title, setTitle] = useState("");
	const [body, setBody] = useState("");
	// The user's edits win over a re-read: a preflight refresh must not
	// overwrite a PR description someone is halfway through writing.
	const touched = useRef(false);

	const load = useCallback(() => {
		landPreflight(roomId, cwd)
			.then((p) => {
				setPre(p);
				setError(undefined);
				if (!touched.current) {
					setTitle(p.suggestedTitle);
					setBody(p.suggestedBody);
				}
			})
			.catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)));
	}, [roomId, cwd]);

	useEffect(() => {
		load();
	}, [load]);

	// A window listener rather than a handler on the dialog: nothing
	// inside it is focused on open, so a keydown on the element would
	// never fire and Escape would silently do nothing.
	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (e.key === "Escape") {
				e.preventDefault();
				onClose();
			}
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [onClose]);

	const run = (work: () => Promise<Done>) => {
		setBusy(true);
		setError(undefined);
		work()
			.then((d) => {
				setDone(d);
				onLanded();
			})
			.catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)))
			.finally(() => {
				setBusy(false);
				// Re-read either way: after a success the counts have
				// changed, and after a failure the usual reason is that
				// the repository moved under the dialog.
				load();
			});
	};

	const doMerge = async () => {
		if (!pre) return;
		const ok = await confirm(
			`${mergeSummary(pre)}?\n\n${dirtyWarning(pre) ?? "Everything on the branch is committed."}`,
			{ title: "Merge to base", kind: "warning" },
		);
		if (!ok) return;
		run(() => landMerge(roomId, cwd).then((outcome) => ({ kind: "merge", outcome }) as Done));
	};

	const doPr = async () => {
		if (!pre) return;
		if (!title.trim()) {
			setError("a pull request needs a title");
			return;
		}
		const ok = await confirm(
			`Push ${pre.branch ?? "this branch"} to ${pre.remote ?? "the remote"} and open a pull request against ${pre.prBase ?? "the base"}?\n\nPushing publishes this branch. Skein's review comments stay here.`,
			{ title: "Push and open a pull request", kind: "warning" },
		);
		if (!ok) return;
		run(() =>
			landOpenPr(roomId, cwd, title.trim(), body).then(
				(result) => ({ kind: "pr", result }) as Done,
			),
		);
	};

	const mergeBlocked = !pre || pre.mergeBlockers.length > 0;
	const prBlocked = !pre || pre.prBlockers.length > 0;

	return (
		<div className="sk-modal-bg" onClick={onClose}>
			<div className="sk-modal rv-land" onClick={(e) => e.stopPropagation()}>
				<div className="sk-modal-head">
					<h2>Land this branch</h2>
					<div className="sub">
						{roomName}
						{pre?.branch ? ` · ${pre.branch}` : ""}
						{pre?.base ? ` → ${pre.base}` : ""}
					</div>
				</div>

				<div className="sk-modal-body">
					{error && <div className="rv-land-err">{error}</div>}

					{!pre && !error && <div className="rv-land-note">reading the branch…</div>}

					{pre && <Summary pre={pre} />}

					{done && <Result done={done} />}

					{pre && !done && (
						<>
							<section className="rv-land-act">
								<div className="rv-land-act-head">
									<h3>Merge to base</h3>
									<button
										type="button"
										className="rv-land-go"
										disabled={busy || mergeBlocked}
										onClick={() => void doMerge()}
									>
										merge into {pre.mergeBase ?? pre.base ?? "base"}
									</button>
								</div>
								<p className="rv-land-why">
									For work nobody else needs to look at. Merges the branch locally; nothing is
									pushed.
									{pre.mergeBase && pre.mergeBase !== pre.base && (
										<>
											{" "}
											The review's base is {pre.base}, a remote-tracking branch — the merge targets
											the local {pre.mergeBase} it follows.
										</>
									)}
								</p>
								<Blockers reasons={pre.mergeBlockers} />
							</section>

							<section className="rv-land-act">
								<div className="rv-land-act-head">
									<h3>Push and open a pull request</h3>
									<button
										type="button"
										className="rv-land-go"
										disabled={busy || prBlocked}
										onClick={() => void doPr()}
									>
										push to {pre.remote ?? "remote"} + open PR
									</button>
								</div>
								<p className="rv-land-why">
									For collaborators to review on GitHub. The pull request starts clean — this room's
									review comments stay here.
								</p>
								<Blockers reasons={pre.prBlockers} />
								{!prBlocked && (
									<div className="rv-land-form">
										<label className="rv-land-field">
											<span>title</span>
											<input
												value={title}
												disabled={busy}
												onChange={(e) => {
													touched.current = true;
													setTitle(e.target.value);
												}}
											/>
										</label>
										<label className="rv-land-field">
											<span>body</span>
											<textarea
												rows={6}
												value={body}
												disabled={busy}
												onChange={(e) => {
													touched.current = true;
													setBody(e.target.value);
												}}
											/>
										</label>
									</div>
								)}
							</section>
						</>
					)}
				</div>

				<div className="sk-modal-foot">
					<button type="button" onClick={onClose}>
						{done ? "done" : "cancel"}
					</button>
				</div>
			</div>
		</div>
	);
};

const Summary = ({ pre }: { pre: LandPreflight }) => {
	const dirty = dirtyWarning(pre);
	return (
		<div className="rv-land-summary">
			<div className="rv-land-counts">
				<span>
					{pre.ahead} commit{pre.ahead === 1 ? "" : "s"} to land
				</span>
				{pre.behind > 0 && (
					<span className="rv-land-behind">
						{pre.behind} behind {pre.base}
					</span>
				)}
				{pre.upstream && <span className="rv-land-dim">upstream {pre.upstream}</span>}
			</div>
			{pre.commits.length > 0 && (
				<ul className="rv-land-commits">
					{pre.commits.slice(0, 8).map((c, i) => (
						// Commit summaries repeat (`fix review comments`
						// twice is normal), so the index is part of the key.
						<li key={`${i}-${c}`}>{c}</li>
					))}
					{pre.commits.length > 8 && (
						<li className="rv-land-dim">+{pre.commits.length - 8} more</li>
					)}
				</ul>
			)}
			{dirty && <div className="rv-land-warn">{dirty}</div>}
		</div>
	);
};

/// Every reason an action cannot run, verbatim from the backend.
const Blockers = ({ reasons }: { reasons: string[] }) => {
	if (reasons.length === 0) return null;
	return (
		<ul className="rv-land-blockers">
			{reasons.map((r) => (
				<li key={r}>{r}</li>
			))}
		</ul>
	);
};

const Result = ({ done }: { done: Done }) => {
	if (done.kind === "merge") {
		return (
			<div className="rv-land-done">
				<div>{mergeResultLine(done.outcome)}</div>
				{done.outcome.worktree && <div className="rv-land-dim">in {done.outcome.worktree}</div>}
			</div>
		);
	}
	const { result } = done;
	return (
		<div className="rv-land-done">
			<div>
				pushed {result.branch} to {result.remote}
			</div>
			{result.url && (
				<div>
					<button
						type="button"
						className="rv-land-link"
						onClick={() => void openUrl(result.url ?? "")}
					>
						{result.url}
					</button>
					<span className="rv-land-dim">{result.created ? " (opened)" : " (already open)"}</span>
				</div>
			)}
			{/* gh has a long history of eating bodies passed the wrong
			    way. If what GitHub stored is not what we sent, say so —
			    a silently truncated description is worse than a warning. */}
			{result.url && !result.bodyVerified && (
				<div className="rv-land-warn">
					the pull request body GitHub stored does not match what was sent — check it
				</div>
			)}
			{result.prError && (
				<div className="rv-land-warn">
					the branch is on the remote, but the pull request could not be opened: {result.prError}
				</div>
			)}
		</div>
	);
};
