// The one held-mail mirror, writing into the `mailHold` store (#733).

import { createHeldMirror } from "./mailHeldMirror.ts";
import { mailHold } from "./mailHold.ts";

export const mailHeldMirror = createHeldMirror((harnessId, held) => {
	mailHold.set(harnessId, held);
});
