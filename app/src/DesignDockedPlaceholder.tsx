// Stands in for a docked design harness in the main column (#551). The
// preview itself is mounted by the right pane — one pane per harness id —
// so this must never mount DesignHarnessBody.

import "./FilesBody.css";
import "./DesignDockedPlaceholder.css";

interface Props {
	onShow: () => void;
	onUndock: () => void;
}

export const DesignDockedPlaceholder = ({ onShow, onUndock }: Props) => (
	<div className="fp-empty dp-docked">
		<div className="glyph">◐</div>
		<div className="title">Design preview is docked</div>
		<div className="hint">It is shown in the right pane's Design tab.</div>
		<div className="dp-docked-actions">
			<button type="button" onClick={onShow}>
				Show it
			</button>
			<button type="button" title="Move the preview back into this column" onClick={onUndock}>
				Undock
			</button>
		</div>
	</div>
);
