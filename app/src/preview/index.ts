// Preview registry barrel — importing this side-effect-imports
// every provider so they self-register at module load. Anywhere
// that calls `findPreviewProvider` should import this module first.

import "./providers/text.tsx";
import "./providers/image.tsx";

export { findPreviewProvider, type PreviewProvider, type PreviewCtx } from "./registry.ts";
