// The dock toggle in a design harness's toolbar (#551).

export interface DesignDock {
	/** True when the body is the right pane's (the button then undocks). */
	docked: boolean;
	onToggle: () => void;
}

export const DesignDockButton = ({ dock }: { dock: DesignDock }) => (
	<button
		type="button"
		title={
			dock.docked
				? "Undock: move this preview back into the harness column"
				: "Dock to right pane: show this preview in the right pane's Design tab"
		}
		onClick={dock.onToggle}
	>
		{dock.docked ? "Undock" : "Dock"}
	</button>
);
