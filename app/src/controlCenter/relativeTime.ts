// Compact relative time for the Control Center ("3m ago"). Pure: `now`
// is a parameter so it is testable and one tick re-renders every row
// against the same instant.

export function relativeTime(ms: number, now: number): string {
	const secs = Math.max(0, Math.floor((now - ms) / 1000));
	if (secs < 45) return "just now";
	const mins = Math.floor(secs / 60);
	if (mins < 1) return "1m ago";
	if (mins < 60) return `${mins}m ago`;
	const hours = Math.floor(mins / 60);
	if (hours < 24) return `${hours}h ago`;
	return `${Math.floor(hours / 24)}d ago`;
}
