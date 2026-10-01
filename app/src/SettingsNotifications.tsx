// Settings → notification surface toggles (#12 L5e). Each controls one
// surface independently; defaults are in App.tsx (in-app on, OS off).

import "./Settings.css";

export const NotificationSettings = ({
	notifyBadge,
	notifyToast,
	notifyUrgent,
	notifyOs,
	onNotifyBadge,
	onNotifyToast,
	onNotifyUrgent,
	onNotifyOs,
}: {
	notifyBadge: boolean;
	notifyToast: boolean;
	notifyUrgent: boolean;
	notifyOs: boolean;
	onNotifyBadge: (v: boolean) => void;
	onNotifyToast: (v: boolean) => void;
	onNotifyUrgent: (v: boolean) => void;
	onNotifyOs: (v: boolean) => void;
}) => (
	<div className="sk-field">
		<label>Notifications</label>
		<div className="sk-help">
			When an inactive harness goes idle or exits, Skein can surface that several ways. Each is
			independent — leave on what helps and turn off what distracts. Defaults: in-app on, OS
			notification off until you opt in.
		</div>
		<div className="sk-toggles">
			<label className="sk-toggle">
				<input
					type="checkbox"
					checked={notifyBadge}
					onChange={(e) => onNotifyBadge(e.target.checked)}
				/>
				<span className="sk-toggle-label">
					<span className="sk-toggle-title">Tab badges</span>
					<span className="sk-toggle-sub">
						Count on the room tab. Passive; only visible if you look up.
					</span>
				</span>
			</label>
			<label className="sk-toggle">
				<input
					type="checkbox"
					checked={notifyToast}
					onChange={(e) => onNotifyToast(e.target.checked)}
				/>
				<span className="sk-toggle-label">
					<span className="sk-toggle-title">Toast pop-up</span>
					<span className="sk-toggle-sub">
						Bottom-right card that slides in for a few seconds. Click to jump.
					</span>
				</span>
			</label>
			<label className="sk-toggle">
				<input
					type="checkbox"
					checked={notifyUrgent}
					onChange={(e) => onNotifyUrgent(e.target.checked)}
				/>
				<span className="sk-toggle-label">
					<span className="sk-toggle-title">Status-bar urgent indicator</span>
					<span className="sk-toggle-sub">
						Persistent indicator in the bottom bar pointing to whichever room has the most
						unattended events.
					</span>
				</span>
			</label>
			<label className="sk-toggle">
				<input type="checkbox" checked={notifyOs} onChange={(e) => onNotifyOs(e.target.checked)} />
				<span className="sk-toggle-label">
					<span className="sk-toggle-title">OS notification</span>
					<span className="sk-toggle-sub">
						System banner when Skein isn't focused (you've alt+tabbed away). First enable here
						triggers the macOS permission prompt.
					</span>
				</span>
			</label>
		</div>
	</div>
);
