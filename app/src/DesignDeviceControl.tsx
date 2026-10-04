// The design pane's device control (#528): viewport preset / custom size,
// portrait-landscape swap, and the DPR override. Controlled — every change
// is a whole new DesignDevice (or undefined = None) passed to onChange.

import { useEffect, useState } from "react";
import {
	CUSTOM_MAX,
	CUSTOM_MIN,
	DEVICE_PRESETS,
	type DesignDevice,
	DPR_CHOICES,
	normalizeDevice,
	parseDim,
	portraitSize,
} from "./designDevice.ts";
import "./designDevice.css";

const DPR_TITLE =
	"Overrides window.devicePixelRatio for scripts only — CSS (resolution) media queries and image-set still see the host's ratio. Reloads the preview.";

interface Props {
	device: DesignDevice | undefined;
	onChange: (device: DesignDevice | undefined) => void;
}

export const DesignDeviceControl = ({ device, onChange }: Props) => {
	const preset = device?.preset ?? "none";
	const custom = preset === "custom";
	const landscape = device?.landscape === true;

	const [w, setW] = useState(String(device?.width ?? ""));
	const [h, setH] = useState(String(device?.height ?? ""));
	useEffect(() => {
		setW(String(device?.width ?? ""));
		setH(String(device?.height ?? ""));
	}, [device?.width, device?.height]);

	const emit = (next: DesignDevice) => onChange(normalizeDevice(next));
	const carry = (): Pick<DesignDevice, "landscape" | "dpr"> => ({
		...(landscape ? { landscape: true } : {}),
		...(device?.dpr !== undefined ? { dpr: device.dpr } : {}),
	});

	const pick = (id: string) => {
		if (id === "custom") {
			const size = portraitSize(device);
			emit({ preset: "custom", width: size.width, height: size.height, ...carry() });
		} else {
			emit({ preset: id, ...carry() });
		}
	};

	const commit = () => {
		const nw = parseDim(w);
		const nh = parseDim(h);
		if (nw === undefined || nh === undefined) {
			// Invalid: ignored; show the stored values again.
			setW(String(device?.width ?? ""));
			setH(String(device?.height ?? ""));
			return;
		}
		if (nw === device?.width && nh === device?.height) return;
		emit({ preset: "custom", width: nw, height: nh, ...carry() });
	};
	const onKey = (e: React.KeyboardEvent) => {
		if (e.key === "Enter") commit();
	};

	return (
		<>
			<select
				className="dd-select"
				title="Device size"
				aria-label="Device size"
				value={preset}
				onChange={(e) => pick(e.target.value)}
			>
				<option value="none">None (fill pane)</option>
				{DEVICE_PRESETS.map((p) => (
					<option key={p.id} value={p.id}>
						{p.label} · {p.width}×{p.height}
					</option>
				))}
				<option value="custom">Custom…</option>
			</select>
			{custom && (
				<span className="dd-custom">
					<input
						type="number"
						aria-label="Width"
						min={CUSTOM_MIN}
						max={CUSTOM_MAX}
						value={w}
						onChange={(e) => setW(e.target.value)}
						onBlur={commit}
						onKeyDown={onKey}
					/>
					×
					<input
						type="number"
						aria-label="Height"
						min={CUSTOM_MIN}
						max={CUSTOM_MAX}
						value={h}
						onChange={(e) => setH(e.target.value)}
						onBlur={commit}
						onKeyDown={onKey}
					/>
				</span>
			)}
			<button
				type="button"
				title="Portrait / landscape"
				aria-label="Portrait / landscape"
				className="dd-rotate"
				aria-pressed={landscape}
				disabled={preset === "none"}
				onClick={() => device && emit({ ...device, landscape: !landscape })}
			>
				⟳
			</button>
			<select
				className="dd-select"
				title={DPR_TITLE}
				aria-label="Device pixel ratio"
				value={device?.dpr === undefined ? "" : String(device.dpr)}
				onChange={(e) => {
					const v = e.target.value;
					const base: DesignDevice = device ?? { preset: "none" };
					const { dpr: _drop, ...rest } = base;
					emit(v === "" ? rest : { ...rest, dpr: Number(v) });
				}}
			>
				<option value="">DPR: host</option>
				{DPR_CHOICES.map((d) => (
					<option key={d} value={String(d)}>
						{d}×
					</option>
				))}
			</select>
		</>
	);
};
