// The collapsible file list with its harness filter chips (#212, D4:
// harness is a chip and a filter, never a partition). Split out of
// ReviewPane.tsx (#460); the open/filter state stays in the pane.

import { HChip } from "../components.tsx";
import type { HarnessKind } from "../types.ts";
import type { ReviewFile } from "./api.ts";
import { FileList } from "./FileList.tsx";
import "./ReviewFileSection.css";

export const ReviewFileSection = ({
	files,
	harnesses,
	harnessFilter,
	setHarnessFilter,
	showFiles,
	setShowFiles,
	activePath,
	setActivePath,
	harnessKindOf,
	toggleViewed,
}: {
	files: ReviewFile[];
	harnesses: string[];
	harnessFilter: string | undefined;
	setHarnessFilter: (harnessId: string | undefined) => void;
	showFiles: boolean;
	setShowFiles: (show: boolean) => void;
	activePath: string | undefined;
	setActivePath: (path: string | undefined) => void;
	harnessKindOf: (harnessId: string) => HarnessKind;
	toggleViewed: (f: ReviewFile) => void;
}) => (
	<div className="rv-section">
		<div className="rv-section-bar">
			<button type="button" className="rv-section-head" onClick={() => setShowFiles(!showFiles)}>
				<span className="chev">{showFiles ? "▾" : "▸"}</span>
				<span>files</span>
				<span className="rv-section-count">{files.length}</span>
			</button>
			{/* D4: harness is a chip and a filter, never a partition.
			    Shown only once more than one harness has touched the
			    review — with one, it is a control that does nothing. */}
			{harnesses.length > 1 && (
				<span className="rv-filter">
					<button
						type="button"
						className={`rv-chip${harnessFilter === undefined ? " on" : ""}`}
						onClick={() => setHarnessFilter(undefined)}
						title="show every harness's files"
					>
						all
					</button>
					{harnesses.map((h) => (
						<button
							type="button"
							key={h}
							className={`rv-chip${harnessFilter === h ? " on" : ""}`}
							onClick={() => setHarnessFilter(harnessFilter === h ? undefined : h)}
							title={`only files last written by ${harnessKindOf(h)}`}
						>
							<HChip kind={harnessKindOf(h)} />
						</button>
					))}
				</span>
			)}
		</div>
		{showFiles && (
			<FileList
				files={files}
				activePath={activePath}
				harnessKindOf={harnessKindOf}
				onSelect={setActivePath}
				onToggleViewed={toggleViewed}
			/>
		)}
	</div>
);
