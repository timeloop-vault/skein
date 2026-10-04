// The design pane's per-harness device setting (#528): which viewport the
// preview is framed at. Pure — no React; the UI wiring lives elsewhere.

/** The persisted shape, stored on Harness as `designDevice` and round-tripped
 *  as plain JSON by Rust. `preset` is "none", a DEVICE_PRESETS id, or
 *  "custom" (which uses width/height). `dpr` absent = the host's DPR.
 *
 *  `touch` is mobile emulation (#529 + #530): mouse drags become touch
 *  gestures and hover/pointer media features report a phone. One switch,
 *  because hover styles under emulated touch would be inconsistent.
 *
 *  New flags are optional fields here plus a line in `deviceParams`. Rust
 *  stores it opaquely, so no Rust change is needed. */
export interface DesignDevice {
	preset: string;
	width?: number;
	height?: number;
	landscape?: boolean;
	dpr?: number;
	touch?: boolean;
}

/** Portrait CSS px. */
export const DEVICE_PRESETS: readonly {
	id: string;
	label: string;
	width: number;
	height: number;
}[] = [
	{ id: "iphone-16-pro", label: "iPhone 16 Pro", width: 402, height: 874 },
	{ id: "iphone-14", label: "iPhone 14", width: 390, height: 844 },
	{ id: "android", label: "Android", width: 412, height: 892 },
	{ id: "ipad-air", label: "iPad Air", width: 820, height: 1180 },
];

export const DPR_CHOICES: readonly number[] = [1, 2, 3];

/** CSS px bounds for a custom size. */
export const CUSTOM_MIN = 100;
export const CUSTOM_MAX = 4000;

export const DPR_MIN = 0.5;
export const DPR_MAX = 4;

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const clampDim = (n: number): number => Math.min(CUSTOM_MAX, Math.max(CUSTOM_MIN, Math.round(n)));

/** Validate a stored value (a JSON blob: may be anything, including a
 *  future version's extra fields). Undefined means "fills the pane". */
export const normalizeDevice = (raw: unknown): DesignDevice | undefined => {
	if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return undefined;
	const r = raw as Record<string, unknown>;
	const preset = r.preset;
	if (typeof preset !== "string") return undefined;
	const out: DesignDevice = { preset };
	if (preset === "none") {
		// Fills the pane, but DPR and touch are independent of the size.
		if (isNum(r.dpr) && r.dpr >= DPR_MIN && r.dpr <= DPR_MAX) out.dpr = r.dpr;
		if (r.touch === true) out.touch = true;
		return out.dpr === undefined && out.touch === undefined ? undefined : out;
	}
	if (preset === "custom") {
		if (!isNum(r.width) || !isNum(r.height)) return undefined;
		out.width = clampDim(r.width);
		out.height = clampDim(r.height);
	} else if (!DEVICE_PRESETS.some((p) => p.id === preset)) {
		return undefined;
	}
	if (r.landscape === true) out.landscape = true;
	if (isNum(r.dpr) && r.dpr >= DPR_MIN && r.dpr <= DPR_MAX) out.dpr = r.dpr;
	if (r.touch === true) out.touch = true;
	return out;
};

const DEVICE_KEYS = ["preset", "width", "height", "landscape", "dpr", "touch"];

/** Strict counterpart of `normalizeDevice` for agent input (#548): refuses
 *  what the toolbar would silently clamp or drop. null clears the setting.
 *  An accepted value is passed through `normalizeDevice`, so it is exactly
 *  what the toolbar would store. */
export const validateDevice = (
	raw: unknown,
): { ok: true; value: DesignDevice | undefined } | { ok: false; error: string } => {
	const refuse = (m: string) => ({ ok: false as const, error: m });
	if (raw === null) return { ok: true, value: undefined };
	if (typeof raw !== "object" || Array.isArray(raw)) {
		return refuse("device must be an object or null");
	}
	const r = raw as Record<string, unknown>;
	const unknown = Object.keys(r).filter((k) => !DEVICE_KEYS.includes(k));
	if (unknown.length > 0) {
		return refuse(
			`unknown device keys: ${unknown.join(", ")} (allowed: ${DEVICE_KEYS.join(", ")})`,
		);
	}
	const preset = r.preset;
	const ids = DEVICE_PRESETS.map((p) => p.id);
	if (typeof preset !== "string" || !["none", "custom", ...ids].includes(preset)) {
		return refuse(`preset must be one of: none, custom, ${ids.join(", ")}`);
	}
	if (preset === "custom") {
		for (const k of ["width", "height"] as const) {
			const v = r[k];
			if (v === undefined) return refuse(`${k} is required with preset "custom"`);
			if (!isNum(v) || !Number.isInteger(v) || v < CUSTOM_MIN || v > CUSTOM_MAX) {
				return refuse(`${k} must be an integer from ${CUSTOM_MIN} to ${CUSTOM_MAX}`);
			}
		}
	} else if (r.width !== undefined || r.height !== undefined) {
		return refuse('width and height are only allowed with preset "custom"');
	}
	if (r.landscape !== undefined) {
		if (typeof r.landscape !== "boolean") return refuse("landscape must be a boolean");
		if (preset === "none") return refuse('landscape is not allowed with preset "none"');
	}
	if (r.dpr !== undefined && (!isNum(r.dpr) || r.dpr < DPR_MIN || r.dpr > DPR_MAX)) {
		return refuse(`dpr must be a number from ${DPR_MIN} to ${DPR_MAX}`);
	}
	if (r.touch !== undefined && typeof r.touch !== "boolean") {
		return refuse("touch must be a boolean");
	}
	return { ok: true, value: normalizeDevice(r) };
};

/** The device's size in portrait CSS px (orientation undone), as seeded into
 *  a custom size; 390×844 when it has none. */
export const portraitSize = (
	device: DesignDevice | undefined,
): { width: number; height: number } => {
	const s = frameSize(device);
	if (s === null) return { width: 390, height: 844 };
	return device?.landscape === true ? { width: s.height, height: s.width } : s;
};

/** A custom-size input's text → a valid dimension, or undefined (ignored). */
export const parseDim = (text: string): number | undefined => {
	const t = text.trim();
	if (!/^\d+$/.test(t)) return undefined;
	const n = Number(t);
	return n >= CUSTOM_MIN && n <= CUSTOM_MAX ? n : undefined;
};

/** The frame's CSS size (swapped when landscape); null = fill the pane. */
export const frameSize = (
	device: DesignDevice | undefined,
): { width: number; height: number } | null => {
	if (device === undefined || device.preset === "none") return null;
	let w: number;
	let h: number;
	if (device.preset === "custom") {
		if (device.width === undefined || device.height === undefined) return null;
		w = device.width;
		h = device.height;
	} else {
		const p = DEVICE_PRESETS.find((x) => x.id === device.preset);
		if (p === undefined) return null;
		w = p.width;
		h = p.height;
	}
	return device.landscape === true ? { width: h, height: w } : { width: w, height: h };
};

/** Scale that fits `frame` in `avail`; never upscales, and 1 while the
 *  pane is unmeasured. Always finite and > 0. */
export const fitScale = (
	frame: { width: number; height: number },
	avail: { width: number; height: number },
): number => {
	if (!(avail.width > 0) || !(avail.height > 0)) return 1;
	if (!(frame.width > 0) || !(frame.height > 0)) return 1;
	const s = Math.min(1, avail.width / frame.width, avail.height / frame.height);
	return Number.isFinite(s) && s > 0 ? s : 1;
};

/** Query parameters the preview server reads to inject shims at load.
 *  `dpr`, which `app/src-tauri/src/design/serve.rs` accepts in [0.5, 4] and
 *  turns into a `window.devicePixelRatio` getter override; and `touch=1`,
 *  which injects the touch-gesture and hover/pointer media shims. */
export const deviceParams = (device: DesignDevice | undefined): [string, string][] => {
	const out: [string, string][] = [];
	if (device?.dpr !== undefined) out.push(["dpr", String(device.dpr)]);
	if (device?.touch === true) out.push(["touch", "1"]);
	return out;
};

/** Short toolbar text, e.g. "iPhone 14 · 390×844 · 2×". */
export const deviceLabel = (device: DesignDevice | undefined): string => {
	const size = frameSize(device);
	if (device === undefined || size === null)
		return device?.touch === true ? "None · touch" : "None";
	const dims = `${size.width}×${size.height}`;
	const name =
		device.preset === "custom"
			? "Custom"
			: (DEVICE_PRESETS.find((p) => p.id === device.preset)?.label ?? device.preset);
	const dpr = device.dpr !== undefined ? ` · ${device.dpr}×` : "";
	const touch = device.touch === true ? " · touch" : "";
	return `${name} · ${dims}${dpr}${touch}`;
};
