import * as React from "react";

export function useMeetingDeadline(
  deadlineMs: number | null,
  onDeadline: () => void,
): number | null {
  const [now, setNow] = React.useState(Date.now());
  const firedFor = React.useRef<number | null>(null);
  const onDeadlineEvent = React.useEffectEvent(onDeadline);

  React.useEffect(() => {
    if (deadlineMs === null) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [deadlineMs]);

  const remainingMs =
    deadlineMs === null ? null : Math.max(0, deadlineMs - now);
  React.useEffect(() => {
    if (
      deadlineMs === null ||
      remainingMs === null ||
      remainingMs > 0 ||
      firedFor.current === deadlineMs
    ) {
      return;
    }
    firedFor.current = deadlineMs;
    onDeadlineEvent();
  }, [deadlineMs, remainingMs]);
  return remainingMs;
}

export function meetingDeadlineLabel(remainingMs: number | null): string {
  if (remainingMs === null) return "No active deadline";
  if (remainingMs <= 0) return "Checking authoritative state…";
  const seconds = Math.ceil(remainingMs / 1_000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
