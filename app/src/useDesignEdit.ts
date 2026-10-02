// useDesignEdit — edit mode on a design pane (#436): the element the
// reviewer picked to edit, its computed style and the page's tokens, and
// the batch of changes the page streams back. "Propose" turns the batch
// into ONE element thread carrying a structured proposal; Skein never
// writes source — the agent applies it. The frame stays with DesignBody;
// this hook only talks to it through `post`.

import { useCallback, useState } from "react";
import { anchorFor } from "./designComments.ts";
import { applyChange, type Batch, batchToProposal, emptyBatch } from "./designEditBatch.ts";
import type { HostMessage } from "./designPreview.ts";
import type { DesignToken, EditBeacon } from "./designProposal.ts";
import type { ElementDescriptor } from "./elementAnchor.ts";
import { addThread } from "./review/api.ts";

export const useDesignEdit = ({
	roomId,
	cwd,
	entry,
	post,
}: {
	roomId: string;
	cwd: string;
	entry: string | undefined;
	post: (msg: HostMessage) => void;
}) => {
	const [active, setActive] = useState(false);
	const [target, setTarget] = useState<ElementDescriptor | null>(null);
	const [computed, setComputed] = useState<Record<string, string>>({});
	const [tokens, setTokens] = useState<DesignToken[]>([]);
	const [batch, setBatch] = useState<Batch>(emptyBatch());
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const reset = useCallback(() => {
		setActive(false);
		setTarget(null);
		setComputed({});
		setTokens([]);
		setBatch(emptyBatch());
		setError(null);
	}, []);

	/** Leave edit mode, putting the page back as it was. */
	const stop = useCallback(() => {
		post({ type: "edit-end", revert: true });
		reset();
	}, [post, reset]);

	const start = useCallback(() => {
		setActive(true);
		setError(null);
		post({ type: "edit-start" });
	}, [post]);

	/** A beacon from the frame; false when it is not an edit beacon. */
	const onBeacon = useCallback(
		(b: EditBeacon) => {
			if (b.type === "edit-picked") {
				setTarget(b.element);
				setComputed(b.computed);
				setTokens(b.tokens);
				setBatch(emptyBatch());
				setError(null);
			} else if (b.type === "edit-change") {
				setBatch((prev) => applyChange(prev, b.change));
			} else {
				stop();
			}
		},
		[stop],
	);

	const setProperty = useCallback(
		(property: string, value: string) => post({ type: "edit-set", property, value }),
		[post],
	);

	const proposal = batchToProposal(batch);

	const propose = async (note: string) => {
		if (!target || entry === undefined || !proposal) return;
		setBusy(true);
		setError(null);
		try {
			await addThread(roomId, cwd, {
				scope: "element",
				filePath: entry,
				anchorLines: [],
				body: note,
				element: anchorFor(target, entry),
				proposal,
			});
			// The overlay re-applies from the stored thread, not from this edit.
			stop();
		} catch (e) {
			setError(String(e));
		} finally {
			setBusy(false);
		}
	};

	return {
		active,
		target,
		computed,
		tokens,
		batch,
		proposal,
		busy,
		error,
		start,
		stop,
		reset,
		onBeacon,
		setProperty,
		propose,
	};
};
