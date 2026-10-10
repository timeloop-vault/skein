/// Silence after which the tail sentinel reads "idle" with a static dot
/// (handover §5.3 item 6; the 90 s figure is §12's suggestion, tunable).
/// Shared with the card head's "· idle 2h 14m" meta in LiveContext.
///
/// The idle basis itself is `max(derived.lastActionTs, lastLiveArrivalMs)`
/// from `useRoomActions` (#593). The receipt half matters because live rows
/// can carry ts=0 (opencode step rows): on timestamps alone a tool-less
/// opencode answer would read "idle" while it streams in.
export const IDLE_AFTER_MS = 90_000;
