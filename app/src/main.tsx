import { getCurrentWindow } from "@tauri-apps/api/window";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App.tsx";
import { ControlCenterPopout } from "./controlCenter/ControlCenterPopout.tsx";
import { POPOUT_LABEL } from "./controlCenter/popoutProtocol.ts";
import "./tokens.css";
import "./base.css";

const rootEl = document.getElementById("root");
if (!rootEl) {
	throw new Error("Skein: #root element is missing from index.html");
}

// The pop-out window must not mount App: it spawns PTYs and owns autosave.
const isPopout = getCurrentWindow().label === POPOUT_LABEL;

createRoot(rootEl).render(<StrictMode>{isPopout ? <ControlCenterPopout /> : <App />}</StrictMode>);
