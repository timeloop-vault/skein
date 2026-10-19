// #627: asks before Skein writes its MCP entry into one of the user's own
// Hermes profiles. Consent is per profile and lives in the file, so nothing
// is remembered on Skein's side.

import { useEffect, useRef, useState } from "react";
import {
	type EntryPreview,
	hermesErrorText,
	injectProfileEntry,
	previewProfileEntry,
} from "./hermesProfiles.ts";
import "./modal.css";
import "./HermesConsentDialog.css";

export const HermesConsentDialog = ({
	profile,
	mode,
	reason,
	onProceed,
	onCancel,
}: {
	profile: string;
	/** `ask`: Skein can add the entry. `manual`: the edit was refused. */
	mode: "ask" | "manual";
	/** The listing's reason for a refusal, shown in `manual` mode. */
	reason: string | null;
	/** Create the harness (after a successful inject, or without the tools). */
	onProceed: () => void;
	onCancel: () => void;
}) => {
	const [preview, setPreview] = useState<EntryPreview | null>(null);
	const [previewError, setPreviewError] = useState<string | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const primary = useRef<HTMLButtonElement | null>(null);
	const onProceedRef = useRef(onProceed);
	onProceedRef.current = onProceed;

	useEffect(() => {
		let cancelled = false;
		previewProfileEntry(profile).then(
			(p) => {
				if (!cancelled) setPreview(p);
			},
			(e: unknown) => {
				if (cancelled) return;
				const text = hermesErrorText(e);
				// The entry was added meanwhile: consent is moot, carry on.
				if (mode === "ask" && text.includes("already present")) onProceedRef.current();
				else setPreviewError(text);
			},
		);
		return () => {
			cancelled = true;
		};
	}, [profile, mode]);

	useEffect(() => {
		primary.current?.focus();
	}, []);

	const add = async () => {
		setBusy(true);
		setError(null);
		try {
			await injectProfileEntry(profile);
			onProceed();
		} catch (e) {
			setError(hermesErrorText(e));
			setBusy(false);
		}
	};

	const refusal = mode === "manual" ? (reason ?? previewError) : null;

	return (
		<div
			className="sk-modal-bg"
			onClick={(e) => {
				// Rendered inside New Room's backdrop: don't let the click bubble
				// to its cancel and close the form too.
				e.stopPropagation();
				onCancel();
			}}
		>
			<div
				className="sk-modal sk-hermes-consent"
				onClick={(e) => e.stopPropagation()}
				onKeyDown={(e) => {
					if (e.key === "Escape") {
						e.stopPropagation();
						onCancel();
					}
				}}
			>
				<div className="sk-modal-head">
					<h2>Review tools in "{profile}"?</h2>
					<div className="sub">
						{mode === "ask"
							? "Skein's review tools reach Hermes through an MCP entry in the profile's config."
							: "Skein can't edit this profile's config safely."}
					</div>
				</div>
				<div className="sk-modal-body">
					{refusal && <div className="sk-hermes-consent-err">{refusal}</div>}
					{preview ? (
						<>
							<div className="sk-hermes-consent-path">
								{mode === "ask" ? "Appended to " : "Add by hand to "}
								<code>{preview.configPath}</code>
							</div>
							<pre className="sk-hermes-consent-lines">{preview.lines.join("\n")}</pre>
						</>
					) : (
						!previewError && <div className="sk-hermes-consent-path">reading the config…</div>
					)}
					{previewError && mode === "ask" && (
						<div className="sk-hermes-consent-err">{previewError}</div>
					)}
					{error && <div className="sk-hermes-consent-err">{error}</div>}
				</div>
				<div className="sk-modal-foot">
					<button className="sk-btn" type="button" onClick={onCancel} disabled={busy}>
						Cancel
					</button>
					<button
						className={`sk-btn ${mode === "manual" ? "primary" : ""}`}
						type="button"
						onClick={onProceed}
						disabled={busy}
						ref={mode === "manual" ? primary : undefined}
					>
						Run without review tools
					</button>
					{mode === "ask" && (
						<button
							className="sk-btn primary"
							type="button"
							ref={primary}
							disabled={busy || !preview}
							onClick={() => void add()}
						>
							{busy ? "Adding…" : "Add Skein's entry"}
						</button>
					)}
				</div>
			</div>
		</div>
	);
};
