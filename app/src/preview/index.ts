// Preview registry barrel — importing this side-effect-imports
// every provider so they self-register at module load. Anywhere
// that calls `findPreviewProviders` should import this module first.

import "./providers/text.tsx";
import "./providers/image.tsx";
import "./providers/markdown.tsx";
import "./providers/hex.tsx";

export { findPreviewProviders, type PreviewProvider, type PreviewCtx } from "./registry.ts";
