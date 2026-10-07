// Temporary views (#560, #561): the Control Center (over the rooms) and the
// "+ harness" picker (over a room's harnesses) each take over the area their
// tab strip normally selects from. The rule: while one is open NO existing
// tab renders as active, and any user selection — including the tab that
// was active before — dismisses the view and lands there.

/** The id a strip should draw as active: none while a temp view owns the area. */
export function shownActiveId(activeId: string | null, tempViewOpen: boolean): string | null {
	return tempViewOpen ? null : activeId;
}

/** Selecting a harness in `roomId` closes the picker only if it is open for that room. */
export function pickerAfterHarnessSelect(showPicker: string | null, roomId: string): string | null {
	return showPicker === roomId ? null : showPicker;
}
