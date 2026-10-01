import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App.tsx";
import "./tokens.css";
import "./base.css";

const rootEl = document.getElementById("root");
if (!rootEl) {
	throw new Error("Skein: #root element is missing from index.html");
}

createRoot(rootEl).render(
	<StrictMode>
		<App />
	</StrictMode>,
);
